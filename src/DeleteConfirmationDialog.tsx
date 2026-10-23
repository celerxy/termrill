/** 删除确认只负责展示；实际删除与部分失败重试仍由 FileActions 管理。 */
import {
  AlertTriangle,
  File,
  Folder,
  Link2,
  LoaderCircle,
  ShieldCheck,
  Trash2,
  X,
} from 'lucide-react';
import { basename, type FileEntry } from './types';
import './deleteConfirmation.css';

type Props = {
  paths: string[];
  remote: boolean;
  entries: FileEntry[];
  recursive: boolean;
  busy: boolean;
  error: string;
  elapsedMs: number;
  phase: 'deleting' | 'refreshing' | null;
  detail: string;
  stageElapsedMs: number;
  history: string;
  onRecursiveChange: (value: boolean) => void;
  onClose: () => void;
  onConfirm: () => void;
};

export default function DeleteConfirmationDialog({
  paths,
  remote,
  entries,
  recursive,
  busy,
  error,
  elapsedMs,
  phase,
  detail,
  stageElapsedMs,
  history,
  onRecursiveChange,
  onClose,
  onConfirm,
}: Props) {
  const entryByPath = new Map(entries.map((entry) => [entry.path, entry]));
  return (
    <div className="modal-backdrop">
      <section
        className="modal delete-confirmation"
        role="dialog"
        aria-modal="true"
        aria-labelledby="delete-confirmation-title"
        aria-describedby="delete-confirmation-warning"
      >
        <header className="delete-confirmation-header">
          <div className="delete-title-icon">
            <Trash2 size={24} />
          </div>
          <div>
            <h2 id="delete-confirmation-title">确认永久删除</h2>
            <p>请确认以下项目，删除后无法恢复</p>
          </div>
          <button
            type="button"
            className="delete-close"
            aria-label="关闭删除确认"
            disabled={busy}
            onClick={onClose}
          >
            <X size={20} />
          </button>
        </header>
        <div className="delete-confirmation-body">
          <div className="delete-warning" id="delete-confirmation-warning">
            <AlertTriangle size={19} />
            <span>不进入回收站，已删除的内容无法恢复。</span>
          </div>
          <div className="delete-items" aria-label="待删除项目">
            {paths.map((path) => {
              const entry = entryByPath.get(path);
              const Icon = entry?.symlink ? Link2 : entry?.directory ? Folder : File;
              const kind = entry
                ? entry.symlink
                  ? '链接'
                  : entry.directory
                    ? '文件夹'
                    : '文件'
                : '项目';
              // 仅拆分显示路径，不修改提交后端的原路径，也不探测链接目标。
              const separator = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
              const parent = separator >= 0 ? path.slice(0, separator + 1) : path;
              return (
                <article className="delete-item" key={path}>
                  <div
                    className={`delete-item-icon ${entry?.directory && !entry.symlink ? 'is-folder' : ''}`}
                  >
                    <Icon size={27} />
                  </div>
                  <div className="delete-item-info">
                    <div className="delete-item-heading">
                      <strong>{entry?.name ?? basename(path)}</strong>
                      <span>{kind}</span>
                    </div>
                    <small>{remote ? '远程位置' : '本地位置'}</small>
                    <div className="delete-item-path">{parent}</div>
                  </div>
                </article>
              );
            })}
          </div>
          <label className="delete-recursive-option">
            <input
              type="checkbox"
              checked={recursive}
              disabled={busy}
              onChange={(event) => onRecursiveChange(event.target.checked)}
            />
            <span>
              <strong>同时删除文件夹内的全部内容</strong>
              <small>
                {recursive
                  ? '目录内容将永久删除；链接只删除自身。'
                  : '未勾选时，仅删除文件和空目录。'}
              </small>
            </span>
          </label>
          <p className="delete-selection-count" aria-live="polite">
            已选择 {paths.length} 项
          </p>
          {(phase || elapsedMs > 0) && (
            <div className="delete-operation-time" role="timer" aria-label="删除操作用时">
              <div className="delete-operation-heading">
                {phase && <LoaderCircle size={16} className="delete-spinner" />}
                <span>
                  {phase === 'refreshing'
                    ? '刷新列表中'
                    : phase
                      ? detail || '正在删除'
                      : '本次操作已结束'}
                </span>
                <strong>总用时 {(elapsedMs / 1000).toFixed(1)} 秒</strong>
              </div>
              {phase && (
                <div className="delete-current-stage">
                  当前阶段已用 {(stageElapsedMs / 1000).toFixed(1)} 秒
                </div>
              )}
              {history && (
                <details className="delete-stage-history">
                  <summary>已完成阶段耗时</summary>
                  <p>{history}</p>
                </details>
              )}
            </div>
          )}
          {error && (
            <div className="delete-error" role="alert">
              {error}
              <small>部分项目可能已删除；重试只处理剩余项目。</small>
            </div>
          )}
        </div>
        <footer className="delete-confirmation-footer">
          <span>
            <ShieldCheck size={17} />
            永久删除，请谨慎操作
          </span>
          <div>
            <button type="button" disabled={busy} onClick={onClose}>
              取消
            </button>
            <button
              type="button"
              className="delete-danger"
              disabled={busy || paths.length === 0}
              onClick={onConfirm}
            >
              {busy ? <LoaderCircle size={17} className="delete-spinner" /> : <Trash2 size={17} />}
              {busy ? '删除中…' : '永久删除'}
            </button>
          </div>
        </footer>
      </section>
    </div>
  );
}
