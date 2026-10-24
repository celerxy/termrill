/** 恢复意图只保存在当前工作区内存中，不保存凭据，不跨端点或取消操作继续执行。 */
import type { Endpoint, Profile } from './types';

/** 只说明现有手动恢复约束，不自动重连或改动其他任务。 */
export function transferRecoveryHint(connected: boolean, activeTasks: number) {
  if (activeTasks > 0) {
    return connected
      ? '可使用当前连接重试；重新连接会断开会话，请先完成或取消其他任务（含暂停任务）。'
      : '请先完成或取消其他任务（含暂停任务），再连接原服务器续传。';
  }
  return connected
    ? '重试会核对已有内容；若当前连接已失效，可选择重新连接后续传。'
    : '点击“连接后续传”重新认证原服务器；已有临时内容会重新核对，不会自动覆盖目标文件。';
}

function endpointIdentity(endpoint: Endpoint | null) {
  return endpoint
    ? [endpoint.host, endpoint.port, endpoint.username, endpoint.auth, endpoint.keyPath]
    : null;
}

export function recoveryProfileIdentity(profile: Profile) {
  return JSON.stringify([
    profile.id,
    endpointIdentity(profile.target),
    endpointIdentity(profile.jump),
  ]);
}

export class TransferRecoveryIntent {
  private pending: { id: string; identity: string } | null = null;

  prepare(id: string, profile: Profile) {
    if (this.pending) return false;
    this.pending = { id, identity: recoveryProfileIdentity(profile) };
    return true;
  }

  cancel() {
    this.pending = null;
  }

  /** 认证完成后最多消费一次；即使端点不匹配也清除旧意图。 */
  take(profile: Profile) {
    const pending = this.pending;
    this.pending = null;
    return pending?.identity === recoveryProfileIdentity(profile) ? pending.id : null;
  }
}
