//! 下载共享尚未派发的分块；快连接领取更多工作，每条连接保留独立且有界的在途预算。
use futures::{
    future::BoxFuture,
    stream::{self, BoxStream, FuturesUnordered},
    StreamExt,
};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowPolicy {
    Transfer,
    Preview,
    BulkPreview,
}
/// 多连接普通下载的起步窗口；空闲连接可以领取其余未派发范围。
const TRANSFER_INITIAL_REQUESTS: usize = 8;
/// 普通下载慢连接最少保留的在途请求数，128 KiB 分片时为 512 KiB。
const TRANSFER_MINIMUM_REQUESTS: usize = 4;

impl WindowPolicy {
    pub(super) fn initial(self, connections: usize, window: usize) -> usize {
        match self {
            _ if connections == 1 => window,
            Self::Transfer => window.min(TRANSFER_INITIAL_REQUESTS),
            Self::Preview => window.min(8),
            Self::BulkPreview => window,
        }
    }
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Transfer => "adaptive_transfer",
            Self::Preview => "adaptive_preview",
            Self::BulkPreview => "fixed_preview",
        }
    }
    /// 根据刚完成请求从派发到收齐的耗时调整该连接的在途上限。
    ///
    /// 耗时包含同一连接上前序 DATA 的排队时间，约等于“该连接已领取的工作需要多久收完”。
    /// 普通下载让每条连接保留约 1–4 秒自身吞吐量的工作：低于 1 秒说明窗口限制了吞吐，
    /// 加倍；超过 4 秒说明该连接拥塞或丢包，减半但不少于四个请求，把剩余范围留给
    /// 其他连接领取。高延迟高带宽链路的耗时随窗口增大才超过 1 秒，此时窗口已覆盖往返。
    /// 预览保持原有更短的阈值，且不低于起步窗口。
    fn next_limit(
        self,
        current: usize,
        initial: usize,
        maximum: usize,
        elapsed: Duration,
    ) -> usize {
        let (grow_below, shrink_above, minimum) = match self {
            Self::BulkPreview => return maximum,
            Self::Transfer => (
                Duration::from_secs(1),
                Duration::from_secs(4),
                TRANSFER_MINIMUM_REQUESTS.min(initial),
            ),
            Self::Preview => (Duration::from_millis(500), Duration::from_secs(2), initial),
        };
        if elapsed <= grow_below {
            maximum.min(current.saturating_mul(2))
        } else if elapsed >= shrink_above {
            (current / 2).max(minimum)
        } else {
            current
        }
    }
}

struct Schedule<'a, T> {
    next: u64,
    cursor: usize,
    limits: Vec<usize>,
    active: Vec<usize>,
    samples: Vec<WindowSamples>,
    pending: FuturesUnordered<BoxFuture<'a, (usize, Duration, T)>>,
}

#[derive(Default)]
struct WindowSamples {
    completed: usize,
    elapsed: Duration,
}
impl WindowSamples {
    fn record(&mut self, elapsed: Duration, window: usize) -> Option<Duration> {
        self.completed += 1;
        self.elapsed += elapsed;
        // 一批响应只评价一次窗口，避免同批慢响应连续把 64 缩到 4，
        // 或前三个快速响应就把 8 扩到 64，给共用代理出口堆积突发数据。
        if self.completed < window {
            return None;
        }
        let mean = self.elapsed / self.completed as u32;
        *self = Self::default();
        Some(mean)
    }
}

/// `available(slot)` 为 false 的连接不再领取新范围；其在途请求由调用方转交并收齐。
/// 全部连接都不可用且没有在途请求时流提前结束，调用方必须核对收到的总长度。
pub(super) fn ranges<'a, F, Fut, A>(
    size: u64,
    block: u64,
    connections: usize,
    window: usize,
    policy: WindowPolicy,
    request: &'a F,
    available: &'a A,
) -> BoxStream<'a, Fut::Output>
where
    F: Fn(usize, u64) -> Fut + Sync,
    Fut: std::future::Future + Send + 'a,
    Fut::Output: Send + 'a,
    A: Fn(usize) -> bool + Sync,
{
    assert!(block > 0 && connections > 0 && window > 0);
    // 多连接时普通下载与预览都先小窗口起步，避免文件在开始时就被全部平分给各连接；
    // 慢连接只持有自身窗口，其余范围留给先空闲的连接。整包预览仍固定填满窗口。
    let initial = policy.initial(connections, window);
    let schedule = Schedule {
        next: 0,
        cursor: 0,
        limits: vec![initial; connections],
        active: vec![0; connections],
        samples: (0..connections).map(|_| WindowSamples::default()).collect(),
        pending: FuturesUnordered::new(),
    };
    stream::unfold(schedule, move |mut schedule| async move {
        while schedule.next < size {
            let slot = (0..connections)
                .map(|step| (schedule.cursor + step) % connections)
                .find(|&slot| schedule.active[slot] < schedule.limits[slot] && available(slot));
            let Some(slot) = slot else {
                break;
            };
            let offset = schedule.next;
            schedule.next = size.min(offset.saturating_add(block));
            schedule.cursor = (slot + 1) % connections;
            schedule.active[slot] += 1;
            // 派发时立即创建请求，使连接归属在可用性判定的同一时刻确定。
            let requested = request(slot, offset);
            schedule.pending.push(Box::pin(async move {
                let started = Instant::now();
                let result = requested.await;
                (slot, started.elapsed(), result)
            }));
        }
        let (slot, elapsed, result) = schedule.pending.next().await?;
        schedule.active[slot] -= 1;
        if connections > 1 && policy == WindowPolicy::Transfer {
            if let Some(mean) = schedule.samples[slot].record(elapsed, schedule.limits[slot]) {
                let previous = schedule.limits[slot];
                schedule.limits[slot] = policy.next_limit(previous, initial, window, mean);
                crate::performance_log!("download phase=read_window connection={} previous_requests={} requests={} mean_response_ms={}",
                    slot, previous, schedule.limits[slot], mean.as_millis());
            }
        } else if connections > 1 {
            // 等待包含前序 DATA 的排队时间，不能据此把预读压到两块。
            // 缩窗保留策略规定的下限，避免高延迟链路在每批响应间断粮；不重放已发送 READ。
            schedule.limits[slot] =
                policy.next_limit(schedule.limits[slot], initial, window, elapsed);
        }
        Some((result, schedule))
    })
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn slow_response_batch_changes_window_once_instead_of_repeatedly_halving() {
        let mut samples = WindowSamples::default();
        for _ in 0..63 {
            assert!(samples.record(Duration::from_secs(5), 64).is_none());
        }
        let mean = samples.record(Duration::from_secs(5), 64).unwrap();
        let next = WindowPolicy::Transfer.next_limit(64, 8, 64, mean);
        assert_eq!(next, 32);
        for _ in 0..31 {
            assert!(samples.record(Duration::from_millis(500), next).is_none());
        }
        let mean = samples.record(Duration::from_millis(500), next).unwrap();
        assert_eq!(WindowPolicy::Transfer.next_limit(next, 8, 64, mean), 64);
    }

    #[test]
    fn ordinary_download_window_tracks_each_connection_backlog() {
        assert_eq!(WindowPolicy::Transfer.initial(3, 64), 8);
        assert_eq!(WindowPolicy::Transfer.initial(1, 64), 64);
        assert_eq!(WindowPolicy::Preview.initial(3, 64), 8);
        assert_eq!(WindowPolicy::Preview.initial(1, 64), 64);
        assert_eq!(WindowPolicy::BulkPreview.initial(3, 64), 64);
        assert_eq!(
            WindowPolicy::BulkPreview.next_limit(64, 64, 64, Duration::from_secs(14)),
            64
        );
        // 高延迟链路：0.8 秒内收齐仍说明窗口偏小，继续加倍。
        assert_eq!(
            WindowPolicy::Transfer.next_limit(8, 8, 64, Duration::from_millis(800)),
            16
        );
        assert_eq!(
            WindowPolicy::Transfer.next_limit(32, 8, 64, Duration::from_secs(2)),
            32
        );
        assert_eq!(
            WindowPolicy::Transfer.next_limit(64, 8, 64, Duration::from_secs(14)),
            32
        );
        assert_eq!(
            WindowPolicy::Transfer.next_limit(8, 8, 64, Duration::from_secs(14)),
            TRANSFER_MINIMUM_REQUESTS,
            "丢包连接只保留下限窗口，其余范围留给其他连接"
        );
        assert_eq!(
            WindowPolicy::Transfer.next_limit(4, 8, 64, Duration::from_secs(50)),
            TRANSFER_MINIMUM_REQUESTS
        );
        assert_eq!(
            WindowPolicy::Preview.next_limit(32, 8, 64, Duration::from_secs(14)),
            16
        );
        tauri::async_runtime::block_on(async {
            let started: Vec<_> = (0..3).map(|_| AtomicUsize::new(0)).collect();
            let gates: Vec<_> = (0..3).map(|_| tokio::sync::Semaphore::new(0)).collect();
            let request = |slot: usize, offset: u64| {
                let (started, gate) = (&started[slot], &gates[slot]);
                async move {
                    started.fetch_add(1, Ordering::Relaxed);
                    gate.acquire().await.unwrap().forget();
                    offset
                }
            };
            let mut chunks = ranges(513, 1, 3, 64, WindowPolicy::Transfer, &request, &|_| true);
            assert!(futures::poll!(chunks.next()).is_pending());
            for count in &started {
                assert_eq!(
                    count.load(Ordering::Relaxed),
                    8,
                    "多连接开始时不能把整个文件平分给各连接"
                );
            }
            for gate in &gates {
                gate.add_permits(513);
            }
            let mut offsets = chunks.collect::<Vec<_>>().await;
            offsets.sort_unstable();
            assert_eq!(offsets, (0..513).collect::<Vec<_>>());
        });
    }

    #[test]
    fn fast_connection_claims_unassigned_ranges_without_duplicates_or_exceeding_windows() {
        for policy in [WindowPolicy::Preview, WindowPolicy::Transfer] {
            claim_unassigned_ranges(policy);
        }
    }

    fn claim_unassigned_ranges(policy: WindowPolicy) {
        tauri::async_runtime::block_on(async {
            let started: Vec<_> = (0..4).map(|_| AtomicUsize::new(0)).collect();
            let active: Vec<_> = (0..4).map(|_| AtomicUsize::new(0)).collect();
            let gates: Vec<_> = (0..4).map(|_| tokio::sync::Semaphore::new(0)).collect();
            let request = |slot: usize, offset: u64| {
                let (started, active, gate) = (&started[slot], &active[slot], &gates[slot]);
                async move {
                    started.fetch_add(1, Ordering::Relaxed);
                    assert!(active.fetch_add(1, Ordering::Relaxed) < 64);
                    gate.acquire().await.unwrap().forget();
                    active.fetch_sub(1, Ordering::Relaxed);
                    offset
                }
            };
            let mut pending = ranges(1025, 8, 4, 64, policy, &request, &|_| true);
            assert!(futures::poll!(pending.next()).is_pending());
            let slow_reserved: usize = started[1..].iter().map(|n| n.load(Ordering::Relaxed)).sum();
            assert!(slow_reserved > 0 && slow_reserved < 129 / 2);
            gates[0].add_permits(129);
            let mut offsets = Vec::new();
            for _ in 0..129 - slow_reserved {
                offsets.push(pending.next().await.unwrap());
            }
            assert!(
                started[0].load(Ordering::Relaxed) > 129 / 2,
                "快连接必须能领取原固定分配中属于慢连接的未发送分块"
            );
            for gate in &gates[1..] {
                gate.add_permits(64);
            }
            offsets.extend(pending.collect::<Vec<_>>().await);
            offsets.sort_unstable();
            assert_eq!(offsets, (0..1025).step_by(8).collect::<Vec<_>>());
            for count in active {
                assert_eq!(count.load(Ordering::Relaxed), 0);
            }
        });
    }

    #[test]
    fn serialized_slow_connection_receives_less_work_than_fixed_striping() {
        tauri::async_runtime::block_on(async {
            let gate = tokio::sync::Semaphore::new(1);
            let request = |slot: usize, offset: u64| {
                let gate = &gate;
                async move {
                    if slot == 1 {
                        let _permit = gate.acquire().await.unwrap();
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    } else {
                        tokio::task::yield_now().await;
                    }
                    (offset, slot)
                }
            };
            let started = Instant::now();
            let fixed = stream::select_all((0..3).map(|slot| {
                stream::iter(
                    (slot..96)
                        .step_by(3)
                        .map(move |offset| request(slot as usize, offset)),
                )
                .buffer_unordered(64)
                .boxed()
            }))
            .collect::<Vec<_>>()
            .await;
            let fixed_ms = started.elapsed().as_millis();
            let started = Instant::now();
            let adaptive = ranges(96, 1, 3, 64, WindowPolicy::Preview, &request, &|_| true)
                .collect::<Vec<_>>()
                .await;
            let slow =
                |values: &[(u64, usize)]| values.iter().filter(|(_, slot)| *slot == 1).count();
            assert!(
                slow(&adaptive) < slow(&fixed) / 2,
                "慢连接不能继续背负固定三分之一的文件"
            );
            let mut offsets: Vec<_> = adaptive.iter().map(|(offset, _)| *offset).collect();
            offsets.sort_unstable();
            assert_eq!(offsets, (0..96).collect::<Vec<_>>());
            println!("DOWNLOAD_SCHEDULER fixed_ms={fixed_ms} adaptive_ms={} fixed_slow_blocks={} adaptive_slow_blocks={}",
                started.elapsed().as_millis(), slow(&fixed), slow(&adaptive));
        });
    }

    #[test]
    fn queued_slow_responses_preserve_initial_prefetch_window() {
        tauri::async_runtime::block_on(async {
            let started = [AtomicUsize::new(0), AtomicUsize::new(0)];
            let gates = [
                tokio::sync::Semaphore::new(0),
                tokio::sync::Semaphore::new(0),
            ];
            let request = |slot: usize, offset: u64| {
                let (started, gate) = (&started[slot], &gates[slot]);
                async move {
                    started.fetch_add(1, Ordering::Relaxed);
                    gate.acquire().await.unwrap().forget();
                    offset
                }
            };
            let mut pending = ranges(100, 1, 2, 64, WindowPolicy::Preview, &request, &|_| true);
            assert!(futures::poll!(pending.next()).is_pending());
            assert_eq!(started[0].load(Ordering::Relaxed), 8);
            tokio::time::sleep(Duration::from_millis(2050)).await;
            gates[0].add_permits(1);
            assert!(pending.next().await.is_some());
            assert!(futures::poll!(pending.next()).is_pending());
            assert_eq!(
                started[0].load(Ordering::Relaxed),
                9,
                "长响应后仍须补足八个预读请求，不能误把排队延迟当成缩至两块的依据"
            );
            gates[0].add_permits(100);
            gates[1].add_permits(100);
            let remaining = pending.collect::<Vec<_>>().await;
            assert_eq!(remaining.len(), 99);
        });
    }

    #[test]
    fn unavailable_connection_receives_no_new_ranges() {
        tauri::async_runtime::block_on(async {
            let lost = std::sync::atomic::AtomicBool::new(false);
            let started_after_loss = AtomicUsize::new(0);
            let request = |slot: usize, offset: u64| {
                // 派发时记录，断开前已派发的在途请求不计入。
                if slot == 1 && lost.load(Ordering::Relaxed) {
                    started_after_loss.fetch_add(1, Ordering::Relaxed);
                }
                async move {
                    tokio::task::yield_now().await;
                    offset
                }
            };
            let available = |slot: usize| slot != 1 || !lost.load(Ordering::Relaxed);
            let mut pending = ranges(200, 1, 3, 64, WindowPolicy::Transfer, &request, &available);
            let mut offsets = Vec::new();
            for _ in 0..10 {
                offsets.push(pending.next().await.unwrap());
            }
            lost.store(true, Ordering::Relaxed);
            offsets.extend(pending.collect::<Vec<_>>().await);
            offsets.sort_unstable();
            assert_eq!(
                offsets,
                (0..200).collect::<Vec<_>>(),
                "断开连接已领取的范围收齐，其余范围由存活连接完成，不重复不遗漏"
            );
            assert_eq!(
                started_after_loss.load(Ordering::Relaxed),
                0,
                "断开的连接不能再领取新范围"
            );
        });
    }

    #[test]
    fn empty_and_single_connection_keep_exact_ranges() {
        tauri::async_runtime::block_on(async {
            let request = |slot, offset| std::future::ready((slot, offset));
            assert!(
                ranges(0, 8, 1, 64, WindowPolicy::Transfer, &request, &|_| true)
                    .collect::<Vec<_>>()
                    .await
                    .is_empty()
            );
            let mut result = ranges(17, 8, 1, 64, WindowPolicy::Transfer, &request, &|_| true)
                .collect::<Vec<_>>()
                .await;
            result.sort_unstable();
            assert_eq!(result, vec![(0, 0), (0, 8), (0, 16)]);
        });
    }
}
