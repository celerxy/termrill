//! 下载独占租用上传共用的已验证传输池，避免数据流堵住浏览与预览会话。
use super::*;
use futures::{stream, StreamExt};

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute(
    job: &Job,
    primary: &std::sync::Mutex<crate::ssh::Connection>,
    backend: &Backend,
    event: &mut TransferEvent,
    cancel: &AtomicBool,
    pause: &AtomicBool,
    send: &dyn Fn(&TransferEvent),
    checkpoint: &dyn Fn(&str, String) -> Result<()>,
    limiter: &crate::transfer_rate::RateLimiter,
) -> Result<()> {
    let (request, closed, algorithms) = {
        let connection = lock(primary)?;
        (
            connection.request.clone(),
            connection.closed.clone(),
            connection.host_algorithms.clone(),
        )
    };
    let (trusted, concurrency) = {
        let config = lock(&backend.config)?;
        (
            native_async::pool::session_trust(&request, &config.known_hosts),
            config.transfer_settings.concurrency,
        )
    };
    cancelled(cancel, &closed)?;
    let started = Instant::now();
    let pooled = native_async::pool::take(&closed, &trusted)?;
    let reused = pooled.is_some();
    let native = match pooled {
        Some(native) => native,
        None => {
            native_async::controlled(
                native_async::pool::tracked(
                    native_async::ssh::Connection::connect(
                        &request,
                        &trusted,
                        &algorithms,
                        Some(backend),
                    ),
                    &closed,
                    &trusted,
                ),
                cancel,
                &closed,
            )
            .await?
        }
    };
    crate::performance_log!(
        "[TermRill transfer] task={} direction=download phase=connection reused={} elapsed_ms={}",
        job.id,
        reused,
        started.elapsed().as_millis()
    );
    // 不缓存归档/预览句柄；下载协程全部结束后租约才能回池。
    let mut remote = crate::remote_async::Remote::owned(native, closed.clone());
    let result = async {
        event.status = "scanning".into();
        send(event);
        let items = download_async::scan_items(&remote, job, cancel).await?;
        // 多文件也租用连接组，避免所有小文件共用一个 SFTP 流。
        if items
            .iter()
            .any(|item| !item.directory && item.size >= 4 * 1024 * 1024)
            || items.iter().filter(|item| !item.directory).take(2).count() > 1
        {
            let additional = concurrency.clamp(1, 4) - 1;
            let mut preparing = stream::iter(0..additional)
                .map(|_| async {
                    if let Some(native) = native_async::pool::take(&closed, &trusted)? {
                        return Ok(native);
                    }
                    native_async::controlled(
                        native_async::pool::tracked(
                            native_async::ssh::Connection::connect(
                                &request,
                                &trusted,
                                &algorithms,
                                Some(backend),
                            ),
                            &closed,
                            &trusted,
                        ),
                        cancel,
                        &closed,
                    )
                    .await
                })
                .buffer_unordered(additional.max(1));
            let mut peers = Vec::new();
            let mut failure = None;
            while let Some(connection) = preparing.next().await {
                match connection {
                    Ok(connection) => peers.push(connection),
                    Err(error) => {
                        failure.get_or_insert(error);
                    }
                }
            }
            remote.set_download_peers(peers);
            if let Some(error) = failure {
                return Err(error);
            }
        }
        crate::performance_log!(
            "[TermRill transfer] task={} direction=download data_connections={}",
            job.id,
            remote.download_peers.len() + 1
        );
        download_async::execute(
            job,
            &remote,
            items,
            event,
            cancel,
            pause,
            send,
            checkpoint,
            limiter,
            concurrency,
        )
        .await
    }
    .await;
    let reusable =
        result.is_ok() && !cancel.load(Ordering::Relaxed) && !closed.load(Ordering::Relaxed);
    let mut releasing = futures::stream::FuturesUnordered::new();
    // 收齐全部租约的归还或关闭，独立连接并行释放，避免逐条等待 CLOSE 超时。
    // 失败或取消的会话不回池，不能将未收齐的响应带入后续任务。
    for native in remote
        .download_peers
        .into_iter()
        .chain(std::iter::once(remote.native))
    {
        let closed = closed.clone();
        let trusted = trusted.clone();
        releasing.push(async move {
            if reusable {
                native_async::pool::put(native, closed, trusted).await
            } else {
                native.close().await;
                Ok(())
            }
        });
    }
    let mut release_failure = None;
    while let Some(released) = releasing.next().await {
        if let Err(error) = released {
            release_failure.get_or_insert(error);
        }
    }
    result?;
    release_failure.map_or(Ok(()), Err)
}
