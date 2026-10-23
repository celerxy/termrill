//! 动态遍历队列：新发现的子项立即供空闲工作连接处理，没有目录层级屏障。
use super::*;
use std::{collections::VecDeque, sync::Condvar, time::Duration};

struct Queue {
    pending: VecDeque<(Entry, usize)>,
    active: usize,
    count: usize,
    bytes: usize,
    directories: BTreeMap<usize, Vec<String>>,
    error: Option<AppError>,
}

pub(super) fn run(
    pool: &[Arc<Sftp>],
    entries: Vec<Entry>,
    closed: &AtomicBool,
) -> Result<BTreeMap<usize, Vec<String>>> {
    let bytes = entries.iter().map(|entry| entry.0.len()).sum();
    if entries.len() > 100_000 || bytes > 32 * 1024 * 1024 {
        return Err(AppError::new("目录遍历超限，尚未删除"));
    }
    let state = Mutex::new(Queue {
        count: entries.len(),
        bytes,
        active: 0,
        pending: entries.into_iter().map(|entry| (entry, 1)).collect(),
        directories: BTreeMap::new(),
        error: None,
    });
    let ready = Condvar::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = pool
            .iter()
            .enumerate()
            .map(|(worker, sftp)| {
                let state = &state;
                let ready = &ready;
                scope.spawn(move || {
                    let trace_id = crate::files::delete_trace_id();
                    let trace = crate::files::DeleteTrace::new(&trace_id);
                    crate::performance_log!(
                        "[TermRill delete] id={trace_id} phase=pipeline_worker worker={worker}"
                    );
                    loop {
                        let work = {
                            let Ok(mut queue) = state.lock() else {
                                ready.notify_all();
                                return;
                            };
                            loop {
                                if queue.error.is_some() {
                                    return;
                                }
                                if closed.load(Ordering::Relaxed) {
                                    queue.error =
                                        Some(AppError::new("连接已关闭，部分项目可能已删除"));
                                    ready.notify_all();
                                    return;
                                }
                                if let Some(work) = queue.pending.pop_front() {
                                    queue.active += 1;
                                    break work;
                                }
                                if queue.active == 0 {
                                    return;
                                }
                                match ready.wait_timeout(queue, Duration::from_millis(100)) {
                                    Ok((value, _)) => queue = value,
                                    Err(_) => {
                                        ready.notify_all();
                                        return;
                                    }
                                }
                            }
                        };
                        let ((path, listed), depth) = work;
                        // 网络处理发生异常时也唤醒等待者，不能留下 active 计数导致永远等待。
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                            || -> Result<Option<Vec<Entry>>> {
                                if depth > 128 {
                                    return Err(AppError::new("目录深度超限，部分项目可能已删除"));
                                }
                                let stat = if listed.perm.is_some() && !listed.is_dir() {
                                    listed
                                } else {
                                    trace.step("directory_lstat", depth, || {
                                        sftp.lstat(Path::new(&path)).map_err(Into::into)
                                    })?
                                };
                                if stat.is_dir() {
                                    Ok(Some(trace.step("directory_readdir", depth, || {
                                        children(sftp, &path)
                                    })?))
                                } else {
                                    trace
                                        .step("unlink", depth, || {
                                            sftp.unlink(Path::new(&path)).map_err(Into::into)
                                        })
                                        .map_err(|error| {
                                            AppError::new(format!(
                                                "删除 {path} 失败：{}；部分项目可能已删除",
                                                error.message
                                            ))
                                        })?;
                                    Ok(None)
                                }
                            },
                        ))
                        .unwrap_or_else(|_| {
                            Err(AppError::new("删除工作线程异常，部分项目可能已删除"))
                        });
                        let Ok(mut queue) = state.lock() else {
                            ready.notify_all();
                            return;
                        };
                        queue.active -= 1;
                        match result {
                            Ok(Some(children)) if queue.error.is_none() => {
                                let added_bytes: usize =
                                    children.iter().map(|entry| entry.0.len()).sum();
                                if queue.count + children.len() > 100_000
                                    || queue.bytes + added_bytes > 32 * 1024 * 1024
                                {
                                    queue.error =
                                        Some(AppError::new("目录遍历超限，部分项目可能已删除"));
                                } else {
                                    queue.count += children.len();
                                    queue.bytes += added_bytes;
                                    queue.pending.extend(
                                        children.into_iter().map(|entry| (entry, depth + 1)),
                                    );
                                    queue.directories.entry(depth).or_default().push(path);
                                }
                            }
                            Ok(_) => {}
                            Err(error) => {
                                queue.error.get_or_insert(error);
                            }
                        }
                        ready.notify_all();
                    }
                })
            })
            .collect();
        for handle in handles {
            if handle.join().is_err() {
                if let Ok(mut queue) = state.lock() {
                    queue
                        .error
                        .get_or_insert_with(|| AppError::new("删除工作线程异常"));
                }
                ready.notify_all();
            }
        }
    });
    let queue = state
        .into_inner()
        .map_err(|_| AppError::new("删除队列锁异常，部分项目可能已删除"))?;
    crate::performance_log!(
        "[TermRill delete] pipeline=true items={} directories={}",
        queue.count,
        queue.directories.values().map(Vec::len).sum::<usize>()
    );
    match queue.error {
        Some(error) => Err(error),
        None => Ok(queue.directories),
    }
}
