//! 原生异步目录分页与排序。
use super::*;
impl Remote {
    pub async fn list(
        &self,
        path: &str,
        mut filter: crate::directory::Filter,
        cancel: &AtomicBool,
    ) -> Result<crate::directory::PageListing> {
        let started = std::time::Instant::now();
        let _permit = self.permit(cancel).await?;
        let resolve = async {
            let resolved = self
                .checked(self.native.sftp.realpath(path.to_owned()), cancel)
                .await?;
            resolved
                .files
                .first()
                .map(|entry| entry.filename.clone())
                .ok_or_else(|| AppError::new("服务器未返回规范目录"))
        };
        let (canonical, handle) = if matches!(path, "." | "/") {
            // 固定的会话工作目录/根目录可并行准备；普通路径仍先解析再打开，
            // 防止用户路径中的符号链接变化使目录内容与规范路径指向不同位置。
            let (canonical, opened) = tokio::join!(
                resolve,
                self.checked(self.native.sftp.opendir(path.to_owned()), cancel)
            );
            let handle = opened?.handle;
            match canonical {
                Ok(canonical) => (canonical, handle),
                Err(error) => {
                    // 并行 OPEN 已成功时，即使 REALPATH 失败也必须收回句柄。
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        self.native.sftp.close(handle),
                    )
                    .await;
                    return Err(error);
                }
            }
        } else {
            let canonical = resolve.await?;
            let handle = self
                .checked(self.native.sftp.opendir(canonical.clone()), cancel)
                .await?
                .handle;
            (canonical, handle)
        };
        let result = async {
            'pages: loop {
                let batch = match self
                    .checked(
                        async { Ok::<_, AppError>(self.native.sftp.readdir(handle.clone()).await) },
                        cancel,
                    )
                    .await?
                {
                    Ok(batch) => batch,
                    Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => {
                        break
                    }
                    Err(error) => return Err(error.into()),
                };
                if batch.files.is_empty() {
                    return Err(AppError::new("目录返回空批次"));
                }
                for entry in batch.files {
                    let name = entry.filename;
                    if name.contains(['/', '\\', '\0']) {
                        return Err(AppError::new("目录返回无效名称"));
                    }
                    if !filter.accepts(&name) {
                        continue;
                    }
                    let path = format!("{}/{}", canonical.trim_end_matches('/'), name);
                    let stat = attrs(&entry.attrs);
                    if stat.is_file() && stat.size.is_some() && stat.mtime.is_some() {
                        filter.regular_metadata.insert(
                            path.clone().into(),
                            (std::time::Instant::now(), stat.clone()),
                        );
                    }
                    filter.entries.push(FileEntry {
                        name,
                        path,
                        directory: stat.is_dir(),
                        symlink: stat.perm.is_some_and(|mode| mode & 0o170000 == 0o120000),
                        size: stat.size.unwrap_or(0),
                        modified: stat.mtime,
                    });
                    if filter.entries.len() > filter.page_size {
                        break 'pages;
                    }
                }
            }
            let parent = std::path::Path::new(&canonical)
                .parent()
                .map(|p| p.to_string_lossy().into_owned());
            Ok(filter.finish(canonical, parent))
        }
        .await;
        let close = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.native.sftp.close(handle),
        )
        .await;
        crate::performance_log!(
            "[TermRill connection] phase=directory elapsed_ms={} success={}",
            started.elapsed().as_millis(),
            result.is_ok() && close.as_ref().is_ok_and(|reply| reply.is_ok())
        );
        match result {
            Ok(page) => {
                close.map_err(|_| AppError::new("关闭目录句柄超时"))??;
                Ok(page)
            }
            Err(error) => Err(error),
        }
    }
}
