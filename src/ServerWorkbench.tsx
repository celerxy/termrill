/** 工作台关闭即卸载采集组件；只有用户主动操作才启动远程命令。 */
import { useEffect, useRef, useState } from 'react';
import { X } from 'lucide-react';
import ServerMetrics from './ServerMetrics';
import SavedCommands from './SavedCommands';
import ConnectionGroups from './ConnectionGroups';
import ConnectionExchange from './ConnectionExchange';
import { useWorkspaceTabs } from './WorkspaceTabs';
import './serverWorkbench.css';
export default function ServerWorkbench({
  connectionId,
  target,
  onClose,
}: {
  connectionId: string | null;
  target: string;
  onClose: () => void;
}) {
  const [tab, setTab] = useState('metrics');
  const dialog = useRef<HTMLElement>(null);
  const { workbench } = useWorkspaceTabs();
  useEffect(() => {
    const previous = document.activeElement;
    dialog.current?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, []);
  return (
    <div className="modal-backdrop">
      <section
        className="modal server-workbench"
        ref={dialog}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby="server-workbench-title"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation();
            onClose();
          }
          if (event.key === 'Tab') {
            const controls = [
              ...event.currentTarget.querySelectorAll<HTMLElement>(
                'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), pre[tabindex]',
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
        <header>
          <div>
            <h2 id="server-workbench-title">服务器工作台</h2>
            <small>{target || '当前未连接'}</small>
          </div>
          <button aria-label="关闭服务器工作台" onClick={onClose}>
            <X size={20} />
          </button>
        </header>
        <nav aria-label="工作台分区">
          {[
            ['metrics', '服务器状态'],
            ['commands', '常用命令'],
            ['groups', '连接分组'],
            ['exchange', '导入与导出'],
          ].map(([id, label]) => (
            <button
              key={id}
              aria-current={tab === id ? 'page' : undefined}
              onClick={() => setTab(id)}
            >
              {label}
            </button>
          ))}
        </nav>
        <div className="server-workbench-content">
          {workbench.error && <p role="alert">{workbench.error}</p>}
          {tab === 'metrics' && <ServerMetrics connectionId={connectionId} />}
          {tab === 'commands' && <SavedCommands connectionId={connectionId} target={target} />}
          {tab === 'groups' && (workbench.ready ? <ConnectionGroups /> : <p>正在读取配置…</p>)}
          {tab === 'exchange' && <ConnectionExchange />}
        </div>
      </section>
    </div>
  );
}
