//! 链接覆盖只替换链接目录项；不打开目标内容、不向链接发送 chmod。
use crate::model::*;
use std::{fs, path::Path};

pub fn install_local(temporary: &Path, destination: &Path) -> Result<()> {
    let previous = match fs::symlink_metadata(destination) {
        Ok(stat) if stat.file_type().is_symlink() => true,
        Ok(_) => return Err(AppError::new("链接覆盖目标不是符号链接")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if !previous {
        // 无旧目标时由调用方直接创建链接，保持原生创建的同名保护。
        return Err(AppError::new("旧链接已消失，请重新创建传输任务"));
    }
    let backup = destination.with_file_name(format!(".termrill-link-backup-{}", identifier()));
    fs::rename(destination, &backup)?;
    let result = (|| -> Result<()> {
        if !fs::symlink_metadata(&backup)?.file_type().is_symlink() {
            return Err(AppError::new("旧项目类型在替换前已变化"));
        }
        // 同目录 rename 安装链接目录项，不触碰链接目标；仍存在并发替换竞态。
        if fs::symlink_metadata(destination).is_ok() {
            return Err(AppError::new("目标在替换时被其他程序重新创建"));
        }
        fs::rename(temporary, destination)?;
        Ok(())
    })();
    if let Err(error) = result {
        if fs::symlink_metadata(destination).is_ok() || fs::rename(&backup, destination).is_err() {
            return Err(AppError::new(format!(
                "链接替换失败：{}；旧项目保存在 {}",
                error.message,
                backup.display()
            )));
        }
        return Err(error);
    }
    remove_local(&backup).map_err(|error| {
        AppError::new(format!(
            "新链接已安装，但备份 {} 未能清理：{}",
            backup.display(),
            error.message
        ))
    })
}

pub fn remove_local(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if fs::symlink_metadata(path)?.file_attributes() & 0x10 != 0 {
            fs::remove_dir(path)?;
            return Ok(());
        }
    }
    fs::remove_file(path)?;
    Ok(())
}
