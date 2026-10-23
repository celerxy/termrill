//! 文件完成时追加摘要日志；启动/退出/重试保存快照并压缩日志，避免逐文件重写累计清单。
use crate::{model::*, transfer::Job};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
type Jobs = BTreeMap<String, (Job, Profile)>;
#[derive(serde::Serialize, serde::Deserialize)]
struct Entry {
    path: String,
    digest: String,
}

fn journal_path(config: &Path, id: &str) -> Result<PathBuf> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(AppError::new("任务日志标识无效"));
    }
    Ok(config
        .with_file_name("transfer-progress")
        .join(format!("{id}.jsonl")))
}

pub fn checkpoint(backend: &Backend, id: &str, path: &str, digest: String) -> Result<()> {
    checkpoint_batch(backend, id, &[(path.into(), digest)])
}

/// 下载调度只投递有界完成记录；独立线程批量持久化，返回前收齐落盘结果。
/// 写入失败停止接收有效记录但继续排空队列，避免生产者或收尾线程互相等待。
pub fn with_download_checkpoints(
    backend: &Backend,
    id: &str,
    run: impl FnOnce(&dyn Fn(&str, String) -> Result<()>) -> Result<()>,
) -> Result<()> {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let failed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::sync_channel::<(String, String)>(32);
        let failure = &failed;
        let worker = scope.spawn(move || {
            let mut error = None;
            while let Ok(first) = receiver.recv() {
                let mut batch = vec![first];
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(10);
                while batch.len() < 16 {
                    let Some(remaining) =
                        deadline.checked_duration_since(std::time::Instant::now())
                    else {
                        break;
                    };
                    match receiver.recv_timeout(remaining) {
                        Ok(entry) => batch.push(entry),
                        Err(_) => break,
                    }
                }
                if error.is_none() {
                    if let Err(problem) = checkpoint_batch(backend, id, &batch) {
                        failure.store(true, Ordering::Release);
                        error = Some(problem);
                    }
                }
            }
            error.map_or(Ok(()), Err)
        });
        let checkpoint = |path: &str, digest: String| {
            if failed.load(Ordering::Acquire) {
                return Err(AppError::new("下载完成记录持久化失败"));
            }
            sender
                .send((path.to_owned(), digest))
                .map_err(|_| AppError::new("下载记录线程已停止"))
        };
        let result = run(&checkpoint);
        drop(sender);
        let persisted = worker
            .join()
            .map_err(|_| AppError::new("下载记录线程异常退出"))?;
        result?;
        persisted
    })
}

/// 每批最多 16 项，共享一次持久化；落盘成功后才更新已完成清单。
pub fn checkpoint_batch(backend: &Backend, id: &str, entries: &[(String, String)]) -> Result<()> {
    if entries.is_empty() || entries.len() > 16 {
        return Err(AppError::new("任务摘要批次大小无效"));
    }
    let mut jobs = lock(&backend.jobs)?;
    let (job, _) = jobs
        .get_mut(id)
        .ok_or_else(|| AppError::new("传输任务记录已不存在"))?;
    let journal = journal_path(&backend.config_path, id)?;
    fs::create_dir_all(
        journal
            .parent()
            .ok_or_else(|| AppError::new("日志目录无效"))?,
    )?;
    let mut record = Vec::new();
    for (path, digest) in entries {
        let mut line = serde_json::to_vec(&Entry {
            path: path.clone(),
            digest: digest.clone(),
        })?;
        line.push(b'\n');
        if line.len() > 65536 {
            return Err(AppError::new("任务摘要日志单项过长"));
        }
        record.extend_from_slice(&line);
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(journal)?;
    let length = file.metadata()?.len();
    if length > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut last = [0];
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            let start = length.saturating_sub(65536);
            file.seek(SeekFrom::Start(start))?;
            let mut tail = Vec::new();
            file.read_to_end(&mut tail)?;
            let complete = match tail.iter().rposition(|byte| *byte == b'\n') {
                Some(offset) => start + offset as u64 + 1,
                None if start == 0 => 0,
                None => return Err(AppError::new("任务日志尾部无效")),
            };
            file.set_len(complete)?;
        }
    }
    file.write_all(&record)?;
    file.sync_data()?;
    // 持久化成功才更新内存；锁与快照压缩共用，避免丢掉新追加的摘要。
    for (path, digest) in entries {
        job.completed.insert(path.clone(), digest.clone());
    }
    Ok(())
}

pub fn load(snapshot: &Path) -> Result<Jobs> {
    let mut jobs: Jobs = serde_json::from_slice(&fs::read(snapshot)?)?;
    for (id, (job, _)) in &mut jobs {
        let path = journal_path(snapshot, id)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        // 进程中断留下的尾部半行没有完成标记，保守重传；修复尾部再允许追加。
        let complete = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |offset| offset + 1);
        for line in bytes[..complete]
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let entry: Entry = serde_json::from_slice(line)?;
            job.completed.insert(entry.path, entry.digest);
        }
        if complete != bytes.len() {
            let file = fs::OpenOptions::new().write(true).open(path)?;
            file.set_len(complete as u64)?;
            file.sync_all()?;
        }
    }
    Ok(jobs)
}

/// 快照已经包含已完成摘要，旧日志可以删除；中断后的重复日志重放是幂等的。
pub fn compact(backend: &Backend, _jobs: &Jobs) -> Result<()> {
    let directory = backend.config_path.with_file_name("transfer-progress");
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod download_tests {
    use super::*;

    #[test]
    fn queued_download_records_are_durable_before_return_and_failures_propagate() {
        let root =
            std::env::temp_dir().join(format!("termrill-download-journal-{}", crate::identifier()));
        fs::create_dir_all(&root).unwrap();
        let backend = crate::tests::backend(&root);
        let job = Job {
            id: crate::identifier(),
            connection_id: "fixture".into(),
            local: root.display().to_string(),
            remote: "/test".into(),
            direction: "download".into(),
            overwrite: false,
            completed: BTreeMap::new(),
            selection: None,
        };
        backend.jobs.lock().unwrap().insert(
            job.id.clone(),
            (job.clone(), crate::tests::request(22).profile),
        );
        crate::transfer::persist_jobs(&backend).unwrap();
        with_download_checkpoints(&backend, &job.id, |checkpoint| {
            for index in 0..65 {
                checkpoint(&format!("/test/{index}"), format!("digest-{index}"))?;
            }
            Ok(())
        })
        .unwrap();
        let recovered = load(&root.join("transfers.json")).unwrap();
        assert_eq!(recovered[&job.id].0.completed.len(), 65);
        assert_eq!(recovered[&job.id].0.completed["/test/64"], "digest-64");
        backend.jobs.lock().unwrap().remove(&job.id);
        assert!(with_download_checkpoints(&backend, &job.id, |checkpoint| {
            checkpoint("/test/failure", "digest".into())
        })
        .is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
