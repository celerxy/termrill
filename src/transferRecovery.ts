/** 恢复意图只保存在当前工作区内存中，不保存凭据，不跨端点或取消操作继续执行。 */
import type { Endpoint, Profile } from './types';

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
