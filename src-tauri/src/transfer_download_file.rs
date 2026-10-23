//! 完成顺序接收与按偏移写盘并行；失败只保留连续前缀，完整校验后安装。
use super::*;
use tokio::io::{AsyncSeekExt, SeekFrom};
struct Destination {
    file: tokio::fs::File,
    prefix: u64,
    received: u64,
    position: u64,
    contiguous: u64,
    ranges: std::collections::BTreeMap<u64, u64>,
    pending: Vec<(u64, u64)>,
    pending_bytes: usize,
    digest: Sha256,
    hashed: u64,
    hash_cache: std::collections::BTreeMap<u64, Vec<u8>>,
    hash_cache_bytes: usize,
    hash_cache_limit: usize,
}
/// 乱序正文的摘要缓存上限；超出的范围只写盘，之后按连续前缀从磁盘补读。
const HASH_CACHE_BYTES: usize = 8 * 1024 * 1024;
/// 从磁盘补读摘要的单次读取长度。
const HASH_READ_BYTES: usize = 256 * 1024;
// 文件上下文与控制信号分别借用，避免复制任务和原子状态。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn file(
    remote: &Remote,
    job: &Job,
    index: usize,
    item: &Item,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    limiter: &crate::transfer_rate::RateLimiter,
    progress: &tokio::sync::mpsc::UnboundedSender<u64>,
) -> Result<String> {
    let temporary = item.local.with_file_name(job.temporary_name(index));
    crate::blocking({
        let temporary = temporary.clone();
        move || {
            local_parents(&temporary)?;
            match fs::symlink_metadata(&temporary) {
                Ok(stat) if local_link(&stat) || !stat.is_file() => {
                    Err(AppError::new("续传临时路径类型无效"))
                }
                Ok(_) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
    })
    .await?;
    let file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&temporary)
        .await?;
    let prefix = file.metadata().await?.len();
    if prefix > item.size {
        return Err(AppError::new("续传临时文件大于源文件"));
    }
    let state = Destination::new(file, prefix, HASH_CACHE_BYTES);
    crate::performance_log!(
        "[TermRill transfer] task={} direction=download file_index={} phase=receive_start bytes={} resumed_bytes={}",
        job.id, index, item.size, prefix
    );
    let finalize = receive(remote, item, state, cancel, pause, limiter, progress).await;
    if finalize.is_err() {
        if cancel.load(Ordering::Relaxed) {
            let _ = tokio::fs::remove_file(&temporary).await;
        }
        return finalize;
    }
    cancelled(cancel, &remote.closed)?;
    crate::blocking({
        let item = item.clone();
        let temporary = temporary.clone();
        let overwrite = job.overwrite;
        move || {
            local_parents(&item.local)?;
            if let Some(stat) = existing(&item.local)? {
                if local_link(&stat) || !stat.is_file() {
                    return Err(AppError::new("目标类型已变化"));
                }
            }
            if overwrite && item.local.exists() {
                crate::local_files::replace(&temporary, &item.local)?;
            } else {
                fs::hard_link(&temporary, &item.local)?;
                fs::remove_file(&temporary)?;
            }
            Ok(())
        }
    })
    .await?;
    finalize
}

// 两个借用协程一起收尾，不派生脱离任务的后台读取。写盘失败时丢弃接收端，
// 唤醒发送端并让读取入口完成 CLOSE；远端校验失败时不安装已缓冲的数据。
#[allow(clippy::too_many_arguments)]
async fn receive(
    remote: &Remote,
    item: &Item,
    mut state: Destination,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    limiter: &crate::transfer_rate::RateLimiter,
    progress: &tokio::sync::mpsc::UnboundedSender<u64>,
) -> Result<String> {
    let started = std::time::Instant::now();
    let queue_wait = std::sync::atomic::AtomicU64::new(0);
    let queue_wait = &queue_wait;
    // 下载块最多 128 KiB：64 块队列最多 8 MiB，四连接在途范围另最多 32 MiB。
    // 至少 4 MiB 的文件受任务并发限制；更小的文件只请求实际范围，
    // 并由调度端的活动字节预算约束同时进行的数量。
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<(u64, Vec<u8>)>(64);
    // 写盘端先失败时，读取端只会得到“写盘已停止”；记录此事实以便选择真实原因。
    let writer_stopped = AtomicBool::new(false);
    let writer_stopped = &writer_stopped;
    let path = item.remote.to_string_lossy();
    let reading = async {
        let result = remote
            .read_download_ranges(&path, item.size, cancel, pause, |offset, chunk| {
                let sender = sender.clone();
                async move {
                    let waiting = std::time::Instant::now();
                    let result = sender.send((offset, chunk)).await.map_err(|_| {
                        writer_stopped.store(true, Ordering::Relaxed);
                        AppError::new("下载写盘已停止")
                    });
                    queue_wait.fetch_add(
                        waiting.elapsed().as_micros().min(u64::MAX as u128) as u64,
                        Ordering::Relaxed,
                    );
                    result
                }
            })
            .await;
        drop(sender);
        result
    };
    let writing = async move {
        let mut disk_time = std::time::Duration::ZERO;
        let mut reported = std::time::Instant::now();
        let result = async {
            while let Some(first) = receiver.recv().await {
                // 只合并已经到达的最多八块，不等待缺口，额外正文最多 1 MiB。
                let mut ready = vec![first];
                while ready.len() < 8 {
                    match receiver.try_recv() {
                        Ok(chunk) => ready.push(chunk),
                        Err(_) => break,
                    }
                }
                for (offset, chunk) in merge_ready(ready) {
                limiter
                    .wait_async(chunk.len(), cancel, &remote.closed, pause)
                    .await?;
                let write_started = std::time::Instant::now();
                state.write_range(offset, &chunk).await?;
                let length = chunk.len();
                state.hash_range(offset, chunk);
                disk_time += write_started.elapsed();
                let _ = progress.send(length as u64);
                }
                let catch_up_started = std::time::Instant::now();
                state.catch_up_digest(cancel, &remote.closed).await?;
                disk_time += catch_up_started.elapsed();
                if reported.elapsed() >= std::time::Duration::from_secs(2) {
                    crate::performance_log!(
                        "download phase=progress bytes={} elapsed_ms={} disk_hash_ms={} queue_wait_ms={}",
                        state.received,
                        started.elapsed().as_millis(),
                        disk_time.as_millis(),
                        queue_wait.load(Ordering::Relaxed) / 1000
                    );
                    reported = std::time::Instant::now();
                }
            }
            state.commit().await?;
            cancelled(cancel, &remote.closed)?;
            if state.received != item.size || state.contiguous != item.size {
                return Err(AppError::new("下载范围不完整，保留已完成的连续前缀"));
            }
            let flushing = std::time::Instant::now();
            state.file.flush().await?;
            state.file.sync_all().await?;
            disk_time += flushing.elapsed();
            crate::performance::record(format_args!(
                "download phase=local_write bytes={} disk_hash_ms={}",
                state.received,
                disk_time.as_millis()
            ));
            // 连续范围在内存中递进 SHA256；超出缓存预算的范围已在接收过程中从磁盘补读。
            // 此处只在补读尚未追上时兜底读取剩余尾部。
            let hashing = std::time::Instant::now();
            state.catch_up_digest(cancel, &remote.closed).await?;
            let digest = state.digest(cancel, &remote.closed, pause).await?;
            crate::performance_log!("download phase=local_digest bytes={} elapsed_ms={}", state.received, hashing.elapsed().as_millis());
            Ok::<_, AppError>((digest, state.received))
        }
        .await;
        // 文件长度不能代表乱序下载的已完成前缀。失败必须裁去空洞和零散尾部，
        // 原有续传前缀保留，并在下次尝试重新逐范围核对，不能跳过验证。
        if result.is_err() {
            let _ = state.commit().await;
            let _ = state.file.set_len(state.prefix.max(state.contiguous)).await;
        }
        result
    };
    let (read, written) = tokio::join!(reading, writing);
    crate::performance::record(format_args!(
        "download phase=receive_write elapsed_ms={} bytes={} success={}",
        started.elapsed().as_millis(),
        item.size,
        read.is_ok() && written.is_ok()
    ));
    cancelled(cancel, &remote.closed)?;
    let (digest, bytes) = settle(read, written, writer_stopped.load(Ordering::Relaxed))?;
    if bytes != item.size {
        return Err(AppError::new("源文件大小发生变化"));
    }
    Ok(digest)
}

/// 选择读取与写盘协程中先发生的真实错误。
///
/// 读取先失败（如连接断开）时，写盘端只看到数据不全并报“下载范围不完整”，
/// 必须返回读取错误；写盘先失败时，读取端只看到“下载写盘已停止”，必须返回写盘错误。
fn settle<T>(read: Result<u64>, written: Result<T>, writer_stopped: bool) -> Result<T> {
    match (read, written) {
        (Ok(_), written) => written,
        (Err(read_error), Ok(_)) => Err(read_error),
        (Err(_), Err(write_error)) if writer_stopped => Err(write_error),
        (Err(read_error), Err(_)) => Err(read_error),
    }
}

fn merge_ready(mut chunks: Vec<(u64, Vec<u8>)>) -> Vec<(u64, Vec<u8>)> {
    chunks.sort_unstable_by_key(|(offset, _)| *offset);
    let mut merged: Vec<(u64, Vec<u8>)> = Vec::new();
    for (offset, chunk) in chunks {
        if let Some((start, previous)) = merged.last_mut() {
            if *start + previous.len() as u64 == offset {
                previous.extend(chunk);
                continue;
            }
        }
        merged.push((offset, chunk));
    }
    merged
}

impl Destination {
    fn new(file: tokio::fs::File, prefix: u64, hash_cache_limit: usize) -> Self {
        Self {
            file,
            prefix,
            received: 0,
            position: 0,
            contiguous: 0,
            ranges: Default::default(),
            pending: Vec::new(),
            pending_bytes: 0,
            digest: Sha256::new(),
            hashed: 0,
            hash_cache: Default::default(),
            hash_cache_bytes: 0,
            hash_cache_limit,
        }
    }

    async fn write_range(&mut self, offset: u64, chunk: &[u8]) -> Result<()> {
        if self.position != offset {
            self.commit().await?;
            self.file.seek(SeekFrom::Start(offset)).await?;
        }
        let old = self.prefix.saturating_sub(offset).min(chunk.len() as u64) as usize;
        if old > 0 {
            self.commit().await?;
            let mut previous = vec![0; old];
            self.file.read_exact(&mut previous).await?;
            if previous != chunk[..old] {
                return Err(AppError::new("续传前缀与远程内容不同，未替换目标"));
            }
        }
        self.file.write_all(&chunk[old..]).await?;
        self.position = offset + chunk.len() as u64;
        self.received += chunk.len() as u64;
        self.pending.push((offset, self.position));
        self.pending_bytes += chunk.len();
        if self.pending_bytes >= 512 * 1024 {
            self.commit().await?;
        }
        Ok(())
    }

    async fn commit(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        // 批量确认 Tokio 内部写入完成，成功后才推进安全续传边界；不逐块 fsync。
        self.file.flush().await?;
        let pending = std::mem::take(&mut self.pending);
        for (start, end) in pending {
            self.complete_range(start, end);
        }
        self.pending_bytes = 0;
        Ok(())
    }

    fn hash_range(&mut self, offset: u64, chunk: Vec<u8>) {
        if offset == self.hashed {
            self.digest.update(&chunk);
            self.hashed += chunk.len() as u64;
            while let Some(next) = self.hash_cache.remove(&self.hashed) {
                self.hash_cache_bytes -= next.len();
                self.digest.update(&next);
                self.hashed += next.len() as u64;
            }
        } else if self.hash_cache_bytes + chunk.len() <= self.hash_cache_limit {
            self.hash_cache_bytes += chunk.len();
            self.hash_cache.insert(offset, chunk);
        }
        // 缺口长期未返回时不能无限缓存正文。超预算的范围已写入临时文件，
        // 由 catch_up_digest 在连续前缀越过后补读，不阻塞请求补充或增大内存上限。
    }

    /// 让摘要追上已确认的连续前缀。
    ///
    /// 每个字节在 `hash_range` 中处理一次：要么进入摘要，要么进入缓存，要么因缓存
    /// 超预算被丢弃。`contiguous` 只在 `commit` 确认写入后推进，所以 `hashed` 落后于
    /// `contiguous` 时，缺失位置必然是已写盘的被丢弃范围。只读取缓存键之间的缺口，
    /// 不重复读取缓存中已有的正文；读取会移动文件游标，结束后更新 `position`，
    /// 使下一次写入重新定位。
    async fn catch_up_digest(&mut self, cancel: &AtomicBool, closed: &AtomicBool) -> Result<()> {
        if self.hashed >= self.contiguous {
            return Ok(());
        }
        let mut buffer = Vec::new();
        while self.hashed < self.contiguous {
            cancelled(cancel, closed)?;
            if let Some(cached) = self.hash_cache.remove(&self.hashed) {
                self.hash_cache_bytes -= cached.len();
                self.digest.update(&cached);
                self.hashed += cached.len() as u64;
                continue;
            }
            let gap_end = self
                .hash_cache
                .range(self.hashed..)
                .next()
                .map_or(self.contiguous, |(&offset, _)| offset)
                .min(self.contiguous);
            let length = (gap_end - self.hashed).min(HASH_READ_BYTES as u64) as usize;
            buffer.resize(length, 0);
            self.file.seek(SeekFrom::Start(self.hashed)).await?;
            self.file.read_exact(&mut buffer).await?;
            self.digest.update(&buffer);
            self.hashed += length as u64;
            self.position = self.hashed;
        }
        Ok(())
    }

    fn complete_range(&mut self, mut start: u64, mut end: u64) {
        if let Some((&previous, &boundary)) = self.ranges.range(..start).next_back() {
            if boundary == start {
                self.ranges.remove(&previous);
                start = previous;
            }
        }
        if let Some(boundary) = self.ranges.remove(&end) {
            end = boundary;
        }
        self.ranges.insert(start, end);
        while let Some(end) = self.ranges.remove(&self.contiguous) {
            self.contiguous = end;
        }
        // 相邻成功范围立即合并，记录数量受在途缺口数约束，不随文件大小增长。
    }

    async fn digest(
        &mut self,
        cancel: &AtomicBool,
        closed: &AtomicBool,
        pause: &AtomicBool,
    ) -> Result<String> {
        if self.hashed == self.received {
            return Ok(format!("{:x}", self.digest.clone().finalize()));
        }
        self.hash_cache.clear();
        self.hash_cache_bytes = 0;
        self.file.seek(SeekFrom::Start(self.hashed)).await?;
        let mut buffer = vec![0; 256 * 1024];
        loop {
            cancelled(cancel, closed)?;
            while pause.load(Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                cancelled(cancel, closed)?;
            }
            let count = self.file.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            self.digest.update(&buffer[..count]);
            self.hashed += count as u64;
        }
        Ok(format!("{:x}", self.digest.clone().finalize()))
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;

    async fn receive_block(state: &mut Destination, source: &[u8], offset: usize) {
        let chunk = source[offset..offset + 8].to_vec();
        state.write_range(offset as u64, &chunk).await.unwrap();
        state.hash_range(offset as u64, chunk);
    }

    #[test]
    fn settle_reports_the_side_that_failed_first() {
        let lost = || Err::<u64, _>(AppError::new("下载数据连接已全部断开"));
        let incomplete = || Err::<(), _>(AppError::new("下载范围不完整，保留已完成的连续前缀"));
        assert_eq!(
            settle(lost(), incomplete(), false).unwrap_err().message,
            "下载数据连接已全部断开",
            "读取先失败时不能被写盘端的范围不完整掩盖"
        );
        let disk = || Err::<(), _>(AppError::new("磁盘已满"));
        let stopped = || Err::<u64, _>(AppError::new("下载写盘已停止"));
        assert_eq!(
            settle(stopped(), disk(), true).unwrap_err().message,
            "磁盘已满"
        );
        assert_eq!(
            settle(lost(), Ok(()), false).unwrap_err().message,
            "下载数据连接已全部断开"
        );
        assert!(settle(Ok(1), Ok(()), false).is_ok());
    }

    #[test]
    fn dropped_digest_ranges_are_read_back_as_prefix_becomes_contiguous() {
        tauri::async_runtime::block_on(async {
            let path = std::env::temp_dir().join(format!(
                "termrill-digest-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let source: Vec<u8> = (0..64u8).collect();
            let file = tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
                .await
                .unwrap();
            let (cancel, closed, pause) = (
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
            );
            // 缓存只容纳一个块：8 进入缓存，16..40 超预算被丢弃。
            let mut state = Destination::new(file, 0, 8);
            for offset in [8, 16, 24, 32, 0] {
                receive_block(&mut state, &source, offset).await;
            }
            assert_eq!(state.hashed, 16, "首块到达后只能接上缓存中的块");
            state.commit().await.unwrap();
            state.catch_up_digest(&cancel, &closed).await.unwrap();
            assert_eq!(
                state.hashed, 40,
                "连续前缀越过缺口后须从磁盘补读被丢弃的范围"
            );
            for offset in [48, 40, 56] {
                receive_block(&mut state, &source, offset).await;
            }
            state.commit().await.unwrap();
            state.catch_up_digest(&cancel, &closed).await.unwrap();
            assert_eq!(state.hashed, 64, "完成时摘要已追上全文，不需要结尾整段重读");
            let digest = state.digest(&cancel, &closed, &pause).await.unwrap();
            assert_eq!(digest, format!("{:x}", Sha256::digest(&source)));
            drop(state);
            let written = tokio::fs::read(&path).await.unwrap();
            tokio::fs::remove_file(&path).await.unwrap();
            assert_eq!(written, source, "补读移动游标后，后续写入仍须落在正确偏移");
        });
    }

    #[test]
    fn arrived_ranges_merge_without_filling_missing_bytes() {
        let merged = super::merge_ready(vec![(8, vec![8, 9]), (2, vec![2, 3]), (0, vec![0, 1])]);
        assert_eq!(merged, vec![(0, vec![0, 1, 2, 3]), (8, vec![8, 9])]);
    }
}
