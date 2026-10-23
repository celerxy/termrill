//! 原生异步临时文件写入与安装保护。
use super::*;

impl Remote {
    /// 独占临时文件写入；确认 CLOSE 后复用上传的原子安装与备份恢复逻辑。
    pub async fn install_bytes(
        &self,
        path: &str,
        bytes: &[u8],
        overwrite: bool,
        cancel: &AtomicBool,
        expected_revision: Option<&str>,
    ) -> Result<()> {
        self.install_bytes_progress(path, bytes, overwrite, cancel, expected_revision, |_, _| {})
            .await
    }
    pub(crate) async fn install_bytes_progress(
        &self,
        path: &str,
        bytes: &[u8],
        overwrite: bool,
        cancel: &AtomicBool,
        expected_revision: Option<&str>,
        progress: impl Fn(u64, bool),
    ) -> Result<()> {
        crate::remote_delete::parents(self, path, cancel).await?;
        let previous = match self
            .checked(
                async { Ok::<_, AppError>(self.native.sftp.lstat(path.to_owned()).await) },
                cancel,
            )
            .await?
        {
            Ok(value) => Some(attrs(&value.attrs)),
            Err(error) if missing(&error) => None,
            Err(error) => return Err(error.into()),
        };
        if previous.as_ref().is_some_and(|stat| !stat.is_file())
            || (previous.is_some() && !overwrite)
        {
            return Err(AppError::new("目标已存在或类型冲突"));
        }
        let temporary = std::path::Path::new(path)
            .with_file_name(format!(".termrill-archive-{}.part", identifier()));
        let temporary = temporary.to_string_lossy().into_owned();
        let _permit = self.permit(cancel).await?;
        let handle = self
            .checked(
                self.native.sftp.open(
                    temporary.clone(),
                    OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                    FileAttributes {
                        permissions: Some(0o600),
                        ..Default::default()
                    },
                ),
                cancel,
            )
            .await?
            .handle;
        let result = async {
            let block = self.native.write_size.min(32768);
            let mut pending = bytes.chunks(block).enumerate();
            let mut writes = futures::stream::FuturesUnordered::new();
            let mut failure = None;
            let mut acknowledged = 0u64;
            loop {
                while failure.is_none() && writes.len() < 16 {
                    let Some((index, chunk)) = pending.next() else {
                        break;
                    };
                    let write_handle = handle.clone();
                    writes.push(async move {
                        let result = self
                            .checked(
                                self.native.sftp.write(
                                    write_handle,
                                    (index * block) as u64,
                                    chunk.to_vec(),
                                ),
                                cancel,
                            )
                            .await;
                        (result, chunk.len() as u64)
                    });
                }
                let Some((result, length)) = writes.next().await else {
                    break;
                };
                if let Err(error) = result {
                    failure.get_or_insert(error);
                } else {
                    acknowledged += length;
                    progress(acknowledged, false);
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
            self.checked(self.native.sftp.close(handle.clone()), cancel)
                .await?;
            drop(_permit);
            progress(acknowledged, true);
            if let Some(expected) = expected_revision {
                use sha2::{Digest, Sha256};
                let current = self.read(path, 2 * 1024 * 1024, cancel, |_| {}).await?;
                if format!("{:x}", Sha256::digest(current)) != expected {
                    return Err(AppError::new("远程文件已被修改，请重新打开后合并内容"));
                }
            }
            crate::remote_delete::parents(self, path, cancel).await?;
            let current = match self
                .checked(
                    async { Ok::<_, AppError>(self.native.sftp.lstat(path.to_owned()).await) },
                    cancel,
                )
                .await?
            {
                Ok(value) => Some(attrs(&value.attrs)),
                Err(error) if missing(&error) => None,
                Err(error) => return Err(error.into()),
            };
            if current.as_ref().is_some_and(|stat| !stat.is_file())
                || (!overwrite && current.is_some())
            {
                return Err(AppError::new("目标已存在或类型已变化"));
            }
            if current.as_ref().map(|stat| (stat.size, stat.mtime))
                != previous.as_ref().map(|stat| (stat.size, stat.mtime))
            {
                return Err(AppError::new("目标在写入期间已变化"));
            }
            let item = crate::transfer::Item {
                local: std::path::PathBuf::new(),
                remote: path.into(),
                directory: false,
                size: bytes.len() as u64,
                link: None,
                link_directory: false,
                remote_target: previous,
            };
            crate::transfer::native_async::install(&self.native, &temporary, path, &item, overwrite)
                .await
        }
        .await;
        if result.is_err() {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.native.sftp.close(handle),
            )
            .await;
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.native.sftp.remove(temporary),
            )
            .await;
        }
        result
    }
}
