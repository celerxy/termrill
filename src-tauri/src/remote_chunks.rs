//! 顺序分块读取的句柄所有权、版本检查与取消。
use super::*;
impl Remote {
    pub async fn chunk(
        &self,
        request: &str,
        path: &str,
        offset: u64,
        stamp: Option<&str>,
    ) -> Result<crate::text_chunks::Chunk> {
        use base64::Engine;
        let cancel = {
            let mut owner = lock(&self.reader_stop)?;
            if let Some((current, flag)) = &*owner {
                if current == request {
                    flag.clone()
                } else {
                    flag.store(true, Ordering::Relaxed);
                    let flag = Arc::new(AtomicBool::new(false));
                    *owner = Some((request.into(), flag.clone()));
                    flag
                }
            } else {
                let flag = Arc::new(AtomicBool::new(false));
                *owner = Some((request.into(), flag.clone()));
                flag
            }
        };
        let mut current = self.reader.lock().await;
        if current
            .as_ref()
            .is_some_and(|reader| reader.request != request || reader.path != path)
        {
            if let Some(reader) = current.take() {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    self.native.sftp.close(reader.handle),
                )
                .await;
            }
        }
        if current.is_none() {
            if offset != 0 {
                return Err(AppError::new("预览句柄已释放，请重新打开"));
            }
            let permit = self.permit(&cancel).await?;
            let stat = self.stat(path, &cancel).await?;
            if !attrs(&stat).is_file() {
                return Err(AppError::new("仅支持普通文本文件"));
            }
            let size = stat.size.ok_or_else(|| AppError::new("无法获取文件大小"))?;
            let handle = self
                .checked(
                    self.native.sftp.open(
                        path.to_owned(),
                        OpenFlags::READ,
                        FileAttributes::default(),
                    ),
                    &cancel,
                )
                .await?
                .handle;
            let opened = self
                .checked(self.native.sftp.fstat(handle.clone()), &cancel)
                .await;
            match opened {
                Ok(opened)
                    if attrs(&opened.attrs).is_file()
                        && opened.attrs.size == stat.size
                        && opened.attrs.mtime == stat.mtime => {}
                result => {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        self.native.sftp.close(handle),
                    )
                    .await;
                    result?;
                    return Err(AppError::new("文件在打开时已变化"));
                }
            }
            *current = Some(Reader {
                request: request.into(),
                path: path.into(),
                handle,
                size,
                mtime: stat.mtime,
                next: 0,
                verified: 0,
                _permit: permit,
            });
        }
        let reader = current
            .as_mut()
            .ok_or_else(|| AppError::new("预览句柄不可用"))?;
        let version = format!("{}:{:?}", reader.size, reader.mtime.map(u64::from));
        let result = async {
            if reader.next != offset || stamp.is_some_and(|value| value != version) {
                return Err(AppError::new("文件或读取位置已变化"));
            }
            if offset - reader.verified >= 256 * 1024 {
                let stat = self
                    .checked(self.native.sftp.fstat(reader.handle.clone()), &cancel)
                    .await?
                    .attrs;
                if stat.size != Some(reader.size)
                    || stat.mtime != reader.mtime
                    || !attrs(&stat).is_file()
                {
                    return Err(AppError::new("文件已变化，请重新打开"));
                }
                reader.verified = offset;
            }
            let count = (reader.size - offset)
                .min(if offset == 0 { 8192 } else { 32768 })
                .min(self.native.read_size as u64);
            let bytes = if count == 0 {
                vec![]
            } else {
                self.checked(
                    self.native
                        .sftp
                        .read(reader.handle.clone(), offset, count as u32),
                    &cancel,
                )
                .await?
                .data
            };
            if bytes.len() as u64 > count {
                return Err(AppError::new("服务器返回超出请求范围的数据"));
            }
            if bytes.is_empty() && offset < reader.size {
                return Err(AppError::new("文件读取提前结束"));
            }
            reader.next += bytes.len() as u64;
            Ok(crate::text_chunks::Chunk {
                encoding: (offset == 0)
                    .then(|| crate::text_encoding::detect(&bytes, reader.next >= reader.size)),
                bytes: base64::engine::general_purpose::STANDARD.encode(bytes),
                next: reader.next,
                total: reader.size,
                stamp: version,
            })
        }
        .await;
        if result.is_err() || reader.next == reader.size {
            if let Some(reader) = current.take() {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    self.native.sftp.close(reader.handle),
                )
                .await;
            }
        }
        result
    }
    pub async fn close_reader(&self, request: &str) -> Result<()> {
        if let Some((current, flag)) = &*lock(&self.reader_stop)? {
            if current == request {
                flag.store(true, Ordering::Relaxed);
            }
        }
        let mut current = self.reader.lock().await;
        if current
            .as_ref()
            .is_some_and(|reader| reader.request == request)
        {
            if let Some(reader) = current.take() {
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    self.native.sftp.close(reader.handle),
                )
                .await
                .map_err(|_| AppError::new("关闭读取句柄超时"))??;
            }
        }
        Ok(())
    }
}
