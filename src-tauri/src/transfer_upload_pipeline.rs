//! 有界读盘/发送流水线；按连续确认范围恢复前缀，不把乱序文件长度视为进度。
use super::*;
use std::collections::BTreeMap;
use tokio::io::BufReader;

// 预算单位为 32 KiB；大分片领取多个单位，扩大分片不扩大总载荷。
pub(super) const MAX_WRITES: usize = 256;
// 所有文件共享总正文预算，每条连接另有八 MiB 硬上限。
pub(super) const GROUP_UNITS: usize = 1024;

fn request_limit(units: usize, block: usize) -> usize {
    (units * BLOCK / block).clamp(1, MAX_WRITES)
}

// 未收到 WRITE 确认前每条连接只试一块；确认后快速倍增，最终仍受原窗口约束。
fn startup_limit(confirmations: usize, maximum: usize) -> usize {
    (1usize << confirmations.min(8)).min(maximum)
}

struct WriteWatchdog {
    since: Instant,
    confirmed: u64,
}
impl WriteWatchdog {
    fn stalled(&mut self, now: Instant, waiting: bool, paused: bool, confirmed: u64) -> bool {
        if !waiting || paused || confirmed != self.confirmed {
            self.since = now;
            self.confirmed = confirmed;
            return false;
        }
        now.saturating_duration_since(self.since) >= Duration::from_secs(30)
    }
}
// 进度消费者可能忙于检查点落盘；正文发送不能等待显示队列。
fn report_progress(
    progress: &tokio::sync::mpsc::Sender<Progress>,
    pending: &mut usize,
) -> Result<()> {
    if *pending == 0 {
        return Ok(());
    }
    match progress.try_send(Progress::Bytes(*pending)) {
        Ok(()) => {
            *pending = 0;
            Ok(())
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Ok(()),
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            Err(AppError::new("上传进度接收器已关闭"))
        }
    }
}

// 已确认的乱序块只保留偏移/长度，正文已在远端；这不是在途正文内存预算。
// 允许快连接领先连续前缀最多 64 MiB，仍保证失败仅恢复完整连续前缀。
const MAX_AHEAD: u64 = 64 * 1024 * 1024;

fn can_dispatch(highest: u64, prefix: u64, block: usize) -> bool {
    highest.saturating_sub(prefix) <= MAX_AHEAD.saturating_sub(block as u64)
}

pub(super) struct Outcome {
    pub result: Result<()>,
    pub prefix: u64,
    pub closed_cleanly: bool,
}

struct Prefix {
    end: u64,
    completed: BTreeMap<u64, usize>,
}
impl Prefix {
    fn acknowledge(&mut self, offset: u64, length: usize) {
        self.completed.insert(offset, length);
        while let Some(length) = self.completed.remove(&self.end) {
            self.end += length as u64;
        }
    }
}

// 每条连接独立比较；仅确认有收益的窗口可复用。
struct Window {
    limit: usize,
    maximum: usize,
    proven: usize,
    baseline: Option<(f64, usize, f64)>,
    warmup: bool,
    samples: usize,
    rate_sum: f64,
    delay_sum: f64,
    reference_delay: f64,
    cooldown: usize,
    queue_samples: usize,
}
impl Window {
    fn new(maximum: usize, initial: usize) -> Self {
        let limit = initial.clamp(16, maximum);
        Self {
            limit,
            maximum,
            proven: limit,
            baseline: None,
            warmup: true,
            samples: 0,
            rate_sum: 0.0,
            delay_sum: 0.0,
            reference_delay: 0.0,
            cooldown: 0,
            queue_samples: 0,
        }
    }
    fn reset_samples(&mut self) {
        self.samples = 0;
        self.rate_sum = 0.0;
        self.delay_sum = 0.0;
        self.warmup = true;
        self.queue_samples = 0;
    }
    fn retract_stalled_probe(&mut self, idle: Duration) -> bool {
        let delay = self
            .baseline
            .map_or(self.reference_delay, |(_, _, delay)| delay);
        let threshold = Duration::from_secs_f64((delay * 3.0).clamp(4.0, 15.0));
        if idle < threshold || self.limit <= 16 {
            return false;
        }
        self.limit = self
            .baseline
            .map_or(self.limit / 2, |(_, limit, _)| limit)
            .max(16);
        self.proven = self.proven.min(self.limit);
        self.baseline = None;
        self.reset_samples();
        self.cooldown = 15;
        true
    }
    // 排队反馈不等待旧代排空或收益比较冷却；两个确认周期持续恶化才收缩。
    fn relieve_queue(&mut self, delay: f64) -> bool {
        let reference = self
            .baseline
            .map_or(self.reference_delay, |(_, _, delay)| delay);
        if reference <= 0.0 {
            self.reference_delay = delay;
            return false;
        }
        if delay <= reference * 1.5 || self.limit <= 16 {
            self.queue_samples = 0;
            return false;
        }
        self.queue_samples += 1;
        if self.queue_samples < 2 {
            return false;
        }
        self.limit = self
            .baseline
            .map_or(self.limit * 3 / 4, |(_, limit, _)| limit)
            .max(16);
        self.proven = self.proven.min(self.limit);
        self.reference_delay = delay;
        self.baseline = None;
        self.reset_samples();
        self.cooldown = 15;
        true
    }
    fn observe(&mut self, rate: f64, delay: f64) {
        if self.cooldown > 0 {
            // 收缩后的连接若连续恢复低延迟，可提前小步探测；不直接恢复旧的大窗口。
            let recovered_delay = if self.reference_delay > 0.0 {
                self.reference_delay * 0.75
            } else {
                2.0
            };
            if self.limit < WRITES && rate > 0.0 && delay > 0.0 && delay < recovered_delay {
                self.cooldown = self.cooldown.min(3);
            }
            self.cooldown -= 1;
            return;
        }
        if self.warmup {
            self.warmup = false;
            return;
        }
        if rate <= 0.0 {
            return;
        }
        self.samples += 1;
        self.rate_sum += rate;
        self.delay_sum += delay;
        if self.samples < 3 {
            return;
        }
        let rate = self.rate_sum / self.samples as f64;
        let delay = self.delay_sum / self.samples as f64;
        self.samples = 0;
        self.rate_sum = 0.0;
        self.delay_sum = 0.0;
        if let Some((previous_rate, previous_limit, previous_delay)) = self.baseline.take() {
            if rate > previous_rate * 1.1
                && (previous_delay == 0.0 || delay <= previous_delay * 1.25)
            {
                self.proven = self.limit;
                self.reference_delay = delay;
            } else {
                self.limit = previous_limit;
                self.cooldown = 15;
                self.warmup = true;
                return;
            }
        } else if self.reference_delay > 0.0 && delay > self.reference_delay * 1.5 {
            self.limit = (self.limit * 3 / 4).max(16);
            self.proven = self.proven.min(self.limit);
            self.reference_delay = delay;
            self.cooldown = 5;
            self.warmup = true;
            return;
        } else {
            self.reference_delay = delay;
        }
        if self.limit < self.maximum {
            self.baseline = Some((rate, self.limit, delay));
            self.limit = (self.limit + self.limit.div_ceil(4)).min(self.maximum);
            self.warmup = true;
        } else {
            self.cooldown = 5;
        }
    }
}

// 历史只用于启动分配，实时确认继续更新；过期或无效提示回到保守初值。
fn recent_rate(sample: Option<(Instant, f64)>, now: Instant) -> f64 {
    sample
        .filter(|(sampled, rate)| {
            rate.is_finite()
                && *rate > 0.0
                && now
                    .checked_duration_since(*sampled)
                    .is_some_and(|age| age <= Duration::from_secs(30))
        })
        .map_or(1024.0 * 1024.0, |(_, rate)| rate)
}

// 以估计排队时间分配下一块；仍遵守各连接硬预算，零速连接保留探测机会。
fn select_connection(
    active: &[usize],
    rates: &[f64],
    limits: &[usize],
    budgets: &[tokio::sync::Semaphore],
    units: usize,
    tail_latencies: Option<&[f64]>,
) -> Option<usize> {
    active
        .iter()
        .enumerate()
        // 文件自己的名额不足以表示可发送；其他文件可能已经占满共享字节预算。
        .filter(|(slot, count)| {
            **count < limits[*slot] && budgets[*slot].available_permits() >= units
        })
        .min_by(|(left, left_count), (right, right_count)| {
            let bytes = (units * BLOCK) as f64;
            let cost = |slot: usize, count: usize| {
                let queued = (count + 1) as f64 * bytes / rates[slot].max(1.0);
                // 收尾比较近期平滑确认耗时；连接变慢后不能继续使用历史最短值。
                tail_latencies.map_or(queued, |latencies| queued.max(latencies[slot]))
            };
            let left_cost = cost(*left, **left_count);
            let right_cost = cost(*right, **right_count);
            left_cost.total_cmp(&right_cost)
        })
        .map(|(slot, _)| slot)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn write(
    connections: &[&ssh::Connection],
    budgets: &[tokio::sync::Semaphore],
    group_budget: &tokio::sync::Semaphore,
    temporary: &str,
    first_handle: Option<(String, bool)>,
    source: &mut tokio::fs::File,
    digest: &mut Sha256,
    start: u64,
    size: u64,
    cancel: &AtomicBool,
    closed: &AtomicBool,
    pause: &AtomicBool,
    limiter: &crate::transfer_rate::RateLimiter,
    progress: &tokio::sync::mpsc::Sender<Progress>,
    outstanding: &std::sync::atomic::AtomicUsize,
) -> Outcome {
    let handles_started = Instant::now();
    let mut handles = Vec::new();
    let mut failure = None;
    // 不跨连接串行等待 OPEN/FSTAT。收齐结果后才写正文，失败也保留成功句柄用于关闭。
    let mut opening = futures::stream::iter(connections.iter().enumerate())
        .map(|(slot, connection)| {
            let existing = if slot == 0 {
                first_handle.clone()
            } else {
                None
            };
            async move {
                // 新建排他文件的原始句柄仍指向已创建的普通空文件；无需再发 FSTAT。
                // 续传、条带重新 OPEN 的句柄仍须核对类型和起始长度。
                let fresh_handle = existing.as_ref().is_some_and(|(_, fresh)| *fresh) && start == 0;
                let handle = match existing {
                    Some((handle, _)) => Ok(handle),
                    None => controlled(
                        connection
                            .sftp
                            .open(temporary, Flags::WRITE, FileAttributes::default()),
                        cancel,
                        closed,
                    )
                    .await
                    .map(|value| value.handle),
                };
                let handle = match handle {
                    Ok(handle) => handle,
                    Err(error) => return (None, Some(error)),
                };
                let error = if fresh_handle {
                    None
                } else {
                    match controlled(connection.sftp.fstat(handle.clone()), cancel, closed).await {
                        Ok(stat) if regular(&stat.attrs) && stat.attrs.size == Some(start) => None,
                        Ok(_) => Some(AppError::new("并行上传临时文件状态已变化")),
                        Err(error) => Some(error),
                    }
                };
                (Some(handle), error)
            }
        })
        .buffered(connections.len().max(1));
    while let Some((handle, error)) = opening.next().await {
        handles.push(handle);
        if let Some(error) = error {
            failure.get_or_insert(error);
        }
    }
    crate::performance_log!(
        "[TermRill upload] phase=handles_prepare connections={} elapsed_ms={} success={}",
        connections.len(),
        handles_started.elapsed().as_millis(),
        failure.is_none()
    );
    let mut prefix = Prefix {
        end: start,
        completed: BTreeMap::new(),
    };
    if failure.is_none() {
        let stopped = AtomicBool::new(false);
        let awaiting_write = std::sync::atomic::AtomicUsize::new(0);
        let block = connections
            .iter()
            .map(|connection| connection.write_size)
            .min()
            .unwrap_or(BLOCK)
            .min(if size >= 4 * 1024 * 1024 {
                128 * 1024
            } else {
                BLOCK
            });
        let (sender, mut receiver) =
            tokio::sync::mpsc::channel::<(u64, Vec<u8>)>((64 * BLOCK / block).clamp(1, 64));
        let producer = async {
            let mut reader = BufReader::with_capacity(1024 * 1024, source);
            let mut offset = start;
            while offset < size {
                cancelled(cancel, closed)?;
                cancelled(&stopped, closed)?;
                while pause.load(Ordering::Relaxed) {
                    cancelled(cancel, closed)?;
                    cancelled(&stopped, closed)?;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                let length = limiter.block_size(block).min((size - offset) as usize);
                let mut bytes = vec![0; length];
                controlled(reader.read_exact(&mut bytes), &stopped, closed).await?;
                digest.update(&bytes);
                controlled(
                    async {
                        sender
                            .send((offset, bytes))
                            .await
                            .map_err(|_| AppError::new("上传发送入口已关闭"))
                    },
                    &stopped,
                    closed,
                )
                .await?;
                offset += length as u64;
            }
            drop(sender);
            Ok::<_, AppError>(())
        };
        let consumer = async {
            let mut writes = FuturesUnordered::new();
            let mut active = vec![0usize; connections.len()];
            let mut confirmed_requests = vec![0usize; connections.len()];
            let mut finished_reading = false;
            let mut highest = start;
            let mut failure = None;
            let mut windows: Vec<_> = connections
                .iter()
                .map(|connection| {
                    Window::new(MAX_WRITES, connection.upload_window.load(Ordering::Relaxed))
                })
                .collect();
            crate::performance_log!("[TermRill upload] phase=pipeline_start connections={} window_policy=per_connection group_budget_bytes={} maximum_connection_bytes={} startup_requests_per_connection=1 no_progress_timeout_secs=30", connections.len(), GROUP_UNITS * BLOCK, MAX_WRITES * BLOCK);
            let mut generation = vec![0u64; connections.len()];
            let mut previous_requests = vec![0usize; connections.len()];
            let mut unpaused_generation = vec![0u64; connections.len()];
            let rate_now = Instant::now();
            let mut scheduling_rates: Vec<_> = connections
                .iter()
                .map(|connection| {
                    // 提示锁不可用时仍可传输，不能让缓存状态破坏文件操作。
                    recent_rate(
                        connection
                            .upload_rate
                            .lock()
                            .ok()
                            .and_then(|sample| *sample),
                        rate_now,
                    )
                })
                .collect();
            let mut recent_ack_seconds = vec![0.0f64; connections.len()];
            let mut sample = Instant::now();
            let mut sample_bytes = 0u64;
            let mut connection_bytes = vec![0u64; connections.len()];
            let mut transport_samples: Vec<_> = connections
                .iter()
                .map(|connection| {
                    (
                        connection.sent_transport.load(Ordering::Relaxed),
                        connection.received_transport.load(Ordering::Relaxed),
                        connection.received_data.load(Ordering::Relaxed),
                    )
                })
                .collect();
            let mut previous_connection_bytes = connection_bytes.clone();
            let mut acknowledgement_micros = vec![0u128; connections.len()];
            let mut acknowledgement_count = vec![0u64; connections.len()];
            let mut queue_micros = vec![0u128; connections.len()];
            let mut queue_count = vec![0u64; connections.len()];
            let mut acknowledged = 0u64;
            let mut last_confirmation = vec![Instant::now(); connections.len()];
            let mut connection_sample = vec![Instant::now(); connections.len()];
            let transfer_started = Instant::now();
            let mut watchdog = WriteWatchdog {
                since: transfer_started,
                confirmed: 0,
            };
            let mut paused_since: Option<Instant> = None;
            let mut paused_duration = Duration::ZERO;
            let mut pending_progress = 0usize;
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            loop {
                if finished_reading && writes.is_empty() {
                    break;
                }
                let limits: Vec<_> = windows
                    .iter()
                    .enumerate()
                    .map(|(slot, window)| {
                        startup_limit(confirmed_requests[slot], request_limit(window.limit, block))
                    })
                    .collect();
                let available = if group_budget.available_permits() >= block.div_ceil(BLOCK) {
                    select_connection(
                        &active,
                        &scheduling_rates,
                        &limits,
                        budgets,
                        block.div_ceil(BLOCK),
                        (size.saturating_sub(highest)
                            <= (windows.iter().map(|window| window.limit).sum::<usize>() * BLOCK)
                                as u64)
                            .then_some(recent_ack_seconds.as_slice()),
                    )
                } else {
                    None
                };
                tokio::select! {
                    result = writes.next(), if !writes.is_empty() => {
                        let (slot, offset, length, elapsed, request_generation, result): (usize, u64, usize, Duration, u64, Result<()>) = result.expect("已检查请求非空");
                        active[slot] -= 1;
                        let current_sample = request_generation == generation[slot] && !pause.load(Ordering::Relaxed);
                        if request_generation < generation[slot] {
                            previous_requests[slot] -= 1;
                            if previous_requests[slot] == 0 {
                                connection_sample[slot] = Instant::now();
                                previous_connection_bytes[slot] = connection_bytes[slot];
                                acknowledgement_micros[slot] = 0;
                                acknowledgement_count[slot] = 0;
                            }
                        }
                        match result {
                            Ok(()) => {
                                confirmed_requests[slot] = confirmed_requests[slot].saturating_add(1);
                                last_confirmation[slot] = Instant::now();
                                let delay = elapsed.as_secs_f64();
                                if request_generation >= unpaused_generation[slot] && !pause.load(Ordering::Relaxed) && delay > 0.0 {
                                    queue_micros[slot] += elapsed.as_micros();
                                    queue_count[slot] += 1;
                                    recent_ack_seconds[slot] = if recent_ack_seconds[slot] == 0.0 {
                                        delay
                                    } else {
                                        recent_ack_seconds[slot] * 0.75 + delay * 0.25
                                    };
                                }
                                prefix.acknowledge(offset, length);
                                acknowledged += length as u64;
                                connection_bytes[slot] += length as u64;
                                if current_sample {
                                    acknowledgement_micros[slot] += elapsed.as_micros();
                                    acknowledgement_count[slot] += 1;
                                }
                                pending_progress += length;
                                if pending_progress >= 256 * 1024 {
                                    if let Err(error) = report_progress(progress, &mut pending_progress) {
                                        failure.get_or_insert(error);
                                    }
                                }
                            }
                            Err(error) => { failure.get_or_insert(error); }
                        }
                    }
                    chunk = async {
                        let slot = available.expect("接收条件已检查预算");
                        // 先取得两级预算，再从读盘队列取正文；排队领取预算的请求不持有正文。
                        // select 被取消时许可自动释放，不丢失尚未读取的块。
                        let connection_permit = budgets[slot].acquire_many(block.div_ceil(BLOCK) as u32).await.map_err(|_| AppError::new("上传请求预算已关闭"))?;
                        let group_permit = group_budget.acquire_many(block.div_ceil(BLOCK) as u32).await.map_err(|_| AppError::new("上传总预算已关闭"))?;
                        Ok::<_, AppError>(receiver.recv().await.map(|(offset, bytes)| (offset, bytes, connection_permit, group_permit)))
                    }, if !finished_reading && failure.is_none() && !pause.load(Ordering::Relaxed) && available.is_some() && can_dispatch(highest, prefix.end, block) => {
                        match chunk {
                            Ok(Some((offset, bytes, connection_permit, group_permit))) => {
                                let slot = available.expect("接收条件已检查预算");
                                let connection = connections[slot];
                                let handle = handles[slot].as_ref().expect("所有句柄已通过打开和校验").clone();
                                let length = bytes.len();
                                highest = offset + length as u64;
                                active[slot] += 1;
                                let request_generation = generation[slot];
                                let awaiting_write = &awaiting_write;
                                writes.push(async move {
                                    let mut started = None;
                                    let result = async {
                                        let _budget = connection_permit;
                                        let _group = group_permit;
                                        limiter.wait_async(length, cancel, closed, pause).await?;
                                        struct Guard<'a>(&'a std::sync::atomic::AtomicUsize, &'a std::sync::atomic::AtomicUsize);
                                        impl Drop for Guard<'_> { fn drop(&mut self) { self.0.fetch_sub(1, Ordering::Relaxed); self.1.fetch_sub(1, Ordering::Relaxed); } }
                                        outstanding.fetch_add(1, Ordering::Relaxed);
                                        awaiting_write.fetch_add(1, Ordering::Relaxed);
                                        let _guard = Guard(outstanding, awaiting_write);
                                        started = Some(Instant::now());
                                        controlled(connection.sftp.write(handle, offset, bytes), cancel, closed).await?;
                                        Ok::<_, AppError>(())
                                    }.await;
                                    (slot, offset, length, started.map_or(Duration::ZERO, |started| started.elapsed()), request_generation, result)
                                });
                            }
                            Ok(None) => { finished_reading = true; }
                            Err(error) => { failure.get_or_insert(error); }
                        }
                    }
                    _ = tick.tick() => {
                        if let Err(error) = cancelled(cancel, closed) { failure.get_or_insert(error); }
                        if let Err(error) = report_progress(progress, &mut pending_progress) { failure.get_or_insert(error); }
                    }
                }
                if failure.is_some() {
                    stopped.store(true, Ordering::Relaxed);
                    break;
                }
                // 只计算已过限速器、真正等待 WRITE 的时间；暂停、限速和读盘等待不计入。
                if watchdog.stalled(
                    Instant::now(),
                    awaiting_write.load(Ordering::Relaxed) > 0,
                    pause.load(Ordering::Relaxed),
                    acknowledged,
                ) {
                    crate::performance_log!("[TermRill upload] phase=write_stalled timeout_secs=30 confirmed_bytes={} dispatched_bytes={} pending_requests={}",acknowledged,highest,awaiting_write.load(Ordering::Relaxed));
                    failure = Some(AppError {
                        message:
                            "上传写入超时：连续 30 秒未收到确认，请检查网络和服务器后重试；未替换目标文件"
                                .into(),
                        host_key: None,
                        timeout: true,
                    });
                    stopped.store(true, Ordering::Relaxed);
                    break;
                }
                if pause.load(Ordering::Relaxed) {
                    if paused_since.is_none() {
                        paused_since = Some(Instant::now());
                        crate::performance_log!("[TermRill upload] phase=pause");
                    }
                    last_confirmation.fill(Instant::now());
                    connection_sample.fill(Instant::now());
                    sample = Instant::now();
                    sample_bytes = acknowledged;
                    previous_connection_bytes.clone_from(&connection_bytes);
                    acknowledgement_micros.fill(0);
                    acknowledgement_count.fill(0);
                    queue_micros.fill(0);
                    queue_count.fill(0);
                    continue;
                }
                if let Some(started) = paused_since.take() {
                    paused_duration += started.elapsed();
                    for slot in 0..connections.len() {
                        windows[slot].reset_samples();
                        generation[slot] += 1;
                        unpaused_generation[slot] = generation[slot];
                        previous_requests[slot] = active[slot];
                    }
                    crate::performance_log!(
                        "[TermRill upload] phase=resume paused_ms={}",
                        paused_duration.as_millis()
                    );
                }
                for slot in 0..connections.len() {
                    if active[slot] > 0
                        && windows[slot].retract_stalled_probe(last_confirmation[slot].elapsed())
                    {
                        generation[slot] += 1;
                        previous_requests[slot] = active[slot];
                        last_confirmation[slot] = Instant::now();
                        crate::performance_log!("[TermRill upload] phase=window_stall_retract slot={} window_bytes={} pending_requests={}",slot,windows[slot].limit * BLOCK,active[slot]);
                    }
                }
                if sample.elapsed() >= Duration::from_secs(2) {
                    let rate =
                        (acknowledged - sample_bytes) as f64 / sample.elapsed().as_secs_f64();
                    for slot in 0..connections.len() {
                        let connection = connections[slot];
                        let current = (
                            connection.sent_transport.load(Ordering::Relaxed),
                            connection.received_transport.load(Ordering::Relaxed),
                            connection.received_data.load(Ordering::Relaxed),
                        );
                        let previous = transport_samples[slot];
                        crate::performance_log!("[TermRill upload] phase=transport_sample slot={} sent_ssh_bytes={} received_ssh_bytes={} received_sftp_data_bytes={}",slot,current.0.saturating_sub(previous.0),current.1.saturating_sub(previous.1),current.2.saturating_sub(previous.2));
                        transport_samples[slot] = current;
                        if queue_count[slot] < 2 {
                            windows[slot].queue_samples = 0;
                        }
                        let queue_delay = queue_micros[slot] as f64
                            / queue_count[slot].max(1) as f64
                            / 1_000_000.0;
                        if active[slot] > 0
                            && !finished_reading
                            && queue_count[slot] >= 2
                            && windows[slot].relieve_queue(queue_delay)
                        {
                            generation[slot] += 1;
                            previous_requests[slot] = active[slot];
                            crate::performance_log!("[TermRill upload] phase=window_queue_retract slot={} ack_mean_ms={} window_bytes={} pending_requests={}",slot,queue_delay * 1000.0,windows[slot].limit * BLOCK,active[slot]);
                        }
                        queue_micros[slot] = 0;
                        queue_count[slot] = 0;
                        let elapsed = connection_sample[slot].elapsed();
                        if elapsed < Duration::from_secs(2) {
                            continue;
                        }
                        let confirmed_rate =
                            (connection_bytes[slot] - previous_connection_bytes[slot]) as f64
                                / elapsed.as_secs_f64();
                        let mean_micros = acknowledgement_micros[slot]
                            / u128::from(acknowledgement_count[slot].max(1));
                        if active[slot] > 0 && !finished_reading {
                            scheduling_rates[slot] = scheduling_rates[slot] * 0.75
                                + confirmed_rate.max(block as f64 / 2.0) * 0.25;
                            if acknowledgement_count[slot] > 0 {
                                if let Ok(mut hint) = connections[slot].upload_rate.lock() {
                                    *hint = Some((Instant::now(), scheduling_rates[slot]));
                                }
                                if previous_requests[slot] == 0 {
                                    let previous_limit = windows[slot].limit;
                                    windows[slot]
                                        .observe(confirmed_rate, mean_micros as f64 / 1_000_000.0);
                                    if windows[slot].limit != previous_limit {
                                        generation[slot] += 1;
                                        previous_requests[slot] = active[slot];
                                    }
                                }
                            }
                        }
                        crate::performance_log!("[TermRill upload] phase=connection_sample slot={} bytes_per_second={} active_requests={} request_limit={} write_ack_mean_us={} samples={} window_bytes={}",slot,confirmed_rate as u64,active[slot],startup_limit(confirmed_requests[slot],request_limit(windows[slot].limit,block)),mean_micros,acknowledgement_count[slot],windows[slot].limit * BLOCK);
                        previous_connection_bytes[slot] = connection_bytes[slot];
                        acknowledgement_micros[slot] = 0;
                        acknowledgement_count[slot] = 0;
                        connection_sample[slot] = Instant::now();
                    }
                    crate::performance_log!("[TermRill upload] phase=pipeline connections={} window_bytes={} confirmed_bytes={} contiguous_bytes={} bytes_per_second={} dispatched_bytes={} prefix_blocked={} group_available_bytes={}",connections.len(),windows.iter().map(|window|window.limit * BLOCK).sum::<usize>(),acknowledged,prefix.end,rate as u64,highest,!can_dispatch(highest,prefix.end,block),group_budget.available_permits() * BLOCK);
                    sample = Instant::now();
                    sample_bytes = acknowledged;
                }
            }
            stopped.store(true, Ordering::Relaxed);
            // 网络请求已收尾，剩余进度必须交付后才能进入安装确认。
            if pending_progress > 0
                && progress
                    .send(Progress::Bytes(pending_progress))
                    .await
                    .is_err()
            {
                failure.get_or_insert(AppError::new("上传进度接收器已关闭"));
            }
            if failure.is_none() && prefix.end != size {
                failure = Some(AppError::new("上传确认范围不完整"));
            }
            if failure.is_none() {
                for (connection, window) in connections.iter().zip(&windows) {
                    connection
                        .upload_window
                        .store(window.proven, Ordering::Relaxed);
                }
            }
            if let Some(started) = paused_since {
                paused_duration += started.elapsed();
            }
            let active_time = transfer_started.elapsed().saturating_sub(paused_duration);
            crate::performance_log!("[TermRill upload] phase=write_timing elapsed_ms={} paused_ms={} active_ms={} active_bytes_per_second={}",transfer_started.elapsed().as_millis(),paused_duration.as_millis(),active_time.as_millis(),acknowledged as f64 / active_time.as_secs_f64().max(0.001));
            failure.map_or(Ok(()), Err)
        };
        let (produced, consumed) = tokio::join!(producer, consumer);
        failure = consumed.err().or_else(|| produced.err());
    }
    // CLOSE 排在各连接 WRITE 之后；只有所有 CLOSE 确认后才能裁剪并恢复续传前缀。
    let mut closes = FuturesUnordered::new();
    let cleanup = failure.is_some();
    for (connection, handle) in connections.iter().zip(handles) {
        let Some(handle) = handle else {
            continue;
        };
        closes.push(async move {
            if cleanup {
                tokio::time::timeout(Duration::from_secs(5), connection.sftp.close(handle))
                    .await
                    .map_err(|_| AppError::new("上传失败后关闭句柄等待超过 5 秒"))?
                    .map_err(AppError::from)
            } else {
                // 正常提交遵守原有协议等待预算；五秒只用于失败清理，不能误伤慢服务器。
                controlled(connection.sftp.close(handle), cancel, closed).await
            }
        });
    }
    let mut closed_cleanly = true;
    while let Some(result) = closes.next().await {
        if let Err(error) = result {
            closed_cleanly = false;
            failure.get_or_insert(install_stage_error("关闭上传文件", error));
        }
    }
    Outcome {
        result: failure.map_or(Ok(()), Err),
        prefix: prefix.end,
        closed_cleanly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn silent_server_cannot_fill_cached_large_window_before_first_confirmation() {
        let cached = request_limit(MAX_WRITES, 128 * 1024);
        assert_eq!(startup_limit(0, cached), 1);
        assert_eq!(startup_limit(1, cached), 2);
        assert!(startup_limit(8, cached) == cached);
        assert_eq!(startup_limit(100, 1), 1);
    }
    #[test]
    fn watchdog_excludes_pause_idle_and_rate_wait_but_detects_unconfirmed_writes() {
        let begin = Instant::now();
        let mut watch = WriteWatchdog {
            since: begin,
            confirmed: 0,
        };
        assert!(!watch.stalled(begin + Duration::from_secs(29), true, false, 0));
        assert!(watch.stalled(begin + Duration::from_secs(30), true, false, 0));
        assert!(!watch.stalled(begin + Duration::from_secs(90), true, true, 0));
        assert!(!watch.stalled(begin + Duration::from_secs(100), true, false, 0));
        assert!(!watch.stalled(begin + Duration::from_secs(120), true, false, 128 * 1024));
        assert!(!watch.stalled(begin + Duration::from_secs(149), true, false, 128 * 1024));
        assert!(watch.stalled(begin + Duration::from_secs(150), true, false, 128 * 1024));
        assert!(!watch.stalled(begin + Duration::from_secs(200), false, false, 128 * 1024));
        assert!(!watch.stalled(begin + Duration::from_secs(220), true, false, 128 * 1024));
    }
    fn select_connection(active: &[usize], rates: &[f64], limit: usize) -> Option<usize> {
        let budgets: Vec<_> = active
            .iter()
            .map(|_| tokio::sync::Semaphore::new(MAX_WRITES))
            .collect();
        super::select_connection(active, rates, &vec![limit; active.len()], &budgets, 1, None)
    }
    #[test]
    fn shared_budget_full_connection_does_not_hide_an_available_connection() {
        let budgets = [
            tokio::sync::Semaphore::new(8),
            tokio::sync::Semaphore::new(8),
        ];
        let busy = budgets[0].try_acquire_many(6).unwrap();
        // 128 KiB 请求需要四个预算单位；速度最快的连接只剩两个。
        assert_eq!(
            super::select_connection(&[0, 0], &[400.0, 100.0], &[8, 8], &budgets, 4, None),
            Some(1)
        );
        let other = budgets[1].try_acquire_many(8).unwrap();
        assert_eq!(
            super::select_connection(&[0, 0], &[400.0, 100.0], &[8, 8], &budgets, 4, None),
            None
        );
        drop(busy);
        assert_eq!(
            super::select_connection(&[0, 0], &[400.0, 100.0], &[8, 8], &budgets, 4, None),
            Some(0)
        );
        drop(other);
    }
    #[test]
    fn reused_speed_hint_expires_and_cannot_disable_a_connection() {
        let now = Instant::now();
        assert_eq!(recent_rate(Some((now, 4_000_000.0)), now), 4_000_000.0);
        assert_eq!(
            recent_rate(Some((now - Duration::from_secs(31), 4_000_000.0)), now),
            1_048_576.0
        );
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(recent_rate(Some((now, rate)), now), 1_048_576.0);
        }
        assert_eq!(recent_rate(None, now), 1_048_576.0);
    }
    #[test]
    fn tail_prefers_earlier_confirmation_over_a_slow_empty_queue() {
        let budgets = [
            tokio::sync::Semaphore::new(64),
            tokio::sync::Semaphore::new(64),
        ];
        let rates = [1024.0 * 1024.0; 2];
        let latencies = [0.1, 2.0];
        assert_eq!(
            super::select_connection(&[1, 0], &rates, &[8, 8], &budgets, 4, None),
            Some(1)
        );
        assert_eq!(
            super::select_connection(&[1, 0], &rates, &[8, 8], &budgets, 4, Some(&latencies)),
            Some(0)
        );
        assert_eq!(
            super::select_connection(&[8, 0], &rates, &[8, 8], &budgets, 4, Some(&latencies)),
            Some(1),
            "收尾仍遵守预算，并保留可用连接"
        );
    }
    #[test]
    fn slow_progress_consumer_preserves_bytes_without_blocking_network() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        assert!(sender.try_send(Progress::Bytes(32 * 1024)).is_ok());
        let mut pending = 256 * 1024;
        report_progress(&sender, &mut pending).unwrap();
        assert_eq!(pending, 256 * 1024);
        pending += 128 * 1024;
        assert!(matches!(
            receiver.try_recv().unwrap(),
            Progress::Bytes(32768)
        ));
        report_progress(&sender, &mut pending).unwrap();
        assert_eq!(pending, 0);
        assert!(matches!(
            receiver.try_recv().unwrap(),
            Progress::Bytes(393216)
        ));
        drop(receiver);
        pending = 1;
        assert!(report_progress(&sender, &mut pending).is_err());
    }
    #[test]
    fn scheduling_favors_fast_connections_without_exceeding_budget() {
        assert_eq!(select_connection(&[4, 2], &[400.0, 100.0], 8), Some(0));
        assert_eq!(select_connection(&[8, 1], &[400.0, 100.0], 8), Some(1));
        assert_eq!(select_connection(&[8, 2], &[400.0, 100.0], 8), Some(1));
        assert_eq!(select_connection(&[8, 8], &[400.0, 100.0], 8), None);
        assert_eq!(select_connection(&[0, 0], &[0.0, 100.0], 8), Some(1));
    }

    #[test]
    fn independent_windows_keep_only_measured_gain_and_probe_in_small_steps() {
        let mut fast = Window::new(MAX_WRITES, WRITES);
        let mut slow = Window::new(MAX_WRITES, WRITES);
        for _ in 0..4 {
            fast.observe(100.0, 1.0);
            slow.observe(100.0, 1.0);
        }
        assert_eq!(fast.limit, 80);
        assert_eq!(fast.proven, WRITES);
        for _ in 0..4 {
            fast.observe(130.0, 1.1);
            slow.observe(95.0, 2.0);
        }
        assert_eq!(fast.proven, 80);
        assert_eq!(fast.limit, 100);
        assert_eq!(slow.limit, WRITES);
    }
    #[test]
    fn queue_growth_retracts_before_probe_drain_and_ignores_isolated_delay_spikes() {
        let mut window = Window::new(MAX_WRITES, WRITES);
        for _ in 0..4 {
            window.observe(100.0, 1.0);
        }
        assert_eq!(window.limit, 80);
        assert!(!window.relieve_queue(2.0));
        assert!(!window.relieve_queue(1.0), "孤立尖峰不能触发回退");
        assert!(!window.relieve_queue(2.0));
        assert!(window.relieve_queue(2.0));
        assert_eq!(window.limit, WRITES);
        assert!(window.baseline.is_none());
        assert_eq!(window.proven, WRITES);
        // 冷却期仍可收缩持续恶化的稳定窗口，不依赖吞吐比较。
        assert!(!window.relieve_queue(4.0));
        assert!(window.relieve_queue(4.0));
        assert_eq!(window.limit, 48);
    }
    #[test]
    fn recovered_connection_reprobes_early_without_restoring_unverified_large_window() {
        let mut recovered = Window::new(MAX_WRITES, WRITES);
        let mut slow = Window::new(MAX_WRITES, WRITES);
        assert!(recovered.retract_stalled_probe(Duration::from_secs(4)));
        assert!(slow.retract_stalled_probe(Duration::from_secs(4)));
        for _ in 0..7 {
            recovered.observe(1_000_000.0, 0.5);
            slow.observe(10_000.0, 5.0);
        }
        assert_eq!(recovered.limit, 40);
        assert_eq!(recovered.proven, 32, "恢复后只小步探测，不能直接恢复二 MiB");
        assert_eq!(slow.limit, 32);
    }
    #[test]
    fn stalled_connection_reduces_backlog_but_retains_recovery_capacity() {
        let mut window = Window::new(MAX_WRITES, WRITES);
        assert!(!window.retract_stalled_probe(Duration::from_secs(3)));
        assert!(window.retract_stalled_probe(Duration::from_secs(4)));
        assert_eq!(window.limit, 32);
        assert!(window.retract_stalled_probe(Duration::from_secs(20)));
        assert_eq!(window.limit, 16);
        assert!(!window.retract_stalled_probe(Duration::from_secs(60)));
        for _ in 0..19 {
            window.observe(100.0, 5.0);
        }
        assert_eq!(window.limit, 20, "小窗口仍能逐步探测恢复");
    }
    #[tokio::test]
    async fn shared_body_budget_allows_fast_connection_to_borrow_without_exceeding_total() {
        let group = tokio::sync::Semaphore::new(GROUP_UNITS);
        let connections: Vec<_> = (0..8)
            .map(|_| tokio::sync::Semaphore::new(MAX_WRITES))
            .collect();
        let fast = connections[0]
            .acquire_many(MAX_WRITES as u32)
            .await
            .unwrap();
        let body = group.acquire_many(MAX_WRITES as u32).await.unwrap();
        assert_eq!(connections[1].available_permits(), MAX_WRITES);
        let rest = group
            .acquire_many((GROUP_UNITS - MAX_WRITES) as u32)
            .await
            .unwrap();
        assert!(group.try_acquire().is_err());
        drop(body);
        drop(fast);
        drop(rest);
        assert_eq!(group.available_permits(), GROUP_UNITS);
    }
    #[test]
    fn larger_blocks_preserve_the_byte_window() {
        assert_eq!(request_limit(64, 128 * 1024), 16);
        assert_eq!(
            request_limit(MAX_WRITES, 128 * 1024) * 128 * 1024,
            MAX_WRITES * BLOCK
        );
    }
    #[test]
    fn confirmed_out_of_order_ranges_allow_refill_without_unbounded_ahead_dispatch() {
        let prefix = 10 * 1024 * 1024;
        let block = 128 * 1024;
        assert!(can_dispatch(prefix + 32 * 1024 * 1024, prefix, block));
        assert!(can_dispatch(
            prefix + MAX_AHEAD - block as u64,
            prefix,
            block
        ));
        assert!(!can_dispatch(prefix + MAX_AHEAD, prefix, block));
        assert!(!can_dispatch(prefix + MAX_AHEAD + 1, prefix, block));
    }
    #[test]
    fn out_of_order_confirmations_never_promote_a_hole() {
        let mut prefix = Prefix {
            end: 7,
            completed: BTreeMap::new(),
        };
        prefix.acknowledge(17, 10);
        assert_eq!(prefix.end, 7);
        prefix.acknowledge(7, 10);
        assert_eq!(prefix.end, 27);
        prefix.acknowledge(37, 2);
        assert_eq!(prefix.end, 27);
    }
}
