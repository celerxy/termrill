/** 批量权限操作绑定打开时的连接与路径，返回逐项结果；不声称失败后可回滚。 */
import { useEffect, useRef, useState } from 'react';
import { Channel, invoke } from '@tauri-apps/api/core';
import { File, ShieldCheck, X, Link2, AlertTriangle, LoaderCircle } from 'lucide-react';
import { basename, message } from './types';
import './batchPermissions.css';

type Result = {
  changed: number;
  skipped: number;
  unchanged: number;
  checked: number;
  cancelled: boolean;
  errors: string[];
  engine: 'remote' | 'sftp' | 'sftp-parallel';
  fallbackReason: string | null;
  workers: number;
};
type Progress = {
  checked: number;
  changed: number;
  unchanged: number;
  skipped: number;
  failed: number;
  elapsedMs: number;
  phase: string;
  fallbackReason: string | null;
  workers: number;
};
export default function BatchPermissions({
  id,
  paths,
  onClose,
  onChanged,
}: {
  id: string;
  paths: string[];
  onClose: () => void;
  onChanged: () => void;
}) {
  const taskRef = useRef<string | null>(null);
  useEffect(
    () => () => {
      const taskId = taskRef.current;
      taskRef.current = null;
      if (taskId) void invoke('cancel_transfer', { id: taskId }).catch(() => {});
    },
    [],
  );
  const [progress, setProgress] = useState<Progress | null>(null);
  const [stopping, setStopping] = useState(false);
  const [value, setValue] = useState('');
  const [operation, setOperation] = useState<'mode' | 'owner'>('mode');
  const [uid, setUid] = useState('');
  const [gid, setGid] = useState('');
  const [recursive, setRecursive] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<Result | null>(null);
  const [error, setError] = useState('');
  const valid =
    operation === 'mode'
      ? /^[0-7]{3}$/.test(value)
      : [uid, gid].every((text) => /^\d{1,10}$/.test(text) && Number(text) <= 0xffffffff);
  const apply = async () => {
    if (!valid || busy) return;
    const taskId = crypto.randomUUID();
    taskRef.current = taskId;
    setProgress(null);
    setStopping(false);
    const onProgress = new Channel<Progress>();
    onProgress.onmessage = (value) => {
      if (taskRef.current === taskId) setProgress(value);
    };
    setBusy(true);
    setResult(null);
    setError('');
    try {
      setResult(
        await invoke<Result>(
          operation === 'mode' ? 'set_remote_permissions_batch' : 'set_remote_owner_batch',
          {
            id,
            taskId,
            onProgress,
            paths,
            ...(operation === 'mode'
              ? { mode: parseInt(value, 8) }
              : { owner: { uid: Number(uid), gid: Number(gid) } }),
            recursive,
          },
        ),
      );
      onChanged();
    } catch (e) {
      setError(message(e));
    } finally {
      taskRef.current = null;
      setBusy(false);
      setStopping(false);
    }
  };
  return (
    <div className="modal-backdrop">
      <section
        className="modal permissions-modal batch-permissions-design"
        role="dialog"
        aria-modal="true"
        aria-label="批量修改权限"
      >
        <header className="batch-permissions-header">
          <div className="batch-title-icon">
            <ShieldCheck size={24} />
          </div>
          <div>
            <h2>批量修改权限</h2>
            <p>为所选项目统一设置权限或所有者</p>
          </div>
          <button type="button" aria-label="关闭批量权限" disabled={busy} onClick={onClose}>
            <X size={20} />
          </button>
        </header>
        <div className="batch-permissions-body">
          <section className="batch-selection-card" aria-label="所选项目">
            <h3>
              所选项目<span>{paths.length} 项</span>
            </h3>
            <div className="batch-selected-items">
              {paths.map((path) => (
                <div className="batch-selected-item" key={path}>
                  <File size={24} />
                  <div>
                    <strong>{basename(path)}</strong>
                    <p>{path}</p>
                  </div>
                </div>
              ))}
            </div>
          </section>
          <h3 className="batch-operation-title">修改内容</h3>
          <div className="batch-operation-switch" role="group" aria-label="修改内容">
            <button
              type="button"
              aria-pressed={operation === 'mode'}
              disabled={busy}
              onClick={() => {
                setOperation('mode');
                setResult(null);
              }}
            >
              读写与执行权限
            </button>
            <button
              type="button"
              aria-pressed={operation === 'owner'}
              disabled={busy}
              onClick={() => {
                setOperation('owner');
                setResult(null);
              }}
            >
              所有者与用户组
            </button>
          </div>
          <div className="batch-fields">
            {operation === 'mode' ? (
              <label className="batch-field">
                八进制（000–777）
                <input
                  aria-label="批量八进制权限"
                  inputMode="numeric"
                  maxLength={3}
                  disabled={busy}
                  value={value}
                  onChange={(e) => {
                    setValue(e.target.value);
                    setResult(null);
                  }}
                  placeholder="例如 755"
                />
              </label>
            ) : (
              <>
                <label className="batch-field">
                  用户 UID
                  <input
                    aria-label="批量用户 UID"
                    value={uid}
                    disabled={busy}
                    inputMode="numeric"
                    maxLength={10}
                    onChange={(e) => {
                      setUid(e.target.value);
                      setResult(null);
                    }}
                  />
                </label>
                <label className="batch-field">
                  组 GID
                  <input
                    aria-label="批量组 GID"
                    value={gid}
                    disabled={busy}
                    inputMode="numeric"
                    maxLength={10}
                    onChange={(e) => {
                      setGid(e.target.value);
                      setResult(null);
                    }}
                  />
                </label>
              </>
            )}
            {operation === 'mode' && (
              <div className="batch-mode-preview">
                权限预览
                <code>
                  {/^[0-7]{3}$/.test(value)
                    ? [8, 7, 6, 5, 4, 3, 2, 1, 0]
                        .map((bit, index) =>
                          parseInt(value, 8) & (1 << bit) ? 'rwx'[index % 3] : '-',
                        )
                        .join('')
                    : '待填写'}
                </code>
              </div>
            )}
          </div>
          <p className="batch-field-hint">
            {operation === 'mode'
              ? '范围 000–777 · 文件和目录使用同一权限，保留特殊权限位'
              : '同时填写 UID 与 GID；范围 0–4294967295'}
          </p>
          <label className="batch-recursive">
            <input
              type="checkbox"
              checked={recursive}
              disabled={busy}
              onChange={(e) => {
                setRecursive(e.target.checked);
                setResult(null);
              }}
            />
            <span>
              <strong>包含所有子文件和子目录</strong>
              <small>
                {recursive ? '将修改所选项目及其子文件、子目录' : '未勾选时，仅修改所选项目'}
              </small>
            </span>
          </label>
          <div className="batch-skip-hint">
            <Link2 size={16} />
            跳过符号链接和特殊文件
          </div>
          <div className="batch-warning">
            <AlertTriangle size={20} />
            <div>
              <p>
                {operation === 'mode'
                  ? '移除目录执行权限可能导致无法进入目录。'
                  : '修改所有者需要服务器授权，可能清除 setuid/setgid。'}
              </p>
              <p>批量操作可能部分成功，无法自动回滚。</p>
            </div>
          </div>
          {busy && (
            <div className="batch-live-progress" role="status">
              {progress
                ? `${progress.phase} · ${progress.workers} 个工作连接 · 已检查 ${progress.checked} 项 · 已修改 ${progress.changed} · 未变化 ${progress.unchanged} · 跳过 ${progress.skipped} · 失败 ${progress.failed} · ${(progress.elapsedMs / 1000).toFixed(1)} 秒`
                : '正在建立权限操作通道…'}
              {progress?.fallbackReason && <div>加速路径未启用：{progress.fallbackReason}</div>}
            </div>
          )}
          {result && (
            <div role="status">
              <p>
                {result.cancelled ? '已停止；' : '已完成；'}
                {result.engine === 'remote'
                  ? '远端批量处理，'
                  : `SFTP ${result.workers} 个独立连接，`}
                修改 {result.changed} 项，未变化 {result.unchanged} 项，跳过 {result.skipped}{' '}
                项，失败 {result.errors.length} 项。
              </p>
              {result.fallbackReason && <p>加速路径未启用：{result.fallbackReason}</p>}
              <div className="action-paths">
                {result.errors.map((item, index) => (
                  <p className="inline-error" key={index}>
                    {item}
                  </p>
                ))}
              </div>
            </div>
          )}
          {error && (
            <p className="inline-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="batch-permissions-footer">
          <span>
            <ShieldCheck size={17} />
            仅对确认范围应用修改
          </span>
          <div>
            <button
              disabled={stopping}
              onClick={() => {
                if (!busy) {
                  onClose();
                  return;
                }
                const taskId = taskRef.current;
                if (!taskId) return;
                setStopping(true);
                void invoke('cancel_transfer', { id: taskId }).catch((error) => {
                  setError(message(error));
                  setStopping(false);
                });
              }}
            >
              {busy ? (stopping ? '等待当前批次结束…' : '停止操作') : '关闭'}
            </button>
            <button
              className="primary"
              disabled={busy || !valid || !!result}
              onClick={() => void apply()}
            >
              {busy && <LoaderCircle size={16} className="batch-spinner" />}
              {busy ? '修改中…' : operation === 'mode' ? '确认应用权限' : '确认应用所有者'}
            </button>
          </div>
        </footer>
      </section>
    </div>
  );
}
