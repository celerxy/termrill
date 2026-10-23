//! 单任务共享的字节预算；并发线程预约发送时间，等待时仍响应暂停和取消。
use crate::model::*;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

pub struct RateLimiter {
    rate: u64,
    next: Mutex<Instant>,
}

impl RateLimiter {
    pub fn new(rate: u64) -> Self {
        Self {
            rate,
            next: Mutex::new(Instant::now()),
        }
    }
    pub fn block_size(&self, capacity: usize) -> usize {
        if self.rate == 0 {
            capacity
        } else {
            capacity.min((self.rate / 10).max(1) as usize)
        }
    }
    #[cfg(test)]
    pub fn wait(
        &self,
        bytes: usize,
        cancel: &AtomicBool,
        closed: &AtomicBool,
        pause: &AtomicBool,
        mut on_pause: impl FnMut(),
    ) -> Result<()> {
        if self.rate == 0 {
            return Ok(());
        }
        let deadline = {
            let mut next = lock(&self.next)?;
            *next = (*next).max(Instant::now())
                + Duration::from_secs_f64(bytes as f64 / self.rate as f64);
            *next
        };
        loop {
            if cancel.load(Ordering::Relaxed) || closed.load(Ordering::Relaxed) {
                return Err(AppError::new("传输已停止"));
            }
            if pause.load(Ordering::Relaxed) {
                on_pause();
            }
            if !pause.load(Ordering::Relaxed) && Instant::now() >= deadline {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /// 协程预约与旧执行器共用同一任务预算；等待期间释放运行时线程。
    pub async fn wait_async(
        &self,
        bytes: usize,
        cancel: &AtomicBool,
        closed: &AtomicBool,
        pause: &AtomicBool,
    ) -> Result<()> {
        let deadline = if self.rate == 0 {
            Instant::now()
        } else {
            let mut next = lock(&self.next)?;
            *next = (*next).max(Instant::now())
                + Duration::from_secs_f64(bytes as f64 / self.rate as f64);
            *next
        };
        loop {
            crate::transfer::cancelled(cancel, closed)?;
            if !pause.load(Ordering::Relaxed) && Instant::now() >= deadline {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asynchronous_budget_is_shared_and_cancel_interrupts_wait() {
        tauri::async_runtime::block_on(async {
            let limiter = RateLimiter::new(1000);
            let cancel = AtomicBool::new(false);
            let closed = AtomicBool::new(false);
            let pause = AtomicBool::new(false);
            let start = Instant::now();
            // 两个协程合计预约 200 字节，不能各自获得完整任务带宽。
            let (first, second) = tokio::join!(
                limiter.wait_async(100, &cancel, &closed, &pause),
                limiter.wait_async(100, &cancel, &closed, &pause),
            );
            first.unwrap();
            second.unwrap();
            assert!(start.elapsed() >= Duration::from_millis(190));
            cancel.store(true, Ordering::Relaxed);
            assert!(limiter
                .wait_async(100, &cancel, &closed, &pause)
                .await
                .is_err());
        });
    }
}
