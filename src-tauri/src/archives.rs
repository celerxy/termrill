//! ZIP 原生浏览、校验、解压与重建；有界读取，不执行归档中的代码或链接。
use crate::{
    archive_buffer::ArchiveBuffer,
    archive_io::{safe_name, ArchiveIo, ARCHIVE_LIMIT, CONTENT_LIMIT, ENTRY_LIMIT},
    model::*,
};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::{Cursor, Read},
    path::Path,
};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};
#[path = "archive_cache.rs"]
mod cache;
#[path = "archive_decode.rs"]
mod decode;
#[path = "archive_encode.rs"]
mod encode;
#[path = "archive_extra.rs"]
mod extra;
#[path = "archive_zip_index.rs"]
mod index;
#[path = "archive_pack_read.rs"]
mod pack_read;
#[path = "archive_zip_remote.rs"]
mod remote_zip;
#[path = "archive_sources.rs"]
mod sources;
#[cfg(test)]
#[path = "archive_write_tests.rs"]
mod write_tests;

#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub operation: String,
    pub snapshot: Option<String>,
    pub path: String,
    #[serde(default)]
    pub selected: Vec<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    pub target: Option<String>,
    pub password: Option<String>,
    pub compression: Option<String>,
    pub revision: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub skip_existing: bool,
    #[serde(default)]
    pub download: bool,
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    name: String,
    size: u64,
    compressed: u64,
    directory: bool,
    encrypted: bool,
    safe: bool,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    entries: Vec<Entry>,
    revision: String,
    message: String,
    bytes: Option<String>,
}
fn zip_error(error: zip::result::ZipError) -> AppError {
    // 密码不进入错误、日志或持久化状态。
    AppError::new(format!("ZIP 操作失败：{error}"))
}
fn selected(name: &str, choices: &[String]) -> bool {
    choices.iter().any(|choice| {
        name == choice || name.starts_with(&format!("{}/", choice.trim_end_matches('/')))
    })
}
fn entries<R: Read + std::io::Seek>(archive: &mut ZipArchive<R>) -> Result<Vec<Entry>> {
    if archive.len() > ENTRY_LIMIT {
        return Err(AppError::new("压缩包超过 10000 项"));
    }
    let mut result = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(zip_error)?;
        let regular = entry
            .unix_mode()
            .is_none_or(|mode| matches!(mode & 0o170000, 0 | 0o100000 | 0o040000));
        result.push(Entry {
            name: entry.name().into(),
            size: entry.size(),
            compressed: entry.compressed_size(),
            directory: entry.is_dir(),
            encrypted: entry.encrypted(),
            safe: safe_name(entry.name()).is_ok() && regular,
        });
    }
    Ok(result)
}
fn validate(entries: &[Entry]) -> Result<()> {
    let mut names = BTreeSet::new();
    let mut bytes = 0u64;
    for entry in entries {
        if !entry.safe {
            return Err(AppError::new(
                "压缩包包含不安全路径、链接或特殊文件，已拒绝操作",
            ));
        }
        let name = safe_name(&entry.name)?.to_lowercase();
        if !names.insert(name) {
            return Err(AppError::new("压缩包存在重名或大小写冲突"));
        }
        bytes = bytes
            .checked_add(entry.size)
            .ok_or_else(|| AppError::new("解压大小无效"))?;
        if bytes > CONTENT_LIMIT || entry.size > 256 * 1024 * 1024 {
            return Err(AppError::new("解压内容超过 2 GiB 或单项超过 256 MiB"));
        }
    }
    let files: BTreeSet<_> = entries
        .iter()
        .filter(|entry| !entry.directory)
        .map(|entry| entry.name.to_lowercase())
        .collect();
    for entry in entries {
        let mut parent = entry.name.trim_end_matches('/');
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            if files.contains(&prefix.to_lowercase()) {
                return Err(AppError::new("压缩包中文件与目录路径冲突"));
            }
            parent = prefix;
        }
    }
    Ok(())
}
fn read_entry<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    index: usize,
    password: Option<&str>,
    limit: u64,
    io: &ArchiveIo<'_>,
) -> Result<Vec<u8>> {
    let mut entry = match password {
        Some(password) => archive.by_index_decrypt(index, password.as_bytes()),
        None => archive.by_index(index),
    }
    .map_err(zip_error)?;
    let expected_size = entry.size();
    if entry.size() > limit {
        return Err(AppError::new("包内文件超过当前操作的大小上限"));
    }
    let mut bytes = Vec::new();
    let mut chunk = [0; 64 * 1024];
    loop {
        io.check()?;
        let count = entry.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > limit
            || bytes.len() as u64 + count as u64 > expected_size
        {
            return Err(AppError::new("解压内容超过大小限制"));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    if bytes.len() as u64 != expected_size {
        return Err(AppError::new("包内文件实际大小与声明不符"));
    }
    // 读到 EOF 触发 CRC / AES 完整性核对；不能以中央目录可读作为校验成功。
    Ok(bytes)
}
pub fn run(
    request: Request,
    io: &ArchiveIo<'_>,
    local: &ArchiveIo<'_>,
    scope: &str,
) -> Result<Reply> {
    io.check()?;
    let zip_format = request.path.to_lowercase().ends_with(".zip");
    if matches!(request.operation.as_str(), "pack" | "add" | "remove") {
        encode::validate_options(&request)?;
    }
    if request.operation == "pack" {
        return pack(&request, io);
    }
    if matches!(request.operation.as_str(), "remove" | "add") && request.revision.is_none() {
        return Err(AppError::new("修改压缩包前必须先读取列表并提供原始摘要"));
    }
    if zip_format
        && io.native.is_some()
        && matches!(request.operation.as_str(), "list" | "preview")
        && request
            .revision
            .as_ref()
            .is_none_or(|revision| revision.starts_with("zip-index:"))
    {
        return remote_zip::run(&request, io, scope);
    }
    // 只有只读预览复用面板快照；校验、解压和修改仍读取真实源文件。
    let panel_key = format!(
        "{scope}\0{}\0{}",
        request.snapshot.as_deref().unwrap_or(""),
        request.path
    );
    // RAR/7Z 的快照为已解密内容，密码变化不可复用旧快照；摘要仅驻留内存。
    let key = if request.path.to_lowercase().ends_with(".rar")
        || request.path.to_lowercase().ends_with(".7z")
    {
        format!(
            "{panel_key}\0{:x}",
            Sha256::digest(request.password.as_deref().unwrap_or("").as_bytes())
        )
    } else {
        panel_key.clone()
    };
    let cached = if request.operation == "preview" && request.snapshot.is_some() {
        cache::get(&key, request.revision.as_deref())
    } else {
        cache::remove_panel(&panel_key);
        None
    };
    let (bytes, revision) = if let Some(value) = cached {
        io.phase("正在解码包内文件");
        value
    } else {
        io.phase("正在读取压缩包");
        let bytes = io.read(Path::new(&request.path), ARCHIVE_LIMIT)?;
        io.phase("正在解析压缩包");
        let revision = format!("{:x}", Sha256::digest(&bytes));
        // 范围浏览的标识校验中央目录（含条目 CRC），不冒充整包摘要。
        // 解压/修改读取真实全包；安装前仍使用这里计算的全包 SHA256 核对。
        let expected_matches = match request.revision.as_deref() {
            Some(expected) if expected.starts_with("zip-index:") && zip_format => {
                index::Index::load(&mut Cursor::new(bytes.as_slice()))?.revision == expected
            }
            Some(expected) => expected == revision,
            None => true,
        };
        if !expected_matches {
            return Err(AppError::new("压缩包已被修改，请重新打开后操作"));
        }
        let bytes: std::sync::Arc<[u8]> = if zip_format {
            bytes
        } else {
            decode::decode(&request.path, bytes, request.password.as_deref(), io)?
        }
        .into();
        if request.snapshot.is_some() && matches!(request.operation.as_str(), "list" | "preview") {
            cache::insert(key, revision.clone(), bytes.clone());
        }
        (bytes, revision)
    };
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
    let listed = entries(&mut archive)?;
    let mut reply = Reply {
        entries: listed,
        revision,
        message: String::new(),
        bytes: None,
    };
    if request.operation == "list" {
        // 单文件压缩流已在解码阶段展开；小文件随列表返回，预览无需再下载与解码。
        // 缓存属于当前面板快照，刷新会替换，不用于修改或解压的源校验。
        if !zip_format
            && reply.entries.len() == 1
            && !reply.entries[0].directory
            && reply.entries[0].size <= 2 * 1024 * 1024
            && reply.entries[0].safe
        {
            let content = read_entry(&mut archive, 0, None, 2 * 1024 * 1024, io)?;
            reply.bytes = Some(base64::engine::general_purpose::STANDARD.encode(content));
        }
        list_directories(&mut reply)?;
        return Ok(reply);
    }
    validate(&reply.entries)?;
    match request.operation.as_str() {
        "preview" => {
            if request.selected.len() != 1 {
                return Err(AppError::new("请选择一个包内文件"));
            }
            let index = reply
                .entries
                .iter()
                .position(|entry| entry.name == request.selected[0] && !entry.directory)
                .ok_or_else(|| AppError::new("包内文件不存在"))?;
            let bytes = read_entry(
                &mut archive,
                index,
                request.password.as_deref(),
                32 * 1024 * 1024,
                io,
            )?;
            reply.bytes = Some(base64::engine::general_purpose::STANDARD.encode(bytes));
        }
        "verify" => {
            for (index, entry) in reply.entries.iter().enumerate() {
                if !entry.directory {
                    io.phase(format!("校验 {}/{}", index + 1, reply.entries.len()));
                    read_entry(
                        &mut archive,
                        index,
                        request.password.as_deref(),
                        256 * 1024 * 1024,
                        io,
                    )?;
                }
            }
            reply.message = "完整性检查通过".into();
        }
        "extract" => {
            let output = if request.download { local } else { io };
            extract(&request, &mut archive, &reply.entries, io, output)?;
            reply.message = "解压完成".into();
        }
        "remove" | "add" => {
            let bytes = rewrite(&request, &mut archive, &reply.entries, io)?;
            drop(archive);
            let bytes = encode::encode(&request, bytes, io)?;
            io.phase("正在核对原压缩包并安装新版本");
            let current = io.read(Path::new(&request.path), ARCHIVE_LIMIT)?;
            if format!("{:x}", Sha256::digest(current)) != reply.revision {
                return Err(AppError::new("压缩包已被其他程序修改，已停止保存"));
            }
            io.install(Path::new(&request.path), &bytes, true)?;
            reply.message = "压缩包已更新，请重新读取列表".into();
            reply.revision = format!("{:x}", Sha256::digest(bytes));
        }
        _ => return Err(AppError::new("不支持的压缩包操作")),
    }
    Ok(reply)
}
fn list_directories(reply: &mut Reply) -> Result<()> {
    let mut names: BTreeSet<_> = reply
        .entries
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    let mut implicit = Vec::new();
    for entry in &reply.entries {
        let mut parent = entry.name.trim_end_matches('/');
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            let name = format!("{prefix}/");
            if names.insert(name.clone()) {
                implicit.push(Entry {
                    name,
                    size: 0,
                    compressed: 0,
                    directory: true,
                    encrypted: false,
                    safe: safe_name(prefix).is_ok(),
                });
            }
            parent = prefix;
        }
    }
    if names.len() > ENTRY_LIMIT {
        return Err(AppError::new("包内文件与虚拟目录合计超过 10000 项"));
    }
    reply.entries.extend(implicit);
    reply
        .entries
        .sort_by_key(|entry| (entry.name.to_lowercase(), !entry.directory));
    Ok(())
}
fn extract(
    request: &Request,
    archive: &mut ZipArchive<Cursor<std::sync::Arc<[u8]>>>,
    entries: &[Entry],
    input: &ArchiveIo<'_>,
    output: &ArchiveIo<'_>,
) -> Result<()> {
    let target = Path::new(
        request
            .target
            .as_deref()
            .ok_or_else(|| AppError::new("请选择解压目录"))?,
    );
    output.preflight_parents(target)?;
    // 先检查全部选中目标，再开始写文件。并发目录变化仍由安装时检查防护。
    let mut plan = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if !request.selected.is_empty() && !selected(&entry.name, &request.selected) {
            continue;
        }
        let destination = output.join(target, &safe_name(&entry.name)?);
        output.preflight_parents(&destination)?;
        if (!request.download || input.native.is_none()) && destination == Path::new(&request.path)
        {
            return Err(AppError::new("不能解压覆盖源压缩包"));
        }
        let existing = output.stat(&destination)?;
        if let Some(stat) = existing {
            if stat.directory != entry.directory {
                return Err(AppError::new("解压目标类型冲突"));
            }
            if !entry.directory {
                if request.skip_existing {
                    continue;
                }
                if !request.overwrite {
                    return Err(AppError::new(
                        "解压目标包含同名文件，请选择跳过或覆盖后重试",
                    ));
                }
            }
        }
        plan.push((index, destination));
    }
    if plan.is_empty() {
        return Err(AppError::new("没有可解压的项目"));
    }
    output.directory(target)?;
    for (completed, (index, destination)) in plan.iter().enumerate() {
        input.phase(format!("解压 {}/{}", completed + 1, plan.len()));
        let entry = &entries[*index];
        if entry.directory {
            output.directory(destination)?;
        } else {
            let bytes = read_entry(
                archive,
                *index,
                request.password.as_deref(),
                256 * 1024 * 1024,
                input,
            )?;
            output.install(destination, &bytes, request.overwrite)?;
        }
    }
    Ok(())
}
fn rewrite(
    request: &Request,
    archive: &mut ZipArchive<Cursor<std::sync::Arc<[u8]>>>,
    entries: &[Entry],
    io: &ArchiveIo<'_>,
) -> Result<Vec<u8>> {
    if request.operation == "remove" && request.selected.is_empty() {
        return Err(AppError::new("请选择要删除的包内项目"));
    }
    if request.operation == "add" && request.sources.is_empty() {
        return Err(AppError::new("请选择要添加的文件或目录"));
    }
    let mut sources = collect(&request.sources, io)?;
    if request.skip_existing {
        sources.retain(|(name, _, stat)| {
            stat.directory
                || !entries.iter().any(|entry| {
                    !entry.directory && entry.name.to_lowercase() == name.to_lowercase()
                })
        });
    }
    let replacements: BTreeSet<_> = sources
        .iter()
        .map(|(name, _, _)| name.trim_end_matches('/').to_lowercase())
        .collect();
    let mut planned = Vec::new();
    for entry in entries {
        if request.operation == "remove" && selected(&entry.name, &request.selected) {
            continue;
        }
        if replacements.contains(&entry.name.trim_end_matches('/').to_lowercase()) {
            let replacement = sources
                .iter()
                .find(|(name, _, _)| {
                    name.trim_end_matches('/').to_lowercase()
                        == entry.name.trim_end_matches('/').to_lowercase()
                })
                .ok_or_else(|| AppError::new("新增项目名称冲突"))?;
            if entry.directory != replacement.2.directory {
                return Err(AppError::new("新增条目与包内文件类型冲突"));
            }
            if !entry.directory && !request.overwrite {
                return Err(AppError::new("包内已有同名文件，请确认覆盖后重试"));
            }
        } else {
            planned.push(entry.clone());
        }
    }
    for (name, _, stat) in &sources {
        planned.push(Entry {
            name: if stat.directory {
                format!("{name}/")
            } else {
                name.clone()
            },
            size: if stat.directory { 0 } else { stat.size },
            compressed: 0,
            directory: stat.directory,
            encrypted: false,
            safe: true,
        });
    }
    validate(&planned)?;
    if encode::format(&request.path)? != encode::Format::Zip
        && planned.iter().map(|entry| entry.size).sum::<u64>() > ARCHIVE_LIMIT
    {
        return Err(AppError::new(
            "非 ZIP 重建的展开内容超过 512 MiB，未修改原包",
        ));
    }
    // ZIP 保留原始加密数据；写入前验证密码，防止误把新条目以明文混入加密包。
    if entries.iter().any(|entry| entry.encrypted) {
        if request.password.as_deref().is_none_or(str::is_empty) {
            return Err(AppError::new("修改加密 ZIP 必须提供原密码"));
        }
        for (index, entry) in entries.iter().enumerate() {
            if entry.encrypted && !entry.directory {
                read_entry(
                    archive,
                    index,
                    request.password.as_deref(),
                    256 * 1024 * 1024,
                    io,
                )?;
            }
        }
    }
    let mut writer = ZipWriter::new(ArchiveBuffer::default());
    for (index, entry) in entries.iter().enumerate() {
        io.check()?;
        if request.operation == "remove" && selected(&entry.name, &request.selected) {
            continue;
        }
        if replacements.contains(&entry.name.trim_end_matches('/').to_lowercase()) {
            if !entry.directory && !request.overwrite {
                return Err(AppError::new("包内已有同名项目，请确认覆盖后重试"));
            }
            continue;
        }
        io.phase(format!("保留包内项目 {}/{}", index + 1, entries.len()));
        if request.path.to_lowercase().ends_with(".zip")
            && request
                .password
                .as_deref()
                .is_some_and(|password| !password.is_empty())
        {
            if entry.directory {
                writer
                    .add_directory(
                        &entry.name,
                        SimpleFileOptions::default().unix_permissions(0o755),
                    )
                    .map_err(zip_error)?;
            } else {
                let content = read_entry(
                    archive,
                    index,
                    request.password.as_deref(),
                    256 * 1024 * 1024,
                    io,
                )?;
                writer
                    .start_file(&entry.name, encode::zip_options(request)?)
                    .map_err(zip_error)?;
                std::io::Write::write_all(&mut writer, &content)?;
            }
        } else {
            writer
                .raw_copy_file(archive.by_index_raw(index).map_err(zip_error)?)
                .map_err(zip_error)?;
        }
    }
    write_sources(
        &mut writer,
        &sources,
        io,
        None,
        encode::zip_options(request)?,
    )?;
    finish(writer)
}
type Source = (String, std::path::PathBuf, crate::archive_io::Stat);
fn collect(paths: &[String], io: &ArchiveIo<'_>) -> Result<Vec<Source>> {
    if let Some(remote) = io.native {
        return tauri::async_runtime::block_on(sources::collect(paths, io, remote));
    }
    let mut result = Vec::new();
    let mut pending = Vec::new();
    let mut seen = BTreeSet::new();
    for path in paths {
        let path = Path::new(path);
        let name = path
            .file_name()
            .ok_or_else(|| AppError::new("不能打包根目录"))?
            .to_string_lossy();
        pending.push((safe_name(&name)?, path.to_path_buf()));
    }
    let mut size = 0u64;
    let mut directories = 0usize;
    while let Some((name, path)) = pending.pop() {
        io.check()?;
        safe_name(&name)?;
        if result.len() >= ENTRY_LIMIT || !seen.insert(name.to_lowercase()) {
            return Err(AppError::new("打包项目过多或存在重名"));
        }
        io.parents(&path, false)?;
        let stat = io
            .stat(&path)?
            .ok_or_else(|| AppError::new("打包源文件不存在"))?;
        if stat.directory {
            for child in io.children(&path)? {
                let leaf = child
                    .file_name()
                    .ok_or_else(|| AppError::new("文件名无效"))?
                    .to_string_lossy();
                pending.push((format!("{name}/{leaf}"), child));
            }
        } else {
            size = size
                .checked_add(stat.size)
                .ok_or_else(|| AppError::new("文件大小无效"))?;
            if size > CONTENT_LIMIT || stat.size > 256 * 1024 * 1024 {
                return Err(AppError::new("打包内容超过限制"));
            }
        }
        directories += usize::from(stat.directory);
        result.push((name, path, stat));
        io.pack_scan(result.len() - directories, directories);
    }
    Ok(result)
}
fn write_sources(
    writer: &mut ZipWriter<ArchiveBuffer>,
    sources: &[Source],
    io: &ArchiveIo<'_>,
    tracking: pack_read::Tracking<'_>,
    options: zip::write::FileOptions<'_, ()>,
) -> Result<()> {
    if let Some(remote) = io.native {
        return pack_read::write(writer, sources, io, remote, tracking, options);
    }
    for (index, (name, path, stat)) in sources.iter().enumerate() {
        io.check()?;
        if let Some((state, _)) = tracking {
            state.borrow_mut().current_file = name.clone();
            io.pack_progress(&state.borrow(), false);
        } else {
            io.phase(format!("压缩 {}/{}", index + 1, sources.len()));
        }
        if stat.directory {
            writer
                .add_directory(
                    format!("{name}/"),
                    SimpleFileOptions::default().unix_permissions(0o755),
                )
                .map_err(zip_error)?;
        } else {
            let base = tracking.map_or(0, |(state, _)| state.borrow().processed_bytes);
            let update = |count| {
                if let Some((state, generated)) = tracking {
                    let mut progress = state.borrow_mut();
                    progress.processed_bytes = base + count;
                    progress.zip_bytes = generated.load(std::sync::atomic::Ordering::Relaxed);
                    io.pack_progress(&progress, false);
                }
            };
            writer.start_file(name, options).map_err(zip_error)?;
            io.copy_to_progress(path, 256 * 1024 * 1024, writer, update)?;
            if let Some((state, _)) = tracking {
                state.borrow_mut().processed_files += 1;
                io.pack_progress(&state.borrow(), false);
            }
        }
    }
    Ok(())
}
fn finish(writer: ZipWriter<ArchiveBuffer>) -> Result<Vec<u8>> {
    let started = std::time::Instant::now();
    let bytes = writer.finish().map_err(zip_error)?.into_inner();
    crate::performance_log!(
        "[TermRill archive] phase=zip_finish elapsed_ms={} bytes={}",
        started.elapsed().as_millis(),
        bytes.len()
    );
    if bytes.len() as u64 > ARCHIVE_LIMIT {
        return Err(AppError::new("生成的压缩包超过 512 MiB"));
    }
    Ok(bytes)
}
fn pack(request: &Request, io: &ArchiveIo<'_>) -> Result<Reply> {
    if request.sources.is_empty() {
        return Err(AppError::new("请选择要压缩的文件或目录"));
    }
    let state = std::cell::RefCell::new(crate::archive_io::PackProgress {
        kind: "packProgress",
        stage: "scan",
        ..Default::default()
    });
    io.pack_progress(&state.borrow(), true);
    // 已知目标冲突先拒绝，避免读取和压缩完全部来源才发现不可安装；安装前仍重新检查。
    let target = Path::new(&request.path);
    io.preflight_parents(target)?;
    if let Some(existing) = io.stat(target)? {
        if existing.directory || !request.overwrite {
            return Err(AppError::new("目标已存在或类型冲突"));
        }
    }
    let sources = collect(&request.sources, io)?;
    if encode::format(&request.path)? != encode::Format::Zip
        && sources
            .iter()
            .map(|(_, _, stat)| if stat.directory { 0 } else { stat.size })
            .sum::<u64>()
            > ARCHIVE_LIMIT
    {
        return Err(AppError::new("非 ZIP 创建的展开内容超过 512 MiB"));
    }
    if sources
        .iter()
        .any(|(_, path, _)| path == Path::new(&request.path))
    {
        return Err(AppError::new("目标压缩包不能包含在打包来源中"));
    }
    {
        let mut progress = state.borrow_mut();
        progress.stage = "compress";
        progress.files = sources
            .iter()
            .filter(|(_, _, stat)| !stat.directory)
            .count();
        progress.directories = sources.len() - progress.files;
        progress.total_bytes = sources
            .iter()
            .filter(|(_, _, stat)| !stat.directory)
            .map(|(_, _, stat)| stat.size)
            .sum();
        io.pack_progress(&progress, true);
    }
    let generated = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let zip_format = encode::format(&request.path)? == encode::Format::Zip;
    let buffer = if zip_format {
        ArchiveBuffer::tracked(generated.clone())
    } else {
        ArchiveBuffer::default()
    };
    let mut writer = ZipWriter::new(buffer);
    write_sources(
        &mut writer,
        &sources,
        io,
        Some((&state, &generated)),
        encode::zip_options(request)?,
    )?;
    let intermediate = finish(writer)?;
    if !zip_format {
        let mut progress = state.borrow_mut();
        progress.stage = "encode";
        progress.current_file.clear();
        io.pack_progress(&progress, true);
    }
    let bytes = encode::encode(request, intermediate, io)?;
    {
        let mut progress = state.borrow_mut();
        progress.stage = "upload";
        progress.current_file.clear();
        progress.zip_bytes = bytes.len() as u64;
        io.pack_progress(&progress, true);
    }
    io.install_progress(
        Path::new(&request.path),
        &bytes,
        request.overwrite,
        |written, installing| {
            let mut progress = state.borrow_mut();
            let changed = installing && progress.stage != "install";
            progress.stage = if installing { "install" } else { "upload" };
            progress.uploaded_bytes = written;
            io.pack_progress(&progress, changed);
        },
    )?;
    state.borrow_mut().stage = "done";
    io.pack_progress(&state.borrow(), true);
    Ok(Reply {
        entries: Vec::new(),
        revision: format!("{:x}", Sha256::digest(bytes)),
        message: "打包完成".into(),
        bytes: None,
    })
}

/// 面板关闭后释放快照，后台确认远程读取句柄关闭；不保存内容。
pub fn release(scope: &str, snapshot: &str, path: &str) {
    let panel_key = format!("{scope}\0{snapshot}\0{path}");
    cache::remove_panel(&panel_key);
    remote_zip::release(&panel_key);
}
pub fn initialize_cache(path: std::path::PathBuf) {
    remote_zip::initialize_cache(path);
}
