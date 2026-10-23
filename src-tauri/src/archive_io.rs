//! 压缩包读写复用 SFTP；不执行 Shell，不跟随链接，逐文件临时安装。
use crate::model::*;
use std::{
    cell::RefCell,
    fs,
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub const ARCHIVE_LIMIT: u64 = 512 * 1024 * 1024;
pub const CONTENT_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
pub const ENTRY_LIMIT: usize = 10_000;

pub struct ArchiveIo<'a> {
    pub native: Option<&'a std::sync::Arc<crate::remote_async::Remote>>,
    pub cancel: &'a AtomicBool,
    pub closed: Option<&'a AtomicBool>,
    pub progress: tauri::ipc::Channel<String>,
    pub last_progress: &'a RefCell<Option<Instant>>,
}
#[derive(Clone)]
pub struct Stat {
    pub directory: bool,
    pub size: u64,
    pub remote: Option<russh_sftp::protocol::FileAttributes>,
}

/// 结构化压缩进度沿用字符串通道，普通预览消息保持兼容；数值均来自本次操作。
#[derive(Default, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackProgress {
    pub kind: &'static str,
    pub stage: &'static str,
    pub files: usize,
    pub directories: usize,
    pub processed_files: usize,
    pub total_bytes: u64,
    pub processed_bytes: u64,
    pub zip_bytes: u64,
    pub uploaded_bytes: u64,
    pub current_file: String,
}

/// 使用共同的可移植相对名称约束，防止 zip-slip、ADS 与 Windows 保留名称。
pub fn safe_name(name: &str) -> Result<String> {
    let name = name.trim_end_matches('/');
    if name.is_empty()
        || name.len() > 4096
        || name.contains(['\\', '\0', ':'])
        || name.starts_with('/')
    {
        return Err(AppError::new("压缩包包含不安全的路径"));
    }
    for part in name.split('/') {
        let stem = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part.contains(['<', '>', '"', '|', '?', '*'])
            || part.chars().any(char::is_control)
            || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
        {
            return Err(AppError::new("压缩包包含不可安全写入的文件名"));
        }
    }
    if name.split('/').count() > 128 {
        return Err(AppError::new("目录层级超过 128"));
    }
    Ok(name.into())
}
impl ArchiveIo<'_> {
    pub fn pack_progress(&self, state: &PackProgress, force: bool) {
        let mut previous = self.last_progress.borrow_mut();
        if !force && previous.is_some_and(|time| time.elapsed() < Duration::from_millis(100)) {
            return;
        }
        *previous = Some(Instant::now());
        if let Ok(message) = serde_json::to_string(state) {
            let _ = self.progress.send(message);
        }
    }
    pub fn pack_scan(&self, files: usize, directories: usize) {
        self.pack_progress(
            &PackProgress {
                kind: "packProgress",
                stage: "scan",
                files,
                directories,
                ..Default::default()
            },
            false,
        );
    }
    pub fn check(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed)
            || self
                .closed
                .is_some_and(|closed| closed.load(Ordering::Relaxed))
        {
            return Err(AppError::new(
                "操作已取消或连接已断开；已安装的文件不会回滚",
            ));
        }
        Ok(())
    }
    pub fn phase(&self, text: impl Into<String>) {
        // 多个读写阶段共用时钟，避免小文件密集操作产生大量 IPC 与界面重绘。
        let mut previous = self.last_progress.borrow_mut();
        if previous.is_some_and(|time| time.elapsed() < Duration::from_millis(100)) {
            return;
        }
        *previous = Some(Instant::now());
        let _ = self.progress.send(text.into());
    }
    pub(crate) fn valid_path(&self, path: &Path) -> Result<()> {
        let absolute = if self.native.is_some() {
            path.to_string_lossy().starts_with('/')
        } else {
            path.is_absolute()
        };
        if !absolute
            || path.to_string_lossy().contains('\0')
            || path.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(AppError::new("目标路径必须是绝对路径且不能含点分段"));
        }
        for component in path.components() {
            if let std::path::Component::Normal(name) = component {
                safe_name(&name.to_string_lossy())?;
            }
        }
        if self.native.is_some()
            && (!path.to_string_lossy().starts_with('/') || path.to_string_lossy().contains('\\'))
        {
            return Err(AppError::new("远程路径必须使用 POSIX 分隔符"));
        }
        Ok(())
    }
    pub fn join(&self, parent: &Path, name: &str) -> std::path::PathBuf {
        if self.native.is_some() {
            std::path::PathBuf::from(format!(
                "{}/{}",
                parent.to_string_lossy().trim_end_matches('/'),
                name
            ))
        } else {
            parent.join(name)
        }
    }
    pub fn stat(&self, path: &Path) -> Result<Option<Stat>> {
        self.check()?;
        self.valid_path(path)?;
        if let Some(remote) = self.native {
            let stat = tauri::async_runtime::block_on(async {
                match remote
                    .checked(
                        async {
                            Ok::<_, AppError>(
                                remote
                                    .native
                                    .sftp
                                    .lstat(path.to_string_lossy().into_owned())
                                    .await,
                            )
                        },
                        self.cancel,
                    )
                    .await?
                {
                    Ok(value) => Ok(Some(value.attrs)),
                    Err(error) if crate::remote_async::missing(&error) => Ok(None),
                    Err(error) => Err(AppError::from(error)),
                }
            })?;
            let Some(stat) = stat else { return Ok(None) };
            let kind = crate::remote_async::attrs(&stat);
            if !kind.is_file() && !kind.is_dir() {
                return Err(AppError::new("不支持链接或特殊文件"));
            }
            Ok(Some(Stat {
                directory: kind.is_dir(),
                size: stat.size.unwrap_or(ARCHIVE_LIMIT + 1),
                remote: Some(stat),
            }))
        } else {
            let stat = match fs::symlink_metadata(path) {
                Ok(stat) => stat,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            if local_link(&stat) || (!stat.is_file() && !stat.is_dir()) {
                return Err(AppError::new("不支持链接或特殊文件"));
            }
            Ok(Some(Stat {
                directory: stat.is_dir(),
                size: stat.len(),
                remote: None,
            }))
        }
    }
    pub fn parents(&self, path: &Path, create: bool) -> Result<()> {
        let mut parents: Vec<_> = path.ancestors().skip(1).collect();
        parents.reverse();
        for parent in parents {
            match self.stat(parent)? {
                Some(stat) if stat.directory => (),
                None if create => self.mkdir(parent)?,
                _ => return Err(AppError::new("父级缺失或不是普通目录")),
            }
        }
        Ok(())
    }
    pub fn preflight_parents(&self, path: &Path) -> Result<()> {
        for parent in path.ancestors().skip(1) {
            if self.stat(parent)?.is_some_and(|stat| !stat.directory) {
                return Err(AppError::new("解压父级不是普通目录"));
            }
        }
        Ok(())
    }
    pub fn mkdir(&self, path: &Path) -> Result<()> {
        self.check()?;
        if let Some(remote) = self.native {
            tauri::async_runtime::block_on(remote.checked(
                remote.native.sftp.mkdir(
                    path.to_string_lossy().into_owned(),
                    russh_sftp::protocol::FileAttributes {
                        permissions: Some(0o755),
                        ..Default::default()
                    },
                ),
                self.cancel,
            ))?;
        } else {
            fs::create_dir(path)?;
        }
        Ok(())
    }
    pub fn directory(&self, path: &Path) -> Result<()> {
        self.parents(path, true)?;
        match self.stat(path)? {
            Some(stat) if stat.directory => Ok(()),
            Some(_) => Err(AppError::new("目录与现有文件冲突")),
            None => self.mkdir(path),
        }
    }
    pub fn read(&self, path: &Path, limit: u64) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.copy_to(path, limit, &mut bytes)?;
        Ok(bytes)
    }
    /// ZIP 写入直接消费固定大小的读取块，避免额外持有整个源文件；仍逐块检查取消与大小。
    pub fn copy_to(&self, path: &Path, limit: u64, writer: &mut impl Write) -> Result<u64> {
        self.copy_to_progress(path, limit, writer, |_| {})
    }
    pub fn copy_to_progress(
        &self,
        path: &Path,
        limit: u64,
        writer: &mut impl Write,
        progress: impl Fn(u64),
    ) -> Result<u64> {
        if let Some(remote) = self.native {
            let started = Instant::now();
            self.parents(path, false)?;
            let result = tauri::async_runtime::block_on(remote.read_archive_into(
                &path.to_string_lossy(),
                limit,
                self.cancel,
                |chunk| {
                    writer.write_all(chunk)?;
                    Ok(())
                },
                |bytes| {
                    progress(bytes);
                    self.phase(format!(
                        "正在读取压缩包 · 已读取 {:.1} MiB",
                        bytes as f64 / (1024.0 * 1024.0)
                    ))
                },
            ));
            crate::performance_log!(
                "[TermRill archive] engine=async-full read_ms={} bytes={} success={}",
                started.elapsed().as_millis(),
                result.as_ref().copied().unwrap_or(0),
                result.is_ok()
            );
            return result;
        }
        self.parents(path, false)?;
        let stat = self
            .stat(path)?
            .ok_or_else(|| AppError::new("源文件不存在"))?;
        if stat.directory || stat.size > limit {
            return Err(AppError::new("源文件类型或大小超出限制"));
        }
        let mut reader = fs::File::open(path)?;
        let opened = reader.metadata()?;
        if local_link(&opened) || !opened.is_file() || opened.len() != stat.size {
            return Err(AppError::new("源文件打开时发生变化"));
        }
        let mut total = 0u64;
        let mut chunk = [0; 65536];
        loop {
            self.check()?;
            let count = reader.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > stat.size || total > limit {
                return Err(AppError::new("源文件读取期间增长"));
            }
            writer.write_all(&chunk[..count])?;
            progress(total);
        }
        let current = reader.metadata()?;
        if total != stat.size
            || current.len() != opened.len()
            || current.modified().ok() != opened.modified().ok()
        {
            return Err(AppError::new("源文件读取期间发生变化"));
        }
        Ok(total)
    }
    pub fn children(&self, path: &Path) -> Result<Vec<std::path::PathBuf>> {
        let names = if let Some(remote) = self.native {
            tauri::async_runtime::block_on(crate::remote_delete::children(
                remote,
                &path.to_string_lossy(),
                self.cancel,
            ))?
            .into_iter()
            .map(|path| {
                Path::new(&path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
        } else {
            fs::read_dir(path)?
                .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
                .collect::<std::io::Result<Vec<_>>>()?
        };
        if names.len() > ENTRY_LIMIT {
            return Err(AppError::new("目录项目过多"));
        }
        let mut result = Vec::new();
        for name in names {
            if name == "." || name == ".." {
                continue;
            }
            safe_name(&name)?;
            if name.contains('/') {
                return Err(AppError::new("目录返回的文件名无效"));
            }
            result.push(self.join(path, &name));
        }
        Ok(result)
    }
    pub fn install(&self, path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
        self.install_progress(path, bytes, overwrite, |_, _| {})
    }
    pub fn install_progress(
        &self,
        path: &Path,
        bytes: &[u8],
        overwrite: bool,
        progress: impl Fn(u64, bool),
    ) -> Result<()> {
        self.parents(path, true)?;
        if let Some(remote) = self.native {
            return tauri::async_runtime::block_on(remote.install_bytes_progress(
                &path.to_string_lossy(),
                bytes,
                overwrite,
                self.cancel,
                None,
                progress,
            ));
        }
        let exists = self.stat(path)?;
        if exists.as_ref().is_some_and(|stat| stat.directory) || (exists.is_some() && !overwrite) {
            return Err(AppError::new("目标已存在或类型冲突"));
        }
        let parent = path
            .parent()
            .ok_or_else(|| AppError::new("目标不能是根目录"))?;
        let temporary = self.join(parent, &format!(".termrill-archive-{}.part", identifier()));
        // 独占创建成功后才清理此任务临时文件，不能删除预先存在的项目。
        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let result = (|| {
                let mut written = 0u64;
                for chunk in bytes.chunks(64 * 1024) {
                    self.check()?;
                    file.write_all(chunk)?;
                    written += chunk.len() as u64;
                    progress(written, false);
                }
                progress(written, true);
                file.sync_all()?;
                drop(file);
                self.check()?;
                self.parents(path, false)?;
                let stat = self.stat(path)?;
                if stat.as_ref().is_some_and(|stat| stat.directory) {
                    return Err(AppError::new("目标类型已变化"));
                }
                if overwrite && stat.is_some() {
                    fs::set_permissions(&temporary, fs::metadata(path)?.permissions())?;
                    crate::local_files::replace(&temporary, path)?;
                } else {
                    // hard_link 原子拒绝同名目标，随后移除临时名称；不使用会覆盖目标的 rename。
                    fs::hard_link(&temporary, path)?;
                    fs::remove_file(&temporary)?;
                }
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            result
        }
    }
}
