//! 只读面板快照缓存：共享字节避免复制整个归档；全局容量与空闲过期限制。
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

const LIMIT: usize = 128 * 1024 * 1024;
struct Snapshot {
    key: String,
    revision: String,
    bytes: Arc<[u8]>,
    used: Instant,
}
fn storage() -> &'static Mutex<VecDeque<Snapshot>> {
    static CACHE: OnceLock<Mutex<VecDeque<Snapshot>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}
fn expire(values: &mut VecDeque<Snapshot>) {
    values.retain(|value| value.used.elapsed() < Duration::from_secs(120));
}
pub fn get(key: &str, revision: Option<&str>) -> Option<(Arc<[u8]>, String)> {
    let mut values = storage().lock().ok()?;
    expire(&mut values);
    let index = values
        .iter()
        .position(|value| value.key == key && Some(value.revision.as_str()) == revision)?;
    let mut value = values.remove(index)?;
    value.used = Instant::now();
    let result = (value.bytes.clone(), value.revision.clone());
    values.push_back(value);
    Some(result)
}
pub fn insert(key: String, revision: String, bytes: Arc<[u8]>) {
    if bytes.len() > LIMIT {
        return;
    }
    if let Ok(mut values) = storage().lock() {
        expire(&mut values);
        values.retain(|value| value.key != key);
        while values.iter().map(|value| value.bytes.len()).sum::<usize>() + bytes.len() > LIMIT {
            values.pop_front();
        }
        values.push_back(Snapshot {
            key,
            revision,
            bytes,
            used: Instant::now(),
        });
    }
}

/// 关闭或刷新面板时同时清除该面板不同密码对应的解码快照。
pub fn remove_panel(key: &str) {
    if let Ok(mut values) = storage().lock() {
        let prefix = format!("{key}\0");
        values.retain(|value| value.key != key && !value.key.starts_with(&prefix));
    }
}
