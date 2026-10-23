//! 真实编码往返、加密、重建、冲突与取消保护；仅操作自建临时目录。
use super::*;
use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "termrill-archive-write-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("data/空目录")).unwrap();
        fs::write(root.join("data/中文.txt"), "hello 归档\n").unwrap();
        fs::write(root.join("data/empty.txt"), b"").unwrap();
        fs::write(root.join("new.txt"), b"new payload").unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn request(operation: &str, path: &str) -> Request {
    serde_json::from_value(serde_json::json!({"operation": operation, "path": path})).unwrap()
}
fn with_io(action: impl FnOnce(&ArchiveIo<'_>, &AtomicBool)) {
    let cancel = AtomicBool::new(false);
    let clock = RefCell::new(None);
    let io = ArchiveIo {
        native: None,
        cancel: &cancel,
        closed: None,
        progress: tauri::ipc::Channel::new(|_| Ok(())),
        last_progress: &clock,
    };
    action(&io, &cancel);
}
#[test]
fn archive_formats_create_preview_extract_add_remove_and_conflict() {
    let fixture = Fixture::new();
    with_io(|io, _| {
        for extension in [
            "zip", "7z", "tar", "tar.gz", "tar.bz2", "tar.xz", "tgz", "tbz2", "txz",
        ] {
            let path = fixture.path(&format!("archive.{extension}"));
            let mut pack = request("pack", &path);
            pack.sources = vec![fixture.path("data")];
            pack.compression = Some("fast".into());
            run(pack.clone(), io, io, "local-test").unwrap();
            let original = fs::read(&path).unwrap();
            if extension != "zip" {
                assert_ne!(&original[..2], b"PK", "不可把 ZIP 字节写成其他后缀");
            }
            assert!(run(pack, io, io, "local-test").is_err());
            assert_eq!(fs::read(&path).unwrap(), original);
            let listed = run(request("list", &path), io, io, "local-test").unwrap();
            assert!(listed
                .entries
                .iter()
                .any(|entry| entry.name == "data/空目录/"));
            let mut preview = request("preview", &path);
            preview.selected = vec!["data/中文.txt".into()];
            preview.revision = Some(listed.revision.clone());
            let reply = run(preview, io, io, "local-test").unwrap();
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(reply.bytes.unwrap())
                    .unwrap(),
                "hello 归档\n".as_bytes()
            );
            run(request("verify", &path), io, io, "local-test").unwrap();
            let mut extract = request("extract", &path);
            let destination = fixture.path(&format!("out-{extension}"));
            extract.target = Some(destination.clone());
            run(extract, io, io, "local-test").unwrap();
            assert_eq!(
                fs::read(Path::new(&destination).join("data/中文.txt")).unwrap(),
                "hello 归档\n".as_bytes()
            );
            assert!(Path::new(&destination).join("data/空目录").is_dir());
            let mut add = request("add", &path);
            add.sources = vec![fixture.path("new.txt")];
            add.revision = Some(listed.revision);
            let added = run(add, io, io, "local-test").unwrap();
            let mut remove = request("remove", &path);
            remove.selected = vec!["data".into()];
            remove.revision = Some(added.revision);
            run(remove, io, io, "local-test").unwrap();
            let final_list = run(request("list", &path), io, io, "local-test").unwrap();
            assert_eq!(final_list.entries.len(), 1);
            assert_eq!(final_list.entries[0].name, "new.txt");
        }
    });
}
#[test]
fn archive_password_create_rebuild_wrong_password_preserves_source() {
    let fixture = Fixture::new();
    with_io(|io, _| {
        for extension in ["zip", "7z"] {
            let path = fixture.path(&format!("secret.{extension}"));
            let mut pack = request("pack", &path);
            pack.sources = vec![fixture.path("data")];
            pack.password = Some("fixture-归档-password".into());
            run(pack, io, io, "secret-test").unwrap();
            let original = fs::read(&path).unwrap();
            let mut listing = request("list", &path);
            listing.password = Some("fixture-归档-password".into());
            let listed = run(listing, io, io, "secret-test").unwrap();
            let mut add = request("add", &path);
            add.sources = vec![fixture.path("new.txt")];
            add.password = Some("wrong-password".into());
            add.revision = Some(listed.revision.clone());
            assert!(run(add.clone(), io, io, "secret-test").is_err());
            assert_eq!(fs::read(&path).unwrap(), original);
            add.password = Some("fixture-归档-password".into());
            run(add, io, io, "secret-test").unwrap();
            assert!(run(request("verify", &path), io, io, "secret-test").is_err());
            let mut verify = request("verify", &path);
            verify.password = Some("fixture-归档-password".into());
            run(verify, io, io, "secret-test").unwrap();
        }
    });
}
#[test]
fn archive_rejects_read_only_formats_password_mismatch_stale_revision_and_cancel() {
    let fixture = Fixture::new();
    with_io(|io, cancel| {
        for extension in ["rar", "gz", "bz2", "xz"] {
            let path = fixture.path(&format!("unsupported.{extension}"));
            let mut pack = request("pack", &path);
            pack.sources = vec![fixture.path("data")];
            assert!(run(pack, io, io, "test").is_err());
            assert!(!Path::new(&path).exists());
        }
        let path = fixture.path("plain.tar.gz");
        let mut pack = request("pack", &path);
        pack.sources = vec![fixture.path("data")];
        pack.password = Some("not-supported".into());
        assert!(run(pack.clone(), io, io, "test").is_err());
        assert!(!Path::new(&path).exists());
        pack.password = None;
        run(pack.clone(), io, io, "test").unwrap();
        let before = fs::read(&path).unwrap();
        let mut remove = request("remove", &path);
        remove.selected = vec!["data".into()];
        remove.revision = Some("stale".into());
        assert!(run(remove, io, io, "test").is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        cancel.store(true, Ordering::Relaxed);
        pack.overwrite = true;
        assert!(run(pack, io, io, "test").is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    });
}
#[test]
fn archive_encoder_rejects_escape_and_size_declarations() {
    with_io(|io, _| {
        for name in ["../escape.txt", "CON.txt", "folder/../../bad"] {
            let mut writer = ZipWriter::new(ArchiveBuffer::default());
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut writer, b"bad").unwrap();
            assert!(encode::encode(
                &request("pack", "test.tar"),
                writer.finish().unwrap().into_inner(),
                io
            )
            .is_err());
        }
        assert!(validate(&[Entry {
            name: "too-large.bin".into(),
            size: 257 * 1024 * 1024,
            compressed: 1,
            directory: false,
            encrypted: false,
            safe: true
        }])
        .is_err());
    });
}
