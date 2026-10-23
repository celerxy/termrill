use super::*;
use std::io::{Read, Write};

pub(crate) fn backend(root: &Path) -> Backend {
    Backend {
        auth_app: None,
        auth_answers: Mutex::new(BTreeMap::new()),
        config_path: root.join("config.json"),
        config: Mutex::new(Config::default()),
        connections: Mutex::new(BTreeMap::new()),
        closed: Mutex::new(BTreeMap::new()),
        transfers: Mutex::new(BTreeMap::new()),
        pauses: Mutex::new(BTreeMap::new()),
        jobs: Mutex::new(BTreeMap::new()),
        connection_profiles: Mutex::new(BTreeMap::new()),
        terminals: Mutex::new(BTreeMap::new()),
        pending_hosts: Mutex::new(BTreeMap::new()),
    }
}
fn endpoint(port: u16) -> Endpoint {
    Endpoint {
        host: "127.0.0.1".into(),
        port,
        username: "termrill-test".into(),
        auth: "password".into(),
        key_path: String::new(),
    }
}
pub(crate) fn request(port: u16) -> ConnectRequest {
    ConnectRequest {
        profile: Profile {
            id: "fixture".into(),
            name: "fixture".into(),
            target: endpoint(port),
            jump: None,
        },
        target_secrets: Secrets {
            password: "fixture-only-password".into(),
            passphrase: String::new(),
        },
        jump_secrets: Secrets {
            password: "fixture-only-password".into(),
            passphrase: String::new(),
        },
    }
}

#[test]
fn restored_transfers_are_scoped_to_original_profile_and_endpoints() {
    let state = backend(Path::new("."));
    let mut first = request(2222).profile;
    first.id = "server-a".into();
    let mut second = request(2223).profile;
    second.id = "server-b".into();
    for (id, profile) in [("task-a", &first), ("task-b", &second)] {
        lock(&state.jobs).unwrap().insert(
            id.into(),
            (
                transfer::Job {
                    id: id.into(),
                    connection_id: format!("old-{id}"),
                    local: "file.bin".into(),
                    remote: "/file.bin".into(),
                    direction: "upload".into(),
                    overwrite: false,
                    completed: BTreeMap::new(),
                    selection: None,
                },
                profile.clone(),
            ),
        );
    }
    let restored = pending_transfers_for_profile(&state, &first).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].id, "task-a");
    assert_eq!(
        pending_transfers_for_profile(&state, &second).unwrap()[0].id,
        "task-b"
    );
    first.name = "renamed".into();
    assert_eq!(
        pending_transfers_for_profile(&state, &first).unwrap().len(),
        1
    );
    let mut changed = first.clone();
    changed.target.host = "other-server".into();
    assert!(pending_transfers_for_profile(&state, &changed)
        .unwrap()
        .is_empty());
    changed = first.clone();
    changed.target.username = "another-user".into();
    assert!(pending_transfers_for_profile(&state, &changed)
        .unwrap()
        .is_empty());
    changed = first.clone();
    changed.jump = Some(first.target.clone());
    assert!(pending_transfers_for_profile(&state, &changed)
        .unwrap()
        .is_empty());
    // 重连后的活动记录仍归原配置，不能因临时连接 ID 改变而丢失。
    lock(&state.jobs)
        .unwrap()
        .get_mut("task-a")
        .unwrap()
        .0
        .connection_id = "new-connection".into();
    lock(&state.transfers)
        .unwrap()
        .insert("task-a".into(), Arc::new(AtomicBool::new(false)));
    let restored = pending_transfers_for_profile(&state, &first).unwrap();
    assert_eq!(restored[0].connection_id, "new-connection");
    assert_eq!(restored[0].status, "running");
    assert_eq!(lock(&state.jobs).unwrap().len(), 2);
}

#[test]
fn persisted_profile_excludes_credentials() {
    let mut data = serde_json::to_value(request(22).profile).unwrap();
    data["target"]["password"] = "should-not-persist".into();
    let sanitized: Profile = serde_json::from_value(data).unwrap();
    let encoded = serde_json::to_string(&sanitized).unwrap();
    assert!(!encoded.contains("password\":\"should-not-persist"));
    assert!(!encoded.contains("should-not-persist"));
}
#[test]
fn profile_validation_rejects_missing_authentication_details() {
    let mut e = endpoint(22);
    e.port = 0;
    assert!(e.validate().is_err());
    e.port = 22;
    e.auth = "key".into();
    assert!(e.validate().is_err());
    e.key_path = "test-key".into();
    assert!(e.validate().is_ok());
}

pub(crate) fn verified(request: ConnectRequest, state: &Backend) -> ssh::Connection {
    for _ in 0..3 {
        match ssh::connect(request.clone(), state) {
            Ok(c) => return c,
            Err(e) => {
                if let Some(key) = e.host_key {
                    state
                        .config
                        .lock()
                        .unwrap()
                        .known_hosts
                        .insert(key.address, key.fingerprint);
                } else {
                    panic!("SSH connect failed: {}", e.message);
                }
            }
        }
    }
    panic!("host trust did not converge");
}

#[test]
#[ignore = "Requires tests/ssh_fixture.py and TERMRILL_TEST_FIXTURE ready.json"]
fn loopback_ssh_sftp_jump_and_transfer() {
    let fixture: serde_json::Value = serde_json::from_slice(
        &fs::read(std::env::var("TERMRILL_TEST_FIXTURE").expect("set TERMRILL_TEST_FIXTURE"))
            .unwrap(),
    )
    .unwrap();
    let root = PathBuf::from(fixture["root"].as_str().unwrap());
    let state = backend(&root);
    let port = fixture["targetPort"].as_u64().unwrap() as u16;
    let direct = request(port);
    let error = match ssh::connect(direct.clone(), &state) {
        Ok(_) => panic!("unknown host was accepted"),
        Err(e) => e,
    };
    let key = error
        .host_key
        .unwrap_or_else(|| panic!("unknown host challenge missing: {}", error.message));
    assert!(key.previous.is_none());
    assert!(key.fingerprint.starts_with("SHA256:"));
    state
        .config
        .lock()
        .unwrap()
        .known_hosts
        .insert(key.address.clone(), "SHA256:changed".into());
    let error = match ssh::connect(direct.clone(), &state) {
        Ok(_) => panic!("changed host was accepted"),
        Err(e) => e,
    };
    assert_eq!(
        error.host_key.unwrap().previous.as_deref(),
        Some("SHA256:changed")
    );
    let conn = verified(direct.clone(), &state);
    let listing = ssh::listing(&conn.session, "/").unwrap();
    assert!(listing.entries.iter().any(|f| f.name == "welcome.txt"));
    let mut wrong = direct.clone();
    wrong.target_secrets.password = "wrong-password".into();
    assert!(ssh::connect(wrong, &state).is_err());
    let mut private_key = direct.clone();
    private_key.profile.target.auth = "key".into();
    private_key.profile.target.key_path = fixture["keyPath"].as_str().unwrap().into();
    let key_conn = verified(private_key, &state);
    assert!(key_conn.session.authenticated());
    drop(key_conn);
    let mut jumped = direct.clone();
    jumped.profile.jump = Some(endpoint(fixture["jumpPort"].as_u64().unwrap() as u16));
    let jump_conn = verified(jumped, &state);
    assert!(!ssh::listing(&jump_conn.session, "/")
        .unwrap()
        .entries
        .is_empty());
    drop(jump_conn);
    let conn = Mutex::new(conn);
    let source = root.join("upload-source");
    fs::create_dir_all(source.join("nested")).unwrap();
    let data = vec![0x59u8; 262144];
    fs::write(source.join("nested/中文.bin"), &data).unwrap();
    fs::write(source.join("empty.txt"), []).unwrap();
    let mut job = transfer::Job {
        completed: BTreeMap::new(),
        selection: None,
        id: "upload".into(),
        connection_id: "fixture".into(),
        local: source.display().to_string(),
        remote: "/uploaded".into(),
        direction: "upload".into(),
        overwrite: false,
    };
    let cancel = AtomicBool::new(false);
    let event = transfer::execute(&job, &cancel, &conn, |_| {});
    assert_eq!(event.status, "completed", "{:?}", event.error);
    assert_eq!(event.bytes, data.len() as u64);
    assert_eq!(
        fs::read(root.join("remote/uploaded/nested/中文.bin")).unwrap(),
        data
    );
    fs::write(source.join("empty.txt"), b"changed").unwrap();
    let conflict = transfer::execute(&job, &cancel, &conn, |_| {});
    assert_eq!(conflict.status, "failed");
    assert_eq!(
        fs::read(root.join("remote/uploaded/empty.txt")).unwrap(),
        b""
    );
    job.overwrite = true;
    let event = transfer::execute(&job, &cancel, &conn, |_| {});
    assert_eq!(event.status, "completed", "{:?}", event.error);
    assert_eq!(
        fs::read(root.join("remote/uploaded/empty.txt")).unwrap(),
        b"changed"
    );
    job.id = "download".into();
    job.direction = "download".into();
    job.local = root.join("downloaded").display().to_string();
    job.overwrite = false;
    let event = transfer::execute(&job, &cancel, &conn, |_| {});
    assert_eq!(event.status, "completed", "{:?}", event.error);
    assert_eq!(
        fs::read(root.join("downloaded/nested/中文.bin")).unwrap(),
        data
    );
    fs::write(root.join("downloaded/empty.txt"), b"local-original").unwrap();
    let event = transfer::execute(&job, &cancel, &conn, |_| {});
    assert_eq!(event.status, "failed");
    assert_eq!(
        fs::read(root.join("downloaded/empty.txt")).unwrap(),
        b"local-original"
    );
    job.overwrite = true;
    let event = transfer::execute(&job, &cancel, &conn, |_| {});
    assert_eq!(event.status, "completed", "{:?}", event.error);
    job.direction = "upload".into();
    job.local = source.display().to_string();
    job.remote = "/cancelled".into();
    cancel.store(true, Ordering::Relaxed);
    let event = transfer::execute(&job, &cancel, &conn, |_| {});
    assert_eq!(event.status, "cancelled");
    assert!(!root.join("remote/cancelled").exists());
    cancel.store(false, Ordering::Relaxed);
    job.remote = "/cancel-running".into();
    let event = transfer::execute(&job, &cancel, &conn, |e| {
        if e.status == "running" {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert_eq!(event.status, "cancelled");
    assert!(!root.join("remote/cancel-running").exists());
    cancel.store(false, Ordering::Relaxed);
    let large = root.join("large.bin");
    fs::write(&large, vec![7u8; 8 * 1024 * 1024]).unwrap();
    job.local = large.display().to_string();
    job.remote = "/cancel-partial.bin".into();
    let event = transfer::execute(&job, &cancel, &conn, |e| {
        if e.bytes > 0 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert_eq!(event.status, "cancelled");
    assert!(event.bytes > 0);
    assert!(event.bytes < 8 * 1024 * 1024);
    assert!(!root.join("remote/cancel-partial.bin").exists());
    assert!(fs::read_dir(root.join("remote")).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".part")));
    let guard = conn.lock().unwrap();
    let mut channel = guard.session.channel_session().unwrap();
    channel
        .request_pty("xterm", None, Some((80, 24, 0, 0)))
        .unwrap();
    channel.shell().unwrap();
    let mut buffer = [0; 128];
    let n = channel.read(&mut buffer).unwrap();
    assert!(String::from_utf8_lossy(&buffer[..n]).contains("fixture shell"));
    channel.write_all(b"echo-test\n").unwrap();
    let n = channel.read(&mut buffer).unwrap();
    assert_eq!(&buffer[..n], b"echo-test\n");
    channel.request_pty_size(100, 30, None, None).unwrap();
    channel.close().unwrap();
}

#[test]
fn shared_rate_budget_and_cancellation() {
    use crate::transfer_rate::RateLimiter;
    use std::{
        sync::atomic::AtomicBool,
        time::{Duration, Instant},
    };
    let limiter = RateLimiter::new(100_000);
    let cancel = AtomicBool::new(false);
    let closed = AtomicBool::new(false);
    let pause = AtomicBool::new(false);
    let start = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                limiter
                    .wait(10_000, &cancel, &closed, &pause, || {})
                    .unwrap()
            });
        }
    });
    assert!(start.elapsed() >= Duration::from_millis(180));
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(limiter
        .wait(10_000, &cancel, &closed, &pause, || {})
        .is_err());
    assert_eq!(RateLimiter::new(0).block_size(65536), 65536);
}

#[test]
fn settings_defaults_and_limits() {
    let mut legacy = serde_json::to_value(Config::default()).unwrap();
    legacy.as_object_mut().unwrap().remove("transferSettings");
    let config: Config = serde_json::from_value(legacy).unwrap();
    assert_eq!(config.transfer_settings.concurrency, 3);
    let mut settings = config.transfer_settings;
    settings.concurrency = 5;
    assert!(settings.validate().is_err());
    settings.concurrency = 1;
    assert!(settings.validate().is_ok());
}

#[test]
fn journal_recovers_complete_records_and_compacts_snapshot() {
    let root = std::env::temp_dir().join(format!("termrill-journal-{}", identifier()));
    fs::create_dir_all(&root).unwrap();
    let state = backend(&root);
    let job = transfer::Job {
        id: identifier(),
        connection_id: "fixture".into(),
        local: root.display().to_string(),
        remote: "/test".into(),
        direction: "upload".into(),
        overwrite: false,
        completed: BTreeMap::new(),
        selection: None,
    };
    state
        .jobs
        .lock()
        .unwrap()
        .insert(job.id.clone(), (job.clone(), request(22).profile));
    transfer::persist_jobs(&state).unwrap();
    transfer_journal::checkpoint(&state, &job.id, "/test/a", "digest-a".into()).unwrap();
    let journal = root
        .join("transfer-progress")
        .join(format!("{}.jsonl", job.id));
    fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap()
        .write_all(b"{unfinished")
        .unwrap();
    let snapshot = root.join("transfers.json");
    let recovered = transfer_journal::load(&snapshot).unwrap();
    assert_eq!(recovered[&job.id].0.completed["/test/a"], "digest-a");
    transfer_journal::checkpoint(&state, &job.id, "/test/a", "updated-digest".into()).unwrap();
    assert_eq!(
        transfer_journal::load(&snapshot).unwrap()[&job.id]
            .0
            .completed["/test/a"],
        "updated-digest"
    );
    transfer_journal::checkpoint(&state, &job.id, "/test/b", "digest-b".into()).unwrap();
    transfer::persist_jobs(&state).unwrap();
    assert!(!journal.exists());
    assert_eq!(
        transfer_journal::load(&snapshot).unwrap()[&job.id]
            .0
            .completed
            .len(),
        2
    );
    fs::remove_dir_all(&root).unwrap();
}
