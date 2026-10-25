//! 显式进程验收入口：父启动器强制结束本测试进程，重新建进程加载生产任务清单并恢复。
//! 只使用本机 fixture；普通回归未设置阶段变量时不执行进程验收。
use crate::{model::*, tests::backend, *};
use base64::Engine;
use sha2::{Digest, Sha256};

const JOB_ID: &str = "restart-acceptance";

fn prepare(
    root: &Path,
    ready: &serde_json::Value,
    request: &ConnectRequest,
    state: &mut Backend,
) -> transfer::Job {
    fs::create_dir(root).unwrap();
    let key = ready["targetHostKey"]
        .as_str()
        .unwrap()
        .split_once(' ')
        .unwrap()
        .1;
    let public = base64::engine::general_purpose::STANDARD
        .decode(key)
        .unwrap();
    let fingerprint = format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(public))
    );
    let mut config = Config::default();
    config.profiles.push(request.profile.clone());
    config.transfer_settings.concurrency = 1;
    config.known_hosts.insert(
        format!("[127.0.0.1]:{}", request.profile.target.port),
        fingerprint,
    );
    fs::write(
        root.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    state.config = Mutex::new(config);
    let job = transfer::Job {
        id: JOB_ID.into(),
        connection_id: "previous-process".into(),
        local: root.join("download.bin").display().to_string(),
        remote: "/acceptance.bin".into(),
        direction: "download".into(),
        overwrite: true,
        completed: Default::default(),
        selection: None,
    };
    fs::write(&job.local, b"original fixture destination").unwrap();
    state
        .jobs
        .lock()
        .unwrap()
        .insert(JOB_ID.into(), (job.clone(), request.profile.clone()));
    transfer::persist_jobs(state).unwrap();
    job
}

fn restore(root: &Path, request: &ConnectRequest, state: &mut Backend) -> transfer::Job {
    let config: Config =
        serde_json::from_slice(&fs::read(root.join("config.json")).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_vec(&config.profiles[0]).unwrap(),
        serde_json::to_vec(&request.profile).unwrap()
    );
    state.config = Mutex::new(config);
    state.jobs = Mutex::new(transfer_journal::load(&root.join("transfers.json")).unwrap());
    // 启动恢复不能伪造认证会话；认证前仅返回可恢复的失败任务。
    assert!(state.connections.lock().unwrap().is_empty());
    let pending = pending_transfers_for_profile(state, &request.profile).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, "failed");
    state.jobs.lock().unwrap()[JOB_ID].0.clone()
}

fn discard(root: &Path, job: &transfer::Job, state: &Backend) {
    tauri::async_runtime::block_on(discard_transfer_async(JOB_ID, None, state)).unwrap();
    assert_eq!(
        fs::read(&job.local).unwrap(),
        b"original fixture destination"
    );
    assert!(!root.join(job.temporary_name(0)).exists());
    assert!(transfer_journal::load(&root.join("transfers.json"))
        .unwrap()
        .is_empty());
    fs::write(
        root.join("result.json"),
        b"{\"discarded\":true,\"originalPreserved\":true}",
    )
    .unwrap();
}

#[test]
#[ignore = "Opt-in process recovery via tests/process_recovery.py"]
fn process_recovery_phase() {
    let Ok(phase) = std::env::var("TERMRILL_RESTART_PHASE") else {
        return;
    };
    let case = std::env::var("TERMRILL_RESTART_CASE").unwrap();
    assert!([
        "kill-resume",
        "kill-discard",
        "changed-source",
        "disconnect"
    ]
    .contains(&case.as_str()));
    let ready: serde_json::Value =
        serde_json::from_slice(&fs::read(std::env::var("TERMRILL_TEST_FIXTURE").unwrap()).unwrap())
            .unwrap();
    let runtime = PathBuf::from(ready["root"].as_str().unwrap());
    let root = runtime.join(&case);
    let mut state = backend(&root);
    let mut request = crate::tests::request(ready["targetPort"].as_u64().unwrap() as u16);
    request.profile.target.auth = "key".into();
    request.profile.target.key_path = ready["keyPath"].as_str().unwrap().into();
    request.target_secrets.password.clear();
    let job = if phase == "prepare" {
        prepare(&root, &ready, &request, &mut state)
    } else {
        assert!(["resume", "discard", "reject-changed"].contains(&phase.as_str()));
        restore(&root, &request, &mut state)
    };
    if phase == "discard" {
        discard(&root, &job, &state);
        return;
    }
    let connection = Mutex::new(
        tauri::async_runtime::block_on(ssh::connect_benchmark_native(request, &state)).unwrap(),
    );
    let closed = connection.lock().unwrap().closed.clone();
    let announced = AtomicBool::new(false);
    let event = transfer::execute_native_download(
        &job,
        &connection,
        &state,
        &AtomicBool::new(false),
        &AtomicBool::new(false),
        &|event| {
            if phase == "prepare"
                && event.status == "running"
                && event.bytes >= 512 * 1024
                && !announced.swap(true, Ordering::Relaxed)
            {
                fs::write(root.join("interrupted.ready"), b"ready").unwrap();
                if case == "disconnect" {
                    closed.store(true, Ordering::Relaxed);
                } else {
                    // 父进程在到达实际正文写入后结束本进程；不走协程错误清理或析构。
                    loop {
                        std::thread::park_timeout(std::time::Duration::from_secs(1));
                    }
                }
            }
        },
        &|path, digest| transfer_journal::checkpoint(&state, JOB_ID, path, digest),
    );
    if phase == "prepare" {
        assert_eq!(case, "disconnect");
        assert_eq!(event.status, "failed");
        assert_eq!(
            fs::read(&job.local).unwrap(),
            b"original fixture destination"
        );
        fs::write(root.join("disconnected.ready"), b"ready").unwrap();
        return;
    }
    if phase == "reject-changed" {
        assert_eq!(event.status, "failed");
        assert!(event
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("续传前缀"));
        assert_eq!(
            fs::read(&job.local).unwrap(),
            b"original fixture destination"
        );
        assert!(root.join(job.temporary_name(0)).exists());
        assert!(state.jobs.lock().unwrap().contains_key(JOB_ID));
        fs::write(
            root.join("result.json"),
            b"{\"changedSourceRejected\":true,\"originalPreserved\":true}",
        )
        .unwrap();
        return;
    }
    assert_eq!(event.status, "completed");
    assert!(!root.join(job.temporary_name(0)).exists());
    state.jobs.lock().unwrap().remove(JOB_ID);
    transfer::persist_jobs(&state).unwrap();
    assert!(transfer_journal::load(&root.join("transfers.json"))
        .unwrap()
        .is_empty());
    let result = serde_json::json!({"bytes": event.bytes, "resumed": true, "pendingTasks": 0});
    fs::write(
        root.join("result.json"),
        serde_json::to_vec(&result).unwrap(),
    )
    .unwrap();
}
