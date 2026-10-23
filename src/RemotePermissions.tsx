import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  Folder,
  File,
  Link2,
  SlidersHorizontal,
  UserRound,
  ShieldCheck,
  X,
  LoaderCircle,
} from 'lucide-react';
import './remotePermissions.css';
import { basename, formatSize, message } from './types';

type Properties = {
  path: string;
  kind: 'file' | 'directory' | 'symlink' | 'other';
  mode: number | null;
  uid: number | null;
  gid: number | null;
  size: number | null;
  modified: number | null;
  editable: boolean;
};
const octal = (mode: number) => mode.toString(8).padStart(3, '0');
const symbolic = (mode: number) => {
  const text = [8, 7, 6, 5, 4, 3, 2, 1, 0].map((bit, index) =>
    mode & (1 << bit) ? 'rwx'[index % 3] : '-',
  );
  if (mode & 0o4000) text[2] = mode & 0o100 ? 's' : 'S';
  if (mode & 0o2000) text[5] = mode & 0o010 ? 's' : 'S';
  if (mode & 0o1000) text[8] = mode & 0o001 ? 't' : 'T';
  return text.join('');
};

/** 弹窗固定绑定打开时的连接与路径；不将权限修改发送到随后切换的会话。 */
export default function RemotePermissions({
  id,
  path,
  onClose,
  onChanged,
}: {
  id: string;
  path: string;
  onClose: () => void;
  onChanged: () => void;
}) {
  const [properties, setProperties] = useState<Properties | null>(null);
  const [value, setValue] = useState('');
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState('');
  const [saved, setSaved] = useState(false);
  const [uid, setUid] = useState('');
  const [gid, setGid] = useState('');
  useEffect(() => {
    let disposed = false;
    setBusy(true);
    setError('');
    void invoke<Properties>('remote_properties', { id, path })
      .then((result) => {
        if (disposed) return;
        setProperties(result);
        setUid(result.uid === null ? '' : String(result.uid));
        setGid(result.gid === null ? '' : String(result.gid));
        setValue(result.mode === null ? '' : octal(result.mode & 0o777));
      })
      .catch((e) => {
        if (!disposed) setError(message(e));
      })
      .finally(() => {
        if (!disposed) setBusy(false);
      });
    return () => {
      disposed = true;
    };
  }, [id, path]);
  const valid = /^[0-7]{3}$/.test(value);
  const validOwner = [uid, gid].every(
    (text) => /^\d{1,10}$/.test(text) && Number(text) <= 0xffffffff,
  );
  const ownerChanged =
    properties && (Number(uid) !== properties.uid || Number(gid) !== properties.gid);
  const mode = valid ? parseInt(value, 8) : 0;
  const toggle = (mask: number) => {
    setValue(octal(mode ^ mask));
    setSaved(false);
  };
  const save = async () => {
    if (!properties?.editable || properties.mode === null || !valid) return;
    setBusy(true);
    setError('');
    setSaved(false);
    try {
      const result = await invoke<Properties>('set_remote_permissions', {
        id,
        path,
        mode,
        expectedMode: properties.mode,
      });
      setProperties(result);
      setValue(result.mode === null ? '' : octal(result.mode & 0o777));
      setSaved(true);
      onChanged();
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  const saveOwner = async () => {
    if (!properties?.editable || properties.uid === null || properties.gid === null || !validOwner)
      return;
    setBusy(true);
    setError('');
    setSaved(false);
    try {
      const result = await invoke<Properties>('set_remote_owner', {
        id,
        path,
        uid: Number(uid),
        gid: Number(gid),
        expectedUid: properties.uid,
        expectedGid: properties.gid,
      });
      setProperties(result);
      // chown 可能由服务器清除特殊权限位，显示重新读取的实际 mode。
      setValue(result.mode === null ? '' : octal(result.mode & 0o777));
      setSaved(true);
      onChanged();
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="modal-backdrop">
      <section
        className="modal permissions-modal permissions-design"
        role="dialog"
        aria-modal="true"
        aria-label="远程属性与权限"
      >
        <header className="permissions-header">
          <div className="permissions-title-icon">
            <SlidersHorizontal size={24} />
          </div>
          <div>
            <h2>属性与权限</h2>
            <p>查看文件信息，管理远程访问权限</p>
          </div>
          <button type="button" aria-label="关闭属性与权限" disabled={busy} onClick={onClose}>
            <X size={20} />
          </button>
        </header>
        <div className="permissions-body">
          <div className="permissions-file-card">
            <div
              className={`permissions-file-icon ${properties?.kind === 'directory' ? 'is-folder' : ''}`}
            >
              {properties?.kind === 'directory' ? (
                <Folder size={30} />
              ) : properties?.kind === 'symlink' ? (
                <Link2 size={30} />
              ) : (
                <File size={30} />
              )}
            </div>
            <div>
              <strong>{basename(path)}</strong>
              {properties && (
                <span className="permissions-kind">
                  {
                    { file: '文件', directory: '文件夹', symlink: '符号链接', other: '特殊项目' }[
                      properties.kind
                    ]
                  }
                </span>
              )}
              <p>{path}</p>
            </div>
          </div>
          {properties && (
            <>
              <dl className="permissions-metadata">
                <div>
                  <dt>大小</dt>
                  <dd>{properties.size === null ? '服务器未提供' : formatSize(properties.size)}</dd>
                </div>
                <div>
                  <dt>修改时间</dt>
                  <dd>
                    {properties.modified === null
                      ? '服务器未提供'
                      : new Date(properties.modified * 1000).toLocaleString()}
                  </dd>
                </div>
                <div>
                  <dt>用户 UID / 组 GID</dt>
                  <dd>
                    {properties.uid ?? '未知'} / {properties.gid ?? '未知'}
                  </dd>
                </div>
                <div>
                  <dt>当前权限</dt>
                  <dd>
                    {properties.mode === null ? (
                      '服务器未提供'
                    ) : (
                      <>
                        <code>{symbolic(properties.mode)}</code>
                        <span className="permissions-mode-badge">{octal(properties.mode)}</span>
                      </>
                    )}
                  </dd>
                </div>
              </dl>
              {properties.editable && properties.uid !== null && properties.gid !== null && (
                <fieldset disabled={busy} className="permissions-owner-card">
                  <legend className="permissions-section-title">
                    <UserRound size={20} />
                    所有者与用户组<span>仅修改当前项目</span>
                  </legend>
                  <div className="permissions-owner-row">
                    <label className="permission-octal">
                      用户 UID
                      <input
                        aria-label="用户 UID"
                        inputMode="numeric"
                        maxLength={10}
                        value={uid}
                        onChange={(e) => {
                          setUid(e.target.value);
                          setSaved(false);
                        }}
                      />
                    </label>
                    <label className="permission-octal">
                      组 GID
                      <input
                        aria-label="组 GID"
                        inputMode="numeric"
                        maxLength={10}
                        value={gid}
                        onChange={(e) => {
                          setGid(e.target.value);
                          setSaved(false);
                        }}
                      />
                    </label>
                    <button
                      disabled={!validOwner || !ownerChanged}
                      onClick={() => void saveOwner()}
                    >
                      应用 UID / GID
                    </button>
                  </div>
                  <p>更改所有者可能需要管理员权限，并可能清除 setuid/setgid。</p>
                </fieldset>
              )}
              <fieldset
                disabled={busy || !properties.editable}
                className="permissions-grid permissions-mode-card"
              >
                <legend className="permissions-section-title">
                  <ShieldCheck size={20} />
                  读写与执行权限<span>仅当前项目</span>
                </legend>
                <table>
                  <thead>
                    <tr>
                      <th>角色</th>
                      <th>读取 r</th>
                      <th>写入 w</th>
                      <th>{properties.kind === 'directory' ? '进入 x' : '执行 x'}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {['所有者', '用户组', '其他用户'].map((role, row) => (
                      <tr key={role}>
                        <th>{role}</th>
                        {[4, 2, 1].map((bit) => {
                          const mask = bit << ((2 - row) * 3);
                          return (
                            <td key={bit}>
                              <input
                                type="checkbox"
                                aria-label={`${role}${bit === 4 ? '读取' : bit === 2 ? '写入' : '执行或进入'}`}
                                checked={!!(mode & mask)}
                                disabled={!valid}
                                onChange={() => toggle(mask)}
                              />
                            </td>
                          );
                        })}
                      </tr>
                    ))}
                  </tbody>
                </table>
                <div className="permissions-mode-row">
                  <label className="permission-octal">
                    八进制（000–777）
                    <input
                      aria-label="八进制权限"
                      inputMode="numeric"
                      maxLength={3}
                      value={value}
                      onChange={(e) => {
                        setValue(e.target.value);
                        setSaved(false);
                      }}
                    />
                  </label>
                  {valid && properties.editable && (
                    <span className="permissions-preview">
                      将应用<code>{symbolic(((properties.mode ?? 0) & 0o7000) | mode)}</code>
                    </span>
                  )}
                </div>
                <p>不递归；保留特殊权限位，修改结果取决于远程权限。</p>
              </fieldset>
              {!properties.editable && <p>此项目仅支持查看属性。</p>}
            </>
          )}
          {busy && (
            <p role="status" className="permissions-processing">
              <LoaderCircle size={17} />
              正在处理…
            </p>
          )}
          {error && (
            <p className="inline-error" role="alert">
              {error}
            </p>
          )}
          {saved && (
            <p className="success" role="status">
              属性已保存并重新读取。
            </p>
          )}
        </div>
        <footer className="permissions-footer">
          <span>
            <ShieldCheck size={17} />
            更改仅作用于当前项目
          </span>
          <div>
            <button disabled={busy} onClick={onClose}>
              关闭
            </button>
            <button
              className="primary"
              disabled={
                busy || !properties?.editable || !valid || mode === ((properties.mode ?? 0) & 0o777)
              }
              onClick={() => void save()}
            >
              应用权限
            </button>
          </div>
        </footer>
      </section>
    </div>
  );
}
