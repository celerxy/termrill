//! 内存有界的目录分页：每次只保留一页；不把目录枚举结果全部缓存。
use crate::display_path;
use crate::model::*;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{fs, path::Path};

const PAGE_SIZE: usize = 1000;
pub(crate) type RegularMetadata =
    std::collections::BTreeMap<std::path::PathBuf, (std::time::Instant, crate::model::FileStat)>;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub offset: usize,
    pub size: usize,
    pub has_more: bool,
    pub query: String,
    pub show_hidden: bool,
}

#[derive(serde::Serialize)]
pub struct PageListing {
    #[serde(flatten)]
    pub listing: Listing,
    pub page: Page,
    /// 仅后端使用 READDIR 原始属性；前端传入的类型/大小不参与预览信任判断。
    #[serde(skip)]
    pub(crate) regular_metadata: RegularMetadata,
}

/// 过滤发生在分页之前，所以搜索能找到未浏览过的项目。偏移按匹配项计数。
pub struct Filter {
    offset: usize,
    query: String,
    show_hidden: bool,
    matched: usize,
    pub(crate) entries: Vec<FileEntry>,
    pub(crate) page_size: usize,
    pub(crate) regular_metadata: RegularMetadata,
}
impl Filter {
    pub fn new(offset: usize, query: String, show_hidden: bool) -> Result<Self> {
        Self::with_size(offset, query, show_hidden, PAGE_SIZE)
    }
    pub(crate) fn with_size(
        offset: usize,
        query: String,
        show_hidden: bool,
        page_size: usize,
    ) -> Result<Self> {
        if query.len() > 4096 {
            return Err(AppError::new("搜索文本过长"));
        }
        Ok(Self {
            offset,
            query: query.to_lowercase(),
            show_hidden,
            matched: 0,
            entries: Vec::with_capacity(page_size + 1),
            page_size,
            regular_metadata: Default::default(),
        })
    }
    /// 浏览首屏只等 256 个匹配项；其他调用保留默认页大小。
    pub fn browser(offset: usize, query: String, show_hidden: bool) -> Result<Self> {
        Self::with_size(offset, query, show_hidden, 256)
    }
    pub(crate) fn accepts(&mut self, name: &str) -> bool {
        if name == "."
            || name == ".."
            || (!self.show_hidden && name.starts_with('.'))
            || (!self.query.is_empty() && !name.to_lowercase().contains(&self.query))
        {
            return false;
        }
        self.matched += 1;
        self.matched > self.offset
    }
    pub(crate) fn finish(mut self, path: String, parent: Option<String>) -> PageListing {
        let has_more = self.entries.len() > self.page_size;
        self.entries.truncate(self.page_size);
        PageListing {
            regular_metadata: self.regular_metadata,
            listing: Listing {
                path,
                parent,
                entries: sorted(self.entries),
            },
            page: Page {
                offset: self.offset,
                size: self.page_size,
                has_more,
                query: self.query,
                show_hidden: self.show_hidden,
            },
        }
    }
}

pub fn local(path: &Path, mut filter: Filter) -> Result<PageListing> {
    let canonical = fs::canonicalize(path)?;
    for entry in fs::read_dir(&canonical)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !filter.accepts(&name) {
            continue;
        }
        let stat = match fs::symlink_metadata(entry.path()) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        filter.entries.push(FileEntry {
            name,
            path: display_path(&entry.path()),
            directory: stat.is_dir(),
            symlink: local_link(&stat),
            size: stat.len(),
            modified: stat
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|time| time.as_secs()),
        });
        if filter.entries.len() > filter.page_size {
            break;
        }
    }
    Ok(filter.finish(
        display_path(&canonical),
        canonical.parent().map(display_path),
    ))
}

#[cfg(test)]
pub fn remote(
    sftp: &ssh2::Sftp,
    path: &str,
    closed: &AtomicBool,
    filter: Filter,
) -> Result<PageListing> {
    remote_inner(sftp, path, closed, filter, None)
}

#[cfg(test)]
fn step<T>(
    session: Option<&ssh2::Session>,
    closed: &AtomicBool,
    phase: &str,
    mut operation: impl FnMut() -> std::result::Result<T, ssh2::Error>,
) -> std::result::Result<T, ssh2::Error> {
    let started = std::time::Instant::now();
    let mut waits = 0u64;
    loop {
        if closed.load(Ordering::Relaxed) {
            return Err(ssh2::Error::new(
                ssh2::ErrorCode::Session(-7),
                "连接已关闭，目录读取已停止",
            ));
        }
        match operation() {
            Err(error) if error.code() == ssh2::ErrorCode::Session(-37) && session.is_some() => {
                if started.elapsed() >= std::time::Duration::from_secs(60) {
                    return Err(ssh2::Error::new(
                        ssh2::ErrorCode::Session(-9),
                        "目录请求连续 60 秒未完成",
                    ));
                }
                waits += 1;
                // session.is_some() 是此分支的前置条件；每次至多等待 50 ms 检查断开。
                if crate::transfer_io::wait_network(session.unwrap()).is_err() {
                    return Err(ssh2::Error::new(
                        ssh2::ErrorCode::Session(-7),
                        "目录 socket 就绪等待失败",
                    ));
                }
            }
            result => {
                if started.elapsed() >= std::time::Duration::from_millis(500) {
                    crate::performance_log!(
                        "[TermRill directory] operation={} elapsed_ms={} waits={} success={}",
                        phase,
                        started.elapsed().as_millis(),
                        waits,
                        result.is_ok()
                    );
                }
                return result;
            }
        }
    }
}

#[cfg(test)]
fn remote_inner(
    sftp: &ssh2::Sftp,
    path: &str,
    closed: &AtomicBool,
    mut filter: Filter,
    session: Option<&ssh2::Session>,
) -> Result<PageListing> {
    let canonical = step(session, closed, "realpath", || {
        sftp.realpath(Path::new(path))
    })?;
    let mut directory = step(session, closed, "opendir", || sftp.opendir(&canonical))?;
    loop {
        if closed.load(Ordering::Relaxed) {
            return Err(AppError::new("连接已关闭，目录读取已停止"));
        }
        let (filename, stat) = match step(session, closed, "readdir", || directory.readdir()) {
            Ok(entry) => entry,
            Err(error) if error.code() == ssh2::ErrorCode::Session(-16) => break,
            Err(error) => return Err(error.into()),
        };
        let Some(name) = filename.file_name() else {
            continue;
        };
        let name = name.to_string_lossy().into_owned();
        if !filter.accepts(&name) {
            continue;
        }
        if stat.is_file() && stat.size.is_some() && stat.mtime.is_some() {
            filter.regular_metadata.insert(
                canonical.join(&filename),
                (std::time::Instant::now(), stat.clone()),
            );
        }
        filter.entries.push(FileEntry {
            name,
            path: canonical
                .join(filename)
                .to_string_lossy()
                .replace('\\', "/"),
            directory: stat.is_dir(),
            symlink: stat
                .perm
                .map(|mode| mode & 0o170000 == 0o120000)
                .unwrap_or(false),
            size: stat.size.unwrap_or(0),
            modified: stat.mtime,
        });
        if filter.entries.len() > filter.page_size {
            break;
        }
    }
    // 显式完成 CLOSE，避免非阻塞 Drop 只发送一次 EAGAIN 留下未完成状态。
    step(session, closed, "close", || directory.close())?;
    // 每页重新枚举到偏移处：无需长期持有目录句柄，断开、切换目录无需回收游标。
    // 枚举不是快照；服务器并发增删可能改变页边界，深页和稀疏搜索仍需网络扫描。
    Ok(filter.finish(
        canonical.to_string_lossy().replace('\\', "/"),
        canonical
            .parent()
            .map(|path| path.to_string_lossy().replace('\\', "/")),
    ))
}
