//! 单向目录差异比较：严格模式核对 SHA256；快速模式只比较大小及秒级修改时间。
use crate::model::*;
use sha2::{Digest, Sha256};
#[cfg(test)]
use ssh2::Sftp;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Difference {
    pub relative: String,
    pub remote: String,
    pub status: String,
    pub size: u64,
    pub directory: bool,
}
struct Node {
    path: PathBuf,
    kind: &'static str,
    size: u64,
    modified: Option<u64>,
}
fn stopped(cancel: &AtomicBool, closed: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) || closed.load(Ordering::Relaxed) {
        Err(AppError::new("目录比较已停止"))
    } else {
        Ok(())
    }
}
fn remote_child(root: &Path, name: &str) -> PathBuf {
    format!("{}/{}", root.to_string_lossy().trim_end_matches('/'), name).into()
}
#[cfg(test)]
fn scan(
    root: &Path,
    sftp: Option<&Sftp>,
    prefix: &str,
    output: &mut BTreeMap<String, Node>,
    cancel: &AtomicBool,
    closed: &AtomicBool,
    depth: usize,
) -> Result<()> {
    if depth > 64 || output.len() > 100_000 {
        return Err(AppError::new("比较目录超过 64 层或 100000 项限制"));
    }
    stopped(cancel, closed)?;
    let entries = if let Some(sftp) = sftp {
        sftp.readdir(root)?
            .into_iter()
            .filter_map(|(path, stat)| {
                path.file_name().map(|name| {
                    (
                        name.to_string_lossy().into_owned(),
                        if stat.is_dir() {
                            "directory"
                        } else if stat.is_file() {
                            "file"
                        } else {
                            "unsupported"
                        },
                        stat.size.unwrap_or(0),
                    )
                })
            })
            .collect::<Vec<_>>()
    } else {
        let mut entries = Vec::new();
        for entry in fs::read_dir(root)? {
            stopped(cancel, closed)?;
            let entry = entry?;
            let stat = fs::symlink_metadata(entry.path())?;
            #[cfg(windows)]
            let reparse = {
                use std::os::windows::fs::MetadataExt;
                stat.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let reparse = false;
            entries.push((
                entry
                    .file_name()
                    .to_str()
                    .ok_or_else(|| AppError::new("目录名称不是有效 UTF-8"))?
                    .to_owned(),
                if reparse || stat.file_type().is_symlink() {
                    "unsupported"
                } else if stat.is_dir() {
                    "directory"
                } else if stat.is_file() {
                    "file"
                } else {
                    "unsupported"
                },
                stat.len(),
            ));
        }
        entries
    };
    for (name, kind, size) in entries {
        stopped(cancel, closed)?;
        if output.len() >= 100_000 {
            return Err(AppError::new("比较目录超过 100000 项限制"));
        }
        if [".", ".."].contains(&name.as_str()) {
            continue;
        }
        if name.contains(['/', '\\', ':']) {
            return Err(AppError::new("目录包含不适合跨平台同步的名称"));
        }
        let relative = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let path = if sftp.is_some() {
            remote_child(root, &name)
        } else {
            root.join(&name)
        };
        output.insert(
            relative.clone(),
            Node {
                path: path.clone(),
                kind,
                size,
                modified: None,
            },
        );
        if kind == "directory" {
            scan(&path, sftp, &relative, output, cancel, closed, depth + 1)?;
        }
    }
    Ok(())
}
fn hash(mut reader: impl Read, cancel: &AtomicBool, closed: &AtomicBool) -> Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        stopped(cancel, closed)?;
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
pub fn compare(
    sftp: &Sftp,
    local: &Path,
    remote: &Path,
    direction: &str,
    cancel: &AtomicBool,
    closed: &AtomicBool,
) -> Result<Vec<Difference>> {
    if !["upload", "download"].contains(&direction)
        || !local.is_absolute()
        || !remote.to_string_lossy().starts_with('/')
        || remote.to_string_lossy().contains('\\')
        || remote
            .to_string_lossy()
            .split('/')
            .any(|part| [".", ".."].contains(&part))
        || local
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(AppError::new("比较方向或目录路径无效"));
    }
    for ancestor in local.ancestors() {
        let stat = fs::symlink_metadata(ancestor)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if stat.file_attributes() & 0x400 != 0 {
                return Err(AppError::new("比较目录不能包含目录联接"));
            }
        }
        if !stat.is_dir() {
            return Err(AppError::new("本地比较路径不是普通目录"));
        }
    }
    for ancestor in remote.ancestors() {
        if !ancestor.as_os_str().is_empty() && !sftp.lstat(ancestor)?.is_dir() {
            return Err(AppError::new("远程比较路径不能包含链接或非目录父级"));
        }
    }
    let mut local_nodes = BTreeMap::new();
    let mut remote_nodes = BTreeMap::new();
    scan(local, None, "", &mut local_nodes, cancel, closed, 0)?;
    scan(remote, Some(sftp), "", &mut remote_nodes, cancel, closed, 0)?;
    let (source, target) = if direction == "upload" {
        (&local_nodes, &remote_nodes)
    } else {
        (&remote_nodes, &local_nodes)
    };
    let mut differences = Vec::new();
    for (relative, node) in source {
        stopped(cancel, closed)?;
        let status = if node.kind == "unsupported" {
            "skipped"
        } else {
            match target.get(relative) {
                None => "new",
                Some(other) if node.kind != other.kind => "typeConflict",
                Some(_) if node.kind == "directory" => continue,
                Some(other) if node.size != other.size => "changed",
                Some(_) => {
                    let left = &local_nodes[relative];
                    let right = &remote_nodes[relative];
                    if hash(fs::File::open(&left.path)?, cancel, closed)?
                        == hash(sftp.open(&right.path)?, cancel, closed)?
                    {
                        "same"
                    } else {
                        "changed"
                    }
                }
            }
        };
        // 被类型冲突的祖先覆盖的子项不能直接传输。
        let blocked = relative.split('/').enumerate().any(|(index, _)| {
            let ancestor = relative
                .split('/')
                .take(index + 1)
                .collect::<Vec<_>>()
                .join("/");
            source
                .get(&ancestor)
                .is_some_and(|node| node.kind == "directory")
                && target
                    .get(&ancestor)
                    .is_some_and(|node| node.kind != "directory")
        });
        differences.push(Difference {
            relative: relative.clone(),
            remote: remote_child(remote, relative)
                .to_string_lossy()
                .into_owned(),
            status: if blocked { "typeConflict" } else { status }.into(),
            size: node.size,
            directory: node.kind == "directory",
        });
    }
    for (relative, node) in target {
        if !source.contains_key(relative) {
            differences.push(Difference {
                relative: relative.clone(),
                remote: remote_child(remote, relative)
                    .to_string_lossy()
                    .into_owned(),
                status: "targetOnly".into(),
                size: node.size,
                directory: node.kind == "directory",
            });
        }
    }
    Ok(differences)
}

/// 目录扫描和摘要读取使用原生 SFTP；本地校验移到磁盘工作线程。
#[cfg(test)]
pub async fn compare_async(
    remote: &crate::remote_async::Remote,
    local: &Path,
    root: &Path,
    direction: &str,
    cancel: &std::sync::Arc<AtomicBool>,
) -> Result<Vec<Difference>> {
    compare_with_options(remote, local, root, direction, cancel, Default::default()).await
}

pub async fn compare_with_options(
    remote: &crate::remote_async::Remote,
    local: &Path,
    root: &Path,
    direction: &str,
    cancel: &std::sync::Arc<AtomicBool>,
    options: crate::directory_sync_options::Options,
) -> Result<Vec<Difference>> {
    let exclusions = std::sync::Arc::new(options.exclusions()?);
    use futures::{stream::FuturesUnordered, StreamExt};
    if !["upload", "download"].contains(&direction)
        || !local.is_absolute()
        || local.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || !root.to_string_lossy().starts_with('/')
        || root.to_string_lossy().contains(['\\', '\0'])
        || root
            .to_string_lossy()
            .split('/')
            .any(|part| part == "." || part == "..")
    {
        return Err(AppError::new("比较方向或目录路径无效"));
    }
    let local_nodes = crate::blocking({
        let path = local.to_owned();
        let cancel = cancel.clone();
        let closed = remote.closed.clone();
        let exclusions = exclusions.clone();
        move || {
            for ancestor in path.ancestors() {
                let stat = fs::symlink_metadata(ancestor)?;
                if crate::local_link(&stat) || !stat.is_dir() {
                    return Err(AppError::new("本地比较路径包含链接或非目录"));
                }
            }
            let mut nodes = BTreeMap::new();
            scan_local(&path, &mut nodes, &cancel, &closed, &exclusions)?;
            Ok(nodes)
        }
    })
    .await?;
    crate::remote_delete::parents(remote, &root.to_string_lossy(), cancel).await?;
    if !crate::remote_async::attrs(&remote.stat(&root.to_string_lossy(), cancel).await?).is_dir() {
        return Err(AppError::new("远程比较路径不是普通目录"));
    }
    let mut remote_nodes = BTreeMap::new();
    let mut scanned = 0usize;
    let mut pending = std::collections::VecDeque::from([(
        root.to_string_lossy().into_owned(),
        String::new(),
        0usize,
    )]);
    let mut requests = FuturesUnordered::new();
    loop {
        while requests.len() < 16 {
            let Some((path, prefix, depth)) = pending.pop_front() else {
                break;
            };
            requests.push(async move {
                let listing = remote
                    .list(
                        &path,
                        crate::directory::Filter::with_size(0, String::new(), true, 100_000)?,
                        cancel,
                    )
                    .await?;
                Ok::<_, AppError>((listing, prefix, depth))
            });
        }
        let Some(result) = requests.next().await else {
            break;
        };
        let (listing, prefix, depth) = result?;
        if listing.page.has_more || depth > 64 {
            return Err(AppError::new("比较目录超过限制"));
        }
        for entry in listing.listing.entries {
            scanned += 1;
            if scanned > 100_000 || entry.name.contains(['/', '\\', ':']) {
                return Err(AppError::new("比较目录数量超限或名称不安全"));
            }
            let relative = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            if exclusions.contains(&relative) {
                continue;
            }
            let kind = if entry.symlink {
                "unsupported"
            } else if entry.directory {
                "directory"
            } else if listing
                .regular_metadata
                .contains_key(Path::new(&entry.path))
            {
                "file"
            } else {
                "unsupported"
            };
            remote_nodes.insert(
                relative.clone(),
                Node {
                    path: entry.path.clone().into(),
                    kind,
                    size: entry.size,
                    modified: entry.modified,
                },
            );
            if kind == "directory" {
                pending.push_back((entry.path, relative, depth + 1));
            }
        }
    }
    let (source, target) = if direction == "upload" {
        (&local_nodes, &remote_nodes)
    } else {
        (&remote_nodes, &local_nodes)
    };
    let mut hashes = BTreeMap::new();
    let mut files = source.iter().filter(|(relative, node)| {
        !options.quick
            && node.kind == "file"
            && target
                .get(*relative)
                .is_some_and(|other| other.kind == "file" && other.size == node.size)
    });
    let mut active = FuturesUnordered::new();
    loop {
        while active.len() < 8 {
            let Some((relative, _)) = files.next() else {
                break;
            };
            let left = &local_nodes[relative];
            let right = &remote_nodes[relative];
            active.push(async move {
                let local_hash = crate::blocking({
                    let path = left.path.clone();
                    let cancel = cancel.clone();
                    let closed = remote.closed.clone();
                    move || hash(fs::File::open(path)?, &cancel, &closed)
                })
                .await?;
                let mut digest = Sha256::new();
                remote
                    .read_into(
                        &right.path.to_string_lossy(),
                        right.size,
                        cancel,
                        |chunk| {
                            digest.update(chunk);
                            Ok(())
                        },
                        |_| {},
                    )
                    .await?;
                Ok::<_, AppError>((
                    relative.clone(),
                    local_hash == format!("{:x}", digest.finalize()),
                ))
            });
        }
        let Some(result) = active.next().await else {
            break;
        };
        let (relative, same) = result?;
        hashes.insert(relative, same);
    }
    let mut differences = Vec::new();
    for (relative, node) in source {
        stopped(cancel, &remote.closed)?;
        let status = if node.kind == "unsupported" {
            "skipped"
        } else {
            match target.get(relative) {
                None => "new",
                Some(other) if node.kind != other.kind => "typeConflict",
                Some(_) if node.kind == "directory" => continue,
                Some(other) if node.size != other.size => "changed",
                Some(_) => {
                    let same = if options.quick {
                        node.modified.is_some() && node.modified == target[relative].modified
                    } else {
                        hashes[relative]
                    };
                    if same {
                        "same"
                    } else {
                        "changed"
                    }
                }
            }
        };
        let blocked = relative.split('/').enumerate().any(|(index, _)| {
            let ancestor = relative
                .split('/')
                .take(index + 1)
                .collect::<Vec<_>>()
                .join("/");
            source
                .get(&ancestor)
                .is_some_and(|node| node.kind == "directory")
                && target
                    .get(&ancestor)
                    .is_some_and(|node| node.kind != "directory")
        });
        differences.push(Difference {
            relative: relative.clone(),
            remote: remote_child(root, relative).to_string_lossy().into_owned(),
            status: if blocked { "typeConflict" } else { status }.into(),
            size: node.size,
            directory: node.kind == "directory",
        });
    }
    for (relative, node) in target {
        if !source.contains_key(relative) {
            differences.push(Difference {
                relative: relative.clone(),
                remote: remote_child(root, relative).to_string_lossy().into_owned(),
                status: "targetOnly".into(),
                size: node.size,
                directory: node.kind == "directory",
            });
        }
    }
    Ok(differences)
}

fn scan_local(
    root: &Path,
    output: &mut BTreeMap<String, Node>,
    cancel: &AtomicBool,
    closed: &AtomicBool,
    exclusions: &crate::directory_sync_options::Exclusions,
) -> Result<()> {
    let mut pending = vec![(root.to_owned(), String::new(), 0usize)];
    let mut scanned = 0usize;
    while let Some((path, prefix, depth)) = pending.pop() {
        if depth > 64 {
            return Err(AppError::new("本地比较目录超过 64 层"));
        }
        for entry in fs::read_dir(path)? {
            stopped(cancel, closed)?;
            scanned += 1;
            if scanned > 100_000 {
                return Err(AppError::new("比较目录超过 100000 项"));
            }
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| AppError::new("名称不是 UTF-8"))?;
            if name.contains(['/', '\\', ':']) {
                return Err(AppError::new("目录名称不适合跨平台同步"));
            }
            let stat = fs::symlink_metadata(entry.path())?;
            let kind = if crate::local_link(&stat) {
                "unsupported"
            } else if stat.is_dir() {
                "directory"
            } else if stat.is_file() {
                "file"
            } else {
                "unsupported"
            };
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if exclusions.contains(&relative) {
                continue;
            }
            let path = entry.path();
            output.insert(
                relative.clone(),
                Node {
                    path: path.clone(),
                    kind,
                    size: stat.len(),
                    modified: stat
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|time| time.as_secs()),
                },
            );
            if kind == "directory" {
                pending.push((path, relative, depth + 1));
            }
        }
    }
    Ok(())
}
