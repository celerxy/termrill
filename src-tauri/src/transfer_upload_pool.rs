//! 上传连接独占租用：成功归还，失败销毁；SSH 保活由协议驱动，随主会话断开释放。
use super::*;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

struct Idle {
    id: u64,
    closed: Arc<AtomicBool>,
    trusted: BTreeMap<String, String>,
    connection: ssh::Connection,
}

fn pool() -> &'static Mutex<Vec<Idle>> {
    static POOL: OnceLock<Mutex<Vec<Idle>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(Vec::new()))
}

struct Warmup {
    closed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}
fn warmups() -> &'static Mutex<Vec<Warmup>> {
    static WARMUPS: OnceLock<Mutex<Vec<Warmup>>> = OnceLock::new();
    WARMUPS.get_or_init(|| Mutex::new(Vec::new()))
}

// 一次性串行预热，任务开始后不继续补建；已在握手的连接收齐结果后归还或关闭。
pub(in crate::transfer) fn warm(
    primary: Arc<Mutex<crate::ssh::Connection>>,
    backend: Arc<Backend>,
) {
    let (request, closed, algorithms) = {
        let Ok(primary) = lock(&primary) else {
            return;
        };
        (
            primary.request.clone(),
            primary.closed.clone(),
            primary.host_algorithms.clone(),
        )
    };
    drop(primary);
    if request.profile.target.auth == "interactive"
        || request
            .profile
            .jump
            .as_ref()
            .is_some_and(|jump| jump.auth == "interactive")
    {
        return;
    }
    let (trusted, count) = {
        let Ok(config) = lock(&backend.config) else {
            return;
        };
        (
            upload_connections::session_trust(&request, &config.known_hosts),
            config.transfer_settings.concurrency.clamp(1, 4),
        )
    };
    let stop = Arc::new(AtomicBool::new(false));
    {
        let Ok(mut tasks) = lock(warmups()) else {
            return;
        };
        if tasks
            .iter()
            .any(|entry| Arc::ptr_eq(&entry.closed, &closed))
        {
            return;
        }
        tasks.push(Warmup {
            closed: closed.clone(),
            stop: stop.clone(),
        });
    }
    tauri::async_runtime::spawn(async move {
        for _ in 0..count {
            if closed.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
                break;
            }
            let started = Instant::now();
            let result = controlled(
                ssh::Connection::connect_upload(&request, &trusted, &algorithms, Some(&backend)),
                &closed,
                &closed,
            )
            .await;
            crate::performance_log!(
                "[TermRill upload] phase=pool_warmup success={} elapsed_ms={}",
                result.is_ok(),
                started.elapsed().as_millis()
            );
            match result {
                Ok(connection) => {
                    if put(connection, closed.clone(), trusted.clone())
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                // 不后台重试指纹或认证失败，也不触发交互验证码提示。
                Err(_) => break,
            }
        }
        if let Ok(mut tasks) = lock(warmups()) {
            tasks.retain(|entry| !Arc::ptr_eq(&entry.stop, &stop));
        }
    });
}

#[cfg(test)]
pub(in crate::transfer) fn take(
    closed: &Arc<AtomicBool>,
    trusted: &BTreeMap<String, String>,
) -> Result<Option<ssh::Connection>> {
    take_mode(closed, trusted, false)
}

pub(in crate::transfer) fn take_upload(
    closed: &Arc<AtomicBool>,
    trusted: &BTreeMap<String, String>,
) -> Result<Option<ssh::Connection>> {
    take_mode(closed, trusted, true)
}

fn take_mode(
    closed: &Arc<AtomicBool>,
    trusted: &BTreeMap<String, String>,
    upload_uncompressed: bool,
) -> Result<Option<ssh::Connection>> {
    // 即使首条连接尚未准备好，也阻止预热在本任务期间继续启动新握手。
    {
        let tasks = lock(warmups())?;
        for entry in tasks
            .iter()
            .filter(|entry| Arc::ptr_eq(&entry.closed, closed))
        {
            entry.stop.store(true, Ordering::Relaxed);
        }
    }
    let mut idle = lock(pool())?;
    let index = idle.iter().position(|entry| {
        Arc::ptr_eq(&entry.closed, closed)
            && !closed.load(Ordering::Relaxed)
            && entry.trusted == *trusted
            && entry.connection.upload_uncompressed == upload_uncompressed
            && !entry.connection.is_closed()
    });
    Ok(index.map(|index| idle.swap_remove(index).connection))
}

pub(in crate::transfer) async fn put(
    connection: ssh::Connection,
    closed: Arc<AtomicBool>,
    trusted: BTreeMap<String, String>,
) -> Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut connection = Some(connection);
    let idle_limit = if connection
        .as_ref()
        .is_some_and(|value| value.upload_uncompressed)
    {
        8
    } else {
        4
    };
    let accepted = {
        let mut idle = lock(pool())?;
        if idle
            .iter()
            .filter(|entry| {
                Arc::ptr_eq(&entry.closed, &closed)
                    && connection.as_ref().is_some_and(|connection| {
                        entry.connection.upload_uncompressed == connection.upload_uncompressed
                    })
            })
            .count()
            >= idle_limit
            || closed.load(Ordering::Relaxed)
            || connection
                .as_ref()
                .is_none_or(|connection| connection.is_closed())
        {
            false
        } else {
            idle.push(Idle {
                id,
                closed: closed.clone(),
                trusted,
                connection: connection.take().expect("入池之前持有连接"),
            });
            true
        }
    };
    if !accepted {
        if let Some(connection) = connection {
            connection.close().await;
        }
        return Ok(());
    }
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let expired = closed.load(Ordering::Relaxed);
            let connection = {
                let Ok(mut idle) = pool().lock() else {
                    return;
                };
                let Some(index) = idle.iter().position(|entry| entry.id == id) else {
                    return;
                };
                if expired || idle[index].connection.is_closed() {
                    Some(idle.swap_remove(index).connection)
                } else {
                    None
                }
            };
            if let Some(connection) = connection {
                connection.close().await;
                return;
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_upload_stops_only_its_sessions_pending_warmup() {
        let closed = Arc::new(AtomicBool::new(false));
        let other = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let other_stop = Arc::new(AtomicBool::new(false));
        {
            let mut tasks = lock(warmups()).unwrap();
            tasks.push(Warmup {
                closed: closed.clone(),
                stop: stop.clone(),
            });
            tasks.push(Warmup {
                closed: other,
                stop: other_stop.clone(),
            });
        }
        assert!(take_upload(&closed, &BTreeMap::new()).unwrap().is_none());
        assert!(stop.load(Ordering::Relaxed));
        assert!(!other_stop.load(Ordering::Relaxed));
        lock(warmups()).unwrap().retain(|entry| {
            !Arc::ptr_eq(&entry.stop, &stop) && !Arc::ptr_eq(&entry.stop, &other_stop)
        });
    }
    #[test]
    #[ignore = "Requires loopback fixture"]
    fn login_prepares_an_upload_lease_before_the_first_task() {
        let ready: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var("TERMRILL_TEST_FIXTURE").unwrap()).unwrap(),
        )
        .unwrap();
        let directory =
            std::path::PathBuf::from(ready["root"].as_str().unwrap()).join(crate::identifier());
        std::fs::create_dir_all(&directory).unwrap();
        let backend = Arc::new(crate::tests::backend(&directory));
        lock(&backend.config).unwrap().transfer_settings.concurrency = 1;
        let request = crate::tests::request(ready["targetPort"].as_u64().unwrap() as u16);
        let primary = Arc::new(Mutex::new(crate::tests::verified(
            request.clone(),
            &backend,
        )));
        let closed = lock(&primary).unwrap().closed.clone();
        let trusted = upload_connections::session_trust(
            &request,
            &lock(&backend.config).unwrap().known_hosts,
        );
        warm(primary.clone(), backend);
        tauri::async_runtime::block_on(async {
            tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    let ready = lock(pool())
                        .unwrap()
                        .iter()
                        .any(|entry| Arc::ptr_eq(&entry.closed, &closed));
                    if ready {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            let connection = take_upload(&closed, &trusted)
                .unwrap()
                .expect("首个上传任务可直接复用预热连接");
            assert!(connection.upload_uncompressed);
            assert!(!connection.is_closed());
            connection.close().await;
        });
    }
    #[test]
    #[ignore = "Requires loopback fixture"]
    fn idle_upload_connection_is_reused_only_by_its_verified_session() {
        let ready: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var("TERMRILL_TEST_FIXTURE").unwrap()).unwrap(),
        )
        .unwrap();
        let directory =
            std::path::PathBuf::from(ready["root"].as_str().unwrap()).join(crate::identifier());
        std::fs::create_dir_all(&directory).unwrap();
        let backend = crate::tests::backend(&directory);
        let request = crate::tests::request(ready["targetPort"].as_u64().unwrap() as u16);
        let primary = crate::tests::verified(request.clone(), &backend);
        let closed = primary.closed.clone();
        let trusted = lock(&backend.config).unwrap().known_hosts.clone();
        tauri::async_runtime::block_on(async {
            let ordinary = ssh::Connection::connect(
                &request,
                &trusted,
                &primary.host_algorithms,
                Some(&backend),
            )
            .await
            .unwrap();
            put(ordinary, closed.clone(), trusted.clone())
                .await
                .unwrap();
            assert!(
                take_upload(&closed, &trusted).unwrap().is_none(),
                "上传不能租用普通压缩策略连接"
            );
            let ordinary = take(&closed, &trusted)
                .unwrap()
                .expect("普通连接仍属于压缩策略池");
            assert!(!ordinary.upload_uncompressed);
            ordinary.close().await;
            let native = ssh::Connection::connect_upload(
                &request,
                &trusted,
                &primary.host_algorithms,
                Some(&backend),
            )
            .await
            .unwrap();
            let trusted = upload_connections::session_trust(&request, &trusted);
            *native.upload_rate.lock().unwrap() = Some((Instant::now(), 2_000_000.0));
            put(native, closed.clone(), trusted.clone()).await.unwrap();
            assert!(
                take(&closed, &trusted).unwrap().is_none(),
                "压缩策略池不能租用无压缩传输连接"
            );
            let another_session = Arc::new(AtomicBool::new(false));
            assert!(take_upload(&another_session, &trusted).unwrap().is_none());
            let mut changed = trusted.clone();
            changed.insert(
                format!(
                    "[{}]:{}",
                    request.profile.target.host, request.profile.target.port
                ),
                "fingerprint".into(),
            );
            assert!(take_upload(&closed, &changed).unwrap().is_none());
            let mut unrelated = trusted.clone();
            unrelated.insert("[unrelated]:22".into(), "fingerprint".into());
            let native = take_upload(
                &closed,
                &upload_connections::session_trust(&request, &unrelated),
            )
            .unwrap()
            .expect("其他主机信任记录变化后仍能取回同会话已认证连接");
            assert!(
                take_upload(&closed, &trusted).unwrap().is_none(),
                "租用期间不能共享"
            );
            assert!(native.upload_uncompressed);
            assert_eq!(
                native.upload_rate.lock().unwrap().unwrap().1,
                2_000_000.0,
                "速率提示应随同一连接复用"
            );
            assert!(!native.is_closed());
            native.close().await;
        });
    }
}
