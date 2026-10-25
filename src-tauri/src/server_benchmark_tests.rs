//! 显式授权服务器测速入口，仅测试构建启用；启动器独占随机测试目录和本地输出。
use crate::{model::*, tests::backend, *};
use std::time::Instant;

#[test]
#[ignore = "Opt-in authorized server benchmark"]
fn authorized_download() {
    let Ok(path) = std::env::var("TERMRILL_SERVER_BENCHMARK_MANIFEST") else {
        return;
    };
    let manifest: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let profile: Profile = serde_json::from_value(manifest["profile"].clone()).unwrap();
    assert!(profile.jump.is_none());
    assert_eq!(profile.target.auth, "key");
    let remote = manifest["remote"].as_str().unwrap();
    let parts: Vec<_> = remote.split('/').filter(|part| !part.is_empty()).collect();
    assert!(remote.starts_with('/') && parts.len() >= 2);
    assert_eq!(parts.last().copied(), Some("payload.bin"));
    let owner = parts[parts.len() - 2];
    assert!(owner.starts_with(".termrill-speed-"));
    assert_eq!(owner.len(), ".termrill-speed-".len() + 32);
    assert!(owner[".termrill-speed-".len()..]
        .bytes()
        .all(|value| value.is_ascii_hexdigit()));
    assert!(!parts.iter().any(|part| *part == ".." || *part == "."));
    let root = PathBuf::from(manifest["runtime"].as_str().unwrap());
    let allowed = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/.runtime-server-benchmark");
    assert_eq!(
        root.parent().unwrap().canonicalize().unwrap(),
        allowed.canonicalize().unwrap()
    );
    assert_eq!(
        root.file_name().unwrap().to_str().unwrap(),
        format!("run-{}", &owner[".termrill-speed-".len()..])
    );
    let sample = manifest["sample"].as_u64().unwrap();
    assert!(sample < 8);
    let state = backend(&root);
    let expected = manifest["fingerprint"].as_str().unwrap().to_owned();
    state.config.lock().unwrap().known_hosts.insert(
        format!("[{}]:{}", profile.target.host, profile.target.port),
        expected,
    );
    let concurrency = manifest["concurrency"].as_u64().unwrap_or(3) as usize;
    assert!((1..=4).contains(&concurrency));
    state.config.lock().unwrap().transfer_settings.concurrency = concurrency;
    crate::performance::initialize(root.join(format!("performance-{sample}"))).unwrap();
    let request = ConnectRequest {
        profile,
        target_secrets: Secrets::default(),
        jump_secrets: Secrets::default(),
    };
    let started = Instant::now();
    let connection = Mutex::new(
        tauri::async_runtime::block_on(ssh::connect_benchmark_native(request, &state)).unwrap(),
    );
    let connect_ms = started.elapsed().as_secs_f64() * 1000.0;
    let local = root.join(format!("native-{sample}.bin"));
    assert!(!local.exists());
    let job = transfer::Job {
        id: format!("server-benchmark-{sample}"),
        connection_id: "benchmark".into(),
        local: local.display().to_string(),
        remote: remote.into(),
        direction: "download".into(),
        overwrite: false,
        completed: Default::default(),
        selection: None,
    };
    let transfer_started = Instant::now();
    let event = transfer::execute_native_download(
        &job,
        &connection,
        &state,
        &AtomicBool::new(false),
        &AtomicBool::new(false),
        &|_| {},
        &|_, _| Ok(()),
    );
    assert_eq!(
        event.status, "completed",
        "Authorized benchmark download failed"
    );
    assert_eq!(event.bytes, 8 * 1024 * 1024);
    let result = serde_json::json!({"connectMs":connect_ms,"transferMs":transfer_started.elapsed().as_secs_f64()*1000.0,"bytes":event.bytes});
    fs::write(
        root.join(format!("native-{sample}.json")),
        serde_json::to_vec(&result).unwrap(),
    )
    .unwrap();
}
