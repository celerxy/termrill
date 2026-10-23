//! 打包来源乱序收齐、顺序交付；慢块不立即阻止补充 READ。
//! 在途请求与已收齐未消费块合计最多两个窗口，避免慢首块缓存整个来源。
use crate::model::*;
use futures::{stream, stream::BoxStream, stream::FuturesUnordered, StreamExt};
use std::collections::BTreeMap;

pub(super) fn ranges<'a, F, Fut>(
    size: u64,
    block: u64,
    window: usize,
    request: &'a F,
) -> BoxStream<'a, Result<(u64, Vec<u8>)>>
where
    F: Fn(u64) -> Fut + Sync,
    Fut: std::future::Future<Output = Result<(u64, Vec<u8>)>> + Send + 'a,
{
    stream::unfold(
        (
            0u64,
            0u64,
            FuturesUnordered::new(),
            BTreeMap::<u64, Vec<u8>>::new(),
            false,
        ),
        move |(mut next_request, mut next_delivery, mut running, mut ready, failed)| async move {
            if failed {
                return None;
            }
            loop {
                while next_request < size
                    && running.len() < window
                    && running.len() + ready.len() < window * 2
                {
                    running.push(request(next_request));
                    next_request += block;
                }
                if let Some(bytes) = ready.remove(&next_delivery) {
                    let offset = next_delivery;
                    next_delivery += bytes.len() as u64;
                    return Some((
                        Ok((offset, bytes)),
                        (next_request, next_delivery, running, ready, false),
                    ));
                }
                match running.next().await {
                    Some(Ok((offset, bytes))) => {
                        ready.insert(offset, bytes);
                    }
                    Some(Err(error)) => {
                        return Some((
                            Err(error),
                            (next_request, next_delivery, running, ready, true),
                        ))
                    }
                    None => return None,
                }
            }
        },
    )
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn delayed_first_block_refills_but_never_exceeds_reorder_budget() {
        tauri::async_runtime::block_on(async {
            let started = AtomicUsize::new(0);
            let first = tokio::sync::Semaphore::new(0);
            let request = |offset| {
                let started = &started;
                let first = &first;
                async move {
                    started.fetch_add(1, Ordering::Relaxed);
                    if offset == 0 {
                        first.acquire().await.unwrap().forget();
                    }
                    Ok((offset, vec![offset as u8]))
                }
            };
            let mut reads = ranges(20, 1, 4, &request);
            for _ in 0..16 {
                assert!(futures::poll!(reads.next()).is_pending());
            }
            assert_eq!(started.load(Ordering::Relaxed), 8);
            first.add_permits(1);
            let mut actual = Vec::new();
            while let Some(chunk) = reads.next().await {
                actual.extend(chunk.unwrap().1);
            }
            assert_eq!(actual, (0..20).collect::<Vec<u8>>());
        });
    }
}
