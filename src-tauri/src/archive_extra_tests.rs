//! 自建归档夹具验证真实解码、密码、校验、路径及取消行为，不依赖安装外部解压工具。
use super::*;
use std::{cell::RefCell, sync::atomic::AtomicBool};

fn with_io<T>(cancelled: bool, action: impl FnOnce(&ArchiveIo<'_>) -> T) -> T {
    let cancel = AtomicBool::new(cancelled);
    let last_progress = RefCell::new(None);
    let io = ArchiveIo {
        native: None,
        cancel: &cancel,
        closed: None,
        progress: tauri::ipc::Channel::new(|_| Ok(())),
        last_progress: &last_progress,
    };
    action(&io)
}

fn seven_fixture(name: &str, encrypted: bool, header: bool) -> Vec<u8> {
    use sevenz_rust2::{
        encoder_options::{AesEncoderOptions, Lzma2Options},
        ArchiveEntry, ArchiveWriter, Password,
    };
    let mut writer = ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    if encrypted {
        writer.set_content_methods(vec![
            AesEncoderOptions::new(Password::from("测试-password")).into(),
            Lzma2Options::default().into(),
        ]);
        writer.set_encrypt_header(header);
    }
    writer
        .push_archive_entry(
            ArchiveEntry::new_file(name),
            Some(b"name,value\nhello,42\n".as_slice()),
        )
        .unwrap();
    writer
        .push_archive_entry(
            ArchiveEntry::new_directory("empty"),
            None::<Cursor<Vec<u8>>>,
        )
        .unwrap();
    writer.finish().unwrap().into_inner()
}

fn content(bytes: Vec<u8>, name: &str) -> Vec<u8> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut result = Vec::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_end(&mut result)
        .unwrap();
    result
}

#[test]
fn seven_decode_passwords_and_paths() {
    with_io(false, |io| {
        for encrypted in [false, true] {
            for header in [false, true] {
                let bytes = seven_fixture("folder/中文.csv", encrypted, header);
                let password = encrypted.then_some("测试-password");
                let decoded = decode("test.7z", bytes.clone(), password, io).unwrap();
                assert_eq!(
                    content(decoded, "folder/中文.csv"),
                    b"name,value\nhello,42\n"
                );
                if encrypted {
                    assert!(decode("test.7z", bytes.clone(), None, io).is_err());
                    assert!(decode("test.7z", bytes, Some("wrong"), io).is_err());
                }
            }
        }
        for path in [
            "../escape.csv",
            "/absolute.csv",
            "CON.csv",
            "folder/../escape.csv",
        ] {
            assert!(decode("test.7z", seven_fixture(path, false, false), None, io).is_err());
        }
        assert!(decode("test.7z", b"broken".to_vec(), None, io).is_err());
    });
    with_io(true, |io| {
        assert!(decode("test.7z", seven_fixture("ok.csv", false, false), None, io).is_err())
    });
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn rar_header(kind: u8, flags: u16, fields: &[u8]) -> Vec<u8> {
    let mut header = vec![kind];
    header.extend_from_slice(&flags.to_le_bytes());
    header.extend_from_slice(&((7 + fields.len()) as u16).to_le_bytes());
    header.extend_from_slice(fields);
    let mut result = (crc32(&header) as u16).to_le_bytes().to_vec();
    result.extend(header);
    result
}

fn rar_fixture(name: &str, corrupt: bool) -> Vec<u8> {
    // RAR4 Stored：测试数据完全自建，文件内容的 CRC 不正确时必须拒绝。
    let data = b"name,value\nhello,42\n";
    let mut bytes = b"Rar!\x1a\x07\x00".to_vec();
    bytes.extend(rar_header(0x73, 0, &[0; 6]));
    let mut fields = Vec::new();
    fields.extend_from_slice(&(data.len() as u32).to_le_bytes());
    fields.extend_from_slice(&(data.len() as u32).to_le_bytes());
    fields.push(3); // Unix 文件属性。
    fields.extend_from_slice(&(crc32(data) ^ u32::from(corrupt)).to_le_bytes());
    fields.extend_from_slice(&0u32.to_le_bytes());
    fields.extend_from_slice(&[20, 0x30]);
    fields.extend_from_slice(&(name.len() as u16).to_le_bytes());
    fields.extend_from_slice(&0o100644u32.to_le_bytes());
    fields.extend_from_slice(name.as_bytes());
    bytes.extend(rar_header(0x74, 0x8000, &fields));
    bytes.extend_from_slice(data);
    bytes.extend(rar_header(0x7b, 0, &[]));
    bytes
}

#[test]
fn rar_decode_crc_paths_and_cancellation() {
    with_io(false, |io| {
        let decoded = decode("test.rar", rar_fixture("folder/test.csv", false), None, io).unwrap();
        assert_eq!(
            content(decoded, "folder/test.csv"),
            b"name,value\nhello,42\n"
        );
        assert!(decode("test.rar", rar_fixture("ok.csv", true), None, io).is_err());
        for path in [
            "../escape.csv",
            "/absolute.csv",
            "CON.csv",
            "folder/../escape.csv",
        ] {
            assert!(decode("test.rar", rar_fixture(path, false), None, io).is_err());
        }
        assert!(decode("test.rar", b"broken".to_vec(), None, io).is_err());
    });
    with_io(true, |io| {
        assert!(decode("test.rar", rar_fixture("ok.csv", false), None, io).is_err())
    });
}

#[test]
fn expanded_limits_and_collisions_are_rejected() {
    let entry = |name: &str, size| Entry {
        name: name.into(),
        size,
        compressed: 0,
        directory: false,
        encrypted: false,
        safe: true,
    };
    assert!(check_entries(&[entry("large", MEMBER_LIMIT + 1)]).is_err());
    assert!(check_entries(&[
        entry("a", MEMBER_LIMIT),
        entry("b", MEMBER_LIMIT),
        entry("c", 1)
    ])
    .is_err());
    assert!(check_entries(&[entry("a", 1), entry("A", 1)]).is_err());
    assert!(check_entries(&[entry("a", 1), entry("a/b", 1)]).is_err());
}

#[test]
fn archive_operations_list_preview_verify_extract_and_password_cache() {
    with_io(false, |io| {
        let root = std::env::temp_dir().join(format!("termrill-archive-test-{}", identifier()));
        std::fs::create_dir(&root).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        for format in ["7z", "rar"] {
            let name = "folder/test.csv";
            let bytes = if format == "7z" {
                seven_fixture(name, true, true)
            } else {
                rar_fixture(name, false)
            };
            let path = root.join(format!("test.{format}"));
            std::fs::write(&path, bytes).unwrap();
            let mut request: Request = serde_json::from_value(serde_json::json!({
                "operation": "list", "snapshot": "integration", "path": path,
                "password": if format == "7z" { Some("测试-password") } else { None },
            }))
            .unwrap();
            let reply = super::super::run(request.clone(), io, io, "extra-test").unwrap();
            assert!(reply.entries.iter().any(|entry| entry.name == name));
            request.revision = Some(reply.revision);
            request.selected = vec![name.into()];
            request.operation = "preview".into();
            let reply = super::super::run(request.clone(), io, io, "extra-test").unwrap();
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(reply.bytes.unwrap())
                    .unwrap(),
                b"name,value\nhello,42\n"
            );
            if format == "7z" {
                let mut wrong = request.clone();
                wrong.password = Some("wrong".into());
                assert!(super::super::run(wrong, io, io, "extra-test").is_err());
            }
            request.operation = "verify".into();
            super::super::run(request.clone(), io, io, "extra-test").unwrap();
            let target = root.join(format!("output-{format}"));
            std::fs::create_dir(&target).unwrap();
            request.operation = "extract".into();
            request.target = Some(target.to_str().unwrap().into());
            super::super::run(request.clone(), io, io, "extra-test").unwrap();
            assert_eq!(
                std::fs::read(target.join(name)).unwrap(),
                b"name,value\nhello,42\n"
            );
            // 同名目标必须保留，不能由新的格式绕开安装保护。
            assert!(super::super::run(request, io, io, "extra-test").is_err());
            super::super::release("extra-test", "integration", path.to_str().unwrap());
        }
    });
}
