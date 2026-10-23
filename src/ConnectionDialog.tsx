import ErrorNotice from './ErrorNotice';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Monitor, X, Server, LockKeyhole, ShieldCheck, Download, Trash2 } from 'lucide-react';
import { Profile, Secrets, ConnectRequest, emptyEndpoint, emptySecrets, message } from './types';
import EndpointFields from './ConnectionEndpointFields';
import './connectionDialog.css';
export default function ConnectionDialog({
  initial,
  busy,
  recoveryTask,
  onClose,
  onSave,
  onConnect,
}: {
  initial?: Profile;
  busy: boolean;
  recoveryTask?: string | null;
  onClose: () => void;
  onSave: (p: Profile) => Promise<void>;
  onConnect: (r: ConnectRequest) => Promise<void>;
}) {
  const formRef = useRef<HTMLFormElement>(null);
  const [profile, setProfile] = useState<Profile>(
    initial ?? { id: crypto.randomUUID(), name: '', target: emptyEndpoint(), jump: null },
  );
  const [targetSecrets, setTargetSecrets] = useState(emptySecrets());
  const [jumpSecrets, setJumpSecrets] = useState(emptySecrets());
  const [error, setError] = useState('');
  const [remember, setRemember] = useState(false);
  const [credentialSupport, setCredentialSupport] = useState(false);
  useEffect(() => {
    let disposed = false;
    void invoke<boolean>('credential_support')
      .then((supported) => {
        if (!disposed) setCredentialSupport(supported);
      })
      .catch(() => {});
    return () => {
      disposed = true;
    };
  }, []);
  const [saving, setSaving] = useState(false);
  const submit = async (connect: boolean) => {
    setError('');
    setSaving(true);
    try {
      const normalized = {
        ...profile,
        name: profile.name.trim(),
        target: {
          ...profile.target,
          host: profile.target.host.trim(),
          username: profile.target.username.trim(),
        },
        jump: profile.jump
          ? {
              ...profile.jump,
              host: profile.jump.host.trim(),
              username: profile.jump.username.trim(),
            }
          : null,
      };
      if (
        initial &&
        JSON.stringify([initial.target, initial.jump]) !==
          JSON.stringify([normalized.target, normalized.jump])
      ) {
        await invoke('delete_credentials', { profile: initial });
      }
      await onSave(normalized);
      if (remember)
        await invoke('save_credentials', {
          profile: normalized,
          secrets: { targetSecrets, jumpSecrets },
        });
      if (connect) await onConnect({ profile: normalized, targetSecrets, jumpSecrets });
      else onClose();
    } catch (e) {
      setError(message(e));
    } finally {
      setSaving(false);
    }
  };
  return (
    <div className="modal-backdrop">
      <section
        className="modal connection-modal connection-design"
        role="dialog"
        aria-modal="true"
        aria-labelledby="connection-title"
      >
        <header className="connection-header">
          <div className="connection-heading-icon">
            <Monitor size={23} />
          </div>
          <div>
            <h2 id="connection-title">{initial ? '连接服务器' : '新建连接'}</h2>
            <p>
              {recoveryTask
                ? `连接成功后恢复“${recoveryTask}”；续传前会核对已有内容`
                : '配置服务器与认证信息'}
            </p>
          </div>
          <button
            type="button"
            className="connection-close"
            aria-label="关闭"
            disabled={busy || saving}
            onClick={onClose}
          >
            <X size={20} />
          </button>
        </header>
        <form
          ref={formRef}
          onSubmit={(e) => {
            e.preventDefault();
            void submit(true);
          }}
        >
          <div className="connection-body">
            <fieldset disabled={busy || saving}>
              <h3 className="connection-section-title">
                <Monitor size={18} />
                目标服务器
              </h3>
              <label>
                连接名称
                <input
                  required
                  value={profile.name}
                  onChange={(e) => setProfile({ ...profile, name: e.target.value })}
                  placeholder="生产服务器"
                  autoFocus
                />
              </label>
              <EndpointFields
                endpoint={profile.target}
                secrets={targetSecrets}
                onEndpoint={(target) => setProfile({ ...profile, target })}
                onSecrets={setTargetSecrets}
                onError={setError}
              />
              <div className="connection-jump-card">
                <label className="connection-toggle-row">
                  <Server size={22} />
                  <span>
                    <strong>通过 SSH 堡垒机连接</strong>
                    <small>经跳板主机访问目标服务器</small>
                  </span>
                  <input
                    type="checkbox"
                    role="switch"
                    aria-label="通过 SSH 堡垒机连接"
                    checked={!!profile.jump}
                    onChange={(e) => {
                      setProfile({ ...profile, jump: e.target.checked ? emptyEndpoint() : null });
                      setJumpSecrets(emptySecrets());
                    }}
                  />
                </label>
                {profile.jump && (
                  <div className="connection-jump-fields">
                    <h3 className="connection-section-title">
                      <Server size={16} />
                      堡垒机信息<small>堡垒机 → 目标服务器</small>
                    </h3>
                    <EndpointFields
                      endpoint={profile.jump}
                      secrets={jumpSecrets}
                      onEndpoint={(jump) => setProfile({ ...profile, jump })}
                      onSecrets={setJumpSecrets}
                      onError={setError}
                    />
                  </div>
                )}
              </div>
              <div className="connection-credentials">
                <label className="connection-toggle-row">
                  <LockKeyhole size={22} />
                  <span>
                    <strong>系统凭据库</strong>
                    <small>
                      {credentialSupport
                        ? '保存密码或私钥口令，供下次连接使用'
                        : '当前环境不支持系统凭据库'}
                    </small>
                  </span>
                  <input
                    type="checkbox"
                    role="switch"
                    aria-label="保存密码或私钥口令到系统凭据库"
                    checked={remember}
                    disabled={busy || saving || !credentialSupport}
                    onChange={(e) => setRemember(e.target.checked)}
                  />
                </label>
                <div className="connection-credential-actions">
                  <button
                    type="button"
                    disabled={busy || saving || !credentialSupport}
                    onClick={() => {
                      setSaving(true);
                      void invoke<{ targetSecrets: Secrets; jumpSecrets: Secrets } | null>(
                        'load_credentials',
                        { profile },
                      )
                        .then((value) => {
                          if (value) {
                            setTargetSecrets(value.targetSecrets);
                            setJumpSecrets(value.jumpSecrets);
                            setError('');
                          } else setError('此连接没有保存的凭据');
                        })
                        .catch((e) => setError(message(e)))
                        .finally(() => setSaving(false));
                    }}
                  >
                    <Download size={16} />
                    读取凭据
                  </button>
                  <button
                    type="button"
                    disabled={busy || saving || !credentialSupport}
                    onClick={() => {
                      setSaving(true);
                      void invoke('delete_credentials', { profile })
                        .then(() => {
                          setTargetSecrets(emptySecrets());
                          setJumpSecrets(emptySecrets());
                          setRemember(false);
                          setError('');
                        })
                        .catch((e) => setError(message(e)))
                        .finally(() => setSaving(false));
                    }}
                  >
                    <Trash2 size={16} />
                    移除凭据
                  </button>
                </div>
              </div>
            </fieldset>
          </div>
          {error && (
            <ErrorNotice
              error={error}
              scope="连接"
              disabled={busy || saving}
              recovery={{
                label: '重新连接',
                run: () => {
                  if (formRef.current?.reportValidity()) void submit(true);
                },
              }}
            />
          )}
          <footer className="connection-footer">
            <span>
              <ShieldCheck size={17} />
              配置文件不保存密码或口令
            </span>
            <div className="connection-footer-actions">
              <button
                type="button"
                disabled={busy || saving}
                onClick={() => {
                  const form = formRef.current;
                  if (form?.reportValidity()) void submit(false);
                }}
              >
                保存配置
              </button>
              <button type="submit" className="primary" disabled={busy || saving}>
                {busy || saving ? '正在连接…' : '保存并连接'}
              </button>
            </div>
          </footer>
        </form>
      </section>
    </div>
  );
}
