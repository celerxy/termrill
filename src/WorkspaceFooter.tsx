import type { Workspace } from './useTermRillWorkspace';

/** 当前连接和传输状态摘要。 */
export default function WorkspaceFooter({ workspace }: { workspace: Workspace }) {
  const { connectionId, connecting, running } = workspace;
  return (
    <footer>
      <span>
        <span className={connectionId ? 'connected-dot' : 'demo-dot'} />
        {connecting ? '正在连接' : connectionId ? 'SSH 已连接' : '未连接'}
      </span>
      <span>SFTP / SSH</span>
      <span className="footer-right">{running} 个任务进行中</span>
    </footer>
  );
}
