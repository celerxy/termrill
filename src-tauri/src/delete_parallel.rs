//! 标准 SFTP 并发删除；每个线程独占连接，目录在全部子项完成后删除。
use crate::{model::*, ssh};
use ssh2::Sftp;
#[path = "delete_pipeline.rs"]
mod pipeline;
#[path = "delete_requests.rs"]
mod requests;
use std::time::Instant;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

pub fn run(
    path: &str,
    recursive: bool,
    request: ConnectRequest,
    backend: &Backend,
    closed: &AtomicBool,
    transport: (Arc<Sftp>, &ssh2::Session),
    progress: &dyn Fn(&str),
) -> Result<()> {
    let (existing, session) = transport;
    let started = Instant::now();
    let trace_id = crate::files::delete_trace_id();
    let trace = crate::files::DeleteTrace::new(&trace_id);
    let timing = Mutex::new((Instant::now(), "start".to_string()));
    let report = |phase: &str| {
        if let Ok(mut previous) = timing.lock() {
            crate::performance_log!(
                "[TermRill delete] stage={} elapsed_ms={} total_ms={}",
                previous.1,
                previous.0.elapsed().as_millis(),
                started.elapsed().as_millis()
            );
            *previous = (Instant::now(), phase.to_string());
        }
        progress(phase);
    };
    report("检查删除路径");
    crate::files::check_delete_path(Path::new(path), Some(&existing), &trace)?;
    let path = path.trim_end_matches('/').to_string();
    let mut pool = vec![existing];
    let mut owners = Vec::new();
    let stat = trace.step("target_lstat", 0, || {
        pool[0].lstat(Path::new(&path)).map_err(Into::into)
    })?;
    if !stat.is_dir() {
        report("正在删除文件");
        return trace.step("unlink", 0, || {
            pool[0].unlink(Path::new(&path)).map_err(Into::into)
        });
    }
    report("扫描目录");
    let children = trace.step("root_readdir", 0, || children(&pool[0], &path))?;
    if !recursive && !children.is_empty() {
        return Err(AppError::new("目录非空，请勾选递归删除"));
    }
    let mut directories = BTreeMap::from([(0usize, vec![path])]);
    let pending = children;
    crate::performance_log!(
        "[TermRill delete] root_entries={} root_directories={}",
        pending.len(),
        pending.iter().filter(|item| item.1.is_dir()).count()
    );
    let interactive = request.profile.target.auth == "interactive"
        || request
            .profile
            .jump
            .as_ref()
            .is_some_and(|jump| jump.auth == "interactive");
    let workers = worker_count(interactive, &pending);
    report("建立工作连接");
    if closed.load(Ordering::Relaxed) {
        return Err(AppError::new("连接已关闭，删除已停止"));
    }
    // 握手和 SFTP 初始化均在各自线程完成，避免子系统初始化顺序等待。
    let connections = std::thread::scope(|scope| {
        let handles: Vec<_> = (1..workers)
            .map(|_| {
                let request = request.clone();
                scope.spawn(move || -> Result<_> {
                    let owner = ssh::connect(request, backend)?;
                    let sftp = Arc::new(owner.session.sftp()?);
                    Ok((owner, sftp))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err(AppError::new("工作连接建立异常")))
            })
            .collect::<Vec<_>>()
    });
    for connection in connections {
        match connection {
            Ok((owner, sftp)) => {
                pool.push(sftp);
                owners.push(owner);
            }
            Err(error) if error.host_key.is_some() => return Err(error),
            Err(_) => {
                crate::performance_log!("[TermRill delete] extra_connection_failed=true");
                continue;
            } // 服务端限制额外连接时，使用已建立的连接继续。
        }
    }
    if closed.load(Ordering::Relaxed) {
        return Err(AppError::new("连接已关闭，删除已停止"));
    }
    crate::performance_log!("[TermRill delete] workers={}", pool.len());
    report(if pool.len() > 1 {
        "并发扫描并删除文件"
    } else {
        "扫描并删除文件"
    });
    if !pending.is_empty() {
        directories.extend(match requests::Client::prepare(session, closed) {
            Ok(mut client) => client.run(pending)?,
            Err(_) => {
                crate::performance_log!(
                    "[TermRill delete] request_pipeline=false probe_failed=true"
                );
                pipeline::run(&pool, pending, closed)?
            }
        });
    }
    report("删除空目录");
    for paths in directories.values().rev() {
        let level_started = Instant::now();
        remove_directories(&pool, paths, closed)?;
        crate::performance_log!(
            "[TermRill delete] remove_directories={} elapsed_ms={}",
            paths.len(),
            level_started.elapsed().as_millis()
        );
    }
    report("释放工作连接");
    drop(pool);
    drop(owners);
    report("完成");
    crate::performance_log!(
        "[TermRill delete] finished=true total_ms={}",
        started.elapsed().as_millis()
    );
    Ok(())
}

type Entry = (String, crate::model::FileStat);

fn worker_count(interactive: bool, entries: &[Entry]) -> usize {
    // 一个顶层目录也可能包含大量后代；只有已知的小型平面目录才走单连接。
    // 类型缺失时同样不能把条目当成普通文件来估算工作量。
    let may_have_descendants = entries
        .iter()
        .any(|(_, stat)| stat.perm.is_none() || stat.is_dir());
    if interactive || (entries.len() < 64 && !may_have_descendants) {
        1
    } else {
        4
    }
}

fn children(sftp: &Sftp, path: &str) -> Result<Vec<Entry>> {
    let mut children = Vec::new();
    for (child, stat) in sftp.readdir(Path::new(path))? {
        let name = child
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| AppError::new("服务器返回无效名称；部分项目可能已删除"))?;
        if name == "." || name == ".." {
            continue;
        }
        if name.contains(['/', '\\', '\0']) {
            return Err(AppError::new("服务器返回无效名称；部分项目可能已删除"));
        }
        children.push((format!("{path}/{name}"), stat));
        if children.len() > 100_000 {
            return Err(AppError::new("目录项目数量超限；部分项目可能已删除"));
        }
    }
    Ok(children)
}

fn remove_directories(pool: &[Arc<Sftp>], paths: &[String], closed: &AtomicBool) -> Result<()> {
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for sftp in pool {
            let next = &next;
            let failed = &failed;
            handles.push(scope.spawn(move || -> Result<()> {
                let trace_id = crate::files::delete_trace_id();
                let trace = crate::files::DeleteTrace::new(&trace_id);
                loop {
                    if failed.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    if closed.load(Ordering::Relaxed) {
                        return Err(AppError::new("连接已关闭，部分项目可能已删除"));
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index) else {
                        return Ok(());
                    };
                    // RMDIR 不会递归进入被替换的链接。
                    let result = trace.step("rmdir", 0, || {
                        sftp.rmdir(Path::new(path)).map_err(Into::into)
                    });
                    if let Err(error) = result {
                        failed.store(true, Ordering::Relaxed);
                        return Err(AppError::new(format!(
                            "删除 {path} 失败：{}；部分项目可能已删除",
                            error.message
                        )));
                    }
                }
            }));
        }
        let mut error = None;
        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    error.get_or_insert(e);
                }
                Err(_) => {
                    error.get_or_insert_with(|| {
                        AppError::new("删除工作线程异常，部分项目可能已删除")
                    });
                }
            }
        }
        error.map_or(Ok(()), Err)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(perm: Option<u32>) -> Entry {
        (
            "/root/item".into(),
            crate::model::FileStat {
                size: None,
                uid: None,
                gid: None,
                perm,
                atime: None,
                mtime: None,
            },
        )
    }

    #[test]
    fn single_wrapper_directory_does_not_serialize_descendants() {
        assert_eq!(worker_count(false, &[entry(Some(0o040755))]), 4);
        assert_eq!(worker_count(false, &[entry(None)]), 4);
    }

    #[test]
    fn small_flat_and_interactive_tasks_keep_single_connection() {
        assert_eq!(worker_count(false, &[]), 1);
        assert_eq!(worker_count(false, &[entry(Some(0o100644))]), 1);
        assert_eq!(worker_count(false, &[entry(Some(0o120777))]), 1);
        assert_eq!(worker_count(true, &[entry(Some(0o040755))]), 1);
    }
}
