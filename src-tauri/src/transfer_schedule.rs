//! 小文件自适应调度；按文件完成率与平均生命周期比较，不改变大文件窗口。
use std::time::{Duration, Instant};
pub(super) const SMALL_BYTES: u64 = 256 * 1024;

pub(super) struct SmallWindow {
    pub limit: usize,
    maximum: usize,
    started: Instant,
    completed: usize,
    latency: f64,
    previous: Option<(f64, f64)>,
}

impl SmallWindow {
    pub fn new(maximum: usize) -> Self {
        Self {
            limit: maximum.min(8),
            maximum,
            started: Instant::now(),
            completed: 0,
            latency: 0.0,
            previous: None,
        }
    }
    pub fn completed(&mut self, elapsed: Duration) {
        self.completed += 1;
        self.latency += elapsed.as_secs_f64();
    }
    pub fn adjust(&mut self, backlogged: bool) {
        if self.started.elapsed() < Duration::from_secs(2) {
            return;
        }
        if backlogged && self.completed > 0 {
            let rate = self.completed as f64 / self.started.elapsed().as_secs_f64();
            let latency = self.latency / self.completed as f64;
            match self.previous {
                Some((previous_rate, previous_latency))
                    if rate < previous_rate * 0.85 && latency > previous_latency * 1.25 =>
                {
                    self.limit = (self.limit / 2).max(2).min(self.maximum);
                }
                None => self.limit = (self.limit * 2).min(self.maximum),
                Some((previous_rate, previous_latency))
                    if rate >= previous_rate * 0.95 && latency < previous_latency * 1.25 =>
                {
                    self.limit = (self.limit * 2).min(self.maximum);
                }
                _ => {}
            }
            self.previous = Some((rate, latency));
        }
        self.completed = 0;
        self.latency = 0.0;
        self.started = Instant::now();
    }
}

/// 调用方按大小降序提供列表；交错队首和队尾，避免微小/空文件集中在尾段。
pub(super) fn interleave<T>(files: Vec<T>) -> std::collections::VecDeque<T> {
    let mut pending = std::collections::VecDeque::from(files);
    let mut ordered = std::collections::VecDeque::new();
    while let Some(larger) = pending.pop_front() {
        ordered.push_back(larger);
        if let Some(smaller) = pending.pop_back() {
            ordered.push_back(smaller);
        }
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interleaved_queue_keeps_all_files_and_admits_tiny_files_early() {
        assert_eq!(interleave(vec![5, 4, 3, 2, 1, 0]), [5, 0, 4, 1, 3, 2]);
        assert_eq!(interleave(vec![3, 2, 1]), [3, 1, 2]);
        assert!(interleave::<u8>(vec![]).is_empty());
    }

    #[test]
    fn small_window_obeys_server_limit_and_reduces_on_congestion() {
        let mut limited = SmallWindow::new(3);
        limited.started = Instant::now() - Duration::from_secs(4);
        limited.completed(Duration::from_millis(100));
        limited.adjust(true);
        assert_eq!(limited.limit, 3);
        let mut window = SmallWindow::new(32);
        assert_eq!(window.limit, 8);
        window.started = Instant::now() - Duration::from_secs(4);
        for _ in 0..20 {
            window.completed(Duration::from_millis(200));
        }
        window.adjust(true);
        assert_eq!(window.limit, 16);
        window.started = Instant::now() - Duration::from_secs(4);
        for _ in 0..2 {
            window.completed(Duration::from_secs(2));
        }
        window.adjust(true);
        assert_eq!(window.limit, 8);
    }
}
