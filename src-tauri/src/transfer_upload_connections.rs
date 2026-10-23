//! 上传任务独占连接组：准备失败收齐租约，成功归还，失败关闭。
use super::*;

// 连接租约只比较本次实际访问的主机；其他服务器信任记录变化不能使租约失效。
pub(super) fn session_trust(
    request: &ConnectRequest,
    trusted: &std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    std::iter::once(&request.profile.target)
        .chain(request.profile.jump.iter())
        .filter_map(|endpoint| {
            let address = format!("[{}]:{}", endpoint.host, endpoint.port);
            trusted
                .get(&address)
                .map(|fingerprint| (address, fingerprint.clone()))
        })
        .collect()
}

// 小任务和主动限速沿用原连接数，避免额外握手抵消传输收益。
pub(super) fn desired_count(base: usize, largest: u64, unlimited: bool) -> usize {
    if unlimited && base > 1 && largest >= 16 * 1024 * 1024 {
        (base * 2).min(8)
    } else {
        base
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn acquire(
    count: usize,
    minimum: usize,
    request: &ConnectRequest,
    trusted: &std::collections::BTreeMap<String, String>,
    algorithms: &std::collections::BTreeMap<String, String>,
    backend: Option<&Backend>,
    cancel: &AtomicBool,
    closed: &Arc<AtomicBool>,
) -> Result<Vec<ssh::Connection>> {
    let mut pending = futures::stream::iter(0..count)
        .map(|_| async {
            if let Some(connection) = pool::take_upload(closed, trusted)? {
                return Ok(connection);
            }
            controlled(
                ssh::Connection::connect_upload(request, trusted, algorithms, backend),
                cancel,
                closed,
            )
            .await
        })
        .buffer_unordered(count.max(1));
    let mut connections = Vec::new();
    let mut failure = None;
    while let Some(result) = pending.next().await {
        match result {
            Ok(connection) => connections.push(connection),
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    let stopped = cancelled(cancel, closed).err();
    if connections.len() < minimum || stopped.is_some() {
        for connection in connections {
            connection.close().await;
        }
        return Err(stopped
            .or(failure)
            .unwrap_or_else(|| AppError::new("上传连接组准备不完整")));
    }
    // 可选连接失败时保留成功的连接；取消不能退化为继续上传。
    Ok(connections)
}

pub(super) async fn release(
    connections: impl IntoIterator<Item = ssh::Connection>,
    closed: Arc<AtomicBool>,
    trusted: std::collections::BTreeMap<String, String>,
    reusable: bool,
) -> Result<()> {
    let mut releasing = FuturesUnordered::new();
    for connection in connections {
        let closed = closed.clone();
        let trusted = trusted.clone();
        releasing.push(async move {
            if reusable {
                pool::put(connection, closed, trusted).await
            } else {
                connection.close().await;
                Ok(())
            }
        });
    }
    let mut failure = None;
    while let Some(result) = releasing.next().await {
        if let Err(error) = result {
            failure.get_or_insert(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trust_scope_preserves_target_and_jump_changes_but_ignores_unrelated_hosts() {
        let mut request = crate::tests::request(2222);
        let target = format!(
            "[{}]:{}",
            request.profile.target.host, request.profile.target.port
        );
        request.profile.jump = Some(request.profile.target.clone());
        request.profile.jump.as_mut().unwrap().port = 2223;
        let jump = format!("[{}]:2223", request.profile.target.host);
        let trusted = std::collections::BTreeMap::from([
            (target.clone(), "target".into()),
            (jump.clone(), "jump".into()),
        ]);
        let original = session_trust(&request, &trusted);
        let mut changed = trusted.clone();
        changed.insert("[unrelated]:22".into(), "other".into());
        assert_eq!(session_trust(&request, &changed), original);
        changed.insert(target.clone(), "changed".into());
        assert_ne!(session_trust(&request, &changed), original);
        changed = trusted.clone();
        changed.insert(jump.clone(), "changed".into());
        assert_ne!(session_trust(&request, &changed), original);
        changed.remove(&jump);
        assert_ne!(session_trust(&request, &changed), original);
    }
    #[test]
    fn expansion_respects_single_connection_limits_and_task_size() {
        assert_eq!(desired_count(3, 16 * 1024 * 1024, true), 6);
        assert_eq!(desired_count(4, 16 * 1024 * 1024, true), 8);
        assert_eq!(desired_count(1, 100 * 1024 * 1024, true), 1);
        assert_eq!(desired_count(3, 16 * 1024 * 1024, false), 3);
        assert_eq!(desired_count(3, 16 * 1024 * 1024 - 1, true), 3);
    }
}
