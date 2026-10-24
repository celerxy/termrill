//! 显式选择的回环性能入口；复用生产下载、指纹校验及安装流程，不进入应用发行二进制。
use super::*;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::{cell::RefCell, io::Write, time::Instant};

#[test]
#[ignore = "Opt-in loopback benchmark via scripts/benchmark-transfer.py"]
fn loopback_download_benchmark() {
    // 普通故障回归包含 ignored 测试，但不得因此自动执行性能采样。
    let Ok(case) = std::env::var("TERMRILL_BENCHMARK_CASE") else {
        return;
    };
    let case: u32 = case.parse().expect("numeric benchmark case");
    assert!(case < 100);
    let concurrency: usize = std::env::var("TERMRILL_BENCHMARK_CONCURRENCY")
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=4).contains(&concurrency));
    let fixture = Fixture::new();
    let ready: serde_json::Value =
        serde_json::from_slice(&fs::read(std::env::var("TERMRILL_TEST_FIXTURE").unwrap()).unwrap())
            .unwrap();
    let host_key = ready["targetHostKey"].as_str().unwrap();
    let (_, encoded) = host_key.split_once(' ').unwrap();
    let public_key = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    let fingerprint = format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(public_key))
    );
    {
        let mut config = fixture.state.config.lock().unwrap();
        config.transfer_settings.concurrency = concurrency;
        config
            .known_hosts
            .insert(format!("[127.0.0.1]:{}", fixture.port), fingerprint);
    }
    let mut request = request(fixture.port);
    request.profile.target.auth = "key".into();
    request.profile.target.key_path = ready["keyPath"].as_str().unwrap().into();
    request.target_secrets.password.clear();
    let started = Instant::now();
    // 预先绑定本启动器的公钥，不使用测试辅助函数的首次见到即信任逻辑。
    let connection = Mutex::new(
        tauri::async_runtime::block_on(ssh::connect_benchmark_native(request, &fixture.state))
            .unwrap(),
    );
    let connect_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut job = fixture.job("download", "benchmark.bin");
    job.remote = "/benchmark.bin".into();
    job.local = fixture
        .root
        .join(format!("termrill-{case}.bin"))
        .display()
        .to_string();
    assert!(
        !Path::new(&job.local).exists(),
        "never overwrite a previous sample"
    );
    let progress = RefCell::new((0_u64, Instant::now(), 0.0_f64));
    let transfer_started = Instant::now();
    let event = transfer::execute_native_download(
        &job,
        &connection,
        &fixture.state,
        &AtomicBool::new(false),
        &AtomicBool::new(false),
        &|event| {
            let mut sample = progress.borrow_mut();
            if event.bytes > sample.0 {
                sample.2 = sample.2.max(sample.1.elapsed().as_secs_f64() * 1000.0);
                sample.1 = Instant::now();
                sample.0 = event.bytes;
            }
        },
        &|_, _| Ok(()),
    );
    assert_eq!(event.status, "completed", "benchmark transfer failed");
    let result = serde_json::json!({
        "connectMs": connect_ms,
        "transferMs": transfer_started.elapsed().as_secs_f64() * 1000.0,
        "maxProgressGapMs": progress.borrow().2,
        "bytes": event.bytes,
    });
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(fixture.root.join(format!("termrill-{case}.json")))
        .unwrap();
    output
        .write_all(serde_json::to_string(&result).unwrap().as_bytes())
        .unwrap();
}
