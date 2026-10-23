//! 分批扫描与独立删除窗口；子树满足依赖后立即收尾，不等待其他分支。
use super::{state::*, *};
use futures::future::BoxFuture;
const SCAN_WINDOW: usize = 16;
const REQUEST_WINDOW: usize = 40;
const SCANNING_MUTATION_WINDOW: usize = 32;

async fn open(
    remote: &Remote,
    entry: Entry,
    permit: tokio::sync::OwnedSemaphorePermit,
    cancel: &AtomicBool,
    trace: &Trace,
) -> Result<Scanned> {
    let stat = match &entry.listed {
        Some(stat) if stat.permissions.is_some() && !attrs(stat).is_dir() => stat.clone(),
        _ => {
            trace
                .request("lstat", remote.stat(&entry.path, cancel))
                .await?
        }
    };
    if !attrs(&stat).is_dir() {
        return Ok(Scanned::File(entry));
    }
    let handle = trace
        .request(
            "opendir",
            remote.checked(remote.native.sftp.opendir(entry.path.clone()), cancel),
        )
        .await?
        .handle;
    Ok(Scanned::Opened(entry, handle, permit))
}
async fn read(
    remote: &Remote,
    id: usize,
    path: String,
    handle: String,
    cancel: &AtomicBool,
    trace: &Trace,
) -> Result<Scanned> {
    let response = trace
        .request(
            "readdir",
            remote.checked(
                async {
                    match remote.native.sftp.readdir(handle).await {
                        Ok(batch) => Ok(Some(batch)),
                        Err(russh_sftp::client::error::Error::Status(status))
                            if status.status_code == russh_sftp::protocol::StatusCode::Eof =>
                        {
                            Ok(None)
                        }
                        Err(error) => Err(AppError::from(error)),
                    }
                },
                cancel,
            ),
        )
        .await?;
    let Some(batch) = response else {
        return Ok(Scanned::Eof(id));
    };
    if batch.files.is_empty() {
        return Err(AppError::new("目录返回空批次"));
    }
    let mut entries = Vec::new();
    for entry in batch.files {
        if [".", ".."].contains(&entry.filename.as_str()) {
            continue;
        }
        if entry.filename.is_empty() || entry.filename.contains(['/', '\\', '\0']) {
            return Err(AppError::new("服务器返回无效文件名"));
        }
        entries.push((
            format!("{}/{}", path.trim_end_matches('/'), entry.filename),
            entry.attrs,
        ));
    }
    Ok(Scanned::Batch(id, entries))
}
async fn close(
    remote: &Remote,
    id: usize,
    handle: String,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    cancel: &AtomicBool,
    trace: &Trace,
) -> Result<Scanned> {
    let result = trace
        // CLOSE 可能排在已发送的 UNLINK 后；沿用协议请求预算，不能另设五秒截断。
        .request(
            "close",
            remote.checked(remote.native.sftp.close(handle), cancel),
        )
        .await;
    drop(permit);
    result?;
    Ok(Scanned::Closed(id))
}
pub(super) async fn execute(
    remote: &Remote,
    path: &str,
    recursive: bool,
    cancel: &AtomicBool,
    trace: &Trace,
) -> Result<()> {
    // 哨兵只计数根项目，不向服务器发送 RMDIR。
    let mut tree = vec![Directory {
        path: String::new(),
        parent: None,
        depth: 0,
        children: 1,
        closed: true,
        removal_queued: false,
        handle: None,
        permit: None,
    }];
    let mut pending = VecDeque::from([Entry {
        path: path.into(),
        parent: 0,
        depth: 0,
        listed: None,
    }]);
    let mut reads = VecDeque::new();
    let mut closes = VecDeque::new();
    let mut mutations = VecDeque::new();
    let stop_dispatch = AtomicBool::new(false);
    let mut scans: FuturesUnordered<BoxFuture<'_, Result<Scanned>>> = FuturesUnordered::new();
    let mut writes: FuturesUnordered<BoxFuture<'_, Result<(bool, usize)>>> =
        FuturesUnordered::new();
    let mut failure = None;
    let mut discovered = 1usize;
    let mut scan_peak = 0;
    let mut mutation_peak = 0;
    let mut prefer_open = true;
    let mut scan_complete = false;
    loop {
        if remote.closed.load(std::sync::atomic::Ordering::Relaxed) {
            failure.get_or_insert(AppError::new("连接已关闭；部分项目可能已删除"));
        }
        if failure.is_some() {
            stop_dispatch.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        while failure.is_none()
            && scans.len() < SCAN_WINDOW
            && scans.len() + writes.len() < REQUEST_WINDOW
        {
            // CLOSE 优先释放句柄；OPEN/READ 交替推进，避免宽目录挡住子树。
            // 新目录只尝试立即取得预算，失败时继续 READ，不占槽位等待句柄。
            let opening =
                if closes.is_empty() && !pending.is_empty() && (prefer_open || reads.is_empty()) {
                    remote.try_permit()
                } else {
                    None
                };
            let request: BoxFuture<'_, Result<Scanned>> = if let Some(id) = closes.pop_front() {
                let node: &mut Directory = &mut tree[id];
                let handle = node
                    .handle
                    .take()
                    .ok_or_else(|| AppError::new("目录句柄已释放"))?;
                Box::pin(close(remote, id, handle, node.permit.take(), cancel, trace))
            } else if let Some(permit) = opening {
                prefer_open = false;
                let entry = pending
                    .pop_front()
                    .ok_or_else(|| AppError::new("删除队列无效"))?;
                Box::pin(open(remote, entry, permit, cancel, trace))
            } else if let Some(id) = reads.pop_front() {
                prefer_open = true;
                let node: &Directory = &tree[id];
                let handle = node
                    .handle
                    .clone()
                    .ok_or_else(|| AppError::new("目录句柄已释放"))?;
                Box::pin(read(remote, id, node.path.clone(), handle, cancel, trace))
            } else {
                break;
            };
            scans.push(request);
        }
        // 扫描仍需保留推进空间；全部扫描确认结束后，删除独占 40 个任务预算。
        // 只扩大新派发上限，不取消或重发任何在途请求。
        if failure.is_none()
            && !scan_complete
            && scans.is_empty()
            && pending.is_empty()
            && reads.is_empty()
            && closes.is_empty()
        {
            scan_complete = true;
            trace.tail(mutations.len(), writes.len());
        }
        let mutation_limit = if scan_complete {
            REQUEST_WINDOW
        } else {
            SCANNING_MUTATION_WINDOW
        };
        while failure.is_none()
            && writes.len() < mutation_limit
            && scans.len() + writes.len() < REQUEST_WINDOW
        {
            let Some(operation) = mutations.pop_front() else {
                break;
            };
            let stop_dispatch = &stop_dispatch;
            writes.push(Box::pin(async move {
                // FuturesUnordered 中尚未被轮询的任务还未发送；失败后不补发它们。
                // 已经发送的请求不取消，必须等待明确响应，避免未知删除结果。
                if stop_dispatch.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(AppError::new("停止派发后续删除请求"));
                }
                match operation {
                    Mutation::File(path, parent, queued) => {
                        trace.queued("unlink_queue", queued);
                        trace
                            .request(
                                "unlink",
                                remote.checked(remote.native.sftp.remove(path), cancel),
                            )
                            .await?;
                        Ok((false, parent))
                    }
                    Mutation::Directory(path, id, queued) => {
                        trace.queued("rmdir_queue", queued);
                        trace
                            .request(
                                "rmdir",
                                remote.checked(remote.native.sftp.rmdir(path), cancel),
                            )
                            .await?;
                        Ok((true, id))
                    }
                }
            }));
        }
        scan_peak = scan_peak.max(scans.len());
        mutation_peak = mutation_peak.max(writes.len());
        if scans.is_empty() && writes.is_empty() {
            if failure.is_some()
                || (pending.is_empty()
                    && reads.is_empty()
                    && closes.is_empty()
                    && mutations.is_empty())
            {
                break;
            }
            // 其他预览暂占预算时，让出执行线程，仍检查主连接断开。
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            continue;
        }
        tokio::select! {
            Some(result) = scans.next(), if !scans.is_empty() => {
                match result {
                    Ok(Scanned::Opened(entry, handle, permit)) => {
                        let id = tree.len();
                        tree.push(Directory {
                            path: entry.path,
                            parent: Some(entry.parent),
                            depth: entry.depth,
                            children: 0,
                            closed: false,
                            removal_queued: false,
                            handle: Some(handle),
                            permit: Some(permit),
                        });
                        if failure.is_none() {
                            reads.push_back(id);
                        }
                    }
                    Ok(Scanned::File(entry)) => {
                        if failure.is_none() {
                            mutations.push_back(Mutation::File(entry.path, entry.parent, std::time::Instant::now()));
                        }
                    }
                    Ok(Scanned::Batch(id, entries)) => {
                        if failure.is_none() {
                            if !recursive && !entries.is_empty() {
                                failure = Some(AppError::new("目录非空，请确认递归删除"));
                            } else if discovered + entries.len() > 100_000
                                || (!entries.is_empty() && tree[id].depth >= 128)
                            {
                                failure = Some(AppError::new("删除超过层级或项目限制；部分项目可能已删除"));
                            } else {
                                discovered += entries.len();
                                tree[id].children += entries.len();
                                for (path, stat) in entries {
                                    if stat.permissions.is_some() && !attrs(&stat).is_dir() {
                                        mutations.push_back(Mutation::File(path, id, std::time::Instant::now()));
                                    } else {
                                        pending.push_back(Entry {
                                            path,
                                            parent: id,
                                            depth: tree[id].depth + 1,
                                            listed: Some(stat),
                                        });
                                    }
                                }
                                reads.push_back(id);
                            }
                        }
                    }
                    Ok(Scanned::Eof(id)) => {
                        if failure.is_none() {
                            closes.push_back(id);
                        }
                    }
                    Ok(Scanned::Closed(id)) => {
                        tree[id].closed = true;
                        if failure.is_none() {
                            ready(&mut tree, id, &mut mutations);
                        }
                    }
                    Err(error) => {
                        failure.get_or_insert(error);
                    }
                }
            }
            Some(result) = writes.next(), if !writes.is_empty() => {
                match result {
                    Ok((directory, id)) if failure.is_none() => {
                        let parent = if directory {
                            tree[id].parent.ok_or_else(|| AppError::new("目录依赖无效"))?
                        } else {
                            id
                        };
                        if let Err(error) = completed(&mut tree, parent, &mut mutations) {
                            failure = Some(error);
                        }
                    }
                    Ok(_) => {},
                    Err(error) => {
                        failure.get_or_insert(error);
                    }
                }
            }
        }
    }
    // 失败也先收齐在途请求，再关闭已打开目录；不重发未知结果的删除请求。
    let mut cleanup = FuturesUnordered::new();
    for (id, node) in tree.iter_mut().enumerate() {
        if let Some(handle) = node.handle.take() {
            cleanup.push(close(remote, id, handle, node.permit.take(), cancel, trace));
        }
    }
    while let Some(result) = cleanup.next().await {
        if let Err(error) = result {
            failure.get_or_insert(error);
        }
    }
    if failure.is_none() && tree[0].children != 0 {
        failure = Some(AppError::new("删除依赖尚未完成"));
    }
    trace.finish(failure.is_none(), discovered, scan_peak, mutation_peak);
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
