//! 用户明确授权后运行的 KVM /root ZIP 或日志只读测量；不写远程文件，不输出凭据。
use crate::{model::*, transfer::native_async::ssh::Connection};
use futures::{stream, StreamExt};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use std::{
    cell::RefCell,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
    time::Instant,
};

#[test]
#[ignore = "Requires explicit KVM authorization and TERMRILL_LIVE_KVM=1"]
fn live_kvm_read_only_performance() {
    if std::env::var("TERMRILL_LIVE_KVM").as_deref() != Ok("1") {
        eprintln!("未启用授权 KVM 测量，跳过真实主机访问");
        return;
    }
    let app_data = PathBuf::from(std::env::var_os("APPDATA").expect("APPDATA"));
    let config: Config = serde_json::from_slice(
        &std::fs::read(app_data.join("dev.termrill.sftpclient/connections.json"))
            .expect("读取已有配置"),
    )
    .expect("配置格式");
    let candidates: Vec<_> = config
        .profiles
        .iter()
        .filter(|profile| profile.name.to_lowercase().contains("kvm"))
        .collect();
    assert_eq!(candidates.len(), 1, "KVM 配置必须唯一");
    let profile = candidates[0].clone();
    let saved = crate::credentials::load(&profile).expect("读取系统凭据库");
    let (target_secrets, jump_secrets) = saved.map_or_else(
        || (Secrets::default(), Secrets::default()),
        |saved| (saved.target_secrets, saved.jump_secrets),
    );
    let request = ConnectRequest {
        profile,
        target_secrets,
        jump_secrets,
    };
    let log_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/kvm-read-only-results");
    crate::performance::initialize(log_root.clone()).expect("初始化无敏感信息的测量日志");
    let closed = Arc::new(AtomicBool::new(false));
    let cancel = AtomicBool::new(false);
    let native = tauri::async_runtime::block_on(Connection::connect(
        &request,
        &config.known_hosts,
        &Default::default(),
        None,
    ))
    .unwrap_or_else(|_| panic!("KVM 连接失败；保持现有指纹验证，不自动信任"));
    let mut remote = Arc::new(crate::remote_async::Remote::owned(native, closed));
    // 二进制对照仅读取既有文件的有界前缀，不创建远程文件。
    let binary = std::env::var("TERMRILL_KVM_BINARY").as_deref() == Ok("1");
    let download = std::env::var("TERMRILL_KVM_DOWNLOAD").as_deref() == Ok("1");
    // 指定既有普通文件可做同一来源对照；只用于显式授权的只读测量。
    let path = if let Ok(path) = std::env::var("TERMRILL_KVM_FILE") {
        assert!(path.starts_with("/root/") && !path.contains('\0'));
        path
    } else {
        tauri::async_runtime::block_on(async {
            assert!(
                crate::remote_async::attrs(&remote.stat("/root", &cancel).await.unwrap()).is_dir()
            );
            let directory = remote.native.sftp.opendir("/root").await.unwrap().handle;
            let mut selected = None;
            loop {
                match remote.native.sftp.readdir(directory.clone()).await {
                    Ok(batch) => {
                        for file in batch.files {
                            let name = file.filename.to_lowercase();
                            let log = name.ends_with(".log")
                                && file.attrs.size.is_some_and(|size| size <= 8 * 1024 * 1024);
                            let eligible = if binary {
                                name.ends_with(".exe")
                            } else if download {
                                log
                            } else {
                                name.ends_with(".zip") || log
                            };
                            if eligible
                                && !file.filename.contains(['/', '\\'])
                                && crate::remote_async::attrs(&file.attrs).is_file()
                                && file.attrs.size.is_some_and(|size| {
                                    (90_000..=512 * 1024 * 1024).contains(&size)
                                })
                            {
                                selected = Some(format!("/root/{}", file.filename));
                                break;
                            }
                        }
                        if selected.is_some() {
                            break;
                        }
                    }
                    Err(russh_sftp::client::error::Error::Status(status))
                        if status.status_code == StatusCode::Eof =>
                    {
                        break
                    }
                    Err(_) => panic!("读取 /root 列表失败"),
                }
            }
            remote.native.sftp.close(directory).await.unwrap();
            selected.expect("/root 下没有可用于指定只读测量的普通文件（至少90KB）")
        })
    };
    let legacy_reference =
        if binary && std::env::var("TERMRILL_KVM_LEGACY_COMPARE").as_deref() == Ok("1") {
            Some(measure_legacy_prefix(&request, &config, &path, 900_000))
        } else {
            None
        };
    if download {
        let peers = tauri::async_runtime::block_on(async {
            let algorithms = Default::default();
            let mut connections = stream::iter(0..2)
                .map(|_| Connection::connect(&request, &config.known_hosts, &algorithms, None))
                .buffer_unordered(2);
            let mut peers = Vec::new();
            while let Some(connection) = connections.next().await {
                peers.push(connection.expect("多连接下载仍需验证已有指纹并认证"));
            }
            peers
        });
        let owned = Arc::get_mut(&mut remote).expect("测量会话独占，不启动后台维护");
        owned.set_download_peers(peers);
        // 使用既有小日志验证生产下载写盘与多连接分片，不为测量创建远程文件。
        owned.force_download_peers = true;
    }
    if binary && download {
        tauri::async_runtime::block_on(async {
            let baseline = match legacy_reference {
                Some(bytes) => bytes,
                None => {
                    let started = Instant::now();
                    let bytes =
                        binary_prefix_plan(&remote, &path, &cancel, (900_000, 30_000, 64), false)
                            .await;
                    eprintln!(
                        "KVM_BINARY read_size=30000 bytes={} elapsed_ms={}",
                        bytes.len(),
                        started.elapsed().as_millis()
                    );
                    bytes
                }
            };
            let wire_before = tcp_snapshot(TEST_SOCKET.load(std::sync::atomic::Ordering::Relaxed));
            let started = Instant::now();
            let actual = binary_prefix_plan(
                &remote,
                &path,
                &cancel,
                (900_000, remote.native.download_read_size, 64),
                true,
            )
            .await;
            assert!(
                actual == baseline,
                "新分片与旧分片读取的 EXE 前缀必须逐字节一致"
            );
            eprintln!(
                "KVM_BINARY connections={} read_size={} bytes={} elapsed_ms={} verified=true",
                remote.download_peers.len() + 1,
                remote.native.download_read_size,
                actual.len(),
                started.elapsed().as_millis()
            );
            let wire_after = tcp_snapshot(TEST_SOCKET.load(std::sync::atomic::Ordering::Relaxed));
            eprintln!("KVM_NATIVE_TCP last_peer_bytes_in={} rtt_us={} receive_window={} receive_buffer={} reordered_bytes={}",
                wire_after.BytesIn.saturating_sub(wire_before.BytesIn), wire_after.RttUs,
                wire_after.RcvWnd, wire_after.RcvBuf, wire_after.BytesReordered);
        });
        shutdown_download_test(&remote);
        return;
    }
    if binary && std::env::var("TERMRILL_KVM_PARALLEL").as_deref() == Ok("1") {
        measure_parallel_binary(&remote, &request, &config, &path, &cancel);
        tauri::async_runtime::block_on(remote.native.shutdown());
        return;
    }
    if path.to_lowercase().ends_with(".log") {
        let started = Instant::now();
        let mut baseline = Vec::new();
        let bytes = tauri::async_runtime::block_on(remote.read_download_into(
            &path,
            8 * 1024 * 1024,
            &cancel,
            |chunk| {
                baseline.extend(chunk);
                std::future::ready(Ok(()))
            },
        ))
        .expect("日志只读下载，包含源属性核对和关闭句柄");
        eprintln!(
            "KVM_DOWNLOAD ordered=true bytes={} elapsed_ms={}",
            bytes,
            started.elapsed().as_millis()
        );
        if download {
            measure_download_destination(&remote, &path, &baseline, &cancel, &log_root);
        }
        shutdown_download_test(&remote);
        return;
    }
    if !binary {
        measure_archive(&remote, &path, &cancel);
        if std::env::var("TERMRILL_KVM_ARCHIVE_ONLY").as_deref() == Ok("1") {
            tauri::async_runtime::block_on(remote.native.shutdown());
            return;
        }
    }
    // 仅测试使用旧库对照；已存指纹必须匹配，不调用自动接受指纹的 fixture helper。
    let (legacy, legacy_socket) = verified_legacy(&request, &config);
    eprintln!(
        "KVM_BASELINE cipher_receive={:?}",
        legacy.methods(ssh2::MethodType::CryptSc)
    );
    let sftp = legacy.sftp().unwrap();
    let mut source = sftp.open(std::path::Path::new(&path)).unwrap();
    let mut legacy_baseline = vec![0u8; 90_000];
    let started = Instant::now();
    std::io::Read::read_exact(&mut source, &mut legacy_baseline).unwrap();
    eprintln!(
        "KVM_BASELINE bytes=90000 elapsed_ms={}",
        started.elapsed().as_millis()
    );
    tauri::async_runtime::block_on(async {
        let before = remote.stat(&path, &cancel).await.unwrap();
        let handle = remote
            .native
            .sftp
            .open(path.clone(), OpenFlags::READ, FileAttributes::default())
            .await
            .unwrap()
            .handle;
        let mut baseline = Some(legacy_baseline);
        for block in [32768usize] {
            use std::os::windows::io::AsRawSocket;
            tcp_info(
                TEST_SOCKET.load(std::sync::atomic::Ordering::Relaxed),
                "native",
            );
            tcp_info(legacy_socket.as_raw_socket() as usize, "legacy");
            let started = Instant::now();
            let mut pending = stream::iter((0..90_000).step_by(block).map(|offset| {
                let handle = handle.clone();
                let sftp = &remote.native.sftp;
                async move {
                    let wanted = block.min(90_000 - offset);
                    let mut bytes = Vec::new();
                    while bytes.len() < wanted {
                        let data = sftp
                            .read(
                                handle.clone(),
                                (offset + bytes.len()) as u64,
                                (wanted - bytes.len()) as u32,
                            )
                            .await
                            .unwrap()
                            .data;
                        assert!(!data.is_empty() && data.len() <= wanted - bytes.len());
                        bytes.extend(data);
                    }
                    bytes
                }
            }))
            .buffered(32);
            let mut bytes = Vec::new();
            while let Some(chunk) = pending.next().await {
                bytes.extend(chunk);
            }
            eprintln!(
                "KVM_READ block={block} bytes={} elapsed_ms={}",
                bytes.len(),
                started.elapsed().as_millis()
            );
            std::io::Seek::seek(&mut source, std::io::SeekFrom::Start(0)).unwrap();
            let legacy_started = Instant::now();
            let mut old_bytes = vec![0u8; 90000];
            std::io::Read::read_exact(&mut source, &mut old_bytes).unwrap();
            assert!(old_bytes == bytes);
            eprintln!(
                "KVM_BASELINE interleaved_ms={}",
                legacy_started.elapsed().as_millis()
            );
            if let Some(baseline) = &baseline {
                assert!(&bytes == baseline, "不同读取粒度返回数据不一致");
            } else {
                baseline = Some(bytes);
            }
        }
        let after = remote
            .native
            .sftp
            .fstat(handle.clone())
            .await
            .unwrap()
            .attrs;
        remote.native.sftp.close(handle).await.unwrap();
        assert!(
            before.size == after.size && before.mtime == after.mtime,
            "测量期间源文件发生变化"
        );
    });
    tauri::async_runtime::block_on(remote.native.shutdown());
}

fn measure_legacy_prefix(
    request: &ConnectRequest,
    config: &Config,
    path: &str,
    length: usize,
) -> Vec<u8> {
    use std::os::windows::io::AsRawSocket;
    let (session, socket) = verified_legacy(request, config);
    eprintln!(
        "KVM_LEGACY cipher={:?} compression={:?}",
        session.methods(ssh2::MethodType::CryptSc),
        session.methods(ssh2::MethodType::CompSc)
    );
    let sftp = session.sftp().unwrap();
    let mut file = sftp.open(std::path::Path::new(path)).unwrap();
    let before = file.stat().unwrap();
    assert!(before.size.is_some_and(|size| size >= length as u64));
    let socket = socket.as_raw_socket() as usize;
    let wire_before = tcp_snapshot(socket);
    let mut bytes = vec![0; length];
    let started = Instant::now();
    std::io::Read::read_exact(&mut file, &mut bytes).unwrap();
    let elapsed = started.elapsed().as_millis();
    let wire_after = tcp_snapshot(socket);
    let after = file.stat().unwrap();
    assert!(before.size == after.size && before.mtime == after.mtime);
    eprintln!("KVM_LEGACY bytes={} elapsed_ms={} wire_bytes={} rtt_us={} receive_window={} receive_buffer={} reordered_bytes={} verified=true",
        bytes.len(), elapsed, wire_after.BytesIn.saturating_sub(wire_before.BytesIn), wire_after.RttUs,
        wire_after.RcvWnd, wire_after.RcvBuf, wire_after.BytesReordered);
    bytes
}

fn shutdown_download_test(remote: &crate::remote_async::Remote) {
    tauri::async_runtime::block_on(async {
        for peer in remote
            .download_peers
            .iter()
            .chain(std::iter::once(&remote.native))
        {
            peer.shutdown().await;
        }
    });
}

fn measure_download_destination(
    remote: &crate::remote_async::Remote,
    path: &str,
    baseline: &[u8],
    cancel: &AtomicBool,
    directory: &std::path::Path,
) {
    use sha2::{Digest, Sha256};
    let id = identifier();
    let local = directory.join(format!("download-{id}.bin"));
    let temporary = local.with_file_name(format!(".termrill-{id}-0.part"));
    // 非整块续传前缀，验证范围内已有部分核对与余下部分写入同时正确。
    std::fs::write(&temporary, &baseline[..45_001.min(baseline.len())]).unwrap();
    let item = crate::transfer::Item {
        local: local.clone(),
        remote: path.into(),
        directory: false,
        size: baseline.len() as u64,
        link: None,
        link_directory: false,
        remote_target: None,
    };
    let job = crate::transfer::Job {
        id,
        connection_id: "kvm-read-only".into(),
        local: local.to_string_lossy().into_owned(),
        remote: path.into(),
        direction: "download".into(),
        overwrite: false,
        completed: Default::default(),
        selection: None,
    };
    let started = Instant::now();
    let (progress, mut received) = tokio::sync::mpsc::unbounded_channel();
    let digest = tauri::async_runtime::block_on(crate::transfer::measure_download_file(
        remote,
        &job,
        0,
        &item,
        cancel,
        &AtomicBool::new(false),
        &crate::transfer_rate::RateLimiter::new(0),
        &progress,
    ))
    .expect("真实异步范围下载、续传核对和安全安装");
    let elapsed = started.elapsed().as_millis();
    assert_eq!(std::fs::read(&local).unwrap(), baseline);
    assert_eq!(digest, format!("{:x}", Sha256::digest(baseline)));
    let mut total = 0;
    while let Ok(bytes) = received.try_recv() {
        total += bytes;
    }
    assert_eq!(total, baseline.len() as u64);
    assert!(!temporary.exists(), "安装成功后不能留下临时文件");
    eprintln!("KVM_DOWNLOAD ordered=false resumed_prefix=45001 bytes={} elapsed_ms={elapsed} verified=true", baseline.len());
    std::fs::remove_file(local).unwrap();
}

/// 在已有服务器上比较一条和四条已认证连接；总读取量最多 450 KB，全部为只读。
fn measure_parallel_binary(
    remote: &crate::remote_async::Remote,
    request: &ConnectRequest,
    config: &Config,
    path: &str,
    cancel: &AtomicBool,
) {
    tauri::async_runtime::block_on(async {
        let started = Instant::now();
        let baseline = binary_prefix(remote, path, cancel).await;
        eprintln!(
            "KVM_SINGLE bytes={} elapsed_ms={}",
            baseline.len(),
            started.elapsed().as_millis()
        );
        let algorithms = Default::default();
        let mut connections = stream::iter(0..3)
            .map(|_| Connection::connect(request, &config.known_hosts, &algorithms, None))
            .buffer_unordered(3);
        let mut peers = Vec::new();
        while let Some(native) = connections.next().await {
            peers.push(crate::remote_async::Remote::owned(
                native.expect("已验证指纹的额外只读连接"),
                Arc::new(AtomicBool::new(false)),
            ));
        }
        let started = Instant::now();
        let mut reads = stream::iter(std::iter::once(remote).chain(peers.iter()))
            .map(|peer| binary_prefix(peer, path, cancel))
            .buffer_unordered(4);
        let mut total = 0usize;
        while let Some(bytes) = reads.next().await {
            assert!(bytes == baseline, "多连接读取必须返回同一文件前缀");
            total += bytes.len();
        }
        eprintln!(
            "KVM_PARALLEL connections=4 bytes={total} elapsed_ms={}",
            started.elapsed().as_millis()
        );
        drop(reads);
        for peer in peers {
            peer.native.close().await;
        }
    });
}

async fn binary_prefix(
    remote: &crate::remote_async::Remote,
    path: &str,
    cancel: &AtomicBool,
) -> Vec<u8> {
    binary_prefix_plan(remote, path, cancel, (90_000, 30_000, 3), false).await
}

async fn binary_prefix_plan(
    remote: &crate::remote_async::Remote,
    path: &str,
    cancel: &AtomicBool,
    plan: (u64, usize, usize),
    parallel: bool,
) -> Vec<u8> {
    let (length, mut block, window) = plan;
    let before = remote.stat(path, cancel).await.unwrap();
    let handle = remote
        .native
        .sftp
        .open(path.to_owned(), OpenFlags::READ, FileAttributes::default())
        .await
        .unwrap()
        .handle;
    assert!(before.size.is_some_and(|size| size >= length));
    let mut handles = vec![(&remote.native, handle)];
    if parallel {
        for peer in &remote.download_peers {
            let handle = peer
                .sftp
                .open(path.to_owned(), OpenFlags::READ, FileAttributes::default())
                .await
                .unwrap()
                .handle;
            let actual = peer.sftp.fstat(handle.clone()).await.unwrap().attrs;
            assert!(actual.size == before.size && actual.mtime == before.mtime);
            block = block.min(peer.download_read_size);
            handles.push((peer, handle));
        }
    }
    let mut reads = stream::iter((0..length).step_by(block))
        .map(|offset| {
            let (native, handle) = &handles[(offset / block as u64) as usize % handles.len()];
            let handle = handle.clone();
            async move {
                let length = (length - offset).min(block as u64) as usize;
                let mut bytes = Vec::new();
                while bytes.len() < length {
                    let data = native
                        .sftp
                        .read(
                            handle.clone(),
                            offset + bytes.len() as u64,
                            (length - bytes.len()) as u32,
                        )
                        .await
                        .unwrap()
                        .data;
                    assert!(!data.is_empty() && data.len() <= length - bytes.len());
                    bytes.extend(data);
                }
                (offset, bytes)
            }
        })
        .buffer_unordered(window);
    let mut bytes = vec![0; length as usize];
    while let Some((offset, chunk)) = reads.next().await {
        bytes[offset as usize..offset as usize + chunk.len()].copy_from_slice(&chunk);
    }
    drop(reads);
    for (native, handle) in handles {
        let after = native.sftp.fstat(handle.clone()).await.unwrap().attrs;
        native.sftp.close(handle).await.unwrap();
        assert!(
            before.size == after.size && before.mtime == after.mtime,
            "只读测量期间来源发生变化"
        );
    }
    bytes
}

// 旧库默认协商另一把主机密钥；仅选择与已存指纹一致的算法后才认证。
fn verified_legacy(
    request: &ConnectRequest,
    config: &Config,
) -> (ssh2::Session, std::net::TcpStream) {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let endpoint = &request.profile.target;
    assert!(request.profile.jump.is_none() && endpoint.auth == "key");
    let address = format!("[{}]:{}", endpoint.host, endpoint.port);
    let expected = config.known_hosts.get(&address).expect("已有指纹");
    for algorithm in ["ssh-ed25519", "ecdsa-sha2-nistp256", "rsa-sha2-512"] {
        let stream = std::net::TcpStream::connect((endpoint.host.as_str(), endpoint.port)).unwrap();
        stream.set_nodelay(true).unwrap();
        let mut session = ssh2::Session::new().unwrap();
        session.set_timeout(60_000);
        let observer = stream.try_clone().unwrap();
        session.set_tcp_stream(stream);
        session
            .method_pref(ssh2::MethodType::HostKey, algorithm)
            .unwrap();
        if session.handshake().is_err() {
            continue;
        }
        let (key, _) = session.host_key().unwrap();
        let fingerprint = format!(
            "SHA256:{}",
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(key))
        );
        if &fingerprint != expected {
            continue;
        }
        let passphrase = (!request.target_secrets.passphrase.is_empty())
            .then_some(request.target_secrets.passphrase.as_str());
        session
            .userauth_pubkey_file(
                &endpoint.username,
                None,
                std::path::Path::new(&endpoint.key_path),
                passphrase,
            )
            .expect("已校验指纹后的密钥认证");
        assert!(session.authenticated());
        return (session, observer);
    }
    panic!("旧库不能协商与已存指纹匹配的算法，不发送认证凭据");
}

static TEST_SOCKET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub(crate) fn register_socket(socket: &tokio::net::TcpStream) {
    use std::os::windows::io::AsRawSocket;
    if std::env::var("TERMRILL_LIVE_KVM").as_deref() != Ok("1") {
        return;
    }
    TEST_SOCKET.store(
        socket.as_raw_socket() as usize,
        std::sync::atomic::Ordering::Relaxed,
    );
}
fn tcp_info(socket: usize, label: &str) {
    let info = tcp_snapshot(socket);
    eprintln!("KVM_TCP label={label} rtt_us={} min_rtt_us={} receive_window={} receive_buffer={} bytes_in={} retransmit={} duplicate_ack={} timeouts={} mss={} reordered_bytes={}", info.RttUs, info.MinRttUs, info.RcvWnd, info.RcvBuf, info.BytesIn, info.BytesRetrans, info.DupAcksIn, info.TimeoutEpisodes, info.Mss, info.BytesReordered);
}
fn tcp_snapshot(socket: usize) -> windows_sys::Win32::Networking::WinSock::TCP_INFO_v0 {
    use windows_sys::Win32::Networking::WinSock::{TCP_INFO_v0, WSAIoctl, SIO_TCP_INFO};
    let version = 0u32;
    let mut info = TCP_INFO_v0::default();
    let mut returned = 0;
    // 只读查询有效测量 socket；同步 ioctl 不保留调用栈上的指针。
    let result = unsafe {
        WSAIoctl(
            socket,
            SIO_TCP_INFO,
            (&version as *const u32).cast(),
            4,
            (&mut info as *mut TCP_INFO_v0).cast(),
            std::mem::size_of::<TCP_INFO_v0>() as u32,
            &mut returned,
            std::ptr::null_mut(),
            None,
        )
    };
    assert_eq!(result, 0, "只读 TCP 统计");
    info
}

fn measure_archive(remote: &Arc<crate::remote_async::Remote>, path: &str, cancel: &AtomicBool) {
    let progress = RefCell::new(None);
    let io = crate::archive_io::ArchiveIo {
        native: Some(remote),
        cancel,
        closed: None,
        progress: tauri::ipc::Channel::new(|_| Ok(())),
        last_progress: &progress,
    };
    let local = crate::archive_io::ArchiveIo {
        native: None,
        cancel,
        closed: None,
        progress: tauri::ipc::Channel::new(|_| Ok(())),
        last_progress: &progress,
    };
    let mut archive: crate::archives::Request = serde_json::from_value(
        serde_json::json!({"operation":"list","snapshot":"kvm-live-perf","path":path}),
    )
    .unwrap();
    let started = Instant::now();
    let list = serde_json::to_value(
        crate::archives::run(archive.clone(), &io, &local, "kvm-live-perf").unwrap(),
    )
    .unwrap();
    eprintln!("KVM_ARCHIVE list_ms={}", started.elapsed().as_millis());
    let mut images: Vec<_> = list["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| {
            !entry["directory"].as_bool().unwrap_or(true)
                && !entry["encrypted"].as_bool().unwrap_or(true)
                && entry["size"]
                    .as_u64()
                    .is_some_and(|size| size <= 1024 * 1024)
                && entry["name"]
                    .as_str()
                    .is_some_and(|name| name.to_lowercase().ends_with(".jpg"))
        })
        .take(3)
        .collect();
    images.sort_by_key(|image| image["size"].as_u64().unwrap());
    assert!(!images.is_empty(), "ZIP 中缺少用于预览测量的小图片");
    archive.operation = "preview".into();
    archive.revision = list["revision"].as_str().map(str::to_owned);
    for image in images {
        archive.selected = vec![image["name"].as_str().unwrap().into()];
        let started = Instant::now();
        let reply = serde_json::to_value(
            crate::archives::run(archive.clone(), &io, &local, "kvm-live-perf").unwrap(),
        )
        .unwrap();
        assert!(reply["bytes"]
            .as_str()
            .is_some_and(|bytes| !bytes.is_empty()));
        eprintln!(
            "KVM_ARCHIVE image_bytes={} preview_ms={}",
            image["size"],
            started.elapsed().as_millis()
        );
        let started = Instant::now();
        let warm = serde_json::to_value(
            crate::archives::run(archive.clone(), &io, &local, "kvm-live-perf").unwrap(),
        )
        .unwrap();
        assert_eq!(warm["bytes"], reply["bytes"]);
        eprintln!(
            "KVM_ARCHIVE_WARM image_bytes={} preview_ms={}",
            image["size"],
            started.elapsed().as_millis()
        );
    }
    crate::archives::release("kvm-live-perf", "kvm-live-perf", path);
}
