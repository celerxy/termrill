//! russh 原生异步上传：所有文件大小共享协程调度，WRITE 确认与下一块发送重叠。
//! 本模块只在全目录冲突预检成功后写入；确认关闭、源验证、安装依次构成提交边界。
use super::*;
use futures::{stream::FuturesUnordered, FutureExt, StreamExt};
use russh_sftp::{
    client::{error::Error as SftpError, rawsession::RawSftpSession},
    protocol::{FileAttributes, OpenFlags as Flags, StatusCode},
};
use std::time::Duration;
use tokio::io::AsyncReadExt;
#[path = "transfer_async_pool.rs"]
pub(super) mod pool;

pub(super) fn maintain_pool(
    primary: Arc<std::sync::Mutex<crate::ssh::Connection>>,
    backend: Arc<Backend>,
) {
    pool::maintain(primary, backend);
}
#[path = "transfer_async_precheck.rs"]
mod precheck;
#[path = "transfer_async_ssh.rs"]
pub(crate) mod ssh;

const BLOCK: usize = 32 * 1024;
// 每文件最多 2 MiB 在途数据；较高往返延迟下仍可持续填充发送窗口。
const WRITES: usize = 64;
enum Progress {
    Bytes(usize),
    BeforeInstall(tokio::sync::oneshot::Sender<()>),
}

fn remote(path: &Path) -> Result<String> {
    let path = path
        .to_str()
        .ok_or_else(|| AppError::new("上传路径不是有效 UTF-8"))?;
    if path.contains('\0') {
        return Err(AppError::new("上传路径含 NUL"));
    }
    Ok(path.replace('\\', "/"))
}

fn regular(attrs: &FileAttributes) -> bool {
    attrs
        .permissions
        .is_some_and(|mode| mode & 0o170000 == 0o100000)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute(
    job: &Job,
    items: &mut [(usize, Item)],
    connection: &std::sync::Mutex<crate::ssh::Connection>,
    backend: Option<&Backend>,
    event: &mut TransferEvent,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    send: &dyn Fn(&TransferEvent),
    checkpoint: &dyn Fn(&str, String) -> Result<()>,
    limiter: Option<&crate::transfer_rate::RateLimiter>,
) -> Result<()> {
    let (request, closed, mut trusted, algorithms) = {
        let conn = lock(connection)?;
        #[cfg(not(test))]
        let trusted = conn.trusted.clone();
        #[cfg(test)]
        let trusted = {
            let mut trusted = std::collections::BTreeMap::new();
            let endpoint = &conn.request.profile.target;
            let (key, _) = conn
                .session
                .host_key()
                .ok_or_else(|| AppError::new("已连接会话缺少主机密钥"))?;
            use base64::Engine;
            trusted.insert(
                format!("[{}]:{}", endpoint.host, endpoint.port),
                format!(
                    "SHA256:{}",
                    base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(key))
                ),
            );
            trusted
        };
        (
            conn.request.clone(),
            conn.closed.clone(),
            trusted,
            conn.host_algorithms.clone(),
        )
    };
    if let Some(backend) = backend {
        trusted = lock(&backend.config)?.known_hosts.clone();
    }
    let trusted = pool::session_trust(&request, &trusted);
    let count = backend
        .map(|backend| {
            lock(&backend.config).map(|config| config.transfer_settings.concurrency.clamp(1, 4))
        })
        .transpose()?
        .unwrap_or(1);
    let started = Instant::now();
    event.status = "connecting".into();
    send(event);
    let reused = pool::take(&closed, &trusted)?;
    let reuse = reused.is_some();
    let native = if let Some(native) = reused {
        native
    } else {
        controlled(
            pool::tracked(
                ssh::Connection::connect(&request, &trusted, &algorithms, backend),
                &closed,
                &trusted,
            ),
            cancel,
            &closed,
        )
        .await?
    };
    let count = count.min(native.max_files);
    crate::performance_log!(
        "[TermRill transfer] task={} transport=russh phase=connections reused={reuse} elapsed_ms={}",
        job.id,
        started.elapsed().as_millis()
    );
    let unlimited = crate::transfer_rate::RateLimiter::new(0);
    let limiter = limiter.unwrap_or(&unlimited);
    let touched = std::sync::Mutex::new(std::collections::BTreeSet::<String>::new());
    let outstanding = std::sync::atomic::AtomicUsize::new(0);
    let result = async {
        event.status = "prechecking".into();
        send(event);
        precheck::execute(&native, job, items, event, cancel, &closed, send).await?;
        let directory_start = Instant::now();
        event.status = "running".into();
        send(event);
        event.status = "preparing".into();
        send(event);
        // MKDIR 不占文件句柄，同层使用独立的有界协议请求窗口。
        prepare_directories(&native, items, cancel, &closed, 16).await?;
        crate::performance_log!("[TermRill transfer] task={} transport=russh phase=prepare_directories elapsed_ms={}", job.id, directory_start.elapsed().as_millis());
        event.status = "running".into();
        send(event);
        let mut files = items.iter().filter(|(_, item)| !item.directory && !job.completed.contains_key(&item.remote.to_string_lossy().to_string())).collect::<Vec<_>>();
        files.sort_by_key(|(_, item)| std::cmp::Reverse(item.size));
        let (small, large): (Vec<_>, Vec<_>) = files.into_iter().partition(|(_, item)| item.size <= schedule::SMALL_BYTES);
        let mut small = schedule::interleave(small);
        let mut large = std::collections::VecDeque::from(large);
        let mut small_window = schedule::SmallWindow::new(native.max_files);
        let mut active_small = 0usize;
        let mut active_large = 0usize;
        let (progress, mut receiver) = tokio::sync::mpsc::channel::<Progress>(64);
        let stopped = AtomicBool::new(false);
        let mut running = FuturesUnordered::new();
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        let mut last = Instant::now();
        let data_started = Instant::now();
        let initial_bytes = event.bytes;
        let mut diagnostic = Instant::now();
        let mut speed_started = Instant::now();
        let mut speed_bytes = event.bytes;
        let mut failure = None;
        let mut installed_files = 0usize;
        let mut estimate_started = Instant::now();
        loop {
            while failure.is_none() && !pause.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) && !closed.load(Ordering::Relaxed) && running.len() < native.max_files {
                // 优先保留原大文件名额，剩余句柄供小文件协程隐藏打开和收尾等待。
                let selected = if active_large < count && !large.is_empty() { large.pop_front().map(|item| (false, item)) }
                    else if active_small < small_window.limit { small.pop_front().map(|item| (true, item)) } else { None };
                let Some((is_small, (index, item))) = selected else { break; };
                if is_small { active_small += 1; } else { active_large += 1; }
                let stopped = &stopped;
                let native = &native;
                let closed = &closed;
                let progress = &progress;
                let touched = &touched;
                let outstanding = &outstanding;
                running.push(async move {
                    let started = Instant::now();
                    let result = upload_one(native, job, *index, item, stopped, closed, pause, limiter, progress, touched, outstanding).await;
                    (is_small, started.elapsed(), result)
                });
            }
            if running.is_empty() {
                if (!small.is_empty() || !large.is_empty()) && failure.is_none() && pause.load(Ordering::Relaxed) {
                    cancelled(cancel, &closed)?;
                    event.status = "paused".into();
                    event.remaining_seconds = None;
                    estimate_started = Instant::now();
                    installed_files = 0;
                    send(event);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                break;
            }
            tokio::select! {
                result = running.next() => {
                    let (is_small, elapsed, result) = result.expect("上面已检查非空");
                    if is_small { active_small -= 1; if result.is_ok() { small_window.completed(elapsed); } } else { active_large -= 1; }
                    match result {
                        Ok((path, digest)) => { installed_files += 1; if let Err(error) = checkpoint(&path, digest) { failure.get_or_insert(error); stopped.store(true, Ordering::Relaxed); } },
                        Err(error) => { failure.get_or_insert(error); stopped.store(true, Ordering::Relaxed); }
                    }
                }
                bytes = receiver.recv() => {
                    match bytes.expect("协调器持有 sender") {
                        Progress::Bytes(bytes) => { event.bytes += bytes as u64; },
                        Progress::BeforeInstall(reply) => {
                            send(event);
                            if let Err(error) = cancelled(cancel, &closed) { failure.get_or_insert(error); stopped.store(true, Ordering::Relaxed); }
                            let _ = reply.send(());
                        }
                    }
                }
                _ = tick.tick() => { if let Err(error) = cancelled(cancel, &closed) { failure.get_or_insert(error); stopped.store(true, Ordering::Relaxed); } }
            }
            small_window.adjust(!small.is_empty() && !pause.load(Ordering::Relaxed));
            if last.elapsed().as_millis() >= 100 {
                // 已发送的 WRITE 必须收齐确认，才向界面报告真正暂停。
                event.status = if pause.load(Ordering::Relaxed) && outstanding.load(Ordering::Relaxed) == 0 { "paused" } else { "running" }.into();
                // 每秒按已确认字节计算当前速度，准备和暂停时间不稀释传输速度。
                if pause.load(Ordering::Relaxed) {
                    estimate_started = Instant::now();
                    installed_files = 0;
                    event.remaining_seconds = None;
                    event.speed = 0;
                    speed_started = Instant::now();
                    speed_bytes = event.bytes;
                } else if speed_started.elapsed() >= Duration::from_secs(1) {
                    event.speed = (event.bytes.saturating_sub(speed_bytes) as f64 / speed_started.elapsed().as_secs_f64()) as u64;
                    speed_started = Instant::now();
                    speed_bytes = event.bytes;
                }
                if !pause.load(Ordering::Relaxed) {
                    let remaining_files = small.len() + large.len() + running.len();
                    event.remaining_seconds = if installed_files >= 4 && estimate_started.elapsed() >= Duration::from_secs(2) {
                        let file_seconds = remaining_files as f64 * estimate_started.elapsed().as_secs_f64() / installed_files as f64;
                        let byte_seconds = if event.speed > 0 { event.total.saturating_sub(event.bytes) as f64 / event.speed as f64 } else { 0.0 };
                        Some(file_seconds.max(byte_seconds).ceil() as u64)
                    } else { None };
                }
                send(event);
                last = Instant::now();
            }
            if diagnostic.elapsed() >= Duration::from_secs(2) {
                crate::performance_log!("[TermRill transfer] task={} transport=russh phase=write_progress confirmed_bytes={} outstanding={} small_limit={} active_small={} active_large={} elapsed_ms={}", job.id, event.bytes.saturating_sub(initial_bytes), outstanding.load(Ordering::Relaxed), small_window.limit, active_small, active_large, data_started.elapsed().as_millis());
                diagnostic = Instant::now();
            }
        }
        while let Ok(Progress::Bytes(bytes)) = receiver.try_recv() { event.bytes += bytes as u64; }
        event.remaining_seconds = None;
        send(event);
        match failure { Some(error) => Err(error), None => cancelled(cancel, &closed) }
    }.await;
    if cancel.load(Ordering::Relaxed) && !closed.load(Ordering::Relaxed) {
        // 只清理本次确实打开并校验过的临时文件；排队未领取的旧续传文件不能删除。
        let paths = lock(&touched)?.iter().cloned().collect::<Vec<_>>();
        for path in paths {
            let _ = tokio::time::timeout(Duration::from_secs(5), native.sftp.remove(path)).await;
        }
    }
    // 失败后关闭整个专用通道，释放在途文件句柄，不重放任何写入/改名。
    if result.is_err() {
        crate::performance_log!(
            "[TermRill transfer] task={} phase=failure stage={} cancelled={} disconnected={}",
            job.id,
            event.status,
            cancel.load(Ordering::Relaxed),
            closed.load(Ordering::Relaxed)
        );
    }
    let cleanup_started = Instant::now();
    if result.is_ok() && !cancel.load(Ordering::Relaxed) && !closed.load(Ordering::Relaxed) {
        pool::put(native, closed, trusted).await?;
    } else {
        native.close().await;
    }
    crate::performance_log!(
        "[TermRill transfer] task={} transport=russh phase=cleanup elapsed_ms={}",
        job.id,
        cleanup_started.elapsed().as_millis()
    );
    crate::performance_log!("[TermRill transfer] task={} transport=russh phase=total success={} files={} window={WRITES} elapsed_ms={}", job.id, result.is_ok(), items.len(), started.elapsed().as_millis());
    result
}

/// 父层全部创建成功后才处理下一层；同层请求并发，保留父子依赖与链接检查。
async fn prepare_directories(
    native: &ssh::Connection,
    items: &[(usize, Item)],
    cancel: &AtomicBool,
    closed: &AtomicBool,
    count: usize,
) -> Result<()> {
    let mut levels = std::collections::BTreeMap::<usize, Vec<String>>::new();
    for (_, item) in items
        .iter()
        .filter(|(_, item)| item.directory && item.remote_target.is_none())
    {
        levels
            .entry(item.remote.components().count())
            .or_default()
            .push(remote(&item.remote)?);
    }
    for paths in levels.into_values() {
        let mut requests = futures::stream::iter(paths.into_iter().map(|path| async move {
            cancelled(cancel, closed)?;
            if let Err(error) = controlled(
                native.sftp.mkdir(
                    path.clone(),
                    FileAttributes {
                        permissions: Some(0o755),
                        ..Default::default()
                    },
                ),
                cancel,
                closed,
            )
            .await
            {
                // 并发创建竞态只接受真实目录，不能接受末级链接。
                let attrs = controlled(native.sftp.lstat(path), cancel, closed)
                    .await?
                    .attrs;
                if attrs
                    .permissions
                    .is_none_or(|mode| mode & 0o170000 != 0o040000)
                {
                    return Err(error);
                }
            }
            Ok::<_, AppError>(())
        }))
        .buffer_unordered(count);
        // 任一失败即停止派发；外层释放专用连接，不继续创建下一层。
        let mut failure = None;
        while let Some(result) = requests.next().await {
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
            if failure.is_some() {
                break;
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
    }
    Ok(())
}

/// 为单个 await 增加取消观察，不用轮询或重试 SSH 库调用。
pub(super) async fn controlled<T, E: Into<AppError>>(
    future: impl std::future::Future<Output = std::result::Result<T, E>>,
    cancel: &AtomicBool,
    closed: &AtomicBool,
) -> Result<T> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            value = &mut future => return value.map_err(Into::into),
            _ = tokio::time::sleep(Duration::from_millis(20)) => { cancelled(cancel, closed)?; }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn upload_one(
    native: &ssh::Connection,
    job: &Job,
    index: usize,
    item: &Item,
    cancel: &AtomicBool,
    closed: &AtomicBool,
    pause: &AtomicBool,
    limiter: &crate::transfer_rate::RateLimiter,
    progress: &tokio::sync::mpsc::Sender<Progress>,
    touched: &std::sync::Mutex<std::collections::BTreeSet<String>>,
    outstanding: &std::sync::atomic::AtomicUsize,
) -> Result<(String, String)> {
    let target = remote(&item.remote)?;
    let temporary = remote(&item.remote.with_file_name(job.temporary_name(index)))?;
    if let Some(link) = &item.link {
        if link_digest(&tokio::fs::read_link(&item.local).await?) != link_digest(link) {
            return Err(AppError::new("源链接目标已变化"));
        }
        let temporary = remote(
            &item
                .remote
                .with_file_name(format!(".termrill-link-{}", identifier())),
        )?;
        controlled(
            // OpenSSH/libssh2 的线上顺序为目标、链接路径；raw API 按字段顺序编码。
            native.sftp.symlink(remote(link)?, temporary.clone()),
            cancel,
            closed,
        )
        .await?;
        lock(touched)?.insert(temporary.clone());
        let (reply, acknowledged) = tokio::sync::oneshot::channel();
        progress
            .send(Progress::BeforeInstall(reply))
            .await
            .map_err(|_| AppError::new("上传进度接收器已关闭"))?;
        acknowledged
            .await
            .map_err(|_| AppError::new("上传提交确认入口已关闭"))?;
        cancelled(cancel, closed)?;
        if let Err(error) = install(native, &temporary, &target, item, job.overwrite).await {
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                native.sftp.remove(temporary.clone()),
            )
            .await;
            return Err(error);
        }
        lock(touched)?.remove(&temporary);
        return Ok((
            item.remote.to_string_lossy().into_owned(),
            link_digest(link),
        ));
    }
    let mut source = tokio::fs::File::open(&item.local).await?;
    let original = source.metadata().await?;
    if !original.is_file() || original.len() != item.size {
        return Err(AppError::new("源文件在扫描后已变化"));
    }
    let mut digest = Sha256::new();
    let open_started = Instant::now();
    let (handle, mut offset) = open_temporary(
        native,
        &temporary,
        &mut source,
        item.size,
        &mut digest,
        cancel,
        closed,
        progress,
    )
    .await?;
    crate::performance_log!("[TermRill transfer] task={} file_index={index} phase=open_temporary elapsed_ms={} resumed_bytes={offset}", job.id, open_started.elapsed().as_millis());
    lock(touched)?.insert(temporary.clone());
    let result = async {
        let write_started = Instant::now();
        let initial_offset = offset;
        let mut writes = FuturesUnordered::new();
        while offset < item.size || !writes.is_empty() {
            while offset < item.size && writes.len() < WRITES && !pause.load(Ordering::Relaxed) {
                let count = limiter
                    .block_size(native.write_size)
                    .min((item.size - offset).min(BLOCK as u64) as usize);
                let mut data = vec![0; count];
                source.read_exact(&mut data).await?;
                digest.update(&data);
                let at = offset;
                offset += count as u64;
                let handle = handle.clone();
                struct WriteGuard<'a>(&'a std::sync::atomic::AtomicUsize);
                impl Drop for WriteGuard<'_> {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                writes.push(async move {
                    // 限速在协程真正发送前等待，让其他已发送块继续收确认。
                    limiter.wait_async(count, cancel, closed, pause).await?;
                    outstanding.fetch_add(1, Ordering::Relaxed);
                    let guard = WriteGuard(outstanding);
                    let _guard = guard;
                    native
                        .sftp
                        .write(handle, at, data)
                        .await
                        .map(|_| count)
                        .map_err(AppError::from)
                });
            }
            if let Some(result) =
                controlled(writes.next().map(Ok::<_, AppError>), cancel, closed).await?
            {
                let bytes = result?;
                progress
                    .send(Progress::Bytes(bytes))
                    .await
                    .map_err(|_| AppError::new("上传进度接收器已关闭"))?;
            } else if offset < item.size {
                cancelled(cancel, closed)?;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        crate::performance_log!("[TermRill transfer] task={} file_index={index} phase=write_complete bytes={} elapsed_ms={}", job.id, offset - initial_offset, write_started.elapsed().as_millis());
        let close_started = Instant::now();
        controlled(native.sftp.close(handle.clone()), cancel, closed).await.map_err(|error| install_stage_error("关闭上传文件", error))?;
        crate::performance_log!("[TermRill transfer] task={} file_index={index} phase=close_file elapsed_ms={}", job.id, close_started.elapsed().as_millis());
        while pause.load(Ordering::Relaxed) {
            cancelled(cancel, closed)?;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let current = source.metadata().await?;
        if current.len() != original.len() || current.modified()? != original.modified()? {
            return Err(AppError::new("源文件在传输中已变化，未替换目标文件"));
        }
        let (reply, acknowledged) = tokio::sync::oneshot::channel();
        progress
            .send(Progress::BeforeInstall(reply))
            .await
            .map_err(|_| AppError::new("上传进度接收器已关闭"))?;
        acknowledged
            .await
            .map_err(|_| AppError::new("上传提交确认入口已关闭"))?;
        cancelled(cancel, closed)?;
        // 进入提交阶段后不取消改名/恢复，避免取消恰好留下被移走的旧目标。
        let current = source.metadata().await?;
        if current.len() != original.len() || current.modified()? != original.modified()? {
            return Err(AppError::new("源文件在提交前已变化，未替换目标文件"));
        }
        let install_started = Instant::now();
        install(native, &temporary, &target, item, job.overwrite).await?;
        crate::performance_log!("[TermRill transfer] task={} file_index={index} phase=install elapsed_ms={}", job.id, install_started.elapsed().as_millis());
        lock(touched)?.remove(&temporary);
        Ok((
            item.remote.to_string_lossy().into_owned(),
            format!("{:x}", digest.finalize()),
        ))
    }
    .await;
    if result.is_err() {
        // 取消时协调器还会关闭整个通道；仅显式失败尝试关闭，保留临时文件用于续传。
        let _ = tokio::time::timeout(Duration::from_secs(5), native.sftp.close(handle)).await;
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn open_temporary(
    native: &ssh::Connection,
    temporary: &str,
    source: &mut tokio::fs::File,
    size: u64,
    digest: &mut Sha256,
    cancel: &AtomicBool,
    closed: &AtomicBool,
    progress: &tokio::sync::mpsc::Sender<Progress>,
) -> Result<(String, u64)> {
    let sftp = &native.sftp;
    let fresh = controlled(
        sftp.open(
            temporary,
            Flags::WRITE | Flags::CREATE | Flags::EXCLUDE,
            FileAttributes {
                permissions: Some(0o644),
                ..Default::default()
            },
        ),
        cancel,
        closed,
    )
    .await;
    if let Ok(handle) = fresh {
        return Ok((handle.handle, 0));
    }
    let attrs = controlled(sftp.lstat(temporary), cancel, closed)
        .await?
        .attrs;
    if !regular(&attrs) || attrs.size.is_none_or(|bytes| bytes > size) {
        return Err(AppError::new("续传临时文件类型或大小无效"));
    }
    let handle = controlled(
        sftp.open(
            temporary,
            Flags::READ | Flags::WRITE,
            FileAttributes::default(),
        ),
        cancel,
        closed,
    )
    .await?
    .handle;
    let result = async {
        let mut offset = 0;
        while offset < attrs.size.expect("上面已检查存在") {
            let data = controlled(
                sftp.read(
                    handle.clone(),
                    offset,
                    native
                        .read_size
                        .min((attrs.size.unwrap_or(0) - offset) as usize)
                        as u32,
                ),
                cancel,
                closed,
            )
            .await?
            .data;
            if data.is_empty()
                || data.len() > native.read_size
                || data.len() as u64 > attrs.size.unwrap_or(0) - offset
            {
                return Err(AppError::new("续传临时文件提前结束"));
            }
            let mut local = vec![0; data.len()];
            source.read_exact(&mut local).await?;
            if local != data {
                return Err(AppError::new("续传前缀与源文件不一致，未写入目标"));
            }
            digest.update(&local);
            progress
                .send(Progress::Bytes(local.len()))
                .await
                .map_err(|_| AppError::new("上传进度接收器已关闭"))?;
            offset += local.len() as u64;
        }
        Ok((handle.clone(), offset))
    }
    .await;
    if result.is_err() {
        let _ = tokio::time::timeout(Duration::from_secs(5), sftp.close(handle)).await;
    }
    result
}

pub(crate) async fn install(
    native: &ssh::Connection,
    temporary: &str,
    target: &str,
    item: &Item,
    overwrite: bool,
) -> Result<()> {
    let sftp = &native.sftp;
    let previous = if item.link.is_some() {
        match sftp.lstat(target).await {
            Ok(attrs)
                if attrs
                    .attrs
                    .permissions
                    .is_some_and(|mode| mode & 0o170000 == 0o120000) =>
            {
                Some(crate::model::FileStat {
                    perm: attrs.attrs.permissions,
                    size: attrs.attrs.size,
                    uid: attrs.attrs.uid,
                    gid: attrs.attrs.gid,
                    #[cfg(test)]
                    atime: None,
                    mtime: None,
                })
            }
            Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => None,
            Ok(_) => return Err(AppError::new("覆盖目标已不是符号链接")),
            Err(error) => return Err(error.into()),
        }
    } else {
        item.remote_target.clone()
    };
    let Some(previous) = previous else {
        sftp.rename(temporary, target)
            .await
            .map_err(|error| install_stage_error("安装目标文件", error))?;
        return Ok(());
    };
    if !overwrite {
        return Err(AppError::new("目标已存在，未覆盖"));
    }
    if item.link.is_none() && !previous.is_file() {
        return Err(AppError::new("覆盖目标不是普通文件"));
    }
    if item.link.is_none() {
        if let Some(mode) = previous.perm {
            let attributes = sftp
                .lstat(temporary)
                .await
                .map_err(|error| install_stage_error("核对上传临时文件", error))?
                .attrs;
            if !regular(&attributes) || attributes.size != Some(item.size) {
                return Err(AppError::new("上传临时文件类型或大小已变化，未替换目标"));
            }
            // 服务器可能允许写入/改名但不支持 chmod；权限已经一致时不重复设置。
            if attributes
                .permissions
                .is_none_or(|current| current & 0o777 != mode & 0o777)
            {
                sftp.setstat(
                    temporary,
                    FileAttributes {
                        permissions: Some(mode & 0o777),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|error| install_stage_error("保留目标文件权限", error))?;
            }
        }
    }
    if native.posix && item.link.is_none() {
        let mut packet = Vec::new();
        crate::sftp_packet::push_string(&mut packet, temporary.as_bytes())?;
        crate::sftp_packet::push_string(&mut packet, target.as_bytes())?;
        match sftp.extended("posix-rename@openssh.com", packet).await {
            Ok(russh_sftp::protocol::Packet::Status(status))
                if status.status_code == StatusCode::Ok =>
            {
                return Ok(())
            }
            Ok(russh_sftp::protocol::Packet::Status(status))
                if status.status_code == StatusCode::OpUnsupported => {}
            Ok(russh_sftp::protocol::Packet::Status(status)) => {
                return Err(install_stage_error(
                    "原子替换目标文件",
                    SftpError::Status(status),
                ))
            }
            Err(SftpError::Status(status)) if status.status_code == StatusCode::OpUnsupported => {}
            Err(error) => return Err(install_stage_error("原子替换目标文件", error)),
            _ => return Err(AppError::new("原子安装响应无效，未重试改名")),
        }
    }
    let backup = remote(
        &item
            .remote
            .with_file_name(format!(".termrill-backup-{}", identifier())),
    )?;
    sftp.rename(target, backup.clone())
        .await
        .map_err(|error| install_stage_error("备份原目标文件", error))?;
    let commit = async {
        if item.link.is_some() {
            let attrs = sftp.lstat(backup.clone()).await?.attrs;
            if attrs
                .permissions
                .is_none_or(|mode| mode & 0o170000 != 0o120000)
            {
                return Err(AppError::new("旧链接类型在替换前已变化"));
            }
        }
        sftp.rename(temporary, target)
            .await
            .map_err(|error| install_stage_error("安装目标文件", error))?;
        Ok::<_, AppError>(())
    }
    .await;
    if let Err(error) = commit {
        if sftp.rename(backup.clone(), target).await.is_err() {
            return Err(AppError::new(format!("安装失败，原文件保留在 {backup}")));
        }
        return Err(error);
    }
    sftp.remove(backup.clone()).await.map_err(|error| {
        let mut error = AppError::from(error);
        error.message = format!(
            "目标文件已安装，但旧备份清理失败；请勿直接重复上传。备份位置：{backup}；{}",
            error.message
        );
        install_stage_error("清理旧文件备份", error)
    })?;
    Ok(())
}
