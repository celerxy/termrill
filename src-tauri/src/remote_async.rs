//! 浏览与文件操作共用原生异步会话，主连接锁仅用于取得认证快照。
//! 指纹先于认证；不自动重放写入，主会话断开时关闭异步通道。
use crate::{model::*, transfer::native_async::ssh::Connection as Native};
use futures::{stream, StreamExt};
use russh_sftp::{
    client::error::Error as SftpError,
    protocol::{FileAttributes, OpenFlags, StatusCode},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
#[path = "archive_read_pipeline.rs"]
mod archive_read_pipeline;
#[path = "download_control.rs"]
mod download_control;
#[path = "download_pipeline.rs"]
mod download_pipeline;
pub type Cache = Arc<tokio::sync::Mutex<Option<Arc<Remote>>>>;
struct PdfCache {
    path: String,
    version: FileAttributes,
    bytes: Arc<[u8]>,
}
#[derive(Default)]
struct ReadPlan<'a> {
    window: usize,
    snapshot: Option<&'a FileAttributes>,
    ordered: bool,
    peers: Option<&'a [Native]>,
    pause: Option<&'a AtomicBool>,
}
pub struct Remote {
    pub native: Native,
    pub(crate) download_peers: Vec<Native>,
    download_cursor: std::sync::atomic::AtomicUsize,
    download_budget: Arc<tokio::sync::Semaphore>,
    #[cfg(test)]
    pub(crate) force_download_peers: bool,
    pub closed: Arc<AtomicBool>,
    permits: Arc<tokio::sync::Semaphore>,
    reader: tokio::sync::Mutex<Option<Reader>>,
    reader_stop: Mutex<Option<(String, Arc<AtomicBool>)>>,
    // 每个已认证会话仅缓存最近一份完整 PDF，最多 16 MiB；不落盘或跨连接复用。
    pdf_cache: Arc<tokio::sync::Mutex<Option<PdfCache>>>,
}
pub fn attrs(value: &FileAttributes) -> crate::model::FileStat {
    crate::model::FileStat {
        size: value.size,
        uid: value.uid,
        gid: value.gid,
        perm: value.permissions,
        #[cfg(test)]
        atime: value.atime.map(u64::from),
        mtime: value.mtime.map(u64::from),
    }
}
pub fn missing(error: &SftpError) -> bool {
    matches!(error, SftpError::Status(status) if status.status_code == StatusCode::NoSuchFile)
}
/// 只取得预览缓存归属，不恢复浏览会话；正文租约自行验证来源及连接状态。
pub(crate) async fn preview_owner(
    primary: &Mutex<crate::ssh::Connection>,
) -> Result<Option<Arc<Remote>>> {
    let cache = lock(primary)?.remote_async.clone();
    let owner = cache.lock().await.clone();
    Ok(owner)
}
pub async fn get(
    primary: &Mutex<crate::ssh::Connection>,
    backend: &Backend,
) -> Result<Arc<Remote>> {
    let (cache, request, closed, algorithms) = {
        let conn = lock(primary)?;
        (
            conn.remote_async.clone(),
            conn.request.clone(),
            conn.closed.clone(),
            conn.host_algorithms.clone(),
        )
    };
    let trusted = lock(&backend.config)?.known_hosts.clone();
    let mut cached = cache.lock().await;
    if closed.load(Ordering::Relaxed) {
        return Err(AppError::new("连接已关闭"));
    }
    if let Some(remote) = &*cached {
        if !remote.native.is_closed() {
            return Ok(remote.clone());
        }
        // 当前操作尚未派发，失效连接可更换；不重放已经发出的读写请求。
        remote.native.shutdown().await;
    }
    let native = Native::connect(&request, &trusted, &algorithms, Some(backend)).await?;
    let remote = Remote::new(native, closed);
    *cached = Some(remote.clone());
    Ok(remote)
}
impl Remote {
    pub fn new(native: Native, closed: Arc<AtomicBool>) -> Arc<Self> {
        let remote = Arc::new(Self::owned(native, closed));
        let weak = Arc::downgrade(&remote);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let Some(remote) = weak.upgrade() else {
                    break;
                };
                if remote.closed.load(Ordering::Relaxed) {
                    *remote.pdf_cache.lock().await = None;
                    remote.native.shutdown().await;
                    break;
                }
            }
        });
        remote
    }
    /// 传输租约独占拥有连接；调用方收齐任务后归还池或关闭，不另派维护任务。
    pub(crate) fn owned(native: Native, closed: Arc<AtomicBool>) -> Self {
        let permits = Arc::new(tokio::sync::Semaphore::new(native.max_files));
        Remote {
            native,
            download_peers: Vec::new(),
            download_cursor: std::sync::atomic::AtomicUsize::new(0),
            download_budget: Arc::new(tokio::sync::Semaphore::new(download_control::BUDGET_UNITS)),
            #[cfg(test)]
            force_download_peers: false,
            closed,
            permits,
            reader: Default::default(),
            reader_stop: Default::default(),
            pdf_cache: Default::default(),
        }
    }
    pub(crate) fn set_download_peers(&mut self, peers: Vec<Native>) {
        let budget = peers.iter().fold(self.native.max_files, |budget, peer| {
            budget.min(peer.max_files)
        });
        self.permits = Arc::new(tokio::sync::Semaphore::new(budget));
        self.download_peers = peers;
    }
    /// 缓存仍限定原已认证主连接；预览租约可换 TCP 会话，缓存验证在本次租约上完成。
    pub(crate) fn share_preview_cache(&mut self, original: &Self) {
        self.pdf_cache = original.pdf_cache.clone();
    }
    /// 任务独占的所有连接累计 DATA 正文，不计协议头；半个分片到达也能推进速度样本。
    pub(crate) fn received_download_data(&self) -> u64 {
        self.download_peers
            .iter()
            .chain(std::iter::once(&self.native))
            .map(|peer| peer.received_data.load(Ordering::Relaxed))
            .sum()
    }
    pub async fn checked<T, E: Into<AppError>>(
        &self,
        future: impl std::future::Future<Output = std::result::Result<T, E>>,
        cancel: &AtomicBool,
    ) -> Result<T> {
        tokio::pin!(future);
        loop {
            if cancel.load(Ordering::Relaxed) || self.closed.load(Ordering::Relaxed) {
                return Err(AppError::new("操作已取消或连接已关闭"));
            }
            tokio::select! {
                result = &mut future => return result.map_err(Into::into),
                _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {}
            }
        }
    }
    pub async fn stat(&self, path: &str, cancel: &AtomicBool) -> Result<FileAttributes> {
        Ok(self
            .checked(self.live_native().sftp.lstat(path.to_owned()), cancel)
            .await?
            .attrs)
    }
    /// 主数据连接断开后改用仍存活的下载连接查询属性；全部断开时仍返回主连接，
    /// 让请求以连接关闭错误结束。没有下载连接的浏览和预览会话行为不变。
    fn live_native(&self) -> &Native {
        if !self.native.is_closed() {
            return &self.native;
        }
        self.download_peers
            .iter()
            .find(|peer| !peer.is_closed())
            .unwrap_or(&self.native)
    }
    /// 在一条连接上读取一个完整范围；服务器短读时在同一句柄上补足剩余部分。
    async fn read_block(
        &self,
        native: &Native,
        handle: &str,
        offset: u64,
        length: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(length);
        while bytes.len() < length {
            let response = self
                .checked(
                    native.sftp.read(
                        handle.to_owned(),
                        offset + bytes.len() as u64,
                        (length - bytes.len()) as u32,
                    ),
                    cancel,
                )
                .await?;
            if response.data.is_empty() {
                return Err(AppError::new("文件提前结束"));
            }
            bytes.extend(response.data);
            if bytes.len() > length {
                return Err(AppError::new("服务器返回超出请求范围的数据"));
            }
        }
        Ok(bytes)
    }

    async fn read_download_block(
        &self,
        native: &Native,
        handle: &str,
        range: (u64, usize),
        cancel: &AtomicBool,
        pause: Option<&AtomicBool>,
    ) -> Result<Vec<u8>> {
        let result = download_control::while_receiving(
            self.read_block(native, handle, range.0, range.1, cancel),
            || native.received_data.load(Ordering::Relaxed),
            || pause.is_some_and(|pause| pause.load(Ordering::Relaxed)),
        )
        .await;
        match result {
            Some(result) => result,
            None => {
                native.retire_download();
                crate::performance_log!("download phase=connection_stalled idle_seconds=30");
                Err(AppError::new("下载连接持续无数据，已隔离停滞连接"))
            }
        }
    }
    pub async fn read(
        &self,
        path: &str,
        limit: u64,
        cancel: &AtomicBool,
        progress: impl Fn(u64),
    ) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.read_into(
            path,
            limit,
            cancel,
            |chunk| {
                bytes.extend_from_slice(chunk);
                Ok(())
            },
            progress,
        )
        .await?;
        Ok(bytes)
    }
    /// 媒体预览有界乱序读取：每条租约最多 64 个请求，按显式偏移拼回原始文件。
    /// 沿用路径、OPEN/FSTAT、结束校验、取消及 CLOSE；不影响普通文本读取窗口。
    #[cfg(test)]
    pub async fn read_preview(
        &self,
        path: &str,
        limit: u64,
        cancel: &AtomicBool,
        progress: impl Fn(u64),
    ) -> Result<Vec<u8>> {
        self.read_preview_prepared(path, limit, cancel, progress, || async { Ok(Vec::new()) })
            .await
    }
    /// 冷读取借用额外连接；缓存命中不建连，成功收齐请求后回池，失败关闭。
    pub(crate) async fn read_preview_prepared<F>(
        &self,
        path: &str,
        limit: u64,
        cancel: &AtomicBool,
        progress: impl Fn(u64),
        prepare: impl FnOnce() -> F,
    ) -> Result<Vec<u8>>
    where
        F: std::future::Future<Output = Result<Vec<Native>>>,
    {
        let started = std::time::Instant::now();
        let before = self.stat(path, cancel).await?;
        crate::performance_log!(
            "[TermRill preview] phase=metadata elapsed_ms={}",
            started.elapsed().as_millis()
        );
        let size = before
            .size
            .ok_or_else(|| AppError::new("无法取得文件大小"))?;
        if !attrs(&before).is_file() || size > limit || size > usize::MAX as u64 {
            return Err(AppError::new("文件类型或大小超出读取限制"));
        }
        let cached = {
            let mut cache = self.pdf_cache.lock().await;
            let hit = cache
                .as_ref()
                .filter(|entry| {
                    entry.path == path
                        && before.mtime.is_some()
                        && entry.version.size == before.size
                        && entry.version.mtime == before.mtime
                })
                .map(|entry| entry.bytes.clone());
            if hit.is_none() {
                *cache = None;
            }
            hit
        };
        if let Some(bytes) = cached {
            progress(size);
            // 复用也遵守取消与源属性校验；读进度回调不能绕过结束检查。
            let after = self.stat(path, cancel).await?;
            if !attrs(&after).is_file() || after.size != before.size || after.mtime != before.mtime
            {
                *self.pdf_cache.lock().await = None;
                return Err(AppError::new("文件读取期间发生变化"));
            }
            crate::performance_log!("[TermRill preview] cache_hit=true bytes={}", size);
            return Ok(bytes.to_vec());
        }
        let prepare_started = std::time::Instant::now();
        let peers = if size >= 4 * 1024 * 1024 {
            prepare().await?
        } else {
            Vec::new()
        };
        crate::performance_log!(
            "[TermRill preview] phase=auxiliary_prepare connections={} elapsed_ms={}",
            peers.len(),
            prepare_started.elapsed().as_millis()
        );
        // 来源大小已校验，预分配一次，避免乱序响应反复扩容和补零。
        let mut bytes = vec![0; size as usize];
        let mut delivered = 0u64;
        let mut updated = std::time::Instant::now();
        let data_started = std::time::Instant::now();
        let mut first_chunk = true;
        let result = self
            .read_into_async_window(
                path,
                limit,
                cancel,
                |offset, chunk| {
                    if first_chunk {
                        crate::performance_log!(
                            "[TermRill preview] phase=first_chunk elapsed_ms={}",
                            data_started.elapsed().as_millis()
                        );
                        first_chunk = false;
                    }
                    let end = offset.checked_add(chunk.len() as u64);
                    let result = if let Some(end) = end.filter(|end| *end <= size) {
                        bytes[offset as usize..end as usize].copy_from_slice(&chunk);
                        delivered += chunk.len() as u64;
                        if updated.elapsed() >= std::time::Duration::from_millis(100) {
                            progress(delivered);
                            updated = std::time::Instant::now();
                        }
                        Ok(())
                    } else {
                        Err(AppError::new("预览读取范围超过限制"))
                    };
                    std::future::ready(result)
                },
                |_| {},
                ReadPlan {
                    window: 64,
                    snapshot: Some(&before),
                    peers: (!peers.is_empty()).then_some(peers.as_slice()),
                    ..Default::default()
                },
            )
            .await;
        crate::performance_log!(
            "[TermRill preview] phase=body_complete success={} bytes={} elapsed_ms={}",
            result.is_ok(),
            delivered,
            data_started.elapsed().as_millis()
        );
        let cleanup_started = std::time::Instant::now();
        crate::transfer::release_preview_connections(
            peers,
            result.is_ok()
                && !cancel.load(Ordering::Relaxed)
                && !self.closed.load(Ordering::Relaxed),
        )
        .await;
        crate::performance_log!(
            "[TermRill preview] phase=release_auxiliary elapsed_ms={}",
            cleanup_started.elapsed().as_millis()
        );
        result?;
        progress(delivered);
        if bytes.len() <= 16 * 1024 * 1024 && bytes.starts_with(b"%PDF-") && before.mtime.is_some()
        {
            self.checked(async { Ok::<_, SftpError>(()) }, cancel)
                .await?;
            *self.pdf_cache.lock().await = Some(PdfCache {
                path: path.to_owned(),
                version: before,
                bytes: Arc::from(bytes.as_slice()),
            });
        }
        Ok(bytes)
    }
    /// 八个有序范围请求保持网络窗口，消费端逐块处理，内存不随文件大小增长。
    pub async fn read_into(
        &self,
        path: &str,
        limit: u64,
        cancel: &AtomicBool,
        mut consume: impl FnMut(&[u8]) -> Result<()>,
        progress: impl Fn(u64),
    ) -> Result<u64> {
        self.read_into_async(
            path,
            limit,
            cancel,
            |chunk| std::future::ready(consume(&chunk)),
            progress,
        )
        .await
    }
    pub async fn read_into_async<F: std::future::Future<Output = Result<()>>>(
        &self,
        path: &str,
        limit: u64,
        cancel: &AtomicBool,
        mut consume: impl FnMut(Vec<u8>) -> F,
        progress: impl Fn(u64),
    ) -> Result<u64> {
        self.read_into_async_window(
            path,
            limit,
            cancel,
            |_, bytes| consume(bytes),
            progress,
            ReadPlan {
                window: 8,
                ordered: true,
                ..Default::default()
            },
        )
        .await
    }
    /// 按文件顺序交付完整下载内容，供续传预检计算远程摘要。
    /// 先取来源快照再走有序乱序收齐流水：使用下载分块（最高 128 KiB），
    /// 至少 4 MiB 时跨下载连接交错读取，小文件在连接间轮转。每连接 16 个请求，
    /// 收齐未消费块与在途请求合计不超过两倍窗口，四连接时单文件正文最多 16 MiB。
    pub(crate) async fn read_download_into<F: std::future::Future<Output = Result<()>>>(
        &self,
        path: &str,
        limit: u64,
        cancel: &AtomicBool,
        mut consume: impl FnMut(Vec<u8>) -> F,
    ) -> Result<u64> {
        let before = self.stat(path, cancel).await?;
        self.read_into_async_window(
            path,
            limit,
            cancel,
            |_, bytes| consume(bytes),
            |_| {},
            ReadPlan {
                window: 16,
                snapshot: Some(&before),
                ordered: true,
                ..Default::default()
            },
        )
        .await
    }
    /// 下载按完成顺序交付显式偏移，不让迟到范围阻塞其他已完成请求和窗口补充。
    pub(crate) async fn read_download_ranges<F: std::future::Future<Output = Result<()>>>(
        &self,
        path: &str,
        limit: u64,
        cancel: &AtomicBool,
        pause: &AtomicBool,
        consume: impl FnMut(u64, Vec<u8>) -> F,
    ) -> Result<u64> {
        self.read_into_async_window(
            path,
            limit,
            cancel,
            consume,
            |_| {},
            ReadPlan {
                window: 64,
                pause: Some(pause),
                ..Default::default()
            },
        )
        .await
    }
    /// 整包读取使用 32 个范围请求，普通浏览读取仍使用八请求窗口。
    pub async fn read_archive_into(
        &self,
        path: &str,
        limit: u64,
        cancel: &AtomicBool,
        mut consume: impl FnMut(&[u8]) -> Result<()>,
        progress: impl Fn(u64),
    ) -> Result<u64> {
        self.read_into_async_window(
            path,
            limit,
            cancel,
            |_, chunk| std::future::ready(consume(&chunk)),
            progress,
            ReadPlan {
                window: 32,
                ordered: true,
                ..Default::default()
            },
        )
        .await
    }
    /// 打包扫描已取得来源属性；复用此快照，OPEN 后仍核对实际句柄，读完再验证变化。
    pub(crate) async fn read_archive_source<F: std::future::Future<Output = Result<()>>>(
        &self,
        path: &str,
        before: &FileAttributes,
        limit: u64,
        cancel: &AtomicBool,
        mut consume: impl FnMut(Vec<u8>) -> F,
        window: usize,
    ) -> Result<u64> {
        self.read_into_async_window(
            path,
            limit,
            cancel,
            |_, bytes| consume(bytes),
            |_| {},
            ReadPlan {
                window,
                snapshot: Some(before),
                ordered: true,
                ..Default::default()
            },
        )
        .await
    }
    async fn read_into_async_window<F: std::future::Future<Output = Result<()>>>(
        &self,
        path: &str,
        limit: u64,
        cancel: &AtomicBool,
        mut consume: impl FnMut(u64, Vec<u8>) -> F,
        progress: impl Fn(u64),
        read_plan: ReadPlan<'_>,
    ) -> Result<u64> {
        let ReadPlan {
            window,
            snapshot,
            ordered,
            peers,
            pause,
        } = read_plan;
        let peers = peers.unwrap_or(&self.download_peers);
        let _permit = self.permit(cancel).await?;
        let before = match snapshot {
            Some(value) => value.clone(),
            None => self.stat(path, cancel).await?,
        };
        let stat = attrs(&before);
        let size = stat.size.ok_or_else(|| AppError::new("无法取得文件大小"))?;
        if !stat.is_file() || size > limit {
            return Err(AppError::new("文件类型或大小超出读取限制"));
        }
        // 小文件整文件轮转连接，大文件继续跨连接交错分片；同一文件的句柄不跨会话使用。
        // 已断开的连接不再分配新文件，其余连接继续完成任务。
        let connections: Vec<&Native> = std::iter::once(&self.native)
            .chain(peers.iter())
            .filter(|connection| !connection.is_closed())
            .collect();
        if connections.is_empty() {
            return Err(connections_lost(&self.native));
        }
        let native = if (!ordered || snapshot.is_some()) && size < 4 * 1024 * 1024 {
            connections[self.download_cursor.fetch_add(1, Ordering::Relaxed) % connections.len()]
        } else {
            connections[0]
        };
        let others: Vec<&Native> = connections
            .iter()
            .copied()
            .filter(|connection| !std::ptr::eq(*connection, native))
            .collect();
        let handle = self
            .checked(
                native
                    .sftp
                    .open(path.to_owned(), OpenFlags::READ, FileAttributes::default()),
                cancel,
            )
            .await?
            .handle;
        let mut handles = vec![(native, handle.clone())];
        let result = async {
            let opened = self
                .checked(native.sftp.fstat(handle.clone()), cancel)
                .await?
                .attrs;
            if !attrs(&opened).is_file()
                || opened.size != before.size
                || opened.mtime != before.mtime
            {
                return Err(AppError::new("文件在打开时已变化"));
            }
            let parallel = size >= 4 * 1024 * 1024;
            #[cfg(test)]
            let parallel = parallel || self.force_download_peers;
            if (!ordered || snapshot.is_some()) && parallel {
                // 同一份来源的额外 OPEN 并行派发，成功句柄全部登记后再并行 FSTAT。
                // 即使其中一个失败，也收齐其他 OPEN 的结果，让结束清理覆盖所有句柄。
                // 打开期间才断开的辅助连接直接跳过，不作为整个文件的失败。
                let opened_peers = futures::future::join_all(others.iter().map(|&peer| async move {
                    (peer, self.checked(peer.sftp.open(path.to_owned(), OpenFlags::READ,
                        FileAttributes::default()), cancel).await)
                })).await;
                let mut open_error = None;
                for (peer, result) in opened_peers {
                    match result {
                        Ok(response) => handles.push((peer, response.handle)),
                        Err(_) if peer.is_closed() => {}
                        Err(error) => { open_error.get_or_insert(error); }
                    }
                }
                if let Some(error) = open_error { return Err(error); }
                let versions = futures::future::join_all(handles.iter().skip(1).map(|(peer, handle)|
                    self.checked(peer.sftp.fstat(handle.clone()), cancel)
                )).await;
                for ((peer, _), version) in handles.iter().skip(1).zip(versions) {
                    let actual = match version {
                        Ok(response) => response.attrs,
                        Err(_) if peer.is_closed() => continue,
                        Err(error) => return Err(error),
                    };
                    if !attrs(&actual).is_file()
                        || actual.size != opened.size
                        || actual.mtime != opened.mtime
                    {
                        return Err(AppError::new("多连接打开的源文件版本不一致"));
                    }
                }
                // 断开连接上的句柄随会话失效，无需也无法 CLOSE。
                handles.retain(|(connection, _)| !connection.is_closed());
                if handles.is_empty() {
                    return Err(connections_lost(native));
                }
            }
            let block = if !ordered || snapshot.is_some() {
                handles
                    .iter()
                    .fold(native.download_read_size, |size, (peer, _)| {
                        size.min(peer.download_read_size)
                    })
            } else if window > 8 {
                self.native.archive_read_size
            } else {
                self.native.read_size.min(32768)
            } as u64;
            let handles = &handles;
            // 所属连接断开时，把这一整块改到下一条存活连接的句柄重新读取，丢弃已收到的
            // 部分正文；只在连接确已关闭时转交，服务器状态错误、短文件和取消仍按原错误结束。
            // 转交请求不占用目标连接的调度窗口，额外在途量不超过断开连接原有的窗口。
            let request_on = |slot: usize, offset: u64| async move {
                let length = (size - offset).min(block) as usize;
                let mut slot = slot;
                loop {
                    let (native, handle) = &handles[slot];
                    let result = if !ordered && snapshot.is_none() {
                        self.read_download_block(native, handle, (offset, length), cancel, pause).await
                    } else {
                        self.read_block(native, handle, offset, length, cancel).await
                    };
                    match result {
                        Ok(bytes) => return Ok::<_, AppError>((offset, bytes)),
                        Err(_) if native.is_closed() => {
                            let Some(next) = live_slot(handles, slot) else {
                                return Err(connections_lost(native));
                            };
                            crate::performance_log!(
                                "download phase=range_failover from_connection={} to_connection={} offset={} reason={}",
                                slot, next, offset, native.disconnect_reason()
                            );
                            slot = next;
                        }
                        Err(error) => return Err(error),
                    }
                }
            };
            let available = |slot: usize| !handles[slot].0.is_closed();
            let request = |offset: u64| request_on((offset / block) as usize % handles.len(), offset);
            // 所有普通下载文件共用 32 MiB 在途预算；获取预算不计为网络等待。
            // 许可随正文持有至消费者收下该块，失败/取消自动释放，不涉及预览/打包。
            let request_download = |slot, offset| async move {
                let permit = if !ordered && snapshot.is_none() {
                    let units = (block as usize).div_ceil(download_control::BUDGET_UNIT) as u32;
                    let reserve = async {
                        self.download_budget.clone().acquire_many_owned(units).await
                            .map_err(|_| AppError::new("下载正文预算已关闭"))
                    };
                    Some(self.checked(reserve, cancel).await?)
                } else {
                    None
                };
                let (offset, bytes) = request_on(slot, offset).await?;
                Ok::<_, AppError>((offset, bytes, permit))
            };
            // 无序且有来源快照的调用是 PDF/图片预览；普通下载使用独立的起步与缩窗阈值。
            let policy = if snapshot.is_some() && !ordered && size >= 4 * 1024 * 1024 {
                download_pipeline::WindowPolicy::BulkPreview
            } else if snapshot.is_some() { download_pipeline::WindowPolicy::Preview }
                else { download_pipeline::WindowPolicy::Transfer };
            let mut chunks = if ordered && snapshot.is_some() {
                let total_window = window * handles.len();
                crate::performance_log!("[TermRill archive] phase=source_read_plan connections={} requests={} buffered_blocks={} block_bytes={}", handles.len(), total_window, total_window * 2, block);
                archive_read_pipeline::ranges(size, block, total_window, &request)
                    .map(|chunk| chunk.map(|(offset, bytes)| (offset, bytes, None))).boxed()
            } else if ordered {
                stream::iter((0..size).step_by(block as usize).map(&request))
                    .buffered(window)
                    .map(|chunk| chunk.map(|(offset, bytes)| (offset, bytes, None))).boxed()
            } else {
                // 共享未派发范围，快连接领取更多工作；不重放慢连接已经发出的 READ。
                // 四连接最多 4×64×128 KiB=32 MiB 范围正文，不含写盘队列和摘要缓存。
                download_pipeline::ranges(size, block, handles.len(), window, policy, &request_download, &available)
            };
            let ordered_window = if snapshot.is_some() { window * handles.len() } else { window };
            let initial_requests = if ordered { window } else { policy.initial(handles.len(), window) };
            crate::performance_log!("download phase=read_plan connections={} requests_per_connection={} total_request_limit={} block_bytes={} window_policy={} initial_requests_per_connection={} initial_total_requests={}",
                handles.len(), window, if ordered { ordered_window } else { window * handles.len() }, block,
                if ordered { "ordered" } else { policy.name() }, initial_requests,
                if ordered { ordered_window } else { initial_requests * handles.len() });
            let mut total = 0u64;
            while let Some(chunk) = chunks.next().await {
                let (offset, chunk, permit) = chunk?;
                let length = chunk.len();
                consume(offset, chunk).await?;
                drop(permit);
                total += length as u64;
                progress(total);
            }
            // 调度端在没有存活连接时会提前结束，不能把不完整的范围当作读完。
            if total != size {
                return Err(connections_lost(handles[0].0));
            }
            // 各独占会话可以同时确认来源版本，避免完整读取后再串行付出多个 RTT。
            // 收齐全部响应再返回错误，成功租约不能遗留尚未完成的属性请求。
            // 读取期间断开的连接跳过，至少一条存活连接确认来源版本未变。
            let versions = futures::future::join_all(handles.iter().map(|(native, handle)| {
                self.checked(native.sftp.fstat(handle.clone()), cancel)
            }))
            .await;
            let mut confirmed = false;
            for ((native, _), version) in handles.iter().zip(versions) {
                let after = match version {
                    Ok(response) => response.attrs,
                    Err(_) if native.is_closed() => continue,
                    Err(error) => return Err(error),
                };
                if after.size != opened.size || after.mtime != opened.mtime {
                    return Err(AppError::new("文件读取期间发生变化"));
                }
                confirmed = true;
            }
            if !confirmed {
                return Err(connections_lost(handles[0].0));
            }
            Ok(total)
        }
        .await;
        // 已打开的每条连接均需 CLOSE，包括后续连接打开或 FSTAT 失败的情况。
        // 已断开连接上的句柄随会话失效，跳过 CLOSE，其关闭失败也不影响结果。
        let mut closes = futures::stream::FuturesUnordered::new();
        for (native, handle) in handles {
            if native.is_closed() {
                continue;
            }
            closes.push(async move {
                let closed = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    native.sftp.close(handle),
                )
                .await;
                (native, closed)
            });
        }
        let mut close_error = None;
        while let Some((native, close)) = closes.next().await {
            match close {
                Ok(Ok(_)) => {}
                _ if native.is_closed() => {}
                Ok(Err(error)) => {
                    close_error.get_or_insert(AppError::from(error));
                }
                Err(_) => {
                    close_error.get_or_insert(AppError::new("关闭读取句柄超时"));
                }
            }
        }
        match result {
            Ok(bytes) => match close_error {
                Some(error) => Err(error),
                None => Ok(bytes),
            },
            Err(error) => Err(error),
        }
    }
}

/// 从 `slot` 之后按顺序找下一条未断开的连接，不返回 `slot` 自身。
fn live_slot(handles: &[(&Native, String)], slot: usize) -> Option<usize> {
    (1..handles.len())
        .map(|step| (slot + step) % handles.len())
        .find(|&candidate| !handles[candidate].0.is_closed())
}

/// 所有数据连接都已断开时的错误，带上最后一条连接记录的断开原因。
fn connections_lost(native: &Native) -> AppError {
    AppError::new(format!(
        "下载数据连接已断开，没有可继续读取的连接：{}",
        native.disconnect_reason()
    ))
}

struct Reader {
    request: String,
    path: String,
    handle: String,
    size: u64,
    mtime: Option<u32>,
    next: u64,
    verified: u64,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
impl Remote {
    /// 协调器不等待句柄预算，避免待打开目录占满任务槽而阻塞已有目录的读取和关闭。
    pub(crate) fn try_permit(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        self.permits.clone().try_acquire_owned().ok()
    }
    pub(crate) async fn permit(
        &self,
        cancel: &AtomicBool,
    ) -> Result<tokio::sync::OwnedSemaphorePermit> {
        self.checked(
            async {
                self.permits
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| AppError::new("文件句柄预算已关闭"))
            },
            cancel,
        )
        .await
    }
}

#[path = "remote_chunks.rs"]
mod chunks;
#[path = "remote_listing.rs"]
mod listing;
#[path = "remote_write.rs"]
mod write;
