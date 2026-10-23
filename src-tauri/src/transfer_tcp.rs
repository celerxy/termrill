//! 原生 SSH 的 TCP 接收配置；只作用于 TermRill 新建的套接字，不改系统网络设置。
//! 接收缓冲在连接前设置，允许 SYN 协商窗口扩展；Windows ACK 调整不支持时保留系统行为。
use futures::{stream::FuturesUnordered, StreamExt};
use std::{
    collections::VecDeque,
    future::Future,
    net::SocketAddr,
    time::{Duration, Instant},
};
use tokio::net::{TcpSocket, TcpStream};

const RECEIVE_BUFFER: u32 = 4 * 1024 * 1024;

pub(super) async fn connect(host: &str, port: u16) -> std::io::Result<TcpStream> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let started = Instant::now();
    let resolved = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::lookup_host((host, port)),
    )
    .await;
    let addresses = match resolved {
        Ok(result) => result.map(|addresses| addresses.collect::<Vec<_>>()),
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "解析服务器地址超时",
        )),
    };
    crate::performance_log!(
        "[TermRill connection] phase=dns dial_id={} stack={} elapsed_ms={} addresses={} success={} error_kind={:?}",
        id,
        module_path!(),
        started.elapsed().as_millis(),
        addresses.as_ref().map_or(0, Vec::len), addresses.is_ok(), addresses.as_ref().err().map(std::io::Error::kind)
    );
    let addresses = interleave(addresses?);
    let budget = Duration::from_secs(30).saturating_sub(started.elapsed());
    let tcp_started = Instant::now();
    let dial = |address| async move {
        let attempt_started = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(15), connect_address(address))
            .await
            .unwrap_or_else(|_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "TCP 连接超时",
                ))
            });
        crate::performance_log!("[TermRill connection] phase=tcp_attempt dial_id={} stack={} ipv6={} elapsed_ms={} success={} error_kind={:?}", id, module_path!(), address.is_ipv6(), attempt_started.elapsed().as_millis(), result.is_ok(), result.as_ref().err().map(std::io::Error::kind));
        result
    };
    let result = tokio::time::timeout(
        budget,
        race_addresses(addresses, &dial, Duration::from_millis(250)),
    )
    .await
    .unwrap_or_else(|_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "TCP 地址尝试总耗时超限",
        ))
    });
    crate::performance_log!(
        "[TermRill connection] phase=tcp_connect dial_id={} stack={} elapsed_ms={} success={}",
        id,
        module_path!(),
        tcp_started.elapsed().as_millis(),
        result.is_ok()
    );
    result
}

// 保留解析器首选地址族，并交替安排 IPv4/IPv6，避免首选地址族全部超时后才尝试另一族。
fn interleave(addresses: Vec<SocketAddr>) -> VecDeque<SocketAddr> {
    let first_v6 = addresses.first().is_some_and(SocketAddr::is_ipv6);
    let (mut first, mut second): (VecDeque<_>, VecDeque<_>) = addresses
        .into_iter()
        .partition(|address| address.is_ipv6() == first_v6);
    let mut result = VecDeque::new();
    while !first.is_empty() || !second.is_empty() {
        for queue in [&mut first, &mut second] {
            if let Some(address) = queue.pop_front() {
                if !result.contains(&address) {
                    result.push_back(address);
                }
            }
        }
    }
    result
}

async fn race_addresses<T, F, Fut>(
    mut addresses: VecDeque<SocketAddr>,
    dial: &F,
    stagger: Duration,
) -> std::io::Result<T>
where
    F: Fn(SocketAddr) -> Fut,
    Fut: Future<Output = std::io::Result<T>>,
{
    let mut last_error =
        std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "服务器没有可用地址");
    let mut pending = FuturesUnordered::new();
    let mut next_launch = tokio::time::Instant::now();
    loop {
        if pending.is_empty() {
            let Some(address) = addresses.pop_front() else {
                return Err(last_error);
            };
            pending.push(dial(address));
            next_launch = tokio::time::Instant::now() + stagger;
        }
        tokio::select! {
            result = pending.next() => match result {
                Some(Ok(socket)) => return Ok(socket), // 丢弃其余 TCP future；只对赢家进行 SSH 握手。
                Some(Err(error)) => {
                    last_error = error;
                    if let Some(address) = addresses.pop_front() {
                        pending.push(dial(address));
                        next_launch = tokio::time::Instant::now() + stagger;
                    }
                }
                None => return Err(last_error),
            },
            _ = tokio::time::sleep_until(next_launch), if pending.len() < 2 && !addresses.is_empty() => {
                if let Some(address) = addresses.pop_front() { pending.push(dial(address)); }
                next_launch = tokio::time::Instant::now() + stagger;
            }
        }
    }
}

async fn connect_address(address: SocketAddr) -> std::io::Result<TcpStream> {
    let socket = if address.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    let buffer_configured = socket.set_recv_buffer_size(RECEIVE_BUFFER).is_ok();
    let buffer = socket.recv_buffer_size().ok();
    let stream = socket.connect(address).await?;
    stream.set_nodelay(true)?;
    let immediate_ack = immediate_ack(&stream);
    crate::performance_log!(
        "[TermRill connection] tcp_receive_buffer={:?} buffer_configured={} immediate_ack={}",
        buffer,
        buffer_configured,
        immediate_ack
    );
    Ok(stream)
}

#[cfg(windows)]
fn immediate_ack(stream: &TcpStream) -> bool {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{WSAIoctl, SIO_TCP_SET_ACK_FREQUENCY};
    let frequency = 1u32;
    let mut returned = 0;
    // 同步 ioctl 使用仍由 TcpStream 拥有的有效句柄；输入/输出指针不跨调用保存。
    // 只改变本套接字 ACK 频率，不能用 TCP_NODELAY 替代接收方向 ACK 设置。
    unsafe {
        WSAIoctl(
            stream.as_raw_socket() as usize,
            SIO_TCP_SET_ACK_FREQUENCY,
            (&frequency as *const u32).cast(),
            std::mem::size_of::<u32>() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
            None,
        ) == 0
    }
}

#[cfg(not(windows))]
fn immediate_ack(_stream: &TcpStream) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn stalled_address_does_not_hide_healthy_fallback_and_losers_are_dropped() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct Active(Arc<AtomicUsize>);
        impl Drop for Active {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        tauri::async_runtime::block_on(async {
            let active = Arc::new(AtomicUsize::new(0));
            let peak = AtomicUsize::new(0);
            let calls = AtomicUsize::new(0);
            let addresses = (1..=3)
                .map(|port| SocketAddr::from(([127, 0, 0, 1], port)))
                .collect();
            let dial = |address: SocketAddr| {
                let (active, peak, calls) = (active.clone(), &peak, &calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    peak.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    let _active = Active(active);
                    match address.port() {
                        1 => std::future::pending().await,
                        2 => Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused)),
                        _ => Ok(37),
                    }
                }
            };
            let result = tokio::time::timeout(
                Duration::from_secs(1),
                race_addresses(addresses, &dial, Duration::from_millis(10)),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(result, 37);
            assert_eq!(calls.load(Ordering::SeqCst), 3);
            assert_eq!(peak.load(Ordering::SeqCst), 2);
            assert_eq!(
                active.load(Ordering::SeqCst),
                0,
                "胜出后不得遗留等待中的连接"
            );
            calls.store(0, Ordering::SeqCst);
            let stalled = |_| {
                let (active, calls) = (active.clone(), &calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    active.fetch_add(1, Ordering::SeqCst);
                    let _active = Active(active);
                    std::future::pending::<std::io::Result<()>>().await
                }
            };
            let addresses = (1..=4)
                .map(|port| SocketAddr::from(([127, 0, 0, 1], port)))
                .collect();
            assert!(tokio::time::timeout(
                Duration::from_millis(30),
                race_addresses(addresses, &stalled, Duration::from_millis(5))
            )
            .await
            .is_err());
            assert!(calls.load(Ordering::SeqCst) <= 2);
            assert_eq!(
                active.load(Ordering::SeqCst),
                0,
                "超时取消后也必须释放所有地址尝试"
            );
        });
    }

    #[test]
    fn address_families_interleave_without_duplicates_and_empty_input_fails() {
        let v6a: SocketAddr = "[::1]:22".parse().unwrap();
        let v6b: SocketAddr = "[::2]:22".parse().unwrap();
        let v4: SocketAddr = "127.0.0.1:22".parse().unwrap();
        assert_eq!(
            interleave(vec![v6a, v6b, v4, v4]),
            VecDeque::from([v6a, v4, v6b])
        );
        tauri::async_runtime::block_on(async {
            let dial = |_| std::future::ready(Ok::<_, std::io::Error>(()));
            let error = race_addresses(VecDeque::new(), &dial, Duration::from_millis(1))
                .await
                .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::AddrNotAvailable);
        });
    }

    #[test]
    fn configured_socket_preserves_bidirectional_bytes() {
        tauri::async_runtime::block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let client = async {
                let mut socket = connect("127.0.0.1", port).await.unwrap();
                assert!(socket.nodelay().unwrap());
                socket.write_all(b"request").await.unwrap();
                let mut reply = vec![0; 512 * 1024];
                socket.read_exact(&mut reply).await.unwrap();
                assert!(reply.iter().all(|byte| *byte == 37));
                #[cfg(windows)]
                assert!(
                    immediate_ack(&socket),
                    "本机 Windows 应支持每套接字 ACK 配置"
                );
            };
            let server = async {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 7];
                socket.read_exact(&mut request).await.unwrap();
                assert_eq!(&request, b"request");
                socket.write_all(&vec![37; 512 * 1024]).await.unwrap();
            };
            tokio::join!(client, server);
        });
    }
}
