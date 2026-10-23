import type { Workspace } from './useTermRillWorkspace';
import { desktop } from './useTermRillWorkspace';
import { Terminal, Upload, Unplug, Plug, FolderOpen, ChevronDown, Activity } from 'lucide-react';
import { lazy, Suspense, useState } from 'react';
const ServerWorkbench = lazy(() => import('./ServerWorkbench'));

import './workspace.css';
import { SessionTabs } from './WorkspaceTabs';
import CommandPalette from './CommandPalette';

/** 会话标签和工作空间工具栏。 */
export default function WorkspaceHeader({ workspace }: { workspace: Workspace }) {
  const [workbenchOpen, setWorkbenchOpen] = useState(false);
  const {
    connectionId,
    connecting,
    setDialog,
    terminal,
    setTerminal,
    listenersReady,
    profile,
    notify,
    disconnect,
    chooseUpload,
  } = workspace;
  return (
    <>
      <SessionTabs />
      {!desktop && (
        <div className="browser-banner">
          当前为浏览器预览。真实文件和 SSH 功能请使用 <code>npm run tauri dev -- --offline</code>{' '}
          启动桌面应用。
        </div>
      )}
      <header className="workspace-commandbar">
        <div className="workspace-heading">
          <h1>文件传输</h1>
          <p>
            <span className={connectionId ? 'connected-dot' : 'demo-dot'} />
            {profile
              ? `${profile.target.username}@${profile.target.host}:${profile.target.port}`
              : '从本地到服务器'}
          </p>
        </div>
        <div className="header-actions">
          <CommandPalette workspace={workspace} />
          <button disabled={!desktop} onClick={() => setWorkbenchOpen(true)}>
            <Activity size={17} />
            工作台
          </button>
          <button disabled={!connectionId} onClick={() => setTerminal(!terminal)}>
            <Terminal size={17} />
            SSH 终端
          </button>
          <button
            disabled={!connectionId || !workspace.local || !workspace.remote || !listenersReady}
            onClick={() => workspace.setSyncOpen(true)}
          >
            目录同步
          </button>
          <button
            className="primary"
            disabled={!connectionId || !listenersReady}
            onClick={() => void chooseUpload('files')}
          >
            <Upload size={17} />
            上传文件
          </button>
          <details className="upload-options">
            <summary aria-label="更多上传选项">
              <ChevronDown size={15} />
            </summary>
            <button
              title="上传文件夹"
              disabled={!connectionId || !listenersReady}
              onClick={(event) => {
                event.currentTarget.closest('details')?.removeAttribute('open');
                void chooseUpload('folder');
              }}
            >
              <FolderOpen size={18} />
              上传文件夹
            </button>
          </details>
          <span className="command-divider" />
          {connectionId ? (
            <button
              title="断开连接"
              aria-label="断开连接"
              onClick={() => void disconnect().catch(notify)}
            >
              <Unplug size={16} />
            </button>
          ) : (
            <button
              disabled={!desktop || connecting}
              onClick={() => setDialog(profile ? { profile } : {})}
            >
              <Plug size={16} />
              {connecting ? '连接中…' : '连接'}
            </button>
          )}
        </div>
      </header>
      {workbenchOpen && (
        <Suspense fallback={<div role="status">正在加载工作台…</div>}>
          <ServerWorkbench
            key={connectionId ?? 'offline'}
            connectionId={connectionId}
            target={
              profile
                ? `${profile.target.username}@${profile.target.host}:${profile.target.port}`
                : ''
            }
            onClose={() => setWorkbenchOpen(false)}
          />
        </Suspense>
      )}
    </>
  );
}
