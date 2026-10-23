//! 上传完成记录由独立线程批量持久化，正文协程不执行磁盘同步。
//! 队列有界；失败仍排空，终态必须等待线程退出和全部持久化结果。
use crate::model::*;

pub(super) fn with_checkpoints<T>(
    backend: &Backend,
    id: &str,
    run: impl FnOnce(&dyn Fn(&str, String) -> Result<()>) -> T,
) -> (T, Result<()>) {
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
                    if let Err(problem) = if batch.len() == 1 {
                        crate::transfer_journal::checkpoint(
                            backend,
                            id,
                            &batch[0].0,
                            batch[0].1.clone(),
                        )
                    } else {
                        crate::transfer_journal::checkpoint_batch(backend, id, &batch)
                    } {
                        failure.store(true, Ordering::Release);
                        error = Some(problem);
                    }
                }
            }
            error.map_or(Ok(()), Err)
        });
        let checkpoint = |path: &str, digest: String| {
            if failed.load(Ordering::Acquire) {
                return Err(AppError::new("上传完成记录持久化失败"));
            }
            sender
                .send((path.to_owned(), digest))
                .map_err(|_| AppError::new("上传记录线程已停止"))
        };
        let result = run(&checkpoint);
        drop(sender);
        let persisted = worker
            .join()
            .map_err(|_| AppError::new("上传记录线程异常退出"))
            .and_then(|result| result);
        (result, persisted)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::Job;
    use std::{collections::BTreeMap, fs};
    #[test]
    fn queued_upload_records_are_durable_before_return_and_failures_propagate() {
        let root =
            std::env::temp_dir().join(format!("termrill-upload-journal-{}", crate::identifier()));
        fs::create_dir_all(&root).unwrap();
        let backend = crate::tests::backend(&root);
        let job = Job {
            id: crate::identifier(),
            connection_id: "fixture".into(),
            local: root.display().to_string(),
            remote: "/test".into(),
            direction: "upload".into(),
            overwrite: false,
            completed: BTreeMap::new(),
            selection: None,
        };
        backend.jobs.lock().unwrap().insert(
            job.id.clone(),
            (job.clone(), crate::tests::request(22).profile),
        );
        crate::transfer::persist_jobs(&backend).unwrap();
        let (sent, persisted) = with_checkpoints(&backend, &job.id, |checkpoint| {
            for index in 0..65 {
                checkpoint(&format!("/test/{index}"), format!("digest-{index}"))?;
            }
            Ok::<(), AppError>(())
        });
        sent.unwrap();
        persisted.unwrap();
        let recovered = crate::transfer_journal::load(&root.join("transfers.json")).unwrap();
        assert_eq!(recovered[&job.id].0.completed.len(), 65);
        assert_eq!(recovered[&job.id].0.completed["/test/64"], "digest-64");
        backend.jobs.lock().unwrap().remove(&job.id);
        assert!(with_checkpoints(&backend, &job.id, |checkpoint| {
            checkpoint("/test/failure", "digest".into())
        })
        .1
        .is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
