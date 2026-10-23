import { useEffect, useRef } from 'react';
import {
  ArrowRight,
  CloudDownload,
  CloudUpload,
  Download,
  ListChecks,
  Monitor,
  Server,
  ShieldCheck,
  Upload,
  X,
} from 'lucide-react';
import type { TransferPlan } from './types';
import './transferConfirm.css';

type Props = {
  plans: TransferPlan[];
  overwrite: boolean;
  onOverwriteChange: (overwrite: boolean) => void;
  canStart: boolean;
  onClose: () => void;
  onStart: () => void;
};

/** 仅确认既有传输计划；覆盖选项由工作区管理，点击开始后交给原有队列流程。 */
export default function TransferConfirmDialog({
  plans,
  overwrite,
  onOverwriteChange,
  canStart,
  onClose,
  onStart,
}: Props) {
  const dialog = useRef<HTMLElement>(null);
  useEffect(() => {
    const previousFocus = document.activeElement;
    dialog.current?.focus();
    return () => {
      if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus();
    };
  }, []);
  const firstPlan = plans[0];
  if (!firstPlan) return null;
  const uploading = firstPlan.direction === 'upload';
  const UploadIcon = uploading ? CloudUpload : CloudDownload;
  const ActionIcon = uploading ? Upload : Download;
  const selectedCount = plans.reduce((count, plan) => count + (plan.selection?.length ?? 1), 0);
  const syncing = plans.some((plan) => plan.selection !== undefined);
  return (
    <div className="modal-backdrop">
      <section
        className="modal transfer-confirm"
        ref={dialog}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby="transfer-confirm-title"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation();
            onClose();
          }
          if (event.key === 'Tab') {
            // 只将选中的原生单选框纳入 Tab 序列；方向键仍可切换覆盖方式。
            const controls = [
              ...event.currentTarget.querySelectorAll<HTMLElement>(
                'button:not(:disabled), input:checked:not(:disabled)',
              ),
            ];
            const first = controls[0];
            const last = controls.at(-1);
            if (
              event.shiftKey &&
              (document.activeElement === first || document.activeElement === dialog.current)
            ) {
              event.preventDefault();
              last?.focus();
            } else if (!event.shiftKey && document.activeElement === last) {
              event.preventDefault();
              first?.focus();
            }
          }
        }}
      >
        <header className="transfer-confirm-heading">
          <span className="transfer-confirm-icon">
            <UploadIcon size={30} />
          </span>
          <div>
            <h2 id="transfer-confirm-title">{uploading ? '上传到服务器' : '下载到本地'}</h2>
            <p>确认传输目标与同名文件处理方式</p>
          </div>
          <button className="transfer-confirm-close" aria-label="关闭传输确认" onClick={onClose}>
            <X size={21} />
          </button>
        </header>
        <div className="transfer-confirm-content">
          <div className="transfer-confirm-summary">
            <ListChecks size={27} />
            <div>
              <strong>{selectedCount} 项已选择</strong>
              <p>
                {syncing
                  ? '目录差异同步 · 未选择的项目将保留'
                  : '文件与目录传输 · 文件夹会递归传输'}
              </p>
            </div>
          </div>
          <div className="transfer-confirm-targets" aria-label="传输目标">
            {plans.map((plan, index) => {
              const targetPath = plan.direction === 'upload' ? plan.remote : plan.local;
              const TargetIcon = plan.direction === 'upload' ? Server : Monitor;
              return (
                <div className="transfer-confirm-target" key={`${index}:${targetPath}`}>
                  <span className="transfer-target-icon">
                    <TargetIcon size={25} />
                  </span>
                  <div>
                    <small>{plan.selection ? '目标目录' : '目标路径'}</small>
                    <strong title={targetPath}>{targetPath}</strong>
                    {!plan.selection && (
                      <span className="transfer-target-name" title={plan.name}>
                        {plan.name}
                      </span>
                    )}
                  </div>
                  <span className="transfer-direction-tag">
                    <ArrowRight size={17} />
                    {plan.direction === 'upload' ? '本地 → 远程' : '远程 → 本地'}
                  </span>
                </div>
              );
            })}
          </div>
          <fieldset className="transfer-conflict-options">
            <legend>同名文件处理</legend>
            <label className={`transfer-conflict-option ${!overwrite ? 'is-selected' : ''}`}>
              <input
                type="radio"
                name="transfer-conflict"
                checked={!overwrite}
                onChange={() => onOverwriteChange(false)}
              />
              <span>
                <strong>保留目标文件</strong>
                <small>遇到同名文件时停止该任务，并显示错误</small>
              </span>
            </label>
            <label className={`transfer-conflict-option ${overwrite ? 'is-selected' : ''}`}>
              <input
                type="radio"
                name="transfer-conflict"
                checked={overwrite}
                onChange={() => onOverwriteChange(true)}
              />
              <span>
                <strong>覆盖同名文件</strong>
                <small>允许替换目标目录中已存在的同名文件，其他文件保留</small>
              </span>
            </label>
          </fieldset>
          <div className="transfer-confirm-note">
            <ShieldCheck size={23} />
            <p>
              符号链接仅复制链接本身，不复制目标内容。
              <br />
              跨主机后，链接指向的目标可能不可用。
            </p>
          </div>
        </div>
        <footer className="transfer-confirm-footer">
          <span>
            <ActionIcon size={20} />
            准备{uploading ? '上传' : '下载'} {selectedCount} 项
          </span>
          <div>
            <button onClick={onClose}>取消</button>
            <button
              className="primary"
              disabled={!canStart || selectedCount === 0}
              onClick={onStart}
            >
              <ActionIcon size={18} />
              开始传输
            </button>
          </div>
        </footer>
        {!canStart && (
          <p className="transfer-confirm-unavailable" role="status">
            连接或传输监听尚未就绪，请等待或重新连接服务器。
          </p>
        )}
      </section>
    </div>
  );
}
