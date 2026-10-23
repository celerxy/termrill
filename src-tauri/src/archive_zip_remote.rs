//! ZIP Read/Seek 衔接原生异步范围请求；面板复用句柄/索引，负载缓存至多 2 MiB。
use super::*;
use futures::{stream, StreamExt};
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use std::{
    collections::VecDeque,
    io::{Seek, SeekFrom},
    time::Instant,
};
#[path = "archive_zip_pool.rs"]
mod pool;
#[path = "archive_zip_shared.rs"]
mod shared;
struct Reader<'a, 'b> {
    remote: std::sync::Arc<crate::remote_async::Remote>,
    io: &'a ArchiveIo<'b>,
    handle: Option<String>,
    before: FileAttributes,
    size: u64,
    position: u64,
    cache: VecDeque<(u64, Vec<u8>)>,
    downloaded: u64,
    requests: usize,
    started: Instant,
    _permit: Option<tokio::sync::OwnedSemaphorePermit>,
    index: Option<std::sync::Arc<index::Index>>,
    entries: Vec<Entry>,
    payload: Option<std::ops::Range<u64>>,
}
// 解码器常用小缓冲；网络预读由所选条目边界决定，不能跟随解码器退化为 64 KiB。
fn read_ahead(
    position: u64,
    size: u64,
    payload: Option<&std::ops::Range<u64>>,
    output: usize,
) -> usize {
    let available = size.saturating_sub(position);
    if let Some(payload) = payload.filter(|range| range.contains(&position)) {
        return available.min(payload.end - position).min(1024 * 1024) as usize;
    }
    available.min(if output >= 32768 { 1024 * 1024 } else { 65536 }) as usize
}
impl<'a, 'b> Reader<'a, 'b> {
    fn fetch(&mut self, length: usize) -> Result<Vec<u8>> {
        let offset = self.position;
        let remote = &self.remote;
        let io = self.io;
        let received = self.downloaded;
        let requests = std::sync::atomic::AtomicUsize::new(0);
        let request_count = &requests;
        let cancel = self.io.cancel;
        let handle = self
            .handle
            .as_ref()
            .ok_or_else(|| AppError::new("ZIP 读取句柄已关闭"))?;
        let block = self.remote.native.archive_read_size.clamp(1, 128 * 1024);
        let started = Instant::now();
        let cpu_before = crate::performance::cpu_micros();
        let chunks = tauri::async_runtime::block_on(async {
            // 返回内容前统一核对句柄属性；ZIP 解码到 EOF 同时确认 CRC/AES。

            let mut pending = stream::iter((0..length).step_by(block).map(|start| {
                    let length = (length - start).min(block);
                    async move {
                        let mut chunk = Vec::with_capacity(length);
                        while chunk.len() < length {
                            request_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let request_started = Instant::now();
                            let requested = length - chunk.len();
                            let response = remote
                                .checked(
                                    remote.native.sftp.read(
                                        handle.clone(),
                                        offset + start as u64 + chunk.len() as u64,
                                        (length - chunk.len()) as u32,
                                    ),
                                    cancel,
                                )
                                .await?;
                            let response_id = response.id;
                            let data = response.data;
                            crate::performance_log!(
                                "[TermRill archive] phase=read_response id={} batch_offset={} requested={} received={} elapsed_ms={} batch_elapsed_ms={}",
                                response_id, start + chunk.len(), requested, data.len(),
                                request_started.elapsed().as_millis(), started.elapsed().as_millis()
                            );
                            if data.is_empty() || data.len() > length - chunk.len() {
                                return Err(AppError::new("ZIP 范围响应提前结束或越界"));
                            }
                            chunk.extend(data);
                        }
                        Ok::<_, AppError>((start, chunk))
                    }
                }))
                .buffer_unordered(32);
            let mut bytes = vec![0; length];
            let mut completed = 0usize;
            while let Some(chunk) = pending.next().await {
                let (start, chunk) = chunk?;
                bytes[start..start + chunk.len()].copy_from_slice(&chunk);
                completed += chunk.len();
                io.phase(format!(
                    "正在读取压缩包 · 已读取 {:.1} KiB",
                    (received + completed as u64) as f64 / 1024.0
                ));
            }
            Ok::<_, AppError>(bytes)
        });
        let request_count = requests.load(std::sync::atomic::Ordering::Relaxed);
        crate::performance_log!(
            "[TermRill archive] phase=read_batch requests={} bytes={} elapsed_ms={} success={} process_cpu_us={:?}",
            request_count,
            chunks.as_ref().map_or(0, Vec::len),
            started.elapsed().as_millis(),
            chunks.is_ok(),
            cpu_before.zip(crate::performance::cpu_micros()).map(|(before, after)| after.saturating_sub(before))
        );
        self.requests += request_count;
        let chunks = chunks?;
        self.downloaded += chunks.len() as u64;
        self.io.phase(format!(
            "正在读取压缩包 · 已读取 {:.1} KiB",
            self.downloaded as f64 / 1024.0
        ));
        Ok(chunks)
    }
    fn prefetch(&mut self, start: u64, compressed: u64) -> Result<()> {
        if start >= self.size {
            return Err(AppError::new("ZIP 条目偏移越界"));
        }
        // 缓存中的真实局部头给出精确范围；不能以预取窗口全覆盖为命中条件，
        // 否则相邻小图明明已在内存中，仍会因为窗口尾部缺失而重新访问网络。
        let mut header = [0u8; 30];
        let cached_header =
            if let Some((offset, bytes)) = self.cache.iter().find(|(offset, bytes)| {
                start >= *offset && start + 30 <= *offset + bytes.len() as u64
            }) {
                let position = (start - *offset) as usize;
                header.copy_from_slice(&bytes[position..position + 30]);
                true
            } else {
                self.index
                    .as_ref()
                    .and_then(|index| index.read_at(start, &mut header))
                    == Some(30)
            };
        if cached_header && header[..4] == *b"PK\x03\x04" {
            let name = u64::from(u16::from_le_bytes([header[26], header[27]]));
            let extra = u64::from(u16::from_le_bytes([header[28], header[29]]));
            let end = start
                .checked_add(30 + name + extra)
                .and_then(|offset| offset.checked_add(compressed))
                .ok_or_else(|| AppError::new("ZIP 条目范围溢出"))?;
            if self
                .cache
                .iter()
                .any(|(offset, bytes)| start >= *offset && end <= *offset + bytes.len() as u64)
                || self
                    .index
                    .as_ref()
                    .is_some_and(|index| index.covers(start, end))
            {
                return Ok(());
            }
        }
        // 首次读取优先所选条目，不为相邻图片强制下载 256 KiB。
        // 常见局部头在 4 KiB 内；更长的合法名称/扩展字段由 Read 按需补齐，
        // 不能把协议允许的最大头长度当成每个文件必需下载的长度。
        let header_length = if cached_header && header[..4] == *b"PK\x03\x04" {
            30 + u64::from(u16::from_le_bytes([header[26], header[27]]))
                + u64::from(u16::from_le_bytes([header[28], header[29]]))
        } else {
            4096
        };
        let wanted = compressed
            .saturating_add(header_length)
            .clamp(4096, 1024 * 1024)
            .min(self.size - start);
        if self.cache.iter().any(|(offset, bytes)| {
            start >= *offset && start + wanted <= *offset + bytes.len() as u64
        }) {
            return Ok(());
        }
        self.position = start;
        let bytes = self.fetch(wanted as usize)?;
        while self
            .cache
            .iter()
            .map(|(_, bytes)| bytes.len())
            .sum::<usize>()
            + bytes.len()
            > 2 * 1024 * 1024
        {
            self.cache.pop_front();
        }
        self.cache.push_back((start, bytes));
        Ok(())
    }
    fn verify(&self, phase: &'static str) -> Result<()> {
        let started = Instant::now();
        let handle = self
            .handle
            .as_ref()
            .ok_or_else(|| AppError::new("ZIP 句柄已关闭"))?;
        let after = tauri::async_runtime::block_on(self.remote.checked(
            self.remote.native.sftp.fstat(handle.clone()),
            self.io.cancel,
        ))?
        .attrs;
        crate::performance_log!(
            "[TermRill archive] phase={} elapsed_ms={}",
            phase,
            started.elapsed().as_millis()
        );
        if after.size != self.before.size || after.mtime != self.before.mtime {
            return Err(AppError::new("ZIP 在读取期间发生变化，请刷新列表"));
        }
        Ok(())
    }
    fn finish(&mut self, read_success: bool) -> Result<()> {
        let result = if self.downloaded > 0 {
            self.verify("after_read")
        } else {
            Ok(())
        };
        crate::performance_log!("[TermRill archive] engine=async-range downloaded_bytes={} requests={} total_ms={} verified={}", self.downloaded, self.requests, self.started.elapsed().as_millis(), read_success && result.is_ok());
        result
    }
    fn save(&mut self) -> Saved {
        Saved {
            remote: self.remote.clone(),
            handle: self.handle.take(),
            permit: self._permit.take(),
            before: self.before.clone(),
            size: self.size,
            cache: std::mem::take(&mut self.cache),
            index: self.index.take(),
            entries: std::mem::take(&mut self.entries),
        }
    }
}
// OPEN 需传入路径；独立函数让失败的 FSTAT 也能明确关闭句柄。
fn open<'a, 'b>(io: &'a ArchiveIo<'b>, path: &str) -> Result<Reader<'a, 'b>> {
    io.check()?;
    io.valid_path(Path::new(path))?;
    let remote = io.native.ok_or_else(|| AppError::new("缺少远程连接"))?;
    let started = Instant::now();
    let (permit, handle, before) = tauri::async_runtime::block_on(async {
        let permit = remote.permit(io.cancel).await?;
        let metadata_started = Instant::now();
        // 只读属性请求并行；全部祖先确认是普通目录后才发送 OPEN。
        let checks = std::iter::once((path.to_owned(), true)).chain(
            Path::new(path)
                .ancestors()
                .skip(1)
                .map(|parent| (parent.to_string_lossy().into_owned(), false)),
        );
        let mut pending = stream::iter(checks.map(|(path, file)| async move {
            let stat = remote.stat(&path, io.cancel).await?;
            if !file && !crate::remote_async::attrs(&stat).is_dir() {
                return Err(AppError::new("ZIP 父级不是普通目录"));
            }
            Ok::<_, AppError>((file, stat))
        }))
        .buffer_unordered(16);
        let mut before = None;
        while let Some(result) = pending.next().await {
            let (file, stat) = result?;
            if file {
                before = Some(stat);
            }
        }
        crate::performance_log!(
            "[TermRill archive] phase=metadata elapsed_ms={}",
            metadata_started.elapsed().as_millis()
        );
        let before = before.ok_or_else(|| AppError::new("ZIP 属性缺失"))?;
        if !crate::remote_async::attrs(&before).is_file()
            || before.size.is_none_or(|size| size > ARCHIVE_LIMIT)
        {
            return Err(AppError::new("ZIP 文件类型或大小超出限制"));
        }
        let handle = remote
            .checked(
                remote
                    .native
                    .sftp
                    .open(path, OpenFlags::READ, FileAttributes::default()),
                io.cancel,
            )
            .await?
            .handle;
        let opened = remote
            .checked(remote.native.sftp.fstat(handle.clone()), io.cancel)
            .await;
        let valid = opened.as_ref().is_ok_and(|value| {
            value.attrs.size == before.size
                && value.attrs.mtime == before.mtime
                && crate::remote_async::attrs(&value.attrs).is_file()
        });
        if !valid {
            let _ = remote.native.sftp.close(handle).await;
            return Err(AppError::new("ZIP 打开时发生变化或无法验证"));
        }
        Ok::<_, AppError>((permit, handle, before))
    })?;
    Ok(Reader {
        remote: remote.clone(),
        io,
        handle: Some(handle),
        size: before.size.unwrap_or(0),
        before,
        position: 0,
        cache: VecDeque::new(),
        downloaded: 0,
        requests: 0,
        started,
        _permit: Some(permit),
        index: None,
        entries: Vec::new(),
        payload: None,
    })
}
impl Read for Reader<'_, '_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        self.io
            .check()
            .map_err(|error| std::io::Error::other(error.message))?;
        if output.is_empty() || self.position >= self.size {
            return Ok(0);
        }
        if let Some(count) = self
            .index
            .as_ref()
            .and_then(|index| index.read_at(self.position, output))
        {
            self.position += count as u64;
            return Ok(count);
        }
        let cached = self.cache.iter().position(|(offset, bytes)| {
            self.position >= *offset && self.position < *offset + bytes.len() as u64
        });
        let block = if let Some(index) = cached {
            self.cache
                .remove(index)
                .ok_or_else(|| std::io::Error::other("ZIP 缓存索引无效"))?
        } else {
            let length = read_ahead(
                self.position,
                self.size,
                self.payload.as_ref(),
                output.len(),
            );
            (
                self.position,
                self.fetch(length)
                    .map_err(|error| std::io::Error::other(error.message))?,
            )
        };
        let start = (self.position - block.0) as usize;
        let count = output.len().min(block.1.len() - start);
        output[..count].copy_from_slice(&block.1[start..start + count]);
        self.position += count as u64;
        while self
            .cache
            .iter()
            .map(|(_, bytes)| bytes.len())
            .sum::<usize>()
            + block.1.len()
            > 2 * 1024 * 1024
        {
            self.cache.pop_front();
        }
        self.cache.push_back(block);
        Ok(count)
    }
}
impl Seek for Reader<'_, '_> {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        let position = match from {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::End(offset) => i128::from(self.size) + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
        };
        if position < 0 || position > i128::from(self.size) {
            return Err(std::io::Error::other("ZIP 读取位置越界"));
        }
        self.position = position as u64;
        Ok(self.position)
    }
}
impl Drop for Reader<'_, '_> {
    fn drop(&mut self) {
        close(self.remote.clone(), self.handle.take(), self._permit.take());
    }
}
pub(super) fn run(request: &Request, io: &ArchiveIo<'_>, scope: &str) -> Result<Reply> {
    let reservation = request
        .snapshot
        .as_ref()
        .map(|snapshot| {
            pool::checkout(
                format!("{scope}\0{snapshot}\0{}", request.path),
                request.operation == "list",
            )
        })
        .transpose()?;
    let (lease, saved) = match reservation {
        Some((lease, saved)) => (Some(lease), saved),
        None => (None, None),
    };
    let current = io.native.ok_or_else(|| AppError::new("缺少远程连接"))?;
    let mut reader = if let Some(mut saved) =
        saved.filter(|saved| std::sync::Arc::ptr_eq(&saved.remote, current))
    {
        crate::performance_log!("[TermRill archive] phase=session_reuse");
        Reader {
            remote: saved.remote.clone(),
            io,
            handle: saved.handle.take(),
            _permit: saved.permit.take(),
            before: saved.before.clone(),
            size: saved.size,
            cache: std::mem::take(&mut saved.cache),
            index: saved.index.take(),
            entries: std::mem::take(&mut saved.entries),
            payload: None,
            position: 0,
            downloaded: 0,
            requests: 0,
            started: Instant::now(),
        }
    } else {
        open(io, &request.path)?
    };
    if request.operation != "list" {
        shared::restore(&mut reader, &request.path);
        if let Some(index) = reader.index.as_ref() {
            reader.entries = index.entries()?;
        }
    }
    // 被前台取消的预取只保留进入操作前已验证的快照，不缓存半途读取内容。
    let checkpoint = reader
        .index
        .as_ref()
        .map(|index| (index.clone(), reader.cache.clone(), reader.entries.clone()));
    let result = (|| {
        if reader.index.is_none() {
            io.phase("正在读取 ZIP 目录索引");
            let index = index::Index::load(&mut reader)?;
            reader.entries = index.entries()?;
            reader.index = Some(std::sync::Arc::new(index));
            // 刷新仍读取真实中央目录；只有新索引匹配时复用旧负载范围。
            shared::restore(&mut reader, &request.path);
        }
        let index = reader
            .index
            .as_ref()
            .ok_or_else(|| AppError::new("ZIP 索引缓存缺失"))?;
        if request
            .revision
            .as_ref()
            .is_some_and(|expected| expected != &index.revision)
        {
            return Err(AppError::new("ZIP 目录已变化，请重新读取列表"));
        }
        let mut reply = Reply {
            entries: reader.entries.clone(),
            revision: index.revision.clone(),
            message: String::new(),
            bytes: None,
        };
        if request.operation == "list" {
            list_directories(&mut reply)?;
        } else {
            validate(&reply.entries)?;
            if request.selected.len() != 1 {
                return Err(AppError::new("请选择一个包内文件"));
            }
            let entry = reply
                .entries
                .iter()
                .position(|entry| entry.name == request.selected[0] && !entry.directory)
                .ok_or_else(|| AppError::new("包内文件不存在"))?;
            io.phase("正在读取所选包内文件");
            if reply.entries[entry].size > 32 * 1024 * 1024 {
                return Err(AppError::new("包内文件超过预览上限"));
            }
            let start = reader
                .index
                .as_ref()
                .ok_or_else(|| AppError::new("ZIP 索引缺失"))?
                .start(entry)?;
            reader.prefetch(start, reply.entries[entry].compressed)?;
            // 首次预取已包含局部头固定字段；精确限制后续顺序预读，避免读取相邻大条目。
            let mut header = [0u8; 30];
            reader.seek(SeekFrom::Start(start))?;
            reader.read_exact(&mut header)?;
            if header[..4] != *b"PK\x03\x04" {
                return Err(AppError::new("ZIP 局部头签名无效"));
            }
            let header_length = 30
                + u64::from(u16::from_le_bytes([header[26], header[27]]))
                + u64::from(u16::from_le_bytes([header[28], header[29]]));
            let end = start
                .checked_add(header_length)
                .and_then(|offset| offset.checked_add(reply.entries[entry].compressed))
                .filter(|end| *end <= reader.size)
                .ok_or_else(|| AppError::new("ZIP 条目范围越界"))?;
            reader.payload = Some(start..end);
            let mut archive = ZipArchive::new(&mut reader).map_err(zip_error)?;
            let bytes = read_entry(
                &mut archive,
                entry,
                request.password.as_deref(),
                32 * 1024 * 1024,
                io,
            )?;
            reply.bytes = Some(base64::engine::general_purpose::STANDARD.encode(bytes));
        }
        Ok(reply)
    })();
    if io.cancel.load(std::sync::atomic::Ordering::Relaxed) {
        if let (Some(lease), Some((index, cache, entries))) = (lease.as_ref(), checkpoint) {
            reader.index = Some(index);
            reader.cache = cache;
            reader.entries = entries;
            pool::put(lease, reader.save());
        }
        return Err(AppError::new("操作已取消"));
    }
    let close = reader.finish(result.is_ok());
    match result {
        Ok(reply) => {
            close?;
            io.check()?;
            shared::publish(&reader, &request.path);
            if let Some(lease) = lease.as_ref() {
                pool::put(lease, reader.save());
            }
            Ok(reply)
        }
        Err(error) => Err(error),
    }
}

struct Saved {
    remote: std::sync::Arc<crate::remote_async::Remote>,
    handle: Option<String>,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    before: FileAttributes,
    size: u64,
    cache: VecDeque<(u64, Vec<u8>)>,
    index: Option<std::sync::Arc<index::Index>>,
    entries: Vec<Entry>,
}
impl Saved {
    fn memory(&self) -> usize {
        self.cache
            .iter()
            .map(|(_, bytes)| bytes.len())
            .sum::<usize>()
            + self.index.as_ref().map_or(0, |index| index.memory())
            + self
                .entries
                .iter()
                .map(|entry| entry.name.len() + std::mem::size_of::<Entry>())
                .sum::<usize>()
    }
}
fn close(
    remote: std::sync::Arc<crate::remote_async::Remote>,
    handle: Option<String>,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
) {
    if let Some(handle) = handle {
        // 清理在后台确认 CLOSE，句柄预算到确认后才释放，不阻塞面板显示。
        tauri::async_runtime::spawn(async move {
            let started = Instant::now();
            let result = remote.native.sftp.close(handle).await;
            drop(permit);
            crate::performance_log!(
                "[TermRill archive] phase=close elapsed_ms={} success={}",
                started.elapsed().as_millis(),
                result.is_ok()
            );
        });
    }
}
impl Drop for Saved {
    fn drop(&mut self) {
        close(self.remote.clone(), self.handle.take(), self.permit.take());
    }
}
pub(super) fn release(key: &str) {
    pool::release(key);
}
pub(super) fn initialize_cache(path: std::path::PathBuf) {
    shared::initialize(path);
}

#[cfg(test)]
mod tests {
    use super::read_ahead;

    #[test]
    fn tiny_decoder_buffers_keep_payload_window_but_never_read_neighbor_entries() {
        let payload = 100..3_000_100;
        assert_eq!(read_ahead(100, 8_000_000, Some(&payload), 1), 1024 * 1024);
        assert_eq!(read_ahead(3_000_000, 8_000_000, Some(&payload), 1), 100);
        assert_eq!(read_ahead(50, 8_000_000, Some(&payload), 1), 65536);
        assert_eq!(read_ahead(8_000_000, 8_000_000, None, 1), 0);
    }
}
