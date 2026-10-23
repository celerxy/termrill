fn main() {
    tauri_build::build();
    println!("cargo:rerun-if-env-changed=TERMRILL_TEST_MANIFEST");
    // Tauri 只给应用二进制嵌入资源；Windows 单元测试也链接 rfd 的 TaskDialog。
    // 测试 runner 明确启用 Common Controls v6，避免测试在入口点之前退出。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("TERMRILL_TEST_MANIFEST").as_deref() == Ok("1")
    {
        let manifest = std::path::Path::new("tests-manifest.xml")
            .canonicalize()
            .expect("test manifest exists");
        println!("cargo:rerun-if-changed=tests-manifest.xml");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
}
