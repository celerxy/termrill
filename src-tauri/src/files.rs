//! 显式确认后的删除与有界文本编辑；保存保留原编码，通过字节摘要防止覆盖他人修改。
use crate::model::*;
use sha2::{Digest, Sha256};
#[cfg(test)]
use ssh2::{OpenFlags, OpenType, Sftp};
#[cfg(test)]
use std::io::{Read, Write};
use std::{fs, path::Path};

pub(crate) const EDIT_LIMIT: u64 = 2 * 1024 * 1024;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    content: String,
    revision: String,
    encoding: String,
}

#[cfg(test)]
fn read(sftp: &Sftp, path: &Path) -> Result<Vec<u8>> {
    read_with_network(sftp, path, None)
}

/// 每个协议请求在 EAGAIN 后原地继续；不能重新 OPEN 或重放整个文件读取。
#[cfg(test)]
pub(crate) fn network_step<T>(
    network: Option<(&ssh2::Session, &std::sync::atomic::AtomicBool)>,
    phase: &str,
    mut operation: impl FnMut() -> std::io::Result<T>,
) -> Result<T> {
    let started = std::time::Instant::now();
    let mut waits = 0u64;
    let mut ready = 0u64;
    let mut inbound = 0u64;
    let mut outbound = 0u64;
    let mut no_direction = 0u64;
    let mut socket_wait = std::time::Duration::ZERO;
    loop {
        if let Some((_, closed)) = network {
            if closed.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(AppError::new("连接已关闭，文件读取已停止"));
            }
        }
        match operation() {
            Ok(value) => {
                if waits > 0 {
                    crate::performance_log!(
                        "[TermRill text] phase=network_step operation={} elapsed_ms={} waits={} ready={} inbound={} outbound={} none={} socket_wait_ms={}",
                        phase,
                        started.elapsed().as_millis(),
                        waits, ready, inbound, outbound, no_direction, socket_wait.as_millis()
                    );
                }
                return Ok(value);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && network.is_some() => {
                if started.elapsed() >= std::time::Duration::from_secs(60) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("文件 {phase} 请求连续 60 秒未完成"),
                    )
                    .into());
                }
                waits += 1;
                let session = network.unwrap().0;
                match session.block_directions() {
                    ssh2::BlockDirections::Inbound => inbound += 1,
                    ssh2::BlockDirections::Outbound => outbound += 1,
                    ssh2::BlockDirections::Both => {
                        inbound += 1;
                        outbound += 1;
                    }
                    ssh2::BlockDirections::None => no_direction += 1,
                }
                let waiting = std::time::Instant::now();
                ready += u64::from(crate::transfer_io::wait_network_ready(session)?);
                socket_wait += waiting.elapsed();
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
fn read_with_network(
    sftp: &Sftp,
    path: &Path,
    network: Option<(&ssh2::Session, &std::sync::atomic::AtomicBool)>,
) -> Result<Vec<u8>> {
    let started = std::time::Instant::now();
    crate::performance_log!("[TermRill text] phase=stat_begin");
    let stat = network_step(network, "stat", || sftp.lstat(path).map_err(Into::into))?;
    if !stat.is_file() || stat.size.unwrap_or(EDIT_LIMIT + 1) > EDIT_LIMIT {
        return Err(AppError::new("仅支持编辑 2 MiB 以内的普通文本文件"));
    }
    let stat_elapsed = started.elapsed();
    let size = stat
        .size
        .ok_or_else(|| AppError::new("无法获取文本文件大小"))? as usize;
    crate::performance_log!(
        "[TermRill text] phase=open_begin stat_ms={} expected_bytes={}",
        stat_elapsed.as_millis(),
        size
    );
    let mut file = network_step(network, "open", || sftp.open(path).map_err(Into::into))?;
    let opened = started.elapsed();
    let mut bytes = Vec::with_capacity(stat.size.unwrap_or(0) as usize);
    // 查看打开时的长度，不追着持续追加的日志读到 EOF；保存仍核对全文摘要。
    // 保守使用 64 KiB，避免给服务器制造过大的预读突发。
    let mut buffer = vec![0u8; 64 * 1024];
    let mut reads = 0;
    crate::performance_log!(
        "[TermRill text] phase=read_begin open_ms={}",
        opened.saturating_sub(stat_elapsed).as_millis()
    );
    while bytes.len() < size {
        let requested = (size - bytes.len()).min(buffer.len());
        let count = network_step(network, "read", || file.read(&mut buffer[..requested]))?;
        reads += 1;
        if count == 0 {
            return Err(AppError::new("文件在读取期间缩短，请重新打开"));
        }
        if bytes.len() as u64 + count as u64 > EDIT_LIMIT {
            return Err(AppError::new("文件超过 2 MiB"));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let read_finished = started.elapsed();
    crate::performance_log!(
        "[TermRill text] phase=close_begin bytes={} read_ms={}",
        bytes.len(),
        read_finished.saturating_sub(opened).as_millis()
    );
    network_step(network, "close", || file.close().map_err(Into::into))?;
    crate::performance_log!("[TermRill text] phase=read stat_ms={} open_ms={} read_ms={} close_ms={} bytes={} read_calls={}",
        stat_elapsed.as_millis(), opened.saturating_sub(stat_elapsed).as_millis(),
        read_finished.saturating_sub(opened).as_millis(), started.elapsed().saturating_sub(read_finished).as_millis(), bytes.len(), reads);
    Ok(bytes)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<(String, &'static str)> {
    crate::text_encoding::decode(bytes)
}

pub(crate) fn encode(content: &str, encoding: &str) -> Result<Vec<u8>> {
    if content.len() as u64 > EDIT_LIMIT || content.contains('\0') {
        return Err(AppError::new("内容过大或包含二进制字符"));
    }
    let bytes = match encoding {
        "UTF-16 LE" | "UTF-16 BE" | "UTF-16 LE 无 BOM" | "UTF-16 BE 无 BOM" => {
            let little = encoding.starts_with("UTF-16 LE");
            let mut bytes = if encoding.ends_with("无 BOM") {
                Vec::new()
            } else if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in content.encode_utf16() {
                bytes.extend_from_slice(&if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            bytes
        }
        "UTF-8 BOM" => [b"\xef\xbb\xbf".as_slice(), content.as_bytes()].concat(),
        "UTF-8" => content.as_bytes().to_vec(),
        _ => crate::text_encoding::encode_legacy(content, encoding)?,
    };
    if bytes.len() as u64 > EDIT_LIMIT {
        return Err(AppError::new("编码后的内容超过 2 MiB"));
    }
    Ok(bytes)
}
pub(crate) fn document(bytes: &[u8]) -> Result<Document> {
    let (content, encoding) = decode(bytes)?;
    Ok(Document {
        content,
        encoding: encoding.into(),
        revision: format!("{:x}", Sha256::digest(bytes)),
    })
}

#[cfg(test)]
pub fn open(sftp: &Sftp, path: &Path) -> Result<Document> {
    let bytes = read(sftp, path)?;
    measured_document(bytes)
}

#[cfg(test)]
fn measured_document(bytes: Vec<u8>) -> Result<Document> {
    let started = std::time::Instant::now();
    let revision = format!("{:x}", Sha256::digest(&bytes));
    let (content, encoding) = decode(&bytes)?;
    crate::performance_log!(
        "[TermRill text] phase=decode bytes={} elapsed_ms={}",
        bytes.len(),
        started.elapsed().as_millis()
    );
    Ok(Document {
        content,
        revision,
        encoding: encoding.into(),
    })
}

#[cfg(test)]
pub fn save(sftp: &Sftp, path: &Path, content: &str, revision: &str) -> Result<Document> {
    if content.len() as u64 > EDIT_LIMIT || content.contains('\0') {
        return Err(AppError::new("内容过大或包含二进制字符"));
    }
    // SFTP 不支持 compare-and-swap；写入前及安装前检查，仍不能排除最后一瞬间的竞态。
    let unchanged = || -> Result<()> {
        if format!("{:x}", Sha256::digest(read(sftp, path)?)) != revision {
            return Err(AppError::new("远程文件已被修改，请重新打开后合并内容"));
        }
        Ok(())
    };
    // 编码由仍匹配摘要的远程原始文件确定，不接受前端任意指定编码。
    let original = read(sftp, path)?;
    if format!("{:x}", Sha256::digest(&original)) != revision {
        return Err(AppError::new("远程文件已被修改，请重新打开后合并内容"));
    }
    let (_, encoding) = decode(&original)?;
    let bytes = encode(content, encoding)?;
    let temporary = path.with_file_name(format!(".termrill-edit-{}.part", identifier()));
    let result = (|| {
        let mut file = sftp.open_mode(
            &temporary,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUSIVE,
            0o600,
            OpenType::File,
        )?;
        file.write_all(&bytes)?;
        file.close()?;
        unchanged()?;
        crate::transfer::install_remote(sftp, &temporary, path, true)?;
        Ok(Document {
            content: content.into(),
            revision: format!("{:x}", Sha256::digest(&bytes)),
            encoding: encoding.into(),
        })
    })();
    if result.is_err() {
        let _ = sftp.unlink(&temporary);
    }
    result
}

/// 显式选择 recursive 后删除目录内容；链接只删除自身，根目录及含 .. 的路径拒绝操作。
#[cfg(test)]
pub fn delete(path: &Path, sftp: Option<&Sftp>, recursive: bool) -> Result<()> {
    delete_observed(path, sftp, recursive, &delete_trace_id(), &|_| {})
}

#[cfg(test)]
pub(crate) fn delete_trace_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// 删除诊断只记录请求编号、深度和耗时，不记录路径；不重放任何删除请求。
#[cfg(test)]
pub(crate) struct DeleteTrace<'a> {
    id: &'a str,
    request: std::cell::Cell<u64>,
}
#[cfg(test)]
impl DeleteTrace<'_> {
    pub(crate) fn new(id: &str) -> DeleteTrace<'_> {
        DeleteTrace {
            id,
            request: std::cell::Cell::new(0),
        }
    }

    pub(crate) fn step<T>(
        &self,
        operation: &str,
        depth: usize,
        action: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let request = self.request.get() + 1;
        self.request.set(request);
        let started = std::time::Instant::now();
        crate::performance_log!("[TermRill delete] id={} phase=request_begin operation={operation} request={request} depth={depth}", self.id);
        let result = action();
        crate::performance_log!("[TermRill delete] id={} phase=request_end operation={operation} request={request} depth={depth} elapsed_ms={} success={}", self.id, started.elapsed().as_millis(), result.is_ok());
        result
    }
}

#[cfg(test)]
pub(crate) fn delete_observed(
    path: &Path,
    sftp: Option<&Sftp>,
    recursive: bool,
    id: &str,
    progress: &dyn Fn(&str),
) -> Result<()> {
    let trace = DeleteTrace::new(id);
    progress("检查删除路径");
    check_delete_path(path, sftp, &trace)?;
    progress("正在删除");
    delete_inner(path, sftp, recursive, 0, &trace)
}

/// 普通文件入口只执行 UNLINK；如果目标变成目录，服务器拒绝而不递归删除。
#[cfg(test)]
pub(crate) fn delete_file_observed(
    path: &Path,
    sftp: &Sftp,
    id: &str,
    progress: &dyn Fn(&str),
) -> Result<()> {
    let trace = DeleteTrace::new(id);
    progress("检查删除路径");
    check_delete_path(path, Some(sftp), &trace)?;
    progress("正在删除文件");
    trace.step("unlink", 0, || sftp.unlink(path).map_err(Into::into))
}

#[cfg(test)]
pub(crate) fn check_delete_path(
    path: &Path,
    sftp: Option<&Sftp>,
    trace: &DeleteTrace<'_>,
) -> Result<()> {
    let absolute = if sftp.is_some() {
        path.to_string_lossy().starts_with('/')
    } else {
        path.is_absolute()
    };
    if !absolute
        || path.to_string_lossy().contains('\0')
        || path.parent().is_none()
        || path.file_name().is_none()
        || path
            .to_string_lossy()
            .split(['/', '\\'])
            .any(|part| part == ".." || part == ".")
    {
        return Err(AppError::new("不能删除根目录或无效路径"));
    }
    // 显式路径中间的链接也不能成为递归入口。检查与操作之间仍有文件系统竞态。
    if let Some(sftp) = sftp {
        if path.to_string_lossy().contains('\\') {
            return Err(AppError::new("远程路径必须使用 POSIX 分隔符"));
        }
        for (depth, ancestor) in path.ancestors().skip(1).enumerate() {
            if ancestor.as_os_str().is_empty() || ancestor == Path::new("/") {
                continue;
            }
            let stat = trace.step("parent_lstat", depth, || {
                sftp.lstat(ancestor).map_err(Into::into)
            })?;
            if !stat.is_dir() {
                return Err(AppError::new("删除路径包含链接或非目录父级"));
            }
        }
    } else {
        for ancestor in path.ancestors().skip(1) {
            let stat = fs::symlink_metadata(ancestor)?;
            if stat.file_type().is_symlink() {
                return Err(AppError::new("删除路径包含符号链接父级"));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if stat.file_attributes() & 0x400 != 0 {
                    return Err(AppError::new("删除路径包含目录联接父级"));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn delete_inner(
    path: &Path,
    sftp: Option<&Sftp>,
    recursive: bool,
    depth: usize,
    trace: &DeleteTrace<'_>,
) -> Result<()> {
    delete_contents(path, sftp, recursive, depth, trace)
        .map_err(|error| AppError::new(format!("删除 {} 失败：{}", path.display(), error.message)))
}

#[cfg(test)]
fn delete_contents(
    path: &Path,
    sftp: Option<&Sftp>,
    recursive: bool,
    depth: usize,
    trace: &DeleteTrace<'_>,
) -> Result<()> {
    if depth > 128 {
        return Err(AppError::new(
            "目录层级超过 128，已停止删除；部分项目可能已删除",
        ));
    }
    if let Some(sftp) = sftp {
        let stat = trace.step("target_lstat", depth, || {
            sftp.lstat(path).map_err(Into::into)
        })?;
        if stat.is_dir() {
            if !recursive
                && trace
                    .step("readdir", depth, || sftp.readdir(path).map_err(Into::into))?
                    .iter()
                    .any(|(child, _)| {
                        child
                            .file_name()
                            .is_some_and(|name| name != "." && name != "..")
                    })
            {
                return Err(AppError::new(
                    "目录非空，请勾选“包含文件夹内的全部内容（递归删除）”后再删除",
                ));
            }
            if recursive {
                for (child, listed_stat) in
                    trace.step("readdir", depth, || sftp.readdir(path).map_err(Into::into))?
                {
                    let name = child
                        .file_name()
                        .ok_or_else(|| AppError::new("服务器返回无效文件名"))?;
                    if name == "." || name == ".." {
                        continue;
                    }
                    // 不信任服务器返回的完整路径，只允许当前目录下一个名称。
                    if name.to_string_lossy().contains(['/', '\\']) {
                        return Err(AppError::new("服务器返回无效文件名"));
                    }
                    let child = format!(
                        "{}/{}",
                        path.to_string_lossy().trim_end_matches('/'),
                        name.to_string_lossy()
                    );
                    if listed_stat.perm.is_some() && !listed_stat.is_dir() {
                        // READDIR 已给出类型；普通文件/链接直接 unlink，省去逐项 LSTAT。
                        // unlink 不跟随链接；若被并发替换成目录则失败，不递归进入。
                        trace
                            .step("unlink", depth + 1, || {
                                sftp.unlink(Path::new(&child)).map_err(Into::into)
                            })
                            .map_err(|error| {
                                AppError::new(format!("删除 {child} 失败：{}", error.message))
                            })?;
                    } else {
                        // 目录仍重新 LSTAT，防止把新替换的链接作为递归入口。
                        delete_inner(Path::new(&child), Some(sftp), true, depth + 1, trace)?;
                    }
                }
            }
            trace.step("rmdir", depth, || sftp.rmdir(path).map_err(Into::into))?;
        } else {
            trace.step("unlink", depth, || sftp.unlink(path).map_err(Into::into))?;
        }
    } else {
        let stat = fs::symlink_metadata(path)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            // 目录联接是 reparse point；移除链接自身，不能递归进入其目标。
            if stat.file_attributes() & 0x400 != 0 {
                if stat.is_dir() {
                    fs::remove_dir(path)?;
                } else {
                    fs::remove_file(path)?;
                }
                return Ok(());
            }
        }
        if stat.is_dir() && !stat.file_type().is_symlink() {
            if !recursive && fs::read_dir(path)?.next().transpose()?.is_some() {
                return Err(AppError::new(
                    "目录非空，请勾选“包含文件夹内的全部内容（递归删除）”后再删除",
                ));
            }
            if recursive {
                for child in fs::read_dir(path)? {
                    delete_inner(&child?.path(), None, true, depth + 1, trace)?;
                }
            }
            fs::remove_dir(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

/// 本地删除只使用文件系统 API；链接或目录联接只移除自身。
pub fn delete_local_observed(path: &Path, recursive: bool, progress: &dyn Fn(&str)) -> Result<()> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path.to_string_lossy().contains('\0')
        || path
            .to_string_lossy()
            .split(['/', '\\'])
            .any(|part| part == "." || part == "..")
    {
        return Err(AppError::new("不能删除根目录或无效路径"));
    }
    progress("检查删除路径");
    for parent in path.ancestors().skip(1) {
        let stat = fs::symlink_metadata(parent)?;
        if crate::local_link(&stat) || !stat.is_dir() {
            return Err(AppError::new("删除路径包含链接或非目录父级"));
        }
    }
    progress("正在删除");
    delete_local(path, recursive, 0)
}
fn delete_local(path: &Path, recursive: bool, depth: usize) -> Result<()> {
    if depth > 128 {
        return Err(AppError::new("删除目录超过 128 层"));
    }
    let stat = fs::symlink_metadata(path)?;
    if crate::local_link(&stat) {
        crate::links::remove_local(path)?;
    } else if stat.is_dir() {
        if !recursive && fs::read_dir(path)?.next().transpose()?.is_some() {
            return Err(AppError::new("目录非空，请明确选择递归删除"));
        }
        if recursive {
            for child in fs::read_dir(path)? {
                delete_local(&child?.path(), true, depth + 1)?;
            }
        }
        fs::remove_dir(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}
