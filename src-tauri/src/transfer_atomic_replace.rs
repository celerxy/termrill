//! 原子替换兼容处理：仅在明确失败且两端快照未变化时允许备份改名。
use super::*;
use russh_sftp::{
    client::{error::Error as SftpError, rawsession::RawSftpSession},
    protocol::{FileAttributes, Packet, StatusCode},
};

fn unchanged(before: &FileAttributes, after: &FileAttributes) -> bool {
    before.size == after.size
        && before.permissions == after.permissions
        && before.uid == after.uid
        && before.gid == after.gid
        && before.mtime == after.mtime
}

/// true 表示安装已完成；false 表示可使用调用方已有的备份与回滚流程。
pub(super) async fn replace(sftp: &RawSftpSession, temporary: &str, target: &str) -> Result<bool> {
    let stage = "原子替换目标文件";
    let before_source = sftp
        .lstat(temporary)
        .await
        .map_err(|e| install_stage_error(stage, e))?
        .attrs;
    let before_target = sftp
        .lstat(target)
        .await
        .map_err(|e| install_stage_error(stage, e))?
        .attrs;
    if !before_source
        .permissions
        .is_some_and(|mode| mode & 0o170000 == 0o100000)
        || !before_target
            .permissions
            .is_some_and(|mode| mode & 0o170000 == 0o100000)
    {
        return Err(AppError::new("原子替换前文件类型已变化，未覆盖目标"));
    }
    let mut packet = Vec::new();
    crate::sftp_packet::push_string(&mut packet, temporary.as_bytes())?;
    crate::sftp_packet::push_string(&mut packet, target.as_bytes())?;
    let status = match sftp.extended("posix-rename@openssh.com", packet).await {
        Ok(Packet::Status(status)) => status,
        Err(SftpError::Status(status)) => status,
        // 超时/断线没有确定执行结果，绝不能继续移动旧目标。
        Err(error) => return Err(install_stage_error(stage, error)),
        _ => return Err(AppError::new("原子安装响应无效，未重试改名")),
    };
    if status.status_code == StatusCode::Ok {
        return Ok(true);
    }
    if !matches!(
        status.status_code,
        StatusCode::OpUnsupported | StatusCode::Failure
    ) {
        return Err(install_stage_error(stage, SftpError::Status(status)));
    }
    // 部分服务声明扩展但返回通用 FAILURE。确认源仍在、目标未变化后才回退；
    // 属性核对不是跨客户端事务，后续仍保留原备份、安装失败恢复和清理规则。
    let after_source = sftp
        .lstat(temporary)
        .await
        .map_err(|e| install_stage_error(stage, e))?
        .attrs;
    let after_target = sftp
        .lstat(target)
        .await
        .map_err(|e| install_stage_error(stage, e))?
        .attrs;
    if !unchanged(&before_source, &after_source) || !unchanged(&before_target, &after_target) {
        return Err(AppError::new(
            "原子替换失败后文件状态已变化，未继续覆盖；请核对目标与临时文件",
        ));
    }
    Ok(false)
}
