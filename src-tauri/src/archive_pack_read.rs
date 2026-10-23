//! 远程来源有界预读：最多八个文件协程，每连接八个 READ 预算；单来源每连接 32 个。
//! 压缩留在原工作线程，网络流水通过有界 channel 与压缩计算隔离。
//! 活跃文件预读队列合计不超过 8 MiB；消费者退出先停止并释放接收端，再收齐读取和 CLOSE。
//! 范围接收与乱序缓存合计最多两个总请求窗口，预算随实际参与的连接数增长。
use crate::{
    archive_buffer::ArchiveBuffer,
    archive_io::{ArchiveIo, PackProgress},
    model::*,
    remote_async::Remote,
};
use futures::{stream, StreamExt};
use std::{
    cell::RefCell,
    io::Write,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use zip::{write::SimpleFileOptions, ZipWriter};

pub(super) type Tracking<'a> = Option<(&'a RefCell<PackProgress>, &'a AtomicU64)>;
enum Message {
    Chunk(Vec<u8>),
    Finished(Result<u64>, tokio::sync::oneshot::Sender<()>),
}

async fn produce(
    sources: &[super::Source],
    senders: Vec<mpsc::Sender<Message>>,
    remote: &Remote,
    cancel: &AtomicBool,
    stop: &AtomicBool,
) {
    let window = if senders.len() <= 1 { 32 } else { 8 };
    let jobs = sources
        .iter()
        .filter(|(_, _, stat)| !stat.directory)
        .zip(senders);
    let mut reads = stream::iter(jobs.map(|((_, path, stat), sender)| async move {
        let result = match stat.remote.as_ref() {
            Some(metadata) => {
                remote
                    .read_archive_source(
                        &path.to_string_lossy(),
                        metadata,
                        256 * 1024 * 1024,
                        stop,
                        |chunk| {
                            let sender = &sender;
                            async move {
                                sender
                                    .send(Message::Chunk(chunk))
                                    .await
                                    .map_err(|_| AppError::new("压缩消费者已停止"))
                            }
                        },
                        window,
                    )
                    .await
            }
            None => Err(AppError::new("远程打包来源属性缺失")),
        };
        let (acknowledge, consumed) = tokio::sync::oneshot::channel();
        if sender
            .send(Message::Finished(result, acknowledge))
            .await
            .is_ok()
        {
            // 完整的小文件也占据预读槽，直到消费者处理完；不能逐步缓存整个目录。
            let _ = consumed.await;
        }
    }))
    .buffer_unordered(8);
    loop {
        if cancel.load(Ordering::Relaxed) || remote.closed.load(Ordering::Relaxed) {
            stop.store(true, Ordering::Relaxed);
        }
        tokio::select! {
            value = reads.next() => if value.is_none() { break; },
            _ = tokio::time::sleep(Duration::from_millis(20)) => {},
        }
    }
}

pub(super) fn write(
    writer: &mut ZipWriter<ArchiveBuffer>,
    sources: &[super::Source],
    io: &ArchiveIo<'_>,
    remote: &Remote,
    tracking: Tracking<'_>,
    options: zip::write::FileOptions<'_, ()>,
) -> Result<()> {
    let stop = AtomicBool::new(false);
    let mut senders = Vec::new();
    let mut receivers = Vec::new();
    let active_files = sources
        .iter()
        .filter(|(_, _, stat)| !stat.directory)
        .count()
        .clamp(1, 8);
    // 适配协商的块长，队列吸收压缩短暂抖动；文件完成仍等待消费确认，不缓存整目录。
    let capacity = (8 * 1024 * 1024 / active_files / remote.native.download_read_size).clamp(2, 64);
    for _ in sources.iter().filter(|(_, _, stat)| !stat.directory) {
        let (sender, receiver) = mpsc::channel(capacity);
        senders.push(sender);
        receivers.push(receiver);
    }
    let cancel = io.cancel;
    std::thread::scope(|scope| {
        let stop_ref = &stop;
        // 一个作用域线程驱动所有异步预读，不为每个文件建立线程或连接。
        let producer = scope.spawn(move || {
            tauri::async_runtime::block_on(produce(sources, senders, remote, cancel, stop_ref));
        });
        let result = consume(writer, sources, io, tracking, &mut receivers, options);
        stop.store(true, Ordering::Relaxed);
        drop(receivers);
        producer
            .join()
            .map_err(|_| AppError::new("压缩预读线程异常退出"))?;
        result
    })
}

fn consume(
    writer: &mut ZipWriter<ArchiveBuffer>,
    sources: &[super::Source],
    io: &ArchiveIo<'_>,
    tracking: Tracking<'_>,
    receivers: &mut [mpsc::Receiver<Message>],
    options: zip::write::FileOptions<'_, ()>,
) -> Result<()> {
    let started = Instant::now();
    let mut channels = receivers.iter_mut();
    let mut waited = Duration::ZERO;
    let mut compressed = Duration::ZERO;
    let mut total = 0u64;
    let result = (|| {
        for (index, (name, _, stat)) in sources.iter().enumerate() {
            io.check()?;
            if let Some((state, _)) = tracking {
                state.borrow_mut().current_file = name.clone();
                io.pack_progress(&state.borrow(), false);
            } else {
                io.phase(format!("压缩 {}/{}", index + 1, sources.len()));
            }
            let computing = Instant::now();
            if stat.directory {
                writer
                    .add_directory(
                        format!("{name}/"),
                        SimpleFileOptions::default().unix_permissions(0o755),
                    )
                    .map_err(super::zip_error)?;
                compressed += computing.elapsed();
                continue;
            }
            writer.start_file(name, options).map_err(super::zip_error)?;
            compressed += computing.elapsed();
            let receiver = channels
                .next()
                .ok_or_else(|| AppError::new("压缩读取队列缺失"))?;
            total += consume_file(writer, receiver, io, tracking, &mut waited, &mut compressed)?;
        }
        Ok(())
    })();
    crate::performance_log!(
        "[TermRill archive] operation=pack phase=read_compress elapsed_ms={} wait_ms={} compress_ms={} bytes={} success={}",
        started.elapsed().as_millis(), waited.as_millis(), compressed.as_millis(), total, result.is_ok()
    );
    result
}

fn consume_file(
    writer: &mut ZipWriter<ArchiveBuffer>,
    receiver: &mut mpsc::Receiver<Message>,
    io: &ArchiveIo<'_>,
    tracking: Tracking<'_>,
    waited: &mut Duration,
    compressed: &mut Duration,
) -> Result<u64> {
    let mut read = 0u64;
    loop {
        io.check()?;
        let waiting = Instant::now();
        let message = receiver
            .blocking_recv()
            .ok_or_else(|| AppError::new("压缩预读未完成即退出"))?;
        *waited += waiting.elapsed();
        match message {
            Message::Chunk(chunk) => {
                io.check()?;
                let computing = Instant::now();
                writer.write_all(&chunk)?;
                *compressed += computing.elapsed();
                read += chunk.len() as u64;
                if let Some((state, generated)) = tracking {
                    let mut progress = state.borrow_mut();
                    progress.processed_bytes += chunk.len() as u64;
                    progress.zip_bytes = generated.load(Ordering::Relaxed);
                    io.pack_progress(&progress, false);
                }
            }
            Message::Finished(result, acknowledge) => {
                if result? != read {
                    return Err(AppError::new("压缩预读字节数不一致"));
                }
                if let Some((state, _)) = tracking {
                    state.borrow_mut().processed_files += 1;
                    io.pack_progress(&state.borrow(), false);
                }
                let _ = acknowledge.send(());
                return Ok(read);
            }
        }
    }
}
