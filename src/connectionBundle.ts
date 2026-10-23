/** 连接交换仅允许显式字段；拒绝混入凭据或信任记录，导入始终分配新 ID。 */
import type { Endpoint, Profile } from './types';
function object(value: unknown, allowed: string[]) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('配置对象无效');
  const result = value as Record<string, unknown>;
  if (Object.keys(result).some((key) => !allowed.includes(key)))
    throw new Error('配置含不支持字段，请勿导入凭据或信任记录');
  return result;
}
function text(value: unknown, max: number, empty = false) {
  if (
    typeof value !== 'string' ||
    value.length > max ||
    (!empty && !value.trim()) ||
    /[\x00-\x1f]/.test(value)
  )
    throw new Error('配置文本无效或过长');
  return value;
}
function endpoint(value: unknown): Endpoint {
  const item = object(value, ['host', 'port', 'username', 'auth', 'keyPath']);
  const auth = item.auth;
  if (
    !['password', 'key', 'agent', 'interactive'].includes(String(auth)) ||
    typeof item.port !== 'number' ||
    !Number.isInteger(item.port) ||
    item.port < 1 ||
    item.port > 65535
  )
    throw new Error('认证方式或端口无效');
  return {
    host: text(item.host, 512),
    port: item.port,
    username: text(item.username, 256),
    auth: auth as Endpoint['auth'],
    keyPath: text(item.keyPath, 4096, auth !== 'key'),
  };
}
export function exportConnections(profiles: Profile[], groups: Record<string, string>) {
  const safeEndpoint = (item: Endpoint) => ({
    host: item.host,
    port: item.port,
    username: item.username,
    auth: item.auth,
    keyPath: item.keyPath,
  });
  return JSON.stringify(
    {
      format: 'termrill-connections',
      version: 1,
      profiles: profiles.map((item) => ({
        name: item.name,
        target: safeEndpoint(item.target),
        jump: item.jump ? safeEndpoint(item.jump) : null,
        group: groups[item.id] ?? '',
      })),
    },
    null,
    2,
  );
}
export function importConnections(source: string): { profile: Profile; group: string }[] {
  if (new TextEncoder().encode(source).length > 1024 * 1024)
    throw new Error('配置文件不得超过 1 MiB');
  const bundle = object(JSON.parse(source), ['format', 'version', 'profiles']);
  if (
    bundle.format !== 'termrill-connections' ||
    bundle.version !== 1 ||
    !Array.isArray(bundle.profiles) ||
    bundle.profiles.length > 200
  )
    throw new Error('不支持的配置格式或连接超过 200 个');
  return bundle.profiles.map((value) => {
    const item = object(value, ['name', 'target', 'jump', 'group']);
    return {
      profile: {
        id: crypto.randomUUID(),
        name: text(item.name, 160),
        target: endpoint(item.target),
        jump: item.jump === null ? null : endpoint(item.jump),
      },
      group: text(item.group, 40, true),
    };
  });
}
