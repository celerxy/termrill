//! 原生异步下载：范围读取并发，写盘使用 Tokio 文件；完成校验后安装正式文件。
use super::*;
use crate::remote_async::{attrs, Remote};
use futures::{stream::FuturesUnordered, StreamExt};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[path = "transfer_download_scan.rs"]
mod scan;
use scan::scan;

fn existing(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(stat) => Ok(Some(stat)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn local_parents(path: &Path) -> Result<()> {
    for ancestor in path.ancestors().skip(1) {
        match fs::symlink_metadata(ancestor) {
            Ok(stat) if local_link(&stat) || !stat.is_dir() => {
                return Err(AppError::new("本地目标父级包含链接或非目录"))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
async fn digest_remote(remote: &Remote, item: &Item, cancel: &AtomicBool) -> Result<String> {
    let mut digest = Sha256::new();
    remote
        .read_download_into(&item.remote.to_string_lossy(), item.size, cancel, |chunk| {
            digest.update(&chunk);
            std::future::ready(Ok(()))
        })
        .await?;
    Ok(format!("{:x}", digest.finalize()))
}

/// 与 `Remote` 范围读取的跨连接阈值一致：更小的文件整文件使用一条连接。
const SINGLE_CONNECTION_BYTES: u64 = 4 * 1024 * 1024;
/// 单连接文件同时活动的源字节预算。每个文件只请求实际范围，接收队列和摘要缓存
/// 也不超过文件本身，因此该预算约束了这些文件的正文内存，不随窗口放大到 32 个 4 MiB。
const SINGLE_CONNECTION_ACTIVE_BYTES: u64 = 32 * 1024 * 1024;

/// 自适应窗口允许且活动字节不超预算时才启动下一个单连接文件；
/// 没有活动文件时总是放行，避免单个文件超过预算时永远等待。
fn admit_single_connection(active: usize, active_bytes: u64, limit: usize, size: u64) -> bool {
    if active >= limit {
        return false;
    }
    active == 0 || active_bytes.saturating_add(size) <= SINGLE_CONNECTION_ACTIVE_BYTES
}

/// 续传时同时核对的已完成文件数；每个文件的远程读取另受连接句柄许可约束。
const COMPLETED_VERIFY_FILES: usize = 4;

/// 续传前重新核对日志中已完成的项目，不能因为续传而跳过远程和本地摘要校验。
/// 多个文件有界并发，每完成一个文件就推进预检进度。任一项失败立即返回：
/// 其余在途读取随之丢弃，下载会话因失败不把这些连接归还连接池。
async fn verify_completed(
    job: &Job,
    remote: &Remote,
    items: &[Item],
    event: &mut TransferEvent,
    cancel: &AtomicBool,
    send: &dyn Fn(&TransferEvent),
) -> Result<()> {
    let completed_items = items.iter().filter_map(|item| {
        job.completed
            .get(&item.remote.to_string_lossy().to_string())
            .map(|expected| (item, expected.as_str()))
    });
    let mut verifying = futures::stream::iter(completed_items)
        .map(|(item, expected)| verify_completed_item(remote, item, expected, cancel))
        .buffer_unordered(COMPLETED_VERIFY_FILES);
    while let Some(verified_bytes) = verifying.next().await {
        event.bytes += verified_bytes?;
        send(event);
    }
    Ok(())
}

/// 返回核对通过后应计入进度的字节数；链接只比较目标，不计正文字节。
async fn verify_completed_item(
    remote: &Remote,
    item: &Item,
    expected: &str,
    cancel: &AtomicBool,
) -> Result<u64> {
    if let Some(target) = &item.link {
        let local = tokio::fs::read_link(&item.local).await?;
        if link_digest(target) != expected || link_digest(&local) != expected {
            return Err(AppError::new("已完成链接发生变化"));
        }
        return Ok(0);
    }
    let (actual, local) = tokio::join!(
        digest_remote(remote, item, cancel),
        crate::blocking({
            let path = item.local.clone();
            move || {
                hash_reader(
                    fs::File::open(path)?,
                    &AtomicBool::new(false),
                    &AtomicBool::new(false),
                )
            }
        })
    );
    if actual? != expected || local? != expected {
        return Err(AppError::new("已完成文件内容发生变化"));
    }
    Ok(item.size)
}
#[path = "transfer_download_file.rs"]
pub(super) mod destination;
use destination::file;

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute(
    job: &Job,
    remote: &Remote,
    items: Vec<Item>,
    event: &mut TransferEvent,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    send: &dyn Fn(&TransferEvent),
    checkpoint: &dyn Fn(&str, String) -> Result<()>,
    limiter: &crate::transfer_rate::RateLimiter,
    concurrency: usize,
) -> Result<()> {
    event.total = items.iter().map(|item| item.size).sum();
    event.status = "prechecking".into();
    send(event);
    for item in &items {
        cancelled(cancel, &remote.closed)?;
        crate::blocking({
            let item = item.clone();
            let overwrite = job.overwrite;
            let completed = job
                .completed
                .contains_key(&item.remote.to_string_lossy().to_string());
            move || {
                local_parents(&item.local)?;
                if let Some(stat) = existing(&item.local)? {
                    if item.link.is_some() {
                        if !stat.file_type().is_symlink() || (!overwrite && !completed) {
                            return Err(AppError::new("链接目标已存在或类型冲突"));
                        }
                    } else if local_link(&stat)
                        || stat.is_dir() != item.directory
                        || (!item.directory && !overwrite && !completed)
                    {
                        return Err(AppError::new("本地目标已存在或类型冲突"));
                    }
                }
                Ok(())
            }
        })
        .await?;
    }
    verify_completed(job, remote, &items, event, cancel, send).await?;
    for item in items.iter().filter(|item| item.directory) {
        crate::blocking({
            let path = item.local.clone();
            move || {
                local_parents(&path)?;
                fs::create_dir_all(path)?;
                Ok(())
            }
        })
        .await?;
    }
    event.status = "running".into();
    event.speed = 0;
    event.remaining_seconds = None;
    send(event);
    let (progress, mut updates) = tokio::sync::mpsc::unbounded_channel();
    let mut files = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            !item.directory
                && !job
                    .completed
                    .contains_key(&item.remote.to_string_lossy().to_string())
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|(_, item)| std::cmp::Reverse(item.size));
    // 不跨连接分片的文件都由自适应窗口调度；大文件已占满全部连接，仍按任务并发限制。
    let (small, large): (Vec<_>, Vec<_>) = files
        .into_iter()
        .partition(|(_, item)| item.size < SINGLE_CONNECTION_BYTES);
    let mut small = schedule::interleave(small);
    let mut large = std::collections::VecDeque::from(large);
    let mut small_window = schedule::SmallWindow::new(remote.native.max_files);
    let mut active_small = 0usize;
    let mut active_small_bytes = 0u64;
    let mut active_large = 0usize;
    let mut active = FuturesUnordered::new();
    let mut failure = None;
    let started = Instant::now();
    let mut speed_started = Instant::now();
    let mut speed_bytes = remote.received_download_data();
    let mut last_progress = Instant::now();
    let mut transport_sample = Instant::now();
    let mut sampled_cpu = crate::performance::cpu_micros();
    let mut sampled_connections = remote
        .download_peers
        .iter()
        .chain(std::iter::once(&remote.native))
        .map(|native| {
            (
                native.received_transport.load(Ordering::Relaxed),
                native.received_data.load(Ordering::Relaxed),
            )
        })
        .collect::<Vec<_>>();
    loop {
        while active.len() < remote.native.max_files
            && failure.is_none()
            && !cancel.load(Ordering::Relaxed)
            && !pause.load(Ordering::Relaxed)
        {
            let small_admitted = small.front().is_some_and(|(_, item)| {
                admit_single_connection(
                    active_small,
                    active_small_bytes,
                    small_window.limit,
                    item.size,
                )
            });
            let selected = if small_admitted {
                small.pop_front().map(|item| (true, item))
            } else if active_large < concurrency.clamp(1, 4) {
                large.pop_front().map(|item| (false, item))
            } else {
                None
            };
            let Some((is_small, (index, item))) = selected else {
                break;
            };
            let size = item.size;
            if is_small {
                active_small += 1;
                active_small_bytes += size;
            } else {
                active_large += 1;
            }
            let progress = &progress;
            active.push(async move {
                let started = Instant::now();
                let result = async {
                    let digest = if let Some(target) = &item.link {
                        let actual = remote
                            .checked(
                                remote
                                    .native
                                    .sftp
                                    .readlink(item.remote.to_string_lossy().into_owned()),
                                cancel,
                            )
                            .await?;
                        if actual
                            .files
                            .first()
                            .map(|entry| PathBuf::from(&entry.filename))
                            .as_ref()
                            != Some(target)
                        {
                            return Err(AppError::new("远程链接目标已变化"));
                        }
                        crate::blocking({
                            let item = item.clone();
                            let target = target.clone();
                            let overwrite = job.overwrite;
                            move || {
                                local_parents(&item.local)?;
                                if !overwrite {
                                    return create_local_link(
                                        &target,
                                        &item.local,
                                        item.link_directory,
                                    );
                                }
                                let temporary = item
                                    .local
                                    .with_file_name(format!(".termrill-link-{}", identifier()));
                                create_local_link(&target, &temporary, item.link_directory)?;
                                let result = crate::links::install_local(&temporary, &item.local);
                                if result.is_err() {
                                    let _ = crate::links::remove_local(&temporary);
                                }
                                result
                            }
                        })
                        .await?;
                        link_digest(target)
                    } else {
                        file(remote, job, index, item, cancel, pause, limiter, progress).await?
                    };
                    Ok::<_, AppError>((item.remote.to_string_lossy().into_owned(), digest))
                }
                .await;
                (is_small, size, started.elapsed(), result)
            });
        }
        if active.is_empty() {
            if failure.is_none() && (!small.is_empty() || !large.is_empty()) {
                cancelled(cancel, &remote.closed)?;
                event.status = "paused".into();
                event.speed = 0;
                event.remaining_seconds = None;
                send(event);
                speed_started = Instant::now();
                speed_bytes = remote.received_download_data();
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            break;
        }
        tokio::select! {
            Some(bytes) = updates.recv() => { event.bytes += bytes; },
            Some((is_small, size, elapsed, result)) = active.next() => {
                if is_small {
                    active_small -= 1;
                    active_small_bytes -= size;
                    if result.is_ok() { small_window.completed(elapsed); }
                } else { active_large -= 1; }
                match result {
                    Ok((path, digest)) => {
                        if let Err(error) = checkpoint(&path, digest) { failure.get_or_insert(error); }
                    },
                    Err(error) => { failure.get_or_insert(error); }
                }
            },
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
        small_window.adjust(!small.is_empty());
        if transport_sample.elapsed() >= Duration::from_secs(2) {
            let cpu = crate::performance::cpu_micros();
            let cpu_us = cpu
                .zip(sampled_cpu)
                .map(|(now, before)| now.saturating_sub(before));
            for (index, native) in remote
                .download_peers
                .iter()
                .chain(std::iter::once(&remote.native))
                .enumerate()
            {
                let wire = native.received_transport.load(Ordering::Relaxed);
                let data = native.received_data.load(Ordering::Relaxed);
                let (old_wire, old_data) = sampled_connections[index];
                crate::performance_log!("[TermRill transfer] task={} phase=transport_sample connection={} elapsed_ms={} encrypted_bytes={} sftp_bytes={} processed_bytes={} process_cpu_us={:?}",
                    job.id, index, transport_sample.elapsed().as_millis(), wire.saturating_sub(old_wire), data.saturating_sub(old_data), event.bytes, cpu_us);
                sampled_connections[index] = (wire, data);
            }
            sampled_cpu = cpu;
            transport_sample = Instant::now();
        }
        if last_progress.elapsed() >= Duration::from_millis(100) {
            let paused = pause.load(Ordering::Relaxed);
            event.status = if paused { "paused" } else { "running" }.into();
            if paused {
                event.speed = 0;
                speed_started = Instant::now();
                speed_bytes = remote.received_download_data();
            } else if speed_started.elapsed() >= Duration::from_secs(1) {
                let received = remote.received_download_data();
                event.speed = (received.saturating_sub(speed_bytes) as f64
                    / speed_started.elapsed().as_secs_f64()) as u64;
                speed_started = Instant::now();
                speed_bytes = received;
            }
            event.remaining_seconds = if !paused && event.speed > 0 && event.bytes < event.total {
                Some(
                    event
                        .total
                        .saturating_sub(event.bytes)
                        .div_ceil(event.speed),
                )
            } else {
                None
            };
            send(event);
            last_progress = Instant::now();
        }
    }
    while let Ok(bytes) = updates.try_recv() {
        event.bytes += bytes;
    }
    event.remaining_seconds = None;
    crate::performance_log!(
        "[TermRill transfer] task={} direction=download elapsed_ms={} small_limit={} success={}",
        job.id,
        started.elapsed().as_millis(),
        small_window.limit,
        failure.is_none()
            && !cancel.load(Ordering::Relaxed)
            && !remote.closed.load(Ordering::Relaxed)
    );
    if let Some(error) = failure {
        return Err(error);
    }
    cancelled(cancel, &remote.closed)
}

pub(super) async fn scan_items(
    remote: &Remote,
    job: &Job,
    cancel: &AtomicBool,
) -> Result<Vec<Item>> {
    scan(remote, job, cancel).await
}

#[cfg(test)]
mod schedule_tests {
    use super::*;

    #[test]
    fn single_connection_files_respect_window_and_byte_budget() {
        let megabyte = 1024 * 1024;
        assert!(admit_single_connection(0, 0, 8, 3 * megabyte));
        assert!(admit_single_connection(7, 28 * megabyte, 8, 4 * megabyte));
        assert!(
            !admit_single_connection(8, 0, 8, 1),
            "自适应窗口已满时不能继续启动文件"
        );
        assert!(
            !admit_single_connection(9, 30 * megabyte, 16, 3 * megabyte),
            "活动字节超过预算时必须等待在途文件完成"
        );
    }
}
