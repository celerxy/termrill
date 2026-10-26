//! 本机故障注入回归；只操作 fixture 的临时目录，所有场景串行执行。
use crate::{
    model::*,
    tests::{backend, request, verified},
    *,
};
use std::{collections::HashSet, path::PathBuf, sync::Mutex};

#[path = "transfer_benchmark.rs"]
mod benchmark;

#[test]
#[ignore = "Requires loopback fixture"]
fn archive_source_lease_stripes_and_preserves_browser_after_cancel() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let bytes: Vec<u8> = (0..5 * 1024 * 1024)
        .map(|offset| (offset % 251) as u8)
        .collect();
    fs::write(fixture.remote_file("striped-source.bin"), &bytes).unwrap();
    tauri::async_runtime::block_on(async {
        let cancel = AtomicBool::new(false);
        let remote = transfer::acquire_archive(&connection, &fixture.state, &cancel)
            .await
            .unwrap();
        assert!(!remote.download_peers.is_empty());
        let path = format!("{}/striped-source.bin", fixture.remote);
        let before = remote.stat(&path, &cancel).await.unwrap();
        let mut actual = Vec::new();
        remote
            .read_archive_source(
                &path,
                &before,
                bytes.len() as u64,
                &cancel,
                |chunk| {
                    actual.extend(chunk);
                    std::future::ready(Ok(()))
                },
                4,
            )
            .await
            .unwrap();
        assert_eq!(actual, bytes);
        transfer::release_archive(remote, true).await;
        let remote = transfer::acquire_archive(&connection, &fixture.state, &cancel)
            .await
            .unwrap();
        let result = remote
            .read_archive_source(
                &path,
                &before,
                bytes.len() as u64,
                &cancel,
                |_| {
                    cancel.store(true, Ordering::Relaxed);
                    std::future::ready(Ok(()))
                },
                4,
            )
            .await;
        assert!(result.is_err());
        transfer::release_archive(remote, false).await;
        let browser = remote_async::get(&connection, &fixture.state)
            .await
            .unwrap();
        browser.stat(&path, &AtomicBool::new(false)).await.unwrap();
    });
}

#[test]
#[ignore = "Requires loopback fixture"]
fn download_stalled_connection_is_retired_and_ranges_finish_on_healthy_peers() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let bytes: Vec<u8> = (0..6 * 1024 * 1024)
        .map(|offset| (offset % 241) as u8)
        .collect();
    fs::write(fixture.remote_file("stall-source.bin"), &bytes).unwrap();
    fixture.faults(serde_json::json!({"stallReadOnceMs": 35000}));
    tauri::async_runtime::block_on(async {
        let cancel = AtomicBool::new(false);
        let remote = transfer::acquire_archive(&connection, &fixture.state, &cancel)
            .await
            .unwrap();
        assert!(!remote.download_peers.is_empty());
        let path = format!("{}/stall-source.bin", fixture.remote);
        let mut actual = vec![0; bytes.len()];
        let mut received = 0;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(34),
            remote.read_download_ranges(
                &path,
                bytes.len() as u64,
                &cancel,
                &AtomicBool::new(false),
                |offset, chunk| {
                    let start = offset as usize;
                    actual[start..start + chunk.len()].copy_from_slice(&chunk);
                    received += chunk.len();
                    std::future::ready(Ok(()))
                },
            ),
        )
        .await
        .expect("不能等到服务端 35 秒后恢复或原 300 秒超时")
        .unwrap();
        assert_eq!(result, bytes.len() as u64);
        assert_eq!(received, bytes.len(), "转交不重复交付已完成范围");
        assert_eq!(actual, bytes);
        assert!(std::iter::once(&remote.native)
            .chain(remote.download_peers.iter())
            .any(|native| native.is_closed()));
        // 故障租约不能回池；浏览主会话仍保持可用。
        transfer::release_archive(remote, true).await;
        let browser = remote_async::get(&connection, &fixture.state)
            .await
            .unwrap();
        browser.stat(&path, &cancel).await.unwrap();
    });
}

#[test]
#[ignore = "Requires loopback fixture"]
fn download_ranges_fail_over_when_one_connection_disconnects() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let bytes: Vec<u8> = (0..6 * 1024 * 1024)
        .map(|offset| (offset % 241) as u8)
        .collect();
    fs::write(fixture.remote_file("failover-source.bin"), &bytes).unwrap();
    tauri::async_runtime::block_on(async {
        let cancel = AtomicBool::new(false);
        let remote = transfer::acquire_archive(&connection, &fixture.state, &cancel)
            .await
            .unwrap();
        assert!(!remote.download_peers.is_empty());
        let path = format!("{}/failover-source.bin", fixture.remote);
        let mut actual = vec![0u8; bytes.len()];
        let mut received = 0usize;
        let remote_ref = &remote;
        let result = remote
            .read_download_ranges(
                &path,
                bytes.len() as u64,
                &cancel,
                &AtomicBool::new(false),
                |offset, chunk| {
                    let start = offset as usize;
                    actual[start..start + chunk.len()].copy_from_slice(&chunk);
                    received += 1;
                    let disconnect = received == 2;
                    async move {
                        // 收到首批数据后断开一条辅助连接，其在途和后续范围须由其他连接完成。
                        if disconnect {
                            remote_ref.download_peers[0].shutdown().await;
                        }
                        Ok(())
                    }
                },
            )
            .await;
        assert_eq!(result.unwrap(), bytes.len() as u64);
        assert!(remote.download_peers[0].is_closed());
        assert!(actual == bytes, "转交后的范围必须逐字节一致");
        transfer::release_archive(remote, false).await;
    });
}

#[test]
#[ignore = "Requires loopback fixture"]
fn v07_native_archive_pack_rebuild_and_encrypted_preview() {
    use std::cell::RefCell;
    let fixture = Fixture::new();
    let connection = fixture.connect();
    fs::create_dir_all(fixture.remote_file("v07-source/empty")).unwrap();
    fs::write(fixture.remote_file("v07-source/中文.txt"), "remote 归档\n").unwrap();
    fs::write(fixture.remote_file("v07-new.txt"), b"added remotely").unwrap();
    let remote =
        tauri::async_runtime::block_on(remote_async::get(&connection, &fixture.state)).unwrap();
    let cancel = AtomicBool::new(false);
    let clock = RefCell::new(None);
    let io = archive_io::ArchiveIo {
        native: Some(&remote),
        cancel: &cancel,
        closed: Some(&remote.closed),
        progress: tauri::ipc::Channel::new(|_| Ok(())),
        last_progress: &clock,
    };
    let local = archive_io::ArchiveIo {
        native: None,
        cancel: &cancel,
        closed: None,
        progress: tauri::ipc::Channel::new(|_| Ok(())),
        last_progress: &clock,
    };
    for extension in ["zip", "7z", "tar.gz"] {
        let path = format!("{}/v07.{extension}", fixture.remote);
        let password = (extension != "tar.gz").then_some("fixture-archive-password");
        let make = |operation: &str| -> archives::Request {
            serde_json::from_value(
                serde_json::json!({"operation":operation,"path":path,"password":password}),
            )
            .unwrap()
        };
        let mut pack = make("pack");
        pack.sources = vec![format!("{}/v07-source", fixture.remote)];
        pack.compression = Some("fast".into());
        archives::run(pack, &io, &local, "v07-remote").unwrap();
        let listed =
            serde_json::to_value(archives::run(make("list"), &io, &local, "v07-remote").unwrap())
                .unwrap();
        let mut preview = make("preview");
        preview.selected = vec!["v07-source/中文.txt".into()];
        preview.revision = Some(listed["revision"].as_str().unwrap().into());
        let reply =
            serde_json::to_value(archives::run(preview, &io, &local, "v07-remote").unwrap())
                .unwrap();
        use base64::Engine;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(reply["bytes"].as_str().unwrap())
                .unwrap(),
            "remote 归档\n".as_bytes()
        );
        let mut add = make("add");
        add.sources = vec![format!("{}/v07-new.txt", fixture.remote)];
        add.revision = Some(listed["revision"].as_str().unwrap().into());
        archives::run(add, &io, &local, "v07-remote").unwrap();
        archives::run(make("verify"), &io, &local, "v07-remote").unwrap();
        assert!(fixture.remote_file(&format!("v07.{extension}")).is_file());
    }
}

#[test]
#[ignore = "Requires loopback fixture"]
fn workbench_exec_bounds_and_rejection_preserve_sftp() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    tauri::async_runtime::block_on(async {
        let remote = remote_async::get(&connection, &fixture.state)
            .await
            .unwrap();
        let result = remote
            .native
            .workbench_command("fixture-status")
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(result.output.contains("stdout") && result.output.contains("stderr"));
        let large = remote
            .native
            .workbench_command("fixture-large")
            .await
            .unwrap();
        assert!(large.truncated);
        assert_eq!(large.output.len(), 64 * 1024);
        fixture.faults(serde_json::json!({"denyExec": true}));
        assert!(remote
            .native
            .workbench_command("fixture-status")
            .await
            .is_err());
        remote
            .stat(&fixture.remote, &AtomicBool::new(false))
            .await
            .unwrap();
    });
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_preview_lease_isolated_cache_and_cancel() {
    let fixture = Fixture::new();
    fixture.faults(serde_json::json!({"readDelayMs":8,"auditRead":true}));
    let connection = fixture.connect();
    let mut pdf = vec![42; 64 * 1024];
    pdf[..5].copy_from_slice(b"%PDF-");
    fs::write(fixture.remote_file("lease.pdf"), &pdf).unwrap();
    fs::write(
        fixture.remote_file("lease-large.bin"),
        vec![17; 4 * 1024 * 1024],
    )
    .unwrap();
    let path = format!("{}/lease.pdf", fixture.remote);
    tauri::async_runtime::block_on(async {
        let original = remote_async::get(&connection, &fixture.state)
            .await
            .unwrap();
        // 原共享数据会话不可用时，独占租约仍必须读取成功。
        original.native.shutdown().await;
        let cancel = AtomicBool::new(false);
        assert_eq!(
            transfer::read_preview(
                &connection,
                &fixture.state,
                Some(&original),
                &path,
                &cancel,
                |_| {}
            )
            .await
            .unwrap(),
            pdf
        );
        let reads = fs::read_to_string(fixture.root.join("read-events.jsonl"))
            .unwrap()
            .lines()
            .count();
        assert_eq!(
            transfer::read_preview(
                &connection,
                &fixture.state,
                Some(&original),
                &path,
                &cancel,
                |_| {}
            )
            .await
            .unwrap(),
            pdf
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("read-events.jsonl"))
                .unwrap()
                .lines()
                .count(),
            reads,
            "租约更换后仍复用经属性验证的正文缓存"
        );
        let large_path = format!("{}/lease-large.bin", fixture.remote);
        assert!(transfer::read_preview(
            &connection,
            &fixture.state,
            Some(&original),
            &large_path,
            &cancel,
            |_| {
                cancel.store(true, Ordering::Relaxed);
            }
        )
        .await
        .is_err());
        cancel.store(false, Ordering::Relaxed);
        assert_eq!(
            transfer::read_preview(
                &connection,
                &fixture.state,
                Some(&original),
                &path,
                &cancel,
                |_| {}
            )
            .await
            .unwrap(),
            pdf,
            "取消的租约不能污染下一次完整读取"
        );
    });
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_preview_read_window_content_cancel_and_change() {
    let fixture = Fixture::new();
    fixture.faults(serde_json::json!({"readDelayMs":8,"auditRead":true}));
    let connection = fixture.connect();
    let data: Vec<u8> = (0..4 * 1024 * 1024 + 17)
        .map(|index| (index % 251) as u8)
        .collect();
    let local = fixture.remote_file("preview-window.bin");
    fs::write(&local, &data).unwrap();
    let path = format!("{}/preview-window.bin", fixture.remote);
    tauri::async_runtime::block_on(async {
        let remote = remote_async::get(&connection, &fixture.state)
            .await
            .unwrap();
        let cancel = AtomicBool::new(false);
        let started = std::time::Instant::now();
        assert_eq!(
            remote
                .read(&path, 8 * 1024 * 1024, &cancel, |_| {})
                .await
                .unwrap(),
            data
        );
        let baseline = started.elapsed().as_millis();
        let started = std::time::Instant::now();
        assert_eq!(
            remote
                .read_preview(&path, 8 * 1024 * 1024, &cancel, |_| {})
                .await
                .unwrap(),
            data
        );
        println!(
            "PREVIEW_READ bytes={} baseline_ms={} optimized_ms={}",
            data.len(),
            baseline,
            started.elapsed().as_millis()
        );
        let prepared = AtomicBool::new(false);
        let prepared_connections = Mutex::new(Vec::new());
        assert_eq!(
            remote
                .read_preview_prepared(
                    &path,
                    8 * 1024 * 1024,
                    &cancel,
                    |_| {},
                    || async {
                        prepared.store(true, Ordering::Relaxed);
                        let peers =
                            transfer::preview_connections(&connection, &fixture.state, &cancel)
                                .await?;
                        let expected = fixture
                            .state
                            .config
                            .lock()
                            .unwrap()
                            .transfer_settings
                            .concurrency
                            .clamp(1, 4)
                            - 1;
                        assert_eq!(peers.len(), expected, "辅助连接必须按配置建立");
                        *prepared_connections.lock().unwrap() = peers
                            .iter()
                            .map(|peer| peer.received_data.clone())
                            .collect();
                        Ok(peers)
                    }
                )
                .await
                .unwrap(),
            data,
            "多连接乱序响应必须拼回相同字节"
        );
        assert!(prepared.load(Ordering::Relaxed));
        let reused = transfer::preview_connections(&connection, &fixture.state, &cancel)
            .await
            .unwrap();
        assert_eq!(reused.len(), prepared_connections.lock().unwrap().len());
        for peer in &reused {
            assert!(
                prepared_connections
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|previous| Arc::ptr_eq(previous, &peer.received_data)),
                "成功读取的辅助会话应直接复用，而不是重新握手"
            );
        }
        transfer::release_preview_connections(reused, false).await;
        let large_path = format!("{}/cached-large.pdf", fixture.remote);
        let mut large_pdf = data.clone();
        large_pdf[..5].copy_from_slice(b"%PDF-");
        fs::write(fixture.remote_file("cached-large.pdf"), &large_pdf).unwrap();
        assert_eq!(
            remote
                .read_preview(&large_path, 8 * 1024 * 1024, &cancel, |_| {})
                .await
                .unwrap(),
            large_pdf
        );
        let large_reads = fs::read_to_string(fixture.root.join("read-events.jsonl"))
            .unwrap()
            .lines()
            .count();
        assert_eq!(
            remote
                .read_preview_prepared(
                    &large_path,
                    8 * 1024 * 1024,
                    &cancel,
                    |_| {},
                    || async { panic!("大文件缓存命中也不应建立辅助连接") }
                )
                .await
                .unwrap(),
            large_pdf
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("read-events.jsonl"))
                .unwrap()
                .lines()
                .count(),
            large_reads
        );
        let pdf_path = format!("{}/cached.pdf", fixture.remote);
        let mut pdf = vec![37; 64 * 1024];
        pdf[..5].copy_from_slice(b"%PDF-");
        fs::write(fixture.remote_file("cached.pdf"), &pdf).unwrap();
        assert_eq!(
            remote
                .read_preview_prepared(
                    &pdf_path,
                    8 * 1024 * 1024,
                    &cancel,
                    |_| {},
                    || async { panic!("小文件不应准备辅助连接") }
                )
                .await
                .unwrap(),
            pdf
        );
        let audit = fixture.root.join("read-events.jsonl");
        let reads = fs::read_to_string(&audit).unwrap().lines().count();
        assert_eq!(
            remote
                .read_preview(&pdf_path, 8 * 1024 * 1024, &cancel, |_| {})
                .await
                .unwrap(),
            pdf
        );
        assert_eq!(
            fs::read_to_string(&audit).unwrap().lines().count(),
            reads,
            "缓存命中不得重新请求正文"
        );
        let stopped = AtomicBool::new(false);
        assert!(remote
            .read_preview(&pdf_path, 8 * 1024 * 1024, &stopped, |_| stopped
                .store(true, Ordering::Relaxed))
            .await
            .is_err());
        pdf.push(11);
        fs::write(fixture.remote_file("cached.pdf"), &pdf).unwrap();
        assert_eq!(
            remote
                .read_preview(&pdf_path, 8 * 1024 * 1024, &cancel, |_| {})
                .await
                .unwrap(),
            pdf
        );
        assert!(
            fs::read_to_string(&audit).unwrap().lines().count() > reads,
            "源变化必须重读正文"
        );
        assert!(remote
            .read_preview(&pdf_path, 16, &cancel, |_| {})
            .await
            .is_err());
        let stopped = AtomicBool::new(false);
        assert!(remote
            .read_preview_prepared(
                &path,
                8 * 1024 * 1024,
                &stopped,
                |_| {
                    stopped.store(true, Ordering::Relaxed);
                },
                || transfer::preview_connections(&connection, &fixture.state, &stopped)
            )
            .await
            .is_err());
        let changed = AtomicBool::new(false);
        assert!(remote
            .read_preview_prepared(
                &path,
                8 * 1024 * 1024,
                &cancel,
                |_| {
                    if !changed.swap(true, Ordering::Relaxed) {
                        fs::write(&local, b"changed").unwrap();
                    }
                },
                || transfer::preview_connections(&connection, &fixture.state, &cancel)
            )
            .await
            .is_err());
        fs::write(&local, &data).unwrap();
        assert_eq!(
            remote
                .read_preview(&path, 8 * 1024 * 1024, &cancel, |_| {})
                .await
                .unwrap(),
            data
        );
    });
}

struct Fixture {
    root: PathBuf,
    local: PathBuf,
    remote: String,
    state: Backend,
    port: u16,
}

#[test]
#[ignore = "Requires loopback fixture"]
fn connect_ready_does_not_depend_on_first_directory_and_listing_can_retry() {
    let fixture = Fixture::new();
    drop(fixture.connect()); // 首次信任仍使用原指纹确认流程。
    fixture.faults(serde_json::json!({"denyList": true}));
    let port = fixture.port;
    let faults = fixture.root.join("faults.json");
    let state = Arc::new(backend(&fixture.local));
    state.config.lock().unwrap().known_hosts =
        fixture.state.config.lock().unwrap().known_hosts.clone();
    tauri::async_runtime::block_on(async {
        let connected = connect_session(request(port), state.clone()).await.unwrap();
        let primary = state
            .connections
            .lock()
            .unwrap()
            .get(&connected.id)
            .unwrap()
            .clone();
        let remote = remote_async::get(&primary, &state).await.unwrap();
        let cancel = AtomicBool::new(false);
        assert!(remote
            .list(
                ".",
                directory::Filter::browser(0, String::new(), false).unwrap(),
                &cancel
            )
            .await
            .is_err());
        assert!(
            !remote.closed.load(Ordering::Relaxed),
            "目录失败不能丢弃已认证连接"
        );
        fs::write(&faults, b"{}").unwrap();
        assert!(remote
            .list(
                ".",
                directory::Filter::browser(0, String::new(), false).unwrap(),
                &cancel
            )
            .await
            .is_ok());
        remote.closed.store(true, Ordering::Relaxed);
        state.connections.lock().unwrap().clear();
    });
}

#[test]
#[ignore = "Requires loopback fixture"]
fn initial_directory_parallel_open_is_closed_when_realpath_fails() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    fixture.faults(serde_json::json!({"denyRealpath": true, "auditListing": true}));
    tauri::async_runtime::block_on(async {
        let remote = remote_async::get(&connection, &fixture.state)
            .await
            .unwrap();
        let cancel = AtomicBool::new(false);
        assert!(remote
            .list(
                ".",
                directory::Filter::browser(0, String::new(), false).unwrap(),
                &cancel
            )
            .await
            .is_err());
        let events: Vec<serde_json::Value> =
            fs::read_to_string(fixture.root.join("listing-events.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        assert_eq!(
            events.iter().filter(|event| event["kind"] == 11).count(),
            1,
            "并行请求实际打开目录"
        );
        assert_eq!(
            events.iter().filter(|event| event["kind"] == 4).count(),
            1,
            "解析失败仍确认关闭目录句柄"
        );
        fixture.faults(serde_json::json!({}));
        assert!(remote
            .list(
                ".",
                directory::Filter::browser(0, String::new(), false).unwrap(),
                &cancel
            )
            .await
            .is_ok());
        assert!(remote
            .list(
                &fixture.remote,
                directory::Filter::browser(0, String::new(), false).unwrap(),
                &cancel
            )
            .await
            .is_ok());
    });
}
impl Fixture {
    fn new() -> Self {
        let ready: serde_json::Value = serde_json::from_slice(
            &fs::read(std::env::var("TERMRILL_TEST_FIXTURE").expect("run tests/run.ps1")).unwrap(),
        )
        .unwrap();
        let root = PathBuf::from(ready["root"].as_str().unwrap());
        let id = identifier();
        let local = root.join(&id);
        fs::create_dir_all(&local).unwrap();
        fs::create_dir_all(root.join("remote").join(&id)).unwrap();
        Self {
            state: backend(&local),
            root,
            local,
            remote: format!("/{id}"),
            port: ready["targetPort"].as_u64().unwrap() as u16,
        }
    }
    fn connect(&self) -> Mutex<ssh::Connection> {
        Mutex::new(verified(request(self.port), &self.state))
    }
    fn remote_file(&self, name: &str) -> PathBuf {
        self.root
            .join("remote")
            .join(self.remote.trim_start_matches('/'))
            .join(name)
    }
    fn job(&self, direction: &str, name: &str) -> transfer::Job {
        transfer::Job {
            id: identifier(),
            connection_id: "fixture".into(),
            local: self.local.join(name).display().to_string(),
            remote: format!("{}/{name}", self.remote),
            direction: direction.into(),
            overwrite: false,
            completed: BTreeMap::new(),
            selection: None,
        }
    }
    fn faults(&self, value: serde_json::Value) {
        fs::write(
            self.root.join("faults.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }
    fn clear_faults(&self) {
        let _ = fs::remove_file(self.root.join("faults.json"));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.clear_faults();
    }
}

fn payload() -> Vec<u8> {
    (0..2 * 1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect()
}

#[test]
#[ignore = "Requires loopback fixture"]
fn concurrent_small_uploads_preserve_all_contents_and_checkpoints() {
    let fixture = Fixture::new();
    let job = fixture.job("upload", "small-batch");
    fs::create_dir_all(&job.local).unwrap();
    for index in 0..48 {
        fs::write(
            Path::new(&job.local).join(format!("{index}.txt")),
            vec![index as u8; index * 257],
        )
        .unwrap();
    }
    let connection = fixture.connect();
    let saved = Mutex::new(BTreeMap::new());
    let event = transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &connection,
        &fixture.state,
        |_| {},
        |path, digest| {
            saved.lock().unwrap().insert(path.to_owned(), digest);
            Ok(())
        },
    );
    completed(&event);
    assert_eq!(event.bytes, event.total);
    assert_eq!(saved.lock().unwrap().len(), 48);
    for index in 0..48 {
        let path = fixture
            .remote_file("small-batch")
            .join(format!("{index}.txt"));
        assert_eq!(fs::read(path).unwrap(), vec![index as u8; index * 257]);
    }
}

#[test]
#[ignore = "Requires loopback fixture"]
fn concurrent_precheck_resume_rejects_changed_content_before_writing() {
    let fixture = Fixture::new();
    let mut job = fixture.job("upload", "resume-batch");
    fs::create_dir_all(&job.local).unwrap();
    for index in 0..20 {
        let folder = format!("branch-{index}/nested");
        fs::create_dir_all(Path::new(&job.local).join(&folder)).unwrap();
        fs::write(
            Path::new(&job.local).join(&folder).join("file.txt"),
            b"original",
        )
        .unwrap();
    }
    let connection = fixture.connect();
    let saved = Mutex::new(BTreeMap::new());
    let event = transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &connection,
        &fixture.state,
        |_| {},
        |path, digest| {
            saved.lock().unwrap().insert(path.to_owned(), digest);
            Ok(())
        },
    );
    completed(&event);
    job.completed = saved.into_inner().unwrap();
    assert_eq!(job.completed.len(), 20);
    let resumed = transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &connection,
        &fixture.state,
        |_| {},
        |_, _| panic!("已完成文件不应重新提交"),
    );
    completed(&resumed);
    assert_eq!(resumed.bytes, resumed.total);
    fs::write(
        fixture.remote_file("resume-batch/branch-10/nested/file.txt"),
        b"modified",
    )
    .unwrap();
    fs::write(Path::new(&job.local).join("new.txt"), b"must not upload").unwrap();
    let rejected = transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &connection,
        &fixture.state,
        |_| {},
        |_, _| panic!("预检失败不得写入"),
    );
    assert_eq!(rejected.status, "failed");
    assert!(!fixture.remote_file("resume-batch/new.txt").exists());
    assert_eq!(
        fs::read(fixture.remote_file("resume-batch/branch-10/nested/file.txt")).unwrap(),
        b"modified"
    );
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_async_upload_covers_large_empty_files_private_key_and_jump() {
    let fixture = Fixture::new();
    let ready: serde_json::Value =
        serde_json::from_slice(&fs::read(std::env::var("TERMRILL_TEST_FIXTURE").unwrap()).unwrap())
            .unwrap();
    let mut job = fixture.job("upload", "native-mixed");
    fs::create_dir_all(&job.local).unwrap();
    let large = payload().repeat(3);
    fs::write(Path::new(&job.local).join("large.bin"), &large).unwrap();
    fs::write(Path::new(&job.local).join("empty.txt"), []).unwrap();
    fs::write(Path::new(&job.local).join("small.txt"), b"native async").unwrap();
    let nested = Path::new(&job.local).join("nested/deep/tree");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("nested.txt"), b"missing ancestor upload").unwrap();
    let mut key_request = request(fixture.port);
    key_request.profile.target.auth = "key".into();
    key_request.profile.target.key_path = ready["keyPath"].as_str().unwrap().into();
    for jump in [false, true] {
        let mut configured = key_request.clone();
        if jump {
            let mut endpoint = request(fixture.port).profile.target;
            endpoint.port = ready["jumpPort"].as_u64().unwrap() as u16;
            configured.profile.jump = Some(endpoint);
            job.remote = format!("{}/native-jumped", fixture.remote);
        }
        let connection = Mutex::new(verified(configured, &fixture.state));
        let saved = Mutex::new(BTreeMap::new());
        let event = transfer::execute_concurrent(
            &job,
            &AtomicBool::new(false),
            &connection,
            &fixture.state,
            |_| {},
            |path, digest| {
                saved.lock().unwrap().insert(path.to_owned(), digest);
                Ok(())
            },
        );
        completed(&event);
        assert_eq!(event.bytes, event.total);
        assert_eq!(saved.lock().unwrap().len(), 4);
        let remote_directory = fixture
            .root
            .join("remote")
            .join(job.remote.trim_start_matches('/'));
        assert_eq!(
            fs::read(remote_directory.join("nested/deep/tree/nested.txt")).unwrap(),
            b"missing ancestor upload"
        );
        assert_eq!(fs::read(remote_directory.join("large.bin")).unwrap(), large);
        assert!(fs::read(remote_directory.join("empty.txt"))
            .unwrap()
            .is_empty());
        assert_eq!(
            fs::read(remote_directory.join("small.txt")).unwrap(),
            b"native async"
        );
    }
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_async_upload_rejects_changed_fingerprint_before_writing() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let job = fixture.job("upload", "untrusted.bin");
    fs::write(&job.local, b"must not be uploaded").unwrap();
    let address = format!("[127.0.0.1]:{}", fixture.port);
    fixture
        .state
        .config
        .lock()
        .unwrap()
        .known_hosts
        .insert(address.clone(), "SHA256:changed".into());
    let event = transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &connection,
        &fixture.state,
        |_| {},
        |_, _| Ok(()),
    );
    assert_eq!(event.status, "failed");
    assert!(fixture
        .state
        .pending_hosts
        .lock()
        .unwrap()
        .contains_key(&address));
    assert!(!fixture.remote_file("untrusted.bin").exists());
    assert!(!fixture.remote_file(&job.temporary_name(0)).exists());
}

#[test]
#[ignore = "Requires loopback fixture"]
fn concurrent_directory_roundtrip_preserves_nested_files_and_checkpoints() {
    let fixture = Fixture::new();
    let job = fixture.job("upload", "parallel");
    fs::create_dir_all(Path::new(&job.local).join("nested")).unwrap();
    let data = payload();
    for index in 0..6 {
        fs::write(
            Path::new(&job.local).join(format!("nested/{index}.bin")),
            &data,
        )
        .unwrap();
    }
    let saved = Mutex::new(BTreeMap::new());
    // 同一 fixture、同一内容比较串行与并发；时间只作诊断，不设置易波动的阈值。
    let mut serial = job.clone();
    serial.remote = format!("{}-serial", job.remote);
    let serial_connection = fixture.connect();
    let serial_start = std::time::Instant::now();
    completed(&transfer::execute(
        &serial,
        &AtomicBool::new(false),
        &serial_connection,
        |_| {},
    ));
    let serial_elapsed = serial_start.elapsed();
    let parallel_connection = fixture.connect();
    let parallel_start = std::time::Instant::now();
    let event = transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &parallel_connection,
        &fixture.state,
        |_| {},
        |path, digest| {
            saved.lock().unwrap().insert(path.to_owned(), digest);
            Ok(())
        },
    );
    completed(&event);
    println!(
        "six-file upload: serial={serial_elapsed:?}, concurrent={:?}",
        parallel_start.elapsed()
    );
    assert_eq!(event.bytes, event.total);
    assert_eq!(saved.lock().unwrap().len(), 6);
    let mut retry = job.clone();
    retry.overwrite = true;
    retry.completed = saved
        .lock()
        .unwrap()
        .iter()
        .take(2)
        .map(|(path, digest)| (path.clone(), digest.clone()))
        .collect();
    let resumed = Mutex::new(BTreeMap::new());
    let event = transfer::execute_concurrent(
        &retry,
        &AtomicBool::new(false),
        &fixture.connect(),
        &fixture.state,
        |_| {},
        |path, digest| {
            resumed.lock().unwrap().insert(path.to_owned(), digest);
            Ok(())
        },
    );
    completed(&event);
    assert_eq!(event.bytes, event.total);
    assert_eq!(resumed.lock().unwrap().len(), 4);
    let mut download = job.clone();
    download.direction = "download".into();
    download.local = fixture.local.join("downloaded").display().to_string();
    // 并发下载协调器写任务清单，fixture 也必须注册真实任务。
    fixture.state.jobs.lock().unwrap().insert(
        download.id.clone(),
        (download.clone(), request(fixture.port).profile),
    );
    completed(&transfer::execute_concurrent(
        &download,
        &AtomicBool::new(false),
        &fixture.connect(),
        &fixture.state,
        |_| {},
        |_, _| Ok(()),
    ));
    for index in 0..6 {
        assert_eq!(
            fs::read(Path::new(&download.local).join(format!("nested/{index}.bin"))).unwrap(),
            data
        );
    }
}

#[test]
#[ignore = "Requires loopback fixture"]
fn concurrent_failure_stops_workers_and_retries_stable_partial_paths() {
    let fixture = Fixture::new();
    let mut job = fixture.job("upload", "parallel-failure");
    job.overwrite = true;
    fs::create_dir_all(&job.local).unwrap();
    let data = payload();
    for index in 0..6 {
        fs::write(Path::new(&job.local).join(format!("{index}.bin")), &data).unwrap();
    }
    fixture.faults(serde_json::json!({"writePathContains":job.id,"failWriteAfterBytes":262144}));
    let event = transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &fixture.connect(),
        &fixture.state,
        |_| {},
        |_, _| Ok(()),
    );
    assert_eq!(event.status, "failed", "{:?}", event.error);
    fixture.clear_faults();
    completed(&transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &fixture.connect(),
        &fixture.state,
        |_| {},
        |_, _| Ok(()),
    ));
    for index in 0..6 {
        assert_eq!(
            fs::read(fixture.remote_file(&format!("parallel-failure/{index}.bin"))).unwrap(),
            data
        );
    }
}
fn completed(event: &TransferEvent) {
    assert_eq!(event.status, "completed", "{:?}", event.error);
}

#[test]
#[ignore = "Requires loopback fixture"]
fn discard_cleans_only_owned_parts_in_local_and_remote_directories() {
    for direction in ["upload", "download"] {
        let fixture = Fixture::new();
        let job = fixture.job(direction, "cleanup");
        let root = if direction == "upload" {
            fixture.remote_file("cleanup")
        } else {
            PathBuf::from(&job.local)
        };
        fs::create_dir_all(root.join("nested")).unwrap();
        let owned = format!(".termrill-{}-18.part", job.id);
        fs::write(root.join(&owned), b"partial").unwrap();
        fs::write(root.join("nested").join(&owned), b"partial").unwrap();
        for name in [
            "final.bin",
            ".termrill-other-18.part",
            ".termrill-backup-original",
        ] {
            fs::write(root.join(name), b"keep").unwrap();
        }
        let fake_suffix = format!(".termrill-{}-18.part.keep", job.id);
        fs::write(root.join(&fake_suffix), b"keep").unwrap();
        fixture
            .state
            .jobs
            .lock()
            .unwrap()
            .insert(job.id.clone(), (job.clone(), request(fixture.port).profile));
        fixture
            .state
            .connections
            .lock()
            .unwrap()
            .insert("fixture".into(), Arc::new(fixture.connect()));
        discard_transfer_inner(&job.id, Some("fixture"), &fixture.state).unwrap();
        assert!(!root.join(&owned).exists());
        assert!(!root.join("nested").join(&owned).exists());
        assert!(root.join("nested").is_dir());
        assert!(root.join(&fake_suffix).exists());
        for name in [
            "final.bin",
            ".termrill-other-18.part",
            ".termrill-backup-original",
        ] {
            assert_eq!(fs::read(root.join(name)).unwrap(), b"keep");
        }
        assert!(!fixture.state.jobs.lock().unwrap().contains_key(&job.id));
        assert!(fixture.state.transfers.lock().unwrap().is_empty());
    }
}

#[test]
#[ignore = "Requires loopback fixture"]
fn discard_without_matching_server_retains_record_and_partial() {
    let fixture = Fixture::new();
    let job = fixture.job("upload", "cleanup.bin");
    let partial = fixture.remote_file(&job.temporary_name(0));
    fs::write(&partial, b"partial").unwrap();
    fixture
        .state
        .jobs
        .lock()
        .unwrap()
        .insert(job.id.clone(), (job.clone(), request(fixture.port).profile));
    assert!(discard_transfer_inner(&job.id, None, &fixture.state).is_err());
    assert_eq!(fs::read(&partial).unwrap(), b"partial");
    assert!(fixture.state.jobs.lock().unwrap().contains_key(&job.id));
    assert!(fixture.state.transfers.lock().unwrap().is_empty());
    let connection = fixture.connect();
    connection.lock().unwrap().request.profile.target.username = "different-user".into();
    fixture
        .state
        .connections
        .lock()
        .unwrap()
        .insert("other".into(), Arc::new(connection));
    assert!(discard_transfer_inner(&job.id, Some("other"), &fixture.state).is_err());
    assert!(partial.exists());
    assert!(fixture.state.jobs.lock().unwrap().contains_key(&job.id));
}

#[cfg(windows)]
#[test]
#[ignore = "Requires loopback fixture"]
fn discard_locked_local_partial_keeps_record_until_cleanup_succeeds() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let job = fixture.job("download", "cleanup.bin");
    fs::write(&job.local, b"original").unwrap();
    let partial = fixture.local.join(job.temporary_name(0));
    fs::write(&partial, b"partial").unwrap();
    fixture
        .state
        .jobs
        .lock()
        .unwrap()
        .insert(job.id.clone(), (job.clone(), request(fixture.port).profile));
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&partial)
        .unwrap();
    assert!(discard_transfer_inner(&job.id, None, &fixture.state).is_err());
    assert!(partial.exists());
    assert!(fixture.state.jobs.lock().unwrap().contains_key(&job.id));
    assert!(fixture.state.transfers.lock().unwrap().is_empty());
    drop(held);
    discard_transfer_inner(&job.id, None, &fixture.state).unwrap();
    assert!(!partial.exists());
    assert_eq!(fs::read(&job.local).unwrap(), b"original");
    assert!(!fixture.state.jobs.lock().unwrap().contains_key(&job.id));
}

#[test]
#[ignore = "Requires loopback fixture"]
fn concurrent_pause_then_cancel_preserves_original_targets() {
    let fixture = Fixture::new();
    let mut job = fixture.job("upload", "parallel-cancel");
    job.overwrite = true;
    fs::create_dir_all(&job.local).unwrap();
    fs::create_dir_all(fixture.remote_file("parallel-cancel")).unwrap();
    for index in 0..3 {
        fs::write(
            Path::new(&job.local).join(format!("{index}.bin")),
            payload(),
        )
        .unwrap();
        fs::write(
            fixture.remote_file(&format!("parallel-cancel/{index}.bin")),
            b"original",
        )
        .unwrap();
    }
    let cancel = AtomicBool::new(false);
    let pause = AtomicBool::new(false);
    let event = transfer::execute_concurrent_controlled(
        &job,
        &cancel,
        &pause,
        &fixture.connect(),
        &fixture.state,
        |event| {
            if event.bytes > 0 {
                pause.store(true, Ordering::Relaxed);
            }
            if event.status == "paused" {
                cancel.store(true, Ordering::Relaxed);
            }
        },
        |_, _| Ok(()),
    );
    assert_eq!(event.status, "cancelled", "{:?}", event.error);
    for index in 0..3 {
        assert_eq!(
            fs::read(fixture.remote_file(&format!("parallel-cancel/{index}.bin"))).unwrap(),
            b"original"
        );
    }
    assert!(fs::read_dir(fixture.remote_file("parallel-cancel"))
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part")));
}

#[test]
#[ignore = "Requires loopback fixture"]
fn upload_socket_drop_retains_partial_and_resumes_after_reconnect() {
    for _ in 0..20 {
        let fixture = Fixture::new();
        let job = fixture.job("upload", "resume.bin");
        let data = payload();
        fs::write(&job.local, &data).unwrap();
        fixture
            .faults(serde_json::json!({"writePathContains":job.id,"disconnectAfterBytes":262144}));
        let event = transfer::execute(&job, &AtomicBool::new(false), &fixture.connect(), |_| {});
        assert_eq!(event.status, "failed", "{:?}", event.error);
        let partial = fixture.remote_file(&job.temporary_name(0));
        let bytes = fs::read(&partial).unwrap();
        assert!(!bytes.is_empty() && bytes.len() < data.len());
        assert_eq!(bytes, data[..bytes.len()]);
        assert!(!fixture.remote_file("resume.bin").exists());
        fixture.clear_faults();
        completed(&transfer::execute(
            &job,
            &AtomicBool::new(false),
            &fixture.connect(),
            |_| {},
        ));
        assert_eq!(fs::read(fixture.remote_file("resume.bin")).unwrap(), data);
        assert!(!partial.exists());
    }
}

#[test]
#[ignore = "Requires loopback fixture"]
fn download_connection_stop_retains_partial_and_resumes_after_reconnect() {
    let fixture = Fixture::new();
    let job = fixture.job("download", "resume.bin");
    let data = payload();
    fs::write(fixture.remote_file("resume.bin"), &data).unwrap();
    let conn = fixture.connect();
    let closed = conn.lock().unwrap().closed.clone();
    let event = transfer::execute(&job, &AtomicBool::new(false), &conn, |event| {
        if event.bytes > 0 {
            closed.store(true, Ordering::Relaxed);
        }
    });
    assert_eq!(event.status, "failed");
    let partial = fixture.local.join(job.temporary_name(0));
    let bytes = fs::read(&partial).unwrap();
    assert!(!bytes.is_empty() && bytes.len() < data.len());
    assert_eq!(bytes, data[..bytes.len()]);
    completed(&transfer::execute(
        &job,
        &AtomicBool::new(false),
        &fixture.connect(),
        |_| {},
    ));
    assert_eq!(fs::read(&job.local).unwrap(), data);
    assert!(!partial.exists());
}

#[test]
#[ignore = "Requires loopback fixture"]
fn corrupted_partial_is_rejected_without_overwriting_target() {
    let fixture = Fixture::new();
    for direction in ["upload", "download"] {
        let mut job = fixture.job(direction, &format!("{direction}.bin"));
        job.overwrite = true;
        let partial = job.temporary_name(0);
        let (source, target, temporary) = if direction == "upload" {
            (
                PathBuf::from(&job.local),
                fixture.remote_file("upload.bin"),
                fixture.remote_file(&partial),
            )
        } else {
            (
                fixture.remote_file("download.bin"),
                PathBuf::from(&job.local),
                fixture.local.join(&partial),
            )
        };
        fs::write(source, b"correct-source").unwrap();
        fs::write(&target, b"original-target").unwrap();
        fs::write(&temporary, b"wrong").unwrap();
        let event = transfer::execute(&job, &AtomicBool::new(false), &fixture.connect(), |_| {});
        assert_eq!(event.status, "failed", "{direction}");
        assert_eq!(fs::read(target).unwrap(), b"original-target");
        assert_eq!(fs::read(temporary).unwrap(), b"wrong");
    }
}

#[test]
#[ignore = "Requires loopback fixture"]
fn write_failure_preserves_existing_target() {
    let fixture = Fixture::new();
    let mut job = fixture.job("upload", "write-failure.bin");
    job.overwrite = true;
    fs::write(&job.local, payload()).unwrap();
    fs::write(fixture.remote_file("write-failure.bin"), b"original").unwrap();
    fixture.faults(serde_json::json!({"writePathContains":job.id,"failWriteAfterBytes":262144}));
    let event = transfer::execute(&job, &AtomicBool::new(false), &fixture.connect(), |_| {});
    assert_eq!(event.status, "failed");
    assert_eq!(
        fs::read(fixture.remote_file("write-failure.bin")).unwrap(),
        b"original"
    );
    assert!(fixture.remote_file(&job.temporary_name(0)).exists());
}

fn failed_install(fail_restore: bool) {
    let fixture = Fixture::new();
    let mut job = fixture.job("upload", "rollback.bin");
    job.overwrite = true;
    fs::write(&job.local, b"replacement").unwrap();
    fs::write(fixture.remote_file("rollback.bin"), b"original").unwrap();
    fixture.faults(serde_json::json!({"renameDestination":job.remote,"renameSourceContains":format!(".termrill-{}-",job.id),"failRestore":fail_restore}));
    let event = transfer::execute(&job, &AtomicBool::new(false), &fixture.connect(), |_| {});
    assert_eq!(event.status, "failed");
    if fail_restore {
        let backup = fs::read_dir(fixture.remote_file(""))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".termrill-backup-")
            })
            .expect("original backup retained");
        assert_eq!(fs::read(&backup).unwrap(), b"original");
        assert!(event
            .error
            .unwrap()
            .contains(backup.file_name().unwrap().to_str().unwrap()));
    } else {
        assert_eq!(
            fs::read(fixture.remote_file("rollback.bin")).unwrap(),
            b"original"
        );
    }
}
#[test]
#[ignore = "Requires loopback fixture"]
fn failed_remote_install_restores_original() {
    failed_install(false);
}
#[test]
#[ignore = "Requires loopback fixture"]
fn failed_remote_restore_reports_retained_backup() {
    failed_install(true);
}

#[test]
#[ignore = "Requires loopback fixture"]
fn cancellation_preserves_original_and_cleans_current_partial() {
    let fixture = Fixture::new();
    let mut job = fixture.job("upload", "cancel.bin");
    job.overwrite = true;
    fs::write(&job.local, payload()).unwrap();
    fs::write(fixture.remote_file("cancel.bin"), b"original").unwrap();
    let cancel = AtomicBool::new(false);
    let event = transfer::execute(&job, &cancel, &fixture.connect(), |event| {
        if event.bytes > 0 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert_eq!(event.status, "cancelled");
    assert_eq!(
        fs::read(fixture.remote_file("cancel.bin")).unwrap(),
        b"original"
    );
    assert!(!fixture.remote_file(&job.temporary_name(0)).exists());
}

#[test]
#[ignore = "Requires loopback fixture"]
fn source_truncated_during_copy_does_not_replace_existing_target() {
    let fixture = Fixture::new();
    let mut job = fixture.job("upload", "changing.bin");
    job.overwrite = true;
    fs::write(&job.local, payload()).unwrap();
    fs::write(fixture.remote_file("changing.bin"), b"original").unwrap();
    let mutator = fs::OpenOptions::new().write(true).open(&job.local).unwrap();
    let event = transfer::execute(&job, &AtomicBool::new(false), &fixture.connect(), |event| {
        if event.bytes > 0 {
            mutator.set_len(0).unwrap();
        }
    });
    assert_eq!(event.status, "failed", "{:?}", event.error);
    assert_eq!(
        fs::read(fixture.remote_file("changing.bin")).unwrap(),
        b"original"
    );
}

#[test]
#[ignore = "Requires loopback fixture"]
fn pause_keeps_partial_and_continues_without_restarting() {
    let fixture = Fixture::new();
    let job = fixture.job("upload", "pause.bin");
    let data = payload();
    fs::write(&job.local, &data).unwrap();
    let pause = AtomicBool::new(false);
    let requested = AtomicBool::new(false);
    let (sender, receiver) = std::sync::mpsc::channel();
    let receiver = Mutex::new(receiver);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            receiver
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            let partial = fixture.remote_file(&job.temporary_name(0));
            let before = fs::read(&partial).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(200));
            assert_eq!(fs::read(partial).unwrap(), before);
            pause.store(false, Ordering::Relaxed);
        });
        let event = transfer::execute_controlled(
            &job,
            &AtomicBool::new(false),
            &pause,
            &fixture.connect(),
            |event| {
                if event.status == "running"
                    && event.bytes > 0
                    && !requested.swap(true, Ordering::Relaxed)
                {
                    pause.store(true, Ordering::Relaxed);
                }
                if event.status == "paused" {
                    sender.send(()).unwrap();
                }
            },
            |_, _| Ok(()),
        );
        completed(&event);
    });
    assert_eq!(fs::read(fixture.remote_file("pause.bin")).unwrap(), data);
}

#[test]
#[ignore = "Requires loopback fixture"]
fn download_truncation_does_not_replace_existing_target() {
    let fixture = Fixture::new();
    let mut job = fixture.job("download", "changing.bin");
    job.overwrite = true;
    let remote = fixture.remote_file("changing.bin");
    fs::write(&remote, payload()).unwrap();
    fs::write(&job.local, b"original").unwrap();
    let mutator = fs::OpenOptions::new().write(true).open(remote).unwrap();
    let event = transfer::execute(&job, &AtomicBool::new(false), &fixture.connect(), |event| {
        if event.bytes > 0 {
            mutator.set_len(0).unwrap();
        }
    });
    assert_eq!(event.status, "failed", "{:?}", event.error);
    assert_eq!(fs::read(&job.local).unwrap(), b"original");
}

#[test]
#[ignore = "Requires loopback fixture"]
fn pagination_is_bounded_and_searches_unvisited_entries() {
    let fixture = Fixture::new();
    for index in 0..2205 {
        fs::write(fixture.remote_file(&format!("item-{index:04}.txt")), []).unwrap();
    }
    fs::write(fixture.remote_file("zz-needle.txt"), []).unwrap();
    fs::write(fixture.remote_file(".hidden-needle"), []).unwrap();
    let conn = fixture.connect();
    let guard = conn.lock().unwrap();
    let sftp = guard.session.sftp().unwrap();
    let mut seen = HashSet::new();
    for offset in [0, 1000, 2000] {
        let page = directory::remote(
            &sftp,
            &fixture.remote,
            &guard.closed,
            directory::Filter::new(offset, String::new(), false).unwrap(),
        )
        .unwrap();
        assert!(page.listing.entries.len() <= 1000);
        assert_eq!(page.page.has_more, offset < 2000);
        for entry in page.listing.entries {
            assert!(seen.insert(entry.path));
        }
    }
    assert_eq!(seen.len(), 2206);
    let search = directory::remote(
        &sftp,
        &fixture.remote,
        &guard.closed,
        directory::Filter::new(0, "needle".into(), false).unwrap(),
    )
    .unwrap();
    assert_eq!(search.listing.entries.len(), 1);
    assert_eq!(search.listing.entries[0].name, "zz-needle.txt");
    let hidden = directory::remote(
        &sftp,
        &fixture.remote,
        &guard.closed,
        directory::Filter::new(0, "needle".into(), true).unwrap(),
    )
    .unwrap();
    assert_eq!(hidden.listing.entries.len(), 2);
    let local = directory::local(
        &fixture.remote_file(""),
        directory::Filter::new(2000, String::new(), false).unwrap(),
    )
    .unwrap();
    assert_eq!(local.listing.entries.len(), 206);
    assert!(!local.page.has_more);
}

#[test]
#[ignore = "Requires loopback fixture"]
fn editor_refuses_stale_revision_and_recursive_delete_requires_opt_in() {
    let fixture = Fixture::new();
    let conn = fixture.connect();
    let guard = conn.lock().unwrap();
    let sftp = guard.session.sftp().unwrap();
    fs::write(fixture.remote_file("edit.txt"), b"before").unwrap();
    let path = PathBuf::from(format!("{}/edit.txt", fixture.remote));
    let document = serde_json::to_value(files::open(&sftp, &path).unwrap()).unwrap();
    let revision = document["revision"].as_str().unwrap();
    fs::write(fixture.remote_file("edit.txt"), b"changed elsewhere").unwrap();
    assert!(files::save(&sftp, &path, "new", revision).is_err());
    assert_eq!(
        fs::read(fixture.remote_file("edit.txt")).unwrap(),
        b"changed elsewhere"
    );
    let current = serde_json::to_value(files::open(&sftp, &path).unwrap()).unwrap();
    files::save(&sftp, &path, "saved", current["revision"].as_str().unwrap()).unwrap();
    assert_eq!(fs::read(fixture.remote_file("edit.txt")).unwrap(), b"saved");
    fs::create_dir(fixture.remote_file("remove")).unwrap();
    fs::write(fixture.remote_file("remove/child.txt"), b"child").unwrap();
    let directory = PathBuf::from(format!("{}/remove", fixture.remote));
    let error = files::delete(&directory, Some(&sftp), false).err().unwrap();
    assert!(error.message.contains("目录非空"));
    assert!(error.message.contains(&directory.display().to_string()));
    assert!(fixture.remote_file("remove/child.txt").exists());
    fs::create_dir_all(fixture.remote_file("remove/nested")).unwrap();
    for index in 0..120 {
        fs::write(
            fixture.remote_file(&format!("remove/nested/{index}.txt")),
            b"test",
        )
        .unwrap();
    }
    fs::write(fixture.remote_file("outside.txt"), b"keep").unwrap();
    files::delete(&directory, Some(&sftp), true).unwrap();
    assert_eq!(
        fs::read(fixture.remote_file("outside.txt")).unwrap(),
        b"keep"
    );
    assert!(!fixture.remote_file("remove").exists());
    assert!(files::delete(Path::new("/"), Some(&sftp), true).is_err());
}

#[test]
#[ignore = "Requires loopback fixture"]
fn directory_sync_selects_content_changes_and_preserves_extras() {
    let fixture = Fixture::new();
    let mut job = fixture.job("upload", "sync");
    fs::create_dir_all(&job.local).unwrap();
    fs::create_dir_all(fixture.remote_file("sync")).unwrap();
    fs::write(Path::new(&job.local).join("same.txt"), b"same").unwrap();
    fs::write(fixture.remote_file("sync/same.txt"), b"same").unwrap();
    fs::write(Path::new(&job.local).join("changed.txt"), b"next").unwrap();
    fs::write(fixture.remote_file("sync/changed.txt"), b"prev").unwrap();
    fs::write(Path::new(&job.local).join("new.txt"), b"new").unwrap();
    fs::write(fixture.remote_file("sync/extra.txt"), b"keep").unwrap();
    fs::create_dir_all(Path::new(&job.local).join("empty")).unwrap();
    for index in 0..120 {
        fs::write(
            Path::new(&job.local).join(format!("bulk-{index}.txt")),
            format!("item-{index}"),
        )
        .unwrap();
    }
    let conn = fixture.connect();
    let changed_time = fs::metadata(Path::new(&job.local).join("changed.txt"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let sftp = conn.lock().unwrap().directory_sftp().unwrap();
    let changed_remote = PathBuf::from(format!("{}/changed.txt", job.remote));
    let mut stat = sftp.lstat(&changed_remote).unwrap();
    stat.atime = Some(changed_time);
    stat.mtime = Some(changed_time);
    sftp.setstat(&changed_remote, stat).unwrap();
    let rows = directory_sync::compare(
        &conn.lock().unwrap().directory_sftp().unwrap(),
        Path::new(&job.local),
        Path::new(&job.remote),
        "upload",
        &AtomicBool::new(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(rows
        .iter()
        .any(|row| row.relative == "same.txt" && row.status == "same"));
    assert!(rows
        .iter()
        .any(|row| row.relative == "changed.txt" && row.status == "changed"));
    assert!(rows
        .iter()
        .any(|row| row.relative == "extra.txt" && row.status == "targetOnly"));
    job.selection = Some(
        rows.iter()
            .filter(|row| ["new", "changed"].contains(&row.status.as_str()))
            .map(|row| row.remote.clone())
            .collect(),
    );
    job.overwrite = true;
    completed(&transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &conn,
        &fixture.state,
        |_| {},
        |_, _| Ok(()),
    ));
    assert_eq!(
        fs::read(fixture.remote_file("sync/changed.txt")).unwrap(),
        b"next"
    );
    assert_eq!(
        fs::read(fixture.remote_file("sync/extra.txt")).unwrap(),
        b"keep"
    );
    assert!(fixture.remote_file("sync/empty").is_dir());
    assert!(fixture.remote_file("sync/new.txt").is_file());
}

#[test]
#[ignore = "Requires loopback fixture"]
fn pipeline_buffers_measure_upload_and_download_without_assuming_speedup() {
    let fixture = Fixture::new();
    let conn = fixture.connect();
    let conn = conn.lock().unwrap();
    let sftp = conn.session.sftp().unwrap();
    let flag = AtomicBool::new(false);
    let data = payload();
    for size in [65536, 1024 * 1024] {
        let path = PathBuf::from(format!("{}/buffer-{size}.bin", fixture.remote));
        let mut dest = sftp
            .open_mode(
                &path,
                ssh2::OpenFlags::WRITE | ssh2::OpenFlags::CREATE | ssh2::OpenFlags::EXCLUSIVE,
                0o600,
                ssh2::OpenType::File,
            )
            .unwrap();
        let started = std::time::Instant::now();
        {
            let _mode = transfer_io::Nonblocking::new(&conn.session);
            for chunk in data.chunks(size) {
                transfer_io::write(&mut dest, chunk, &flag, &flag, &flag, |_| {}).unwrap();
            }
        }
        dest.close().unwrap();
        let upload = started.elapsed();
        let mut source = sftp.open(&path).unwrap();
        let mut buffer = vec![0; size];
        let mut downloaded = Vec::new();
        let started = std::time::Instant::now();
        {
            let _mode = transfer_io::Nonblocking::new(&conn.session);
            loop {
                let count =
                    transfer_io::read(&mut source, &mut buffer, &flag, &flag, &flag).unwrap();
                if count == 0 {
                    break;
                }
                downloaded.extend_from_slice(&buffer[..count]);
            }
        }
        let download = started.elapsed();
        assert_eq!(downloaded, data);
        println!(
            "buffer={size} bytes, payload={} bytes, upload={upload:?}, download={download:?}",
            data.len()
        );
    }
}

#[test]
#[ignore = "Requires loopback fixture"]
fn deletion_request_pipeline_removes_wrapped_tree_and_keeps_sibling() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let guard = connection.lock().unwrap();
    let sftp = guard.session.sftp().unwrap();
    fs::create_dir_all(fixture.remote_file("remove/wrapper")).unwrap();
    for dir in 0..24 {
        let path = format!("remove/wrapper/{dir}");
        fs::create_dir_all(fixture.remote_file(&path)).unwrap();
        for file in 0..3 {
            fs::write(fixture.remote_file(&format!("{path}/{file}.txt")), b"test").unwrap();
        }
    }
    fs::write(fixture.remote_file("keep.txt"), b"keep").unwrap();
    delete_parallel::run(
        &format!("{}/remove", fixture.remote),
        true,
        guard.request.clone(),
        &fixture.state,
        &guard.closed,
        (std::sync::Arc::new(sftp), &guard.session),
        &|_| {},
    )
    .unwrap();
    assert!(!fixture.remote_file("remove").exists());
    assert_eq!(fs::read(fixture.remote_file("keep.txt")).unwrap(), b"keep");
    assert!(guard.session.is_blocking());
}

#[test]
#[ignore = "Requires loopback fixture"]
fn file_only_delete_rejects_directory_and_root() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let guard = connection.lock().unwrap();
    let sftp = guard.session.sftp().unwrap();
    fs::write(fixture.remote_file("file.txt"), b"remove").unwrap();
    files::delete_file_observed(
        Path::new(&format!("{}/file.txt", fixture.remote)),
        &sftp,
        "file-test",
        &|_| {},
    )
    .unwrap();
    assert!(!fixture.remote_file("file.txt").exists());
    fs::create_dir_all(fixture.remote_file("changed")).unwrap();
    fs::write(fixture.remote_file("changed/keep.txt"), b"keep").unwrap();
    assert!(files::delete_file_observed(
        Path::new(&format!("{}/changed", fixture.remote)),
        &sftp,
        "file-test",
        &|_| {}
    )
    .is_err());
    assert_eq!(
        fs::read(fixture.remote_file("changed/keep.txt")).unwrap(),
        b"keep"
    );
    assert!(files::delete_file_observed(Path::new("/"), &sftp, "file-test", &|_| {}).is_err());
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_async_browse_preview_chunks_install_permissions_and_delete() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let content = "2026-10-04 INFO fixture\n".repeat(4000);
    fs::write(fixture.remote_file("example.log"), content.as_bytes()).unwrap();
    tauri::async_runtime::block_on(async {
        let remote = remote_async::get(&connection, &fixture.state)
            .await
            .unwrap();
        let cancel = AtomicBool::new(false);
        let listing = remote
            .list(
                &fixture.remote,
                directory::Filter::browser(0, String::new(), true).unwrap(),
                &cancel,
            )
            .await
            .unwrap();
        assert!(listing
            .listing
            .entries
            .iter()
            .any(|entry| entry.name == "example.log"));
        let path = format!("{}/example.log", fixture.remote);
        assert_eq!(
            remote
                .read(&path, 1024 * 1024, &cancel, |_| {})
                .await
                .unwrap(),
            content.as_bytes()
        );
        assert_eq!(
            remote
                .read_preview(&path, 1024 * 1024, &cancel, |_| {})
                .await
                .unwrap(),
            content.as_bytes()
        );
        assert!(remote
            .read_preview(&path, 16, &cancel, |_| {})
            .await
            .is_err());
        let stopped = AtomicBool::new(true);
        assert!(remote
            .read_preview(&path, 1024 * 1024, &stopped, |_| {})
            .await
            .is_err());
        let mut offset = 0;
        let mut bytes = Vec::new();
        let mut stamp = None;
        loop {
            let chunk = remote
                .chunk("fixture-reader", &path, offset, stamp.as_deref())
                .await
                .unwrap();
            use base64::Engine;
            bytes.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(&chunk.bytes)
                    .unwrap(),
            );
            offset = chunk.next;
            stamp = Some(chunk.stamp);
            if chunk.next >= chunk.total {
                break;
            }
        }
        assert_eq!(bytes, content.as_bytes());
        let previous = permissions::inspect_async(&remote, &path).await.unwrap();
        let value = serde_json::to_value(previous).unwrap();
        let mode = value["mode"].as_u64().unwrap() as u32;
        let changed = permissions::update_async(&remote, &path, 0o600, mode)
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(changed).unwrap()["mode"], 0o600);
        assert!(permissions::update_async(&remote, &path, 0o644, mode)
            .await
            .is_err());
        let new = format!("{}/new.txt", fixture.remote);
        remote
            .install_bytes(&new, b"new", false, &cancel, None)
            .await
            .unwrap();
        assert!(remote
            .install_bytes(&new, b"bad", false, &cancel, None)
            .await
            .is_err());
        assert_eq!(fs::read(fixture.remote_file("new.txt")).unwrap(), b"new");
        let dir = format!("{}/delete-tree", fixture.remote);
        fs::create_dir_all(fixture.remote_file("delete-tree/sub")).unwrap();
        fs::write(fixture.remote_file("delete-tree/sub/a"), b"a").unwrap();
        fs::write(fixture.remote_file("keep.txt"), b"keep").unwrap();
        assert!(remote_delete::run(&remote, &dir, false, false, &|_| {})
            .await
            .is_err());
        assert!(fixture.remote_file("delete-tree/sub/a").exists());
        remote_delete::run(&remote, &dir, true, false, &|_| {})
            .await
            .unwrap();
        assert!(!fixture.remote_file("delete-tree").exists());
        assert!(fixture.remote_file("keep.txt").exists());
    });
}
#[test]
#[ignore = "Requires loopback fixture"]
fn native_async_download_resumes_verified_prefix_and_refuses_corruption() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let mut job = fixture.job("download", "native.bin");
    let data = payload();
    fs::write(fixture.remote_file("native.bin"), &data).unwrap();
    let temporary = fixture.local.join(job.temporary_name(0));
    fs::write(&temporary, &data[..65536]).unwrap();
    let saved = Mutex::new(BTreeMap::new());
    let event = transfer::execute_native_download(
        &job,
        &connection,
        &fixture.state,
        &AtomicBool::new(false),
        &AtomicBool::new(false),
        &|_| {},
        &|path, digest| {
            saved.lock().unwrap().insert(path.to_owned(), digest);
            Ok(())
        },
    );
    completed(&event);
    assert_eq!(event.bytes, data.len() as u64);
    assert_eq!(fs::read(&job.local).unwrap(), data);
    assert!(!temporary.exists());
    assert_eq!(saved.lock().unwrap().len(), 1);
    job.id = identifier();
    job.overwrite = true;
    let temporary = fixture.local.join(job.temporary_name(0));
    fs::write(&temporary, b"corrupt").unwrap();
    let event = transfer::execute_native_download(
        &job,
        &connection,
        &fixture.state,
        &AtomicBool::new(false),
        &AtomicBool::new(false),
        &|_| {},
        &|_, _| Ok(()),
    );
    assert_eq!(event.status, "failed");
    assert_eq!(fs::read(&job.local).unwrap(), data);
    assert!(temporary.exists());
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_download_cancel_reports_stop_instead_of_missing_ranges() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let mut job = fixture.job("download", "cancel-download.bin");
    job.overwrite = true;
    let data = payload();
    fs::write(fixture.remote_file("cancel-download.bin"), &data).unwrap();
    fs::write(&job.local, b"original").unwrap();
    let item = transfer::Item {
        local: PathBuf::from(&job.local),
        remote: job.remote.clone().into(),
        directory: false,
        size: data.len() as u64,
        link: None,
        link_directory: false,
        remote_target: None,
    };
    tauri::async_runtime::block_on(async {
        let remote = remote_async::get(&connection, &fixture.state)
            .await
            .unwrap();
        let cancel = AtomicBool::new(false);
        let pause = AtomicBool::new(false);
        let limiter = transfer_rate::RateLimiter::new(128 * 1024);
        let (progress, mut updates) = tokio::sync::mpsc::unbounded_channel();
        let download = transfer::measure_download_file(
            &remote, &job, 0, &item, &cancel, &pause, &limiter, &progress,
        );
        let stopping = async {
            assert!(updates.recv().await.unwrap() > 0);
            cancel.store(true, Ordering::Relaxed);
        };
        let (result, ()) = tokio::join!(download, stopping);
        assert_eq!(result.unwrap_err().message, "传输已取消");
        assert_eq!(fs::read(&job.local).unwrap(), b"original");
        assert!(!item.local.with_file_name(job.temporary_name(0)).exists());
    });
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_async_batch_compare_terminal_and_reader_lifecycle() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    fs::create_dir_all(fixture.remote_file("tree/sub")).unwrap();
    fs::write(fixture.remote_file("tree/sub/a.txt"), b"same").unwrap();
    fs::create_dir_all(fixture.local.join("sub")).unwrap();
    fs::write(fixture.local.join("sub/a.txt"), b"same").unwrap();
    tauri::async_runtime::block_on(async {
        let remote = remote_async::get(&connection, &fixture.state)
            .await
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let tree = format!("{}/tree", fixture.remote);
        let result = permissions::batch_async(
            &remote,
            vec![tree.clone()],
            Some(0o755),
            None,
            true,
            &cancel,
            &|_| {},
        )
        .await
        .unwrap();
        let result = serde_json::to_value(result).unwrap();
        assert_eq!(result["checked"], 3);
        assert_eq!(result["errors"].as_array().unwrap().len(), 0);
        let compared = directory_sync::compare_async(
            &remote,
            &fixture.local,
            Path::new(&tree),
            "upload",
            &cancel,
        )
        .await
        .unwrap();
        let compared = serde_json::to_value(compared).unwrap();
        assert!(compared
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["relative"] == "sub/a.txt" && entry["status"] == "same"));
        let path = format!("{tree}/sub/a.txt");
        remote.chunk("old", &path, 0, None).await.unwrap();
        remote.close_reader("old").await.unwrap();
        let current = remote.chunk("new", &path, 0, None).await.unwrap();
        remote.close_reader("old").await.unwrap();
        assert_eq!(current.total, 4);
        assert!(remote_delete::run(&remote, "/", true, false, &|_| {})
            .await
            .is_err());
        let (mut channel, initial) = remote.native.terminal_channel(80, 24).await.unwrap();
        channel.data_bytes(b"native echo\n".to_vec()).await.unwrap();
        let mut output = initial;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !String::from_utf8_lossy(&output).contains("native echo") {
                if let Some(russh::ChannelMsg::Data { data }) = channel.wait().await {
                    output.extend_from_slice(&data);
                }
            }
        })
        .await
        .unwrap();
        assert!(String::from_utf8_lossy(&output).contains("TermRill fixture shell"));
        channel.close().await.unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert!(remote.read(&path, 1024, &cancel, |_| {}).await.is_err());
        assert_eq!(
            fs::read(fixture.remote_file("tree/sub/a.txt")).unwrap(),
            b"same"
        );
    });
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_async_archive_list_and_preview_use_remote_transport() {
    use std::{cell::RefCell, io::Write};
    let fixture = Fixture::new();
    fixture.faults(serde_json::json!({"auditRead": true, "readDelayMs": 80}));
    let connection = fixture.connect();
    let path = format!("{}/example.zip", fixture.remote);
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file("example.log", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"INFO native archive\n").unwrap();
    writer
        .start_file(
            "uncached.log",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(&vec![b'x'; 128 * 1024]).unwrap();
    // 不易压缩的 Deflate 条目跨越多个预读批次，覆盖解码器小缓冲与乱序回填。
    let mut random = 0x1234_5678u32;
    let large_payload: Vec<u8> = (0..3 * 1024 * 1024)
        .map(|_| {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            random as u8
        })
        .collect();
    writer
        .start_file(
            "large.bin",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    writer.write_all(&large_payload).unwrap();
    fs::write(
        fixture.remote_file("example.zip"),
        writer.finish().unwrap().into_inner(),
    )
    .unwrap();
    let remote =
        tauri::async_runtime::block_on(remote_async::get(&connection, &fixture.state)).unwrap();
    let cancel = AtomicBool::new(false);
    let clock = RefCell::new(None);
    let io = archive_io::ArchiveIo {
        native: Some(&remote),
        cancel: &cancel,
        closed: Some(&remote.closed),
        progress: tauri::ipc::Channel::new(|_| Ok(())),
        last_progress: &clock,
    };
    let local = archive_io::ArchiveIo {
        native: None,
        cancel: &cancel,
        closed: None,
        progress: tauri::ipc::Channel::new(|_| Ok(())),
        last_progress: &clock,
    };
    let mut request: archives::Request = serde_json::from_value(
        serde_json::json!({"operation":"list", "path":path, "snapshot":"native-test"}),
    )
    .unwrap();
    let listed =
        serde_json::to_value(archives::run(request.clone(), &io, &local, "fixture").unwrap())
            .unwrap();
    assert_eq!(listed["entries"][0]["name"], "example.log");
    assert_eq!(
        fs::read_to_string(fixture.root.join("read-events.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1,
        "小中央目录与尾记录应只发送一次 READ"
    );
    request.operation = "preview".into();
    request.selected = vec!["example.log".into()];
    request.revision = Some(listed["revision"].as_str().unwrap().into());
    let preview =
        serde_json::to_value(archives::run(request.clone(), &io, &local, "fixture").unwrap())
            .unwrap();
    use base64::Engine;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(preview["bytes"].as_str().unwrap())
            .unwrap(),
        b"INFO native archive\n"
    );
    // 关闭并重新打开面板：中央目录仍刷新，但已校验的负载跨面板复用。
    archives::release("fixture", "native-test", &path);
    request.snapshot = Some("native-reopened".into());
    request.operation = "list".into();
    archives::run(request.clone(), &io, &local, "fixture").unwrap();
    let read_count = || {
        fs::read_to_string(fixture.root.join("read-events.jsonl"))
            .unwrap()
            .lines()
            .count()
    };
    let before_cached_preview = read_count();
    request.operation = "preview".into();
    let reopened =
        serde_json::to_value(archives::run(request.clone(), &io, &local, "fixture").unwrap())
            .unwrap();
    assert_eq!(reopened["bytes"], preview["bytes"]);
    assert_eq!(
        read_count(),
        before_cached_preview,
        "缓存图片不得重复发送 READ"
    );
    // 预取正在等待响应时取消，下一次读取仍复用进入操作前的已验证快照。
    let mut pending = request.clone();
    pending.selected = vec!["uncached.log".into()];
    std::thread::scope(|scope| {
        let pending = scope.spawn(|| {
            let clock = RefCell::new(None);
            let io = archive_io::ArchiveIo {
                native: Some(&remote),
                cancel: &cancel,
                closed: Some(&remote.closed),
                progress: tauri::ipc::Channel::new(|_| Ok(())),
                last_progress: &clock,
            };
            let local = archive_io::ArchiveIo {
                native: None,
                cancel: &cancel,
                closed: None,
                progress: tauri::ipc::Channel::new(|_| Ok(())),
                last_progress: &clock,
            };
            archives::run(pending, &io, &local, "fixture").is_err()
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        cancel.store(true, Ordering::Relaxed);
        assert!(pending.join().unwrap());
    });
    cancel.store(false, Ordering::Relaxed);
    let after_cancel =
        serde_json::to_value(archives::run(request.clone(), &io, &local, "fixture").unwrap())
            .unwrap();
    assert_eq!(after_cancel["bytes"], preview["bytes"]);
    let mut large_request = request.clone();
    large_request.selected = vec!["large.bin".into()];
    let large_reply =
        serde_json::to_value(archives::run(large_request, &io, &local, "fixture").unwrap())
            .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(large_reply["bytes"].as_str().unwrap())
            .unwrap(),
        large_payload
    );
    // 未缓存条目的读取仍在返回前核对属性；删除读取前 FSTAT 不允许源变化被接受。
    fs::OpenOptions::new()
        .append(true)
        .open(fixture.remote_file("example.zip"))
        .unwrap()
        .write_all(b"changed")
        .unwrap();
    request.selected = vec!["uncached.log".into()];
    assert!(archives::run(request, &io, &local, "fixture").is_err());
    archives::release("fixture", "native-reopened", &path);
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_async_delete_streams_batches_and_completes_dependencies_with_one_handle() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    fs::create_dir_all(fixture.remote_file("tree/empty")).unwrap();
    fs::create_dir_all(fixture.remote_file("tree/deep/sub")).unwrap();
    for index in 0..180 {
        fs::write(
            fixture.remote_file(&format!("tree/file-{index:03}")),
            b"data",
        )
        .unwrap();
    }
    fs::write(fixture.remote_file("tree/deep/sub/file"), b"data").unwrap();
    fs::write(fixture.remote_file("sibling"), b"keep").unwrap();
    fixture.faults(serde_json::json!({"auditDelete":true}));
    tauri::async_runtime::block_on(async {
        let request = connection.lock().unwrap().request.clone();
        let trusted = fixture.state.config.lock().unwrap().known_hosts.clone();
        let mut native = transfer::native_async::ssh::Connection::connect(
            &request,
            &trusted,
            &Default::default(),
            Some(&fixture.state),
        )
        .await
        .unwrap();
        native.max_files = 1;
        let remote = remote_async::Remote::new(native, Arc::new(AtomicBool::new(false)));
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            remote_delete::run(
                &remote,
                &format!("{}/tree", fixture.remote),
                true,
                false,
                &|_| {},
            ),
        )
        .await
        .unwrap()
        .unwrap();
    });
    assert!(!fixture.remote_file("tree").exists());
    assert_eq!(fs::read(fixture.remote_file("sibling")).unwrap(), b"keep");
    let events: Vec<serde_json::Value> =
        fs::read_to_string(fixture.root.join("delete-events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    let first_delete = events
        .iter()
        .position(|entry| entry["kind"] == "unlink")
        .unwrap();
    let last_read = events
        .iter()
        .rposition(|entry| entry["kind"] == "readdir")
        .unwrap();
    assert!(first_delete < last_read, "必须在扫描结束前开始删除");
    let root = format!("{}/tree", fixture.remote);
    let root_removed = events
        .iter()
        .position(|entry| entry["kind"] == "rmdir" && entry["path"] == root)
        .unwrap();
    assert_eq!(root_removed, events.len() - 1, "父目录必须等待全部子项确认");
    assert_eq!(
        events
            .iter()
            .filter(|entry| entry["kind"] == "unlink")
            .count(),
        181
    );
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_async_delete_failure_and_nonrecursive_request_preserve_remaining_tree() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    fs::create_dir_all(fixture.remote_file("tree/sub")).unwrap();
    fs::write(fixture.remote_file("tree/sub/protected"), b"keep").unwrap();
    tauri::async_runtime::block_on(async {
        let remote = remote_async::get(&connection, &fixture.state)
            .await
            .unwrap();
        let path = format!("{}/tree", fixture.remote);
        assert!(remote_delete::run(&remote, &path, false, false, &|_| {})
            .await
            .is_err());
        assert!(fixture.remote_file("tree/sub/protected").exists());
        fixture.faults(serde_json::json!({"failDeletePath":format!("{path}/sub/protected")}));
        assert!(remote_delete::run(&remote, &path, true, false, &|_| {})
            .await
            .is_err());
        assert_eq!(
            fs::read(fixture.remote_file("tree/sub/protected")).unwrap(),
            b"keep"
        );
        fixture.clear_faults();
        // 失败句柄已释放，下一次显式操作能正常完成；没有自动重发删除。
        remote_delete::run(&remote, &path, true, false, &|_| {})
            .await
            .unwrap();
        assert!(!fixture.remote_file("tree").exists());
    });
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_async_striped_upload_uses_four_connections_and_repairs_a_hole() {
    let fixture = Fixture::new();
    fixture
        .state
        .config
        .lock()
        .unwrap()
        .transfer_settings
        .concurrency = 4;
    let mut job = fixture.job("upload", "striped.bin");
    job.overwrite = true;
    let data = payload().repeat(3);
    fs::write(&job.local, &data).unwrap();
    let target = fixture.remote_file("striped.bin");
    fs::write(&target, b"original").unwrap();
    let audit = fixture.root.join("write-events.jsonl");
    let _ = fs::remove_file(&audit);
    fixture.faults(serde_json::json!({"auditWrite":true, "writePathContains":job.id, "failWriteAtOffset":32768}));
    let connection = fixture.connect();
    let run = || {
        transfer::execute_concurrent(
            &job,
            &AtomicBool::new(false),
            &connection,
            &fixture.state,
            |_| {},
            |_, _| Ok(()),
        )
    };
    let event = run();
    assert_eq!(event.status, "failed", "{:?}", event.error);
    assert_eq!(fs::read(&target).unwrap(), b"original");
    let partial = fixture.remote_file(&job.temporary_name(0));
    let prefix = fs::read(&partial).unwrap();
    assert_eq!(prefix, data[..prefix.len()]);
    assert_eq!(prefix.len(), 32768);
    let connections: HashSet<_> = fs::read_to_string(&audit)
        .unwrap()
        .lines()
        .map(|line| {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            event["connection"].as_u64().unwrap()
        })
        .collect();
    assert_eq!(
        connections.len(),
        4,
        "单个文件必须在四条连接上发出真实 WRITE"
    );
    fixture.clear_faults();
    let event = run();
    completed(&event);
    assert_eq!(event.bytes, event.total);
    assert_eq!(fs::read(target).unwrap(), data);
    assert!(!partial.exists());
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_async_striped_upload_disconnect_never_reuses_sparse_length() {
    let fixture = Fixture::new();
    fixture
        .state
        .config
        .lock()
        .unwrap()
        .transfer_settings
        .concurrency = 4;
    let job = fixture.job("upload", "striped-disconnect.bin");
    let data = payload().repeat(3);
    fs::write(&job.local, &data).unwrap();
    fixture.faults(serde_json::json!({"writePathContains":job.id, "disconnectAfterBytes":262144}));
    let connection = fixture.connect();
    let run = || {
        transfer::execute_concurrent(
            &job,
            &AtomicBool::new(false),
            &connection,
            &fixture.state,
            |_| {},
            |_, _| Ok(()),
        )
    };
    assert_eq!(run().status, "failed");
    assert!(!fixture.remote_file(&job.temporary_name(0)).exists());
    assert!(!fixture.remote_file("striped-disconnect.bin").exists());
    fixture.clear_faults();
    completed(&run());
    assert_eq!(
        fs::read(fixture.remote_file("striped-disconnect.bin")).unwrap(),
        data
    );
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_async_striped_upload_pause_resume_and_cancel_preserve_target() {
    for stop in [false, true] {
        let fixture = Fixture::new();
        fixture
            .state
            .config
            .lock()
            .unwrap()
            .transfer_settings
            .concurrency = 4;
        let mut job = fixture.job("upload", "striped-pause.bin");
        job.overwrite = true;
        let data = payload().repeat(3);
        fs::write(&job.local, &data).unwrap();
        let target = fixture.remote_file("striped-pause.bin");
        fs::write(&target, b"original").unwrap();
        fixture.faults(serde_json::json!({"writeDelayMs":8}));
        let pause = AtomicBool::new(false);
        let cancel = AtomicBool::new(false);
        let requested = AtomicBool::new(false);
        let observed = AtomicBool::new(false);
        let event = transfer::execute_concurrent_controlled(
            &job,
            &cancel,
            &pause,
            &fixture.connect(),
            &fixture.state,
            |event| {
                if event.status == "running"
                    && event.bytes > 0
                    && !requested.swap(true, Ordering::Relaxed)
                {
                    pause.store(true, Ordering::Relaxed);
                }
                if event.status == "paused" {
                    observed.store(true, Ordering::Relaxed);
                    if stop {
                        cancel.store(true, Ordering::Relaxed);
                    } else {
                        pause.store(false, Ordering::Relaxed);
                    }
                }
            },
            |_, _| Ok(()),
        );
        assert!(
            observed.load(Ordering::Relaxed),
            "暂停必须等 WRITE 收齐后可见"
        );
        if stop {
            assert_eq!(event.status, "cancelled");
            assert_eq!(fs::read(target).unwrap(), b"original");
        } else {
            completed(&event);
            assert_eq!(fs::read(target).unwrap(), data);
        }
    }
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_async_upload_resumes_unaligned_prefix_and_rejects_late_corruption() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let mut job = fixture.job("upload", "native-resume.bin");
    job.overwrite = true;
    let data = payload();
    fs::write(&job.local, &data).unwrap();
    let target = fixture.remote_file("native-resume.bin");
    let temporary = fixture.remote_file(&job.temporary_name(0));
    let prefix_length = 700_001;
    fs::write(&target, b"original").unwrap();
    let mut prefix = data[..prefix_length].to_vec();
    prefix[prefix_length - 1] ^= 1;
    fs::write(&temporary, &prefix).unwrap();
    let run = || {
        transfer::execute_concurrent(
            &job,
            &AtomicBool::new(false),
            &connection,
            &fixture.state,
            |_| {},
            |_, _| Ok(()),
        )
    };
    assert_eq!(run().status, "failed");
    assert_eq!(fs::read(&target).unwrap(), b"original");
    assert_eq!(fs::read(&temporary).unwrap(), prefix);
    fs::write(&temporary, &data[..prefix_length]).unwrap();
    let event = run();
    completed(&event);
    assert_eq!(event.bytes, data.len() as u64);
    assert_eq!(fs::read(&target).unwrap(), data);
    assert!(!temporary.exists());
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_upload_large_writes_obey_advertised_limits() {
    let fixture = Fixture::new();
    let data = payload().repeat(3);
    for limit in [16 * 1024, 128 * 1024] {
        let job = fixture.job("upload", &format!("large-write-{limit}.bin"));
        fs::write(&job.local, &data).unwrap();
        let audit = fixture.root.join("write-events.jsonl");
        let _ = fs::remove_file(&audit);
        fixture.faults(serde_json::json!({"maxWriteLen":limit, "auditWrite":true}));
        let connection = fixture.connect();
        completed(&transfer::execute_concurrent(
            &job,
            &AtomicBool::new(false),
            &connection,
            &fixture.state,
            |_| {},
            |_, _| Ok(()),
        ));
        assert_eq!(
            fs::read(fixture.remote_file(&format!("large-write-{limit}.bin"))).unwrap(),
            data
        );
        let lengths: Vec<_> = fs::read_to_string(&audit)
            .unwrap()
            .lines()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line).unwrap()["length"]
                    .as_u64()
                    .unwrap()
            })
            .collect();
        assert_eq!(lengths.iter().sum::<u64>(), data.len() as u64);
        assert_eq!(*lengths.iter().max().unwrap(), limit);
        assert!(lengths.iter().all(|length| *length <= limit));
        fixture.clear_faults();
    }
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_upload_accelerated_single_file_writes_on_eight_connections() {
    let fixture = Fixture::new();
    fixture
        .state
        .config
        .lock()
        .unwrap()
        .transfer_settings
        .concurrency = 4;
    let job = fixture.job("upload", "eight-streams.bin");
    let data = payload().repeat(8);
    fs::write(&job.local, &data).unwrap();
    let audit = fixture.root.join("write-events.jsonl");
    let _ = fs::remove_file(&audit);
    fixture.faults(serde_json::json!({"auditWrite": true, "maxWriteLen": 131072}));
    let connection = fixture.connect();
    let event = transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &connection,
        &fixture.state,
        |_| {},
        |_, _| Ok(()),
    );
    completed(&event);
    assert_eq!(event.bytes, data.len() as u64);
    assert_eq!(
        fs::read(fixture.remote_file("eight-streams.bin")).unwrap(),
        data
    );
    let connections: HashSet<_> = fs::read_to_string(&audit)
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["connection"]
                .as_u64()
                .unwrap()
        })
        .collect();
    assert_eq!(connections.len(), 8, "单文件必须实际通过八条连接写入");
    fixture.clear_faults();
}

#[test]
#[ignore = "Requires loopback fixture"]
fn parallel_upload_handle_preparation_failure_closes_every_open_handle() {
    let fixture = Fixture::new();
    fixture
        .state
        .config
        .lock()
        .unwrap()
        .transfer_settings
        .concurrency = 4;
    let mut job = fixture.job("upload", "prepare-failure.bin");
    job.overwrite = true;
    let data = payload().repeat(3);
    fs::write(&job.local, &data).unwrap();
    let target = fixture.remote_file("prepare-failure.bin");
    fs::write(&target, b"original").unwrap();
    fixture.faults(serde_json::json!({"failStripedHandleStat": true}));
    let connection = fixture.connect();
    let run = || {
        transfer::execute_concurrent(
            &job,
            &AtomicBool::new(false),
            &connection,
            &fixture.state,
            |_| {},
            |_, _| Ok(()),
        )
    };
    assert_eq!(run().status, "failed");
    assert_eq!(fs::read(&target).unwrap(), b"original");
    let records: Vec<serde_json::Value> =
        fs::read_to_string(fixture.root.join("handle-events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    let opens: HashSet<_> = records
        .iter()
        .filter(|record| record["action"] == "open")
        .map(|record| record["connection"].as_u64().unwrap())
        .collect();
    let closes: HashSet<_> = records
        .iter()
        .filter(|record| record["action"] == "close")
        .map(|record| record["connection"].as_u64().unwrap())
        .collect();
    assert_eq!(opens.len(), 4);
    assert_eq!(opens, closes, "准备失败后每条连接的已打开句柄都必须关闭");
    fixture.clear_faults();
    completed(&run());
    assert_eq!(fs::read(target).unwrap(), data);
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_sync_quick_strict_exclusions_and_cancel() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    fs::create_dir_all(fixture.remote_file("sync/ignored")).unwrap();
    fs::create_dir_all(fixture.local.join("ignored")).unwrap();
    fs::write(fixture.remote_file("sync/same-size.txt"), b"remote").unwrap();
    fs::write(fixture.local.join("same-size.txt"), b"locals").unwrap();
    fs::write(fixture.remote_file("sync/ignored/private.txt"), b"excluded").unwrap();
    fs::write(fixture.local.join("ignored/private.txt"), b"excluded").unwrap();
    let timestamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1700000000);
    for path in [
        fixture.remote_file("sync/same-size.txt"),
        fixture.local.join("same-size.txt"),
    ] {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(timestamp))
            .unwrap();
    }
    tauri::async_runtime::block_on(async {
        let remote = remote_async::get(&connection, &fixture.state)
            .await
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let tree = format!("{}/sync", fixture.remote);
        for (quick, expected) in [(true, "same"), (false, "changed")] {
            let rows = directory_sync::compare_with_options(
                &remote,
                &fixture.local,
                Path::new(&tree),
                "upload",
                &cancel,
                crate::directory_sync_options::Options {
                    quick,
                    exclude: vec!["ignored/".into()],
                },
            )
            .await
            .unwrap();
            assert!(rows.iter().all(|row| !row.relative.starts_with("ignored")));
            assert!(rows
                .iter()
                .any(|row| row.relative == "same-size.txt" && row.status == expected));
        }
        cancel.store(true, Ordering::Relaxed);
        assert!(directory_sync::compare_with_options(
            &remote,
            &fixture.local,
            Path::new(&tree),
            "download",
            &cancel,
            Default::default()
        )
        .await
        .is_err());
    });
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_terminal_accepts_window_updates_and_preserves_real_rejections() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    tauri::async_runtime::block_on(async {
        let remote = remote_async::get(&connection, &fixture.state)
            .await
            .unwrap();
        // fixture 在 PTY 和 Shell 成功/失败应答前均发送 WINDOW_ADJUST。
        for (fault, phase) in [("denyPty", "PTY"), ("denyShell", "Shell")] {
            fixture.faults(serde_json::json!({fault: true}));
            let error = remote.native.terminal_channel(80, 24).await.err().unwrap();
            assert_eq!(error.message, format!("服务器拒绝 {phase} 请求"));
            // 拒绝仅清理终端通道，原 SFTP 会话仍可使用。
            remote
                .stat(&fixture.remote, &AtomicBool::new(false))
                .await
                .unwrap();
        }
        fixture.clear_faults();
        let (mut channel, mut output) = remote.native.terminal_channel(80, 24).await.unwrap();
        channel
            .data_bytes(b"terminal-retry\n".to_vec())
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !String::from_utf8_lossy(&output).contains("terminal-retry") {
                match channel.wait().await {
                    Some(russh::ChannelMsg::Data { data }) => output.extend_from_slice(&data),
                    Some(russh::ChannelMsg::WindowAdjusted { .. }) => {}
                    message => panic!("unexpected terminal message: {message:?}"),
                }
            }
        })
        .await
        .unwrap();
        assert!(String::from_utf8_lossy(&output).contains("TermRill fixture shell"));
        channel.close().await.unwrap();
    });
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_upload_commit_failure_preserves_complete_partial_and_retries_without_writes() {
    for permissions in [false, true] {
        let fixture = Fixture::new();
        let connection = fixture.connect();
        let mut job = fixture.job("upload", "commit-failure.bin");
        job.overwrite = true;
        let data = payload().repeat(3);
        fs::write(&job.local, &data).unwrap();
        let target = fixture.remote_file("commit-failure.bin");
        let temporary = fixture.remote_file(&job.temporary_name(0));
        fs::write(&target, b"original").unwrap();
        if permissions {
            tauri::async_runtime::block_on(async {
                let remote = remote_async::get(&connection, &fixture.state)
                    .await
                    .unwrap();
                remote
                    .native
                    .sftp
                    .setstat(
                        job.remote.clone(),
                        russh_sftp::protocol::FileAttributes {
                            permissions: Some(0o755),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
            });
        }
        fixture.faults(if permissions {
            serde_json::json!({"failPermissions":true})
        } else {
            serde_json::json!({"renameDestination":job.remote,"renameSourceContains":format!(".termrill-{}-",job.id)})
        });
        let run = || {
            transfer::execute_concurrent(
                &job,
                &AtomicBool::new(false),
                &connection,
                &fixture.state,
                |_| {},
                |_, _| Ok(()),
            )
        };
        let event = run();
        assert_eq!(event.status, "failed");
        assert_eq!(event.bytes, data.len() as u64);
        assert!(event.error.unwrap().contains(if permissions {
            "保留目标文件权限"
        } else {
            "安装目标文件"
        }));
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert_eq!(fs::read(&temporary).unwrap(), data);
        // 服务器拒绝任何新 WRITE；能成功说明重试核对完整前缀后只做提交。
        fixture.faults(serde_json::json!({"failWriteAfterBytes":0}));
        completed(&run());
        assert_eq!(fs::read(&target).unwrap(), data);
        assert!(!temporary.exists());
    }
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_upload_staging_name_failure_retains_complete_content_for_inspection() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let job = fixture.job("upload", "staging-failure.bin");
    let data = payload().repeat(3);
    fs::write(&job.local, &data).unwrap();
    let stable_remote = format!("{}/{}", fixture.remote, job.temporary_name(0));
    fixture.faults(
        serde_json::json!({"renameDestination":stable_remote,"renameSourceContains":".striped-"}),
    );
    let event = transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &connection,
        &fixture.state,
        |_| {},
        |_, _| Ok(()),
    );
    assert_eq!(event.status, "failed");
    let error = event.error.unwrap();
    assert!(error.contains("恢复续传临时文件名"));
    assert!(error.contains("暂存内容未主动删除"));
    assert!(!fixture.remote_file("staging-failure.bin").exists());
    let staging = fs::read_dir(fixture.remote_file(""))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("{}.striped-", job.temporary_name(0)))
        })
        .unwrap();
    assert_eq!(fs::read(staging).unwrap(), data);
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_upload_backup_cleanup_failure_reports_already_installed_target() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let mut job = fixture.job("upload", "backup-cleanup.bin");
    job.overwrite = true;
    let data = payload().repeat(3);
    fs::write(&job.local, &data).unwrap();
    let target = fixture.remote_file("backup-cleanup.bin");
    fs::write(&target, b"original").unwrap();
    fixture.faults(serde_json::json!({"renameDestination":job.remote,"failBackupDelete":true}));
    let event = transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &connection,
        &fixture.state,
        |_| {},
        |_, _| Ok(()),
    );
    assert_eq!(event.status, "failed");
    assert!(event
        .error
        .unwrap()
        .contains("目标文件已安装，但旧备份清理失败"));
    assert_eq!(fs::read(&target).unwrap(), data);
    let backup = fs::read_dir(fixture.remote_file(""))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".termrill-backup-")
        })
        .unwrap();
    assert_eq!(fs::read(backup).unwrap(), b"original");
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_upload_matching_permissions_does_not_require_chmod() {
    let fixture = Fixture::new();
    let connection = fixture.connect();
    let mut job = fixture.job("upload", "matching-mode.bin");
    job.overwrite = true;
    let data = payload().repeat(3);
    fs::write(&job.local, &data).unwrap();
    let target = fixture.remote_file("matching-mode.bin");
    fs::write(&target, b"original").unwrap();
    fixture.faults(serde_json::json!({"reportedMode":0o644,"failPermissions":true}));
    completed(&transfer::execute_concurrent(
        &job,
        &AtomicBool::new(false),
        &connection,
        &fixture.state,
        |_| {},
        |_, _| Ok(()),
    ));
    assert_eq!(fs::read(target).unwrap(), data);
}

#[test]
#[ignore = "Requires loopback fixture"]
fn native_upload_atomic_failure_fallback_preserves_rollback_and_rejects_changes() {
    // 小文件走共享连接，大文件走专用上传连接，二者必须使用相同提交规则。
    for size in [128 * 1024, 4 * 1024 * 1024] {
        for fault in ["failure", "rollback", "changed", "denied"] {
            let fixture = Fixture::new();
            let connection = fixture.connect();
            let mut job = fixture.job("upload", "atomic-failure.bin");
            job.overwrite = true;
            let data = vec![53u8; size];
            fs::write(&job.local, &data).unwrap();
            let target = fixture.remote_file("atomic-failure.bin");
            fs::write(&target, b"original").unwrap();
            let mut faults = serde_json::json!({"posixRenameFailure":fault});
            if fault == "rollback" {
                faults["renameDestination"] = job.remote.clone().into();
                faults["renameSourceContains"] = job.temporary_name(0).into();
            }
            fixture.faults(faults);
            let event = transfer::execute_concurrent(
                &job,
                &AtomicBool::new(false),
                &connection,
                &fixture.state,
                |_| {},
                |_, _| Ok(()),
            );
            if fault == "failure" {
                completed(&event);
                assert_eq!(fs::read(&target).unwrap(), data);
            } else {
                assert_eq!(event.status, "failed", "{fault}");
                let error = event.error.unwrap();
                assert!(
                    error.contains(match fault {
                        "rollback" => "安装目标文件",
                        "changed" => "文件状态已变化",
                        _ => "原子替换目标文件",
                    }),
                    "{error}"
                );
                let expected: &[u8] = if fault == "changed" {
                    b"changed by another writer"
                } else {
                    b"original"
                };
                assert_eq!(fs::read(&target).unwrap(), expected);
                assert_eq!(
                    fs::read(fixture.remote_file(&job.temporary_name(0))).unwrap(),
                    data
                );
            }
            assert!(!fs::read_dir(fixture.remote_file(""))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".termrill-backup-")));
        }
    }
}
