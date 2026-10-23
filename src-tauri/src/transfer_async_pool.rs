//! 下载及预览连接独占租用；按当前端点信任快照匹配，失效空闲连接不占池容量。
use super::*;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

pub(in crate::transfer) fn session_trust(
    request: &ConnectRequest,
    trusted: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    std::iter::once(&request.profile.target)
        .chain(request.profile.jump.iter())
        .filter_map(|endpoint| {
            let address = format!("[{}]:{}", endpoint.host, endpoint.port);
            trusted.get(&address).map(|key| (address, key.clone()))
        })
        .collect()
}

pub(super) struct Registration {
    closed: Arc<AtomicBool>,
    trusted: BTreeMap<String, String>,
}

fn registrations() -> &'static Mutex<Vec<std::sync::Weak<Registration>>> {
    static REGISTRY: OnceLock<Mutex<Vec<std::sync::Weak<Registration>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

// 注册先于握手，连接所有者释放后自动消失；预热同时统计建连中、租用中和空闲连接。
pub(in crate::transfer) async fn tracked(
    future: impl std::future::Future<Output = Result<ssh::Connection>>,
    closed: &Arc<AtomicBool>,
    trusted: &BTreeMap<String, String>,
) -> Result<ssh::Connection> {
    let registration = Arc::new(Registration {
        closed: closed.clone(),
        trusted: trusted.clone(),
    });
    lock(registrations())?.push(Arc::downgrade(&registration));
    let mut connection = future.await?;
    connection.pool_registration = Some(registration);
    Ok(connection)
}

fn registered_count(closed: &Arc<AtomicBool>, trusted: &BTreeMap<String, String>) -> Result<usize> {
    let mut entries = lock(registrations())?;
    entries.retain(|entry| entry.strong_count() > 0);
    Ok(entries
        .iter()
        .filter_map(std::sync::Weak::upgrade)
        .filter(|entry| Arc::ptr_eq(&entry.closed, closed) && entry.trusted == *trusted)
        .count())
}

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

// 只替换本主会话的过期信任快照，其他工作区的有效租约不能被清除。
fn outdated(
    owner: &Arc<AtomicBool>,
    snapshot: &BTreeMap<String, String>,
    closed: &Arc<AtomicBool>,
    trusted: &BTreeMap<String, String>,
) -> bool {
    owner.load(Ordering::Relaxed) || (Arc::ptr_eq(owner, closed) && snapshot != trusted)
}

fn prune_idle(
    closed: &Arc<AtomicBool>,
    trusted: &BTreeMap<String, String>,
) -> Result<Vec<ssh::Connection>> {
    let mut idle = lock(pool())?;
    let mut removed = Vec::new();
    let mut index = 0;
    while index < idle.len() {
        let entry = &idle[index];
        if outdated(&entry.closed, &entry.trusted, closed, trusted) || entry.connection.is_closed()
        {
            removed.push(idle.swap_remove(index).connection);
        } else {
            index += 1;
        }
    }
    Ok(removed)
}

fn close_pruned(connections: Vec<ssh::Connection>) {
    if connections.is_empty() {
        return;
    }
    crate::performance_log!(
        "[TermRill transfer] phase=pool_prune connections={}",
        connections.len()
    );
    tauri::async_runtime::spawn(async move {
        futures::future::join_all(connections.into_iter().map(|connection| connection.close()))
            .await;
    });
}

pub(in crate::transfer) fn take(
    closed: &Arc<AtomicBool>,
    trusted: &BTreeMap<String, String>,
) -> Result<Option<ssh::Connection>> {
    close_pruned(prune_idle(closed, trusted)?);
    let mut idle = lock(pool())?;
    let index = idle.iter().position(|entry| {
        Arc::ptr_eq(&entry.closed, closed)
            && !closed.load(Ordering::Relaxed)
            && entry.trusted == *trusted
            && !entry.connection.is_closed()
    });
    Ok(index.map(|index| idle.swap_remove(index).connection))
}

pub(in crate::transfer) async fn put(
    connection: ssh::Connection,
    closed: Arc<AtomicBool>,
    trusted: BTreeMap<String, String>,
) -> Result<()> {
    put_accepted(connection, closed, trusted).await.map(|_| ())
}

async fn put_accepted(
    connection: ssh::Connection,
    closed: Arc<AtomicBool>,
    trusted: BTreeMap<String, String>,
) -> Result<bool> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut connection = Some(connection);
    let accepted = {
        let mut idle = lock(pool())?;
        if idle
            .iter()
            .filter(|entry| Arc::ptr_eq(&entry.closed, &closed))
            .count()
            >= 4
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
        return Ok(false);
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
    Ok(true)
}

/// 完整读完的预览连接沿用建连时的认证归属；未注册的测试会话直接关闭。
pub(in crate::transfer) async fn recycle_preview(connection: ssh::Connection) {
    let owner = connection
        .pool_registration
        .as_ref()
        .map(|entry| (entry.closed.clone(), entry.trusted.clone()));
    if let Some((closed, trusted)) = owner {
        let _ = put(connection, closed, trusted).await;
    } else {
        connection.close().await;
    }
}

/// 按传输并发设置预热空闲连接；不占主会话网络锁，不为交互认证弹出提示。
pub(super) fn maintain(primary: Arc<Mutex<crate::ssh::Connection>>, backend: Arc<Backend>) {
    tauri::async_runtime::spawn(async move {
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
        // 交互认证的额外会话仍由上传显式触发，避免后台重复索取验证码。
        if request.profile.target.auth == "interactive"
            || request
                .profile
                .jump
                .as_ref()
                .is_some_and(|jump| jump.auth == "interactive")
        {
            return;
        }
        loop {
            if closed.load(Ordering::Relaxed) {
                return;
            }
            let (trusted, desired) = {
                let Ok(config) = lock(&backend.config) else {
                    return;
                };
                (
                    session_trust(&request, &config.known_hosts),
                    config.transfer_settings.concurrency.clamp(1, 4),
                )
            };
            let Ok(stale) = prune_idle(&closed, &trusted) else {
                return;
            };
            close_pruned(stale);
            let Ok(count) = registered_count(&closed, &trusted) else {
                return;
            };
            let available = count >= desired;
            if !available {
                let started = Instant::now();
                // 缺少的会话同时握手，每条仍独立验证指纹；注册先于连接，避免预热漏计。
                let mut pending = futures::stream::FuturesUnordered::new();
                for _ in count..desired {
                    pending.push(controlled(
                        tracked(
                            ssh::Connection::connect(
                                &request,
                                &trusted,
                                &algorithms,
                                Some(&backend),
                            ),
                            &closed,
                            &trusted,
                        ),
                        &closed,
                        &closed,
                    ));
                }
                let mut failed = false;
                let mut accepted = 0usize;
                let mut rejected = 0usize;
                // 先完成的连接立即入池，不能为等待一个慢握手而扣住全部已就绪连接。
                while let Some(result) = pending.next().await {
                    match result {
                        Ok(connection) => {
                            match put_accepted(connection, closed.clone(), trusted.clone()).await {
                                Ok(true) => accepted += 1,
                                Ok(false) => rejected += 1,
                                Err(_) => failed = true,
                            }
                        }
                        Err(_) => failed = true,
                    }
                }
                crate::performance_log!(
                    "[TermRill transfer] phase=pool_prepare success={} elapsed_ms={} registered_before={} desired={} accepted={} rejected={}",
                    !failed,
                    started.elapsed().as_millis(), count, desired, accepted, rejected
                );
                // 收齐本轮成功连接再退出，认证/指纹失败不能在后台反复尝试。
                if failed {
                    return;
                }
                // 入池拒绝不是预热成功；并发归还占满容量时必须退避，不能立即再握手。
                if rejected > 0 {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                continue;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Requires loopback fixture"]
    fn foreground_session_defers_pool_until_first_listing_finishes() {
        let ready: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var("TERMRILL_TEST_FIXTURE").unwrap()).unwrap(),
        )
        .unwrap();
        let directory =
            std::path::PathBuf::from(ready["root"].as_str().unwrap()).join(crate::identifier());
        std::fs::create_dir_all(&directory).unwrap();
        let backend = Arc::new(crate::tests::backend(&directory));
        let request = crate::tests::request(ready["targetPort"].as_u64().unwrap() as u16);
        drop(crate::tests::verified(request.clone(), &backend));
        tauri::async_runtime::block_on(async {
            let connected = crate::connect_session(request.clone(), backend.clone())
                .await
                .unwrap();
            let primary = lock(&backend.connections)
                .unwrap()
                .get(&connected.id)
                .unwrap()
                .clone();
            let (closed, listed) = {
                let primary = lock(&primary).unwrap();
                (primary.closed.clone(), primary.first_listing.clone())
            };
            let trusted = session_trust(&request, &lock(&backend.config).unwrap().known_hosts);
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(
                registered_count(&closed, &trusted).unwrap(),
                0,
                "首屏完成前不应发起后台握手"
            );
            listed.notify_one();
            tokio::time::timeout(Duration::from_secs(5), async {
                while registered_count(&closed, &trusted).unwrap() == 0 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            closed.store(true, Ordering::Relaxed);
            lock(&backend.connections).unwrap().clear();
        });
    }

    #[test]
    fn unrelated_host_trust_does_not_invalidate_pool_but_target_and_jump_changes_do() {
        let mut request = crate::tests::request(2222);
        let mut jump = request.profile.target.clone();
        jump.host = "jump.example".into();
        jump.port = 2223;
        request.profile.jump = Some(jump);
        let target = format!(
            "[{}]:{}",
            request.profile.target.host, request.profile.target.port
        );
        let jump = "[jump.example]:2223".to_owned();
        let mut trusted = BTreeMap::from([
            (target.clone(), "target-key".to_owned()),
            (jump.clone(), "jump-key".to_owned()),
            ("[other.example]:22".to_owned(), "unrelated-key".to_owned()),
        ]);
        let scoped = session_trust(&request, &trusted);
        assert_eq!(scoped.len(), 2);
        let owner = Arc::new(AtomicBool::new(false));
        trusted.insert("[other.example]:22".into(), "changed-key".into());
        assert!(!outdated(
            &owner,
            &scoped,
            &owner,
            &session_trust(&request, &trusted)
        ));
        for address in [&target, &jump] {
            let mut changed = trusted.clone();
            changed.insert(address.clone(), "new-key".into());
            assert!(outdated(
                &owner,
                &scoped,
                &owner,
                &session_trust(&request, &changed)
            ));
            changed.remove(address);
            assert!(outdated(
                &owner,
                &scoped,
                &owner,
                &session_trust(&request, &changed)
            ));
        }
        let other_owner = Arc::new(AtomicBool::new(false));
        assert!(!outdated(&owner, &scoped, &other_owner, &BTreeMap::new()));
        owner.store(true, Ordering::Relaxed);
        assert!(outdated(&owner, &scoped, &other_owner, &scoped));
    }

    #[test]
    fn pending_connection_registration_is_released_on_cancellation() {
        tauri::async_runtime::block_on(async {
            let closed = Arc::new(AtomicBool::new(false));
            let trusted = BTreeMap::new();
            {
                let future = tracked(std::future::pending(), &closed, &trusted);
                tokio::pin!(future);
                assert!(futures::poll!(&mut future).is_pending());
                assert_eq!(registered_count(&closed, &trusted).unwrap(), 1);
                assert_eq!(
                    registered_count(&Arc::new(AtomicBool::new(false)), &trusted).unwrap(),
                    0
                );
            }
            assert_eq!(registered_count(&closed, &trusted).unwrap(), 0);
        });
    }

    #[test]
    #[ignore = "Requires loopback fixture"]
    fn session_prepares_upload_connection_before_any_transfer() {
        let ready: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var("TERMRILL_TEST_FIXTURE").unwrap()).unwrap(),
        )
        .unwrap();
        let directory =
            std::path::PathBuf::from(ready["root"].as_str().unwrap()).join(crate::identifier());
        std::fs::create_dir_all(&directory).unwrap();
        let backend = Arc::new(crate::tests::backend(&directory));
        let request = crate::tests::request(ready["targetPort"].as_u64().unwrap() as u16);
        let primary = Arc::new(Mutex::new(crate::tests::verified(
            request.clone(),
            &backend,
        )));
        let closed = lock(&primary).unwrap().closed.clone();
        let trusted = session_trust(&request, &lock(&backend.config).unwrap().known_hosts);
        lock(&backend.connections)
            .unwrap()
            .insert("prewarm".into(), primary.clone());
        let desired = lock(&backend.config)
            .unwrap()
            .transfer_settings
            .concurrency
            .clamp(1, 4);
        maintain(primary, backend);
        tauri::async_runtime::block_on(async {
            let started = Instant::now();
            let mut warmed = Vec::new();
            loop {
                if let Some(native) = take(&closed, &trusted).unwrap() {
                    assert!(!native.is_closed());
                    warmed.push(native);
                    if warmed.len() == desired {
                        break;
                    }
                    continue;
                }
                if started.elapsed() > Duration::from_secs(10) {
                    closed.store(true, Ordering::Relaxed);
                    panic!("主会话未补齐预热连接");
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            for native in warmed {
                put(native, closed.clone(), trusted.clone()).await.unwrap();
            }
            closed.store(true, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(1200)).await;
            assert!(take(&closed, &trusted).unwrap().is_none());
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
        let trusted = session_trust(&request, &lock(&backend.config).unwrap().known_hosts);
        tauri::async_runtime::block_on(async {
            let native = ssh::Connection::connect(
                &request,
                &trusted,
                &primary.host_algorithms,
                Some(&backend),
            )
            .await
            .unwrap();
            put(native, closed.clone(), trusted.clone()).await.unwrap();
            let another_session = Arc::new(AtomicBool::new(false));
            assert!(take(&another_session, &trusted).unwrap().is_none());
            let mut changed = trusted.clone();
            changed.insert("changed".into(), "fingerprint".into());
            let native = take(&closed, &session_trust(&request, &changed))
                .unwrap()
                .expect("同会话取回已认证连接");
            assert!(
                take(&closed, &trusted).unwrap().is_none(),
                "租用期间不能共享"
            );
            assert!(!native.is_closed());
            put(native, closed.clone(), trusted.clone()).await.unwrap();
            changed.insert(
                format!(
                    "[{}]:{}",
                    request.profile.target.host, request.profile.target.port
                ),
                "changed-target".into(),
            );
            assert!(take(&closed, &session_trust(&request, &changed))
                .unwrap()
                .is_none());
            assert!(
                take(&closed, &trusted).unwrap().is_none(),
                "真实指纹改变后，旧空闲连接必须已移出池"
            );
        });
    }
}
