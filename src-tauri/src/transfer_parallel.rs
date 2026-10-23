//! 有界目录并发：最多四个独立 SSH 会话；协调线程独占进度汇总与清单保存。
use super::*;
use std::sync::{mpsc, Mutex};

enum Message {
    Progress(usize, TransferEvent),
    Checkpoint(String, String, mpsc::SyncSender<Result<()>>),
    Finished(usize, TransferEvent),
}

/// 下载工作连接完整验证指纹和认证；无法增加连接时退回串行。
pub(super) fn prepare_connections(
    original: &Mutex<crate::ssh::Connection>,
    backend: &Backend,
    cancel: &AtomicBool,
    count: usize,
) -> Result<Option<Vec<Mutex<crate::ssh::Connection>>>> {
    if count < 2 {
        return Ok(None);
    }
    let (request, parent_closed) = {
        let conn = lock(original)?;
        (conn.request.clone(), conn.closed.clone())
    };
    // 交互式认证可能每次要求验证码，不隐式触发多个认证弹窗。
    if request.profile.target.auth == "interactive"
        || request
            .profile
            .jump
            .as_ref()
            .is_some_and(|jump| jump.auth == "interactive")
    {
        return Ok(None);
    }
    let mut connections = Vec::new();
    cancelled(cancel, &parent_closed)?;
    let connect_started = Instant::now();
    // 各工作连接独立握手，避免多个认证往返顺序累积；开始写入前全部收集结果。
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..count)
            .map(|_| {
                let request = request.clone();
                scope.spawn(move || crate::ssh::connect(request, backend))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err(AppError::new("传输工作连接建立异常")))
            })
            .collect::<Vec<_>>()
    });
    // 先检查全部指纹错误，不能因另一连接容量不足而掩盖指纹变化。
    if let Some(index) = outcomes.iter().position(|result| {
        result
            .as_ref()
            .err()
            .is_some_and(|error| error.host_key.is_some())
    }) {
        return Err(outcomes
            .into_iter()
            .nth(index)
            .expect("索引来自同一结果集合")
            .err()
            .expect("已确认指纹错误"));
    }
    for outcome in outcomes {
        cancelled(cancel, &parent_closed)?;
        let conn = match outcome {
            Ok(conn) => conn,
            // 容量限制、认证失败等情况下退回原连接；指纹变化仍必须报告。
            Err(error) if error.host_key.is_none() => return Ok(None),
            Err(error) => return Err(error),
        };
        connections.push(Mutex::new(conn));
    }
    crate::performance_log!(
        "[TermRill transfer] phase=connections workers={} elapsed_ms={}",
        connections.len(),
        connect_started.elapsed().as_millis()
    );
    Ok(Some(connections))
}

// 复用上层已完成的全目录冲突检查。连接全部准备好后才创建目录和写文件。
#[allow(clippy::too_many_arguments)]
pub(super) fn execute(
    job: &Job,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    original: &Mutex<crate::ssh::Connection>,
    backend: &Backend,
    items: &[(usize, Item)],
    event: &mut TransferEvent,
    send: &dyn Fn(&TransferEvent),
    limiter: Option<&crate::transfer_rate::RateLimiter>,
) -> Result<Option<Result<()>>> {
    let count = items
        .iter()
        .filter(|(_, item)| {
            !item.directory
                && !job
                    .completed
                    .contains_key(&item.remote.to_string_lossy().to_string())
        })
        .count()
        .min(
            lock(&backend.config)?
                .transfer_settings
                .concurrency
                .clamp(1, 4),
        );
    if count < 2 {
        return Ok(None);
    }
    let parent_closed = lock(original)?.closed.clone();
    let connections = match prepare_connections(original, backend, cancel, count)? {
        Some(connections) => connections,
        None => return Ok(None),
    };
    let count = connections.len();
    let stopped = Arc::new(AtomicBool::new(false));
    for connection in &connections {
        lock(connection)?.closed = stopped.clone();
    }
    let mut plans = vec![Vec::new(); count];
    let mut next = 0;
    let directory_start = Instant::now();
    for (index, item) in items {
        cancelled(cancel, &parent_closed)?;
        if item.directory {
            fs::create_dir_all(&item.local)?;
        } else if !job
            .completed
            .contains_key(&item.remote.to_string_lossy().to_string())
        {
            plans[next % count].push((*index, item.clone()));
            next += 1;
        }
    }
    crate::performance_log!(
        "[TermRill transfer] phase=prepare_directories total={} elapsed_ms={}",
        items.iter().filter(|(_, item)| item.directory).count(),
        directory_start.elapsed().as_millis()
    );
    // 大文件先开始；完成后各连接继续领取小文件，不静态绑定文件组。
    let mut files: Vec<_> = plans.into_iter().flatten().collect();
    files.sort_by_key(|(_, item)| std::cmp::Reverse(item.size));
    crate::performance_log!(
        "[TermRill transfer] phase=schedule dynamic=true files={} workers={count}",
        files.len()
    );
    let queue = Mutex::new(std::collections::VecDeque::from(files));
    // 限制未消费进度消息数量，避免极多空文件时积累无界队列。
    let (sender, receiver) = mpsc::sync_channel(64);
    let mut bytes = vec![0; count];
    let completed_bytes = event.bytes;
    let mut finished = 0;
    let mut failure = None;
    let mut pending = std::collections::VecDeque::new();
    let start = Instant::now();
    std::thread::scope(|scope| {
        for (worker, connection) in connections.iter().enumerate() {
            let sender = sender.clone();
            let queue = &queue;
            scope.spawn(move || {
                // 文件安装后允许继续发起下一文件请求；完成状态必须等待所有摘要落盘确认。
                // 每连接最多 16 个待确认项，协调通道仍有总容量 64 的反压。
                let acknowledgements = std::cell::RefCell::new(std::collections::VecDeque::new());
                let progress = |update: &TransferEvent| {
                    let _ = sender.send(Message::Progress(worker, update.clone()));
                };
                let save = |path: &str, digest: String| {
                    let (reply, answer) = mpsc::sync_channel(1);
                    sender
                        .send(Message::Checkpoint(path.into(), digest, reply))
                        .map_err(|_| AppError::new("并发协调线程已关闭"))?;
                    let mut replies = acknowledgements.borrow_mut();
                    replies.push_back(answer);
                    if replies.len() >= 16 {
                        replies
                            .pop_front()
                            .expect("长度已经检查")
                            .recv()
                            .map_err(|_| AppError::new("任务记录保存响应已关闭"))??;
                    }
                    Ok(())
                };
                let mut result = execute_inner(
                    job,
                    cancel,
                    pause,
                    connection,
                    &progress,
                    &save,
                    None,
                    Some(queue),
                    limiter,
                );
                for answer in acknowledgements.into_inner() {
                    let saved = answer
                        .recv()
                        .unwrap_or_else(|_| Err(AppError::new("任务记录保存响应已关闭")));
                    if let Err(error) = saved {
                        result.status = "failed".into();
                        result.error = Some(error.message);
                    }
                }
                let _ = sender.send(Message::Finished(worker, result));
            });
        }
        drop(sender);
        while finished < count {
            if cancel.load(Ordering::Relaxed) || parent_closed.load(Ordering::Relaxed) {
                stopped.store(true, Ordering::Relaxed);
            }
            let message = match pending.pop_front() {
                Some(message) => Ok(message),
                None => receiver.recv_timeout(std::time::Duration::from_millis(50)),
            };
            match message {
                Ok(Message::Checkpoint(path, digest, reply)) => {
                    let mut records = vec![(path, digest)];
                    let mut replies = vec![reply];
                    // 仅合并已经到达的记录，不主动等齐批次；其余消息有界暂存并继续按序处理。
                    for _ in 0..64 {
                        if records.len() >= 16 || pending.len() >= 64 {
                            break;
                        }
                        match receiver.try_recv() {
                            Ok(Message::Checkpoint(path, digest, reply)) => {
                                records.push((path, digest));
                                replies.push(reply);
                            }
                            Ok(other) => pending.push_back(other),
                            Err(_) => break,
                        }
                    }
                    let save_start = Instant::now();
                    let saved =
                        crate::transfer_journal::checkpoint_batch(backend, &job.id, &records);
                    crate::performance_log!("[TermRill transfer] task={} phase=checkpoint_batch records={} success={} elapsed_ms={}", job.id, records.len(), saved.is_ok(), save_start.elapsed().as_millis());
                    for reply in replies {
                        let outcome = match &saved {
                            Ok(()) => Ok(()),
                            Err(error) => Err(AppError::new(error.message.clone())),
                        };
                        let _ = reply.send(outcome);
                    }
                    if let Err(error) = saved {
                        if failure.is_none() {
                            failure = Some(error);
                        }
                        stopped.store(true, Ordering::Relaxed);
                    }
                }
                Ok(Message::Progress(worker, update)) => {
                    bytes[worker] = update.bytes;
                    event.bytes = completed_bytes + bytes.iter().sum::<u64>();
                    event.speed = if pause.load(Ordering::Relaxed) {
                        0
                    } else {
                        (event.bytes as f64 / start.elapsed().as_secs_f64().max(0.001)) as u64
                    };
                    event.status = if pause.load(Ordering::Relaxed) {
                        "paused"
                    } else {
                        "running"
                    }
                    .into();
                    send(event);
                }
                Ok(Message::Finished(worker, update)) => {
                    bytes[worker] = update.bytes;
                    finished += 1;
                    if update.status != "completed" && failure.is_none() {
                        failure = Some(AppError::new(
                            update.error.unwrap_or_else(|| "并发传输失败".into()),
                        ));
                        stopped.store(true, Ordering::Relaxed);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if pause.load(Ordering::Relaxed) && event.status != "paused" {
                        event.status = "paused".into();
                        event.speed = 0;
                        send(event);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    failure = Some(AppError::new("并发工作线程意外退出"));
                    stopped.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
    });
    event.bytes = completed_bytes + bytes.iter().sum::<u64>();
    crate::performance_log!(
        "[TermRill transfer] phase=data success={} workers={count} bytes={} elapsed_ms={}",
        failure.is_none(),
        event.bytes.saturating_sub(completed_bytes),
        start.elapsed().as_millis()
    );
    Ok(Some(match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }))
}
