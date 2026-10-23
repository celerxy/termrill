//! 面板独占读取租约；刷新/关闭使旧租约失效，迟到请求不能重新缓存句柄。
use super::*;
use std::{
    collections::HashMap,
    sync::{Mutex, Once, OnceLock},
    time::Duration,
};
fn generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
struct Slot {
    generation: u64,
    leased: bool,
    closed: bool,
    used: Instant,
    saved: Option<Saved>,
}
fn storage() -> &'static Mutex<HashMap<String, Slot>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}
pub(super) struct Lease {
    key: String,
    generation: u64,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut slots) = storage().lock() {
            if let Some(slot) = slots
                .get_mut(&self.key)
                .filter(|slot| slot.generation == self.generation)
            {
                slot.leased = false;
            }
        }
    }
}
pub(super) fn checkout(key: String, refresh: bool) -> Result<(Lease, Option<Saved>)> {
    static MAINTENANCE: Once = Once::new();
    MAINTENANCE.call_once(|| {
        tauri::async_runtime::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if let Ok(mut slots) = storage().lock() {
                    for slot in slots.values_mut() {
                        if slot.saved.as_ref().is_some_and(|saved| {
                            saved
                                .remote
                                .closed
                                .load(std::sync::atomic::Ordering::Relaxed)
                                || saved.remote.native.is_closed()
                                || slot.used.elapsed() >= Duration::from_secs(120)
                        }) {
                            slot.saved.take();
                        }
                    }
                }
            }
        });
    });
    let mut slots = storage()
        .lock()
        .map_err(|_| AppError::new("ZIP 会话缓存锁不可用"))?;
    if !slots.contains_key(&key) && slots.len() >= 64 {
        if let Some(oldest) = slots
            .iter()
            .filter(|(_, slot)| !slot.leased)
            .min_by_key(|(_, slot)| slot.used)
            .map(|(key, _)| key.clone())
        {
            slots.remove(&oldest);
        } else {
            return Err(AppError::new("同时读取的 ZIP 面板超过限制"));
        }
    }
    let slot = slots.entry(key.clone()).or_insert_with(|| Slot {
        generation: generation(),
        leased: false,
        closed: false,
        used: Instant::now(),
        saved: None,
    });
    if refresh {
        slot.generation = generation();
        slot.closed = false;
        slot.leased = false;
        slot.saved.take();
    }
    if slot.closed || slot.leased {
        return Err(AppError::new("ZIP 面板已关闭或上次读取尚未完成"));
    }
    slot.leased = true;
    slot.used = Instant::now();
    Ok((
        Lease {
            key,
            generation: slot.generation,
        },
        slot.saved.take(),
    ))
}
pub(super) fn put(lease: &Lease, saved: Saved) {
    if let Ok(mut slots) = storage().lock() {
        // 至少留出一个句柄预算，避免空闲面板占满小预算服务器。
        let Some(spare) = saved.remote.try_permit() else {
            return;
        };
        drop(spare);
        if !slots
            .get(&lease.key)
            .is_some_and(|slot| !slot.closed && slot.generation == lease.generation)
        {
            return;
        }
        // 最多八个空闲句柄，索引+数据合计最多 64 MiB；在途数据不计入缓存。
        while slots.values().filter(|slot| slot.saved.is_some()).count() >= 8
            || slots
                .values()
                .filter_map(|slot| slot.saved.as_ref())
                .map(Saved::memory)
                .sum::<usize>()
                + saved.memory()
                > 64 * 1024 * 1024
        {
            let oldest = slots
                .iter()
                .filter(|(_, slot)| slot.saved.is_some())
                .min_by_key(|(_, slot)| slot.used)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else {
                return;
            };
            if let Some(slot) = slots.get_mut(&oldest) {
                slot.saved.take();
            }
        }
        if let Some(slot) = slots.get_mut(&lease.key) {
            slot.used = Instant::now();
            slot.saved = Some(saved);
        }
    }
}
pub(super) fn release(key: &str) {
    if let Ok(mut slots) = storage().lock() {
        if let Some(slot) = slots.get_mut(key) {
            slot.closed = true;
            slot.leased = false;
            slot.generation = generation();
            slot.saved.take();
        }
    }
}
