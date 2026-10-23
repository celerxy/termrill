//! 普通下载的有界正文预算与停收检测；不改变认证、预览或写入重试语义。
use std::time::Duration;
use tokio::time::Instant;

pub(super) const BUDGET_UNIT: usize = 32 * 1024;
pub(super) const BUDGET_UNITS: usize = 32 * 1024 * 1024 / BUDGET_UNIT;
const SAMPLE: Duration = Duration::from_secs(1);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

struct IdleRead {
    sampled: Instant,
    bytes: u64,
    idle: Duration,
}
impl IdleRead {
    fn observe(&mut self, now: Instant, bytes: u64, paused: bool) -> bool {
        let elapsed = now.saturating_duration_since(self.sampled);
        // 流未被轮询时可能在等待消费/写盘，不能把这段反压当成网络停收。
        if paused || bytes != self.bytes || elapsed > SAMPLE * 3 {
            self.idle = Duration::ZERO;
        } else {
            self.idle += elapsed;
        }
        self.sampled = now;
        self.bytes = bytes;
        self.idle >= IDLE_TIMEOUT
    }
}

/// 只有已进入网络 READ 的等待才计时。半块 DATA 持续到达也重置计时；
/// 同连接其他 READ 的正文同样说明连接仍活跃。取消由调用方 checked 负责。
pub(super) async fn while_receiving<T>(
    future: impl std::future::Future<Output = T>,
    received: impl Fn() -> u64,
    paused: impl Fn() -> bool,
) -> Option<T> {
    let mut state = IdleRead {
        sampled: Instant::now(),
        bytes: received(),
        idle: Duration::ZERO,
    };
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            result = &mut future => return Some(result),
            _ = tokio::time::sleep(SAMPLE) => {
                if state.observe(Instant::now(), received(), paused()) {
                    return None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_read_distinguishes_partial_data_and_consumer_backpressure() {
        let start = Instant::now();
        let mut state = IdleRead {
            sampled: start,
            bytes: 0,
            idle: Duration::ZERO,
        };
        for second in 1..30 {
            assert!(!state.observe(start + SAMPLE * second, 0, false));
        }
        assert!(
            !state.observe(start + SAMPLE * 30, 1, false),
            "半块正文也须重置停收时间"
        );
        assert!(
            !state.observe(start + SAMPLE * 90, 1, false),
            "写盘反压不计为网络停收"
        );
        for second in 91..120 {
            assert!(!state.observe(start + SAMPLE * second, 1, false));
        }
        assert!(
            !state.observe(start + SAMPLE * 120, 1, true),
            "显式暂停不隔离连接"
        );
        for second in 121..150 {
            assert!(!state.observe(start + SAMPLE * second, 1, false));
        }
        assert!(state.observe(start + SAMPLE * 150, 1, false));
    }

    #[test]
    fn ready_network_result_does_not_wait_for_sample() {
        tauri::async_runtime::block_on(async {
            assert_eq!(
                while_receiving(std::future::ready(42), || 0, || false).await,
                Some(42)
            );
        });
    }
}
