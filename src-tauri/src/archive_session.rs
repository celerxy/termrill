//! 打包/添加来源独占连接组；不占用浏览会话，成功回池，失败或取消整组关闭。
use super::*;
use crate::remote_async::Remote;
use std::time::Duration;

pub(crate) async fn acquire_archive(
    primary: &std::sync::Mutex<crate::ssh::Connection>,
    backend: &Backend,
    cancel: &AtomicBool,
) -> Result<Arc<Remote>> {
    let started = Instant::now();
    let closed = lock(primary)?.closed.clone();
    let mut peers = super::lease_data_connections(primary, backend, cancel, true).await?;
    let native = peers
        .pop()
        .ok_or_else(|| AppError::new("归档数据连接不可用，请检查连接后重试"))?;
    let mut remote = Remote::owned(native, closed);
    remote.set_download_peers(peers);
    crate::performance_log!(
        "[TermRill archive] phase=data_lease connections={} elapsed_ms={}",
        remote.download_peers.len() + 1,
        started.elapsed().as_millis()
    );
    Ok(Arc::new(remote))
}

pub(crate) async fn release_archive(remote: Arc<Remote>, reusable: bool) {
    let reusable = reusable && !remote.closed.load(Ordering::Relaxed);
    match Arc::try_unwrap(remote) {
        Ok(remote) => {
            let mut connections = remote.download_peers;
            connections.push(remote.native);
            crate::performance_log!(
                "[TermRill archive] phase=data_release connections={} reusable={}",
                connections.len(),
                reusable
            );
            super::release_preview_connections(connections, reusable).await;
        }
        Err(shared) => {
            // 正常路径所有借用读取/句柄已 join；意外残留引用时绝不归还仍可能使用的连接。
            let shutdown = futures::future::join_all(
                std::iter::once(&shared.native)
                    .chain(shared.download_peers.iter())
                    .map(|native| native.shutdown()),
            );
            let _ = tokio::time::timeout(Duration::from_secs(5), shutdown).await;
        }
    }
}
