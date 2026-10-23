/** 目标与堡垒机共用的认证表单；切换认证时清除内存中的旧凭据。 */
import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { FolderOpen, Eye, EyeOff } from 'lucide-react';
import { Endpoint, Secrets, emptySecrets, message } from './types';
/** 显示状态只属于当前输入框，认证方式切换会卸载并恢复遮罩。 */
function SecretField({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const [visible, setVisible] = useState(false);
  return (
    <div className="connection-secret-input">
      <input
        type={visible ? 'text' : 'password'}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        autoComplete="off"
      />
      <button
        type="button"
        aria-label={visible ? '隐藏密码或口令' : '显示密码或口令'}
        aria-pressed={visible}
        onClick={() => setVisible(!visible)}
      >
        {visible ? <EyeOff size={17} /> : <Eye size={17} />}
      </button>
    </div>
  );
}
export default function EndpointFields({
  endpoint,
  secrets,
  onEndpoint,
  onSecrets,
  onError,
}: {
  endpoint: Endpoint;
  secrets: Secrets;
  onEndpoint: (e: Endpoint) => void;
  onSecrets: (s: Secrets) => void;
  onError: (e: string) => void;
}) {
  const set = (patch: Partial<Endpoint>) => onEndpoint({ ...endpoint, ...patch });
  return (
    <>
      <div className="connection-host-row">
        <label>
          主机地址
          <input
            required
            value={endpoint.host}
            onChange={(e) => set({ host: e.target.value })}
            placeholder="192.168.1.10"
            autoComplete="off"
          />
        </label>
        <label>
          端口
          <input
            required
            min="1"
            max="65535"
            value={endpoint.port}
            onChange={(e) => set({ port: Number(e.target.value) })}
            type="number"
          />
        </label>
      </div>
      <div className="connection-auth-row">
        <label>
          用户名
          <input
            required
            value={endpoint.username}
            onChange={(e) => set({ username: e.target.value })}
            placeholder="deploy"
            autoComplete="off"
          />
        </label>
        <label>
          认证方式
          <select
            value={endpoint.auth}
            onChange={(e) => {
              set({ auth: e.target.value as Endpoint['auth'] });
              onSecrets(emptySecrets());
            }}
          >
            <option value="password">密码</option>
            <option value="key">私钥文件</option>
            <option value="agent">SSH Agent</option>
            <option value="interactive">交互式认证（密码/验证码）</option>
          </select>
        </label>
      </div>
      {endpoint.auth === 'password' && (
        <label>
          密码 · 仅用于本次连接
          <SecretField
            value={secrets.password}
            onChange={(value) => onSecrets({ ...secrets, password: value })}
          />
        </label>
      )}
      {endpoint.auth === 'key' && (
        <>
          <label>
            私钥路径
            <div className="key-input">
              <input
                required
                value={endpoint.keyPath}
                onChange={(e) => set({ keyPath: e.target.value })}
                placeholder="选择 RSA PEM 私钥（Windows）"
              />
              <button
                type="button"
                aria-label="选择私钥"
                onClick={async () => {
                  try {
                    const paths = await invoke<string[]>('pick_path', { kind: 'key' });
                    if (paths[0]) set({ keyPath: paths[0] });
                  } catch (e) {
                    onError(message(e));
                  }
                }}
              >
                <FolderOpen size={17} />
              </button>
            </div>
          </label>
          <label>
            私钥口令 · 可留空
            <SecretField
              value={secrets.passphrase}
              onChange={(value) => onSecrets({ ...secrets, passphrase: value })}
            />
          </label>
        </>
      )}
      {endpoint.auth === 'agent' && (
        <p className="form-hint">使用本机 SSH Agent 中已加载的密钥。</p>
      )}
    </>
  );
}
