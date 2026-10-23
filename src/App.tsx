/** 根组件组织独立会话；切换标签保留后台工作空间。 */
import { useTermRillWorkspace } from './useTermRillWorkspace';
import ConnectionSidebar from './ConnectionSidebar';
import WorkspaceHeader from './WorkspaceHeader';
import FileWorkspace from './FileWorkspace';
import TransferQueue from './TransferQueue';
import WorkspaceFooter from './WorkspaceFooter';
import WorkspaceDialogs from './WorkspaceDialogs';
import AuthPrompts from './AuthPrompts';
import { useEffect } from 'react';
import {
  WorkspaceTabs,
  useWorkspaceTabs,
  useReportSession,
  type SessionTab,
} from './WorkspaceTabs';
import { SessionVisibility } from './SessionVisibility';
import { invoke, isTauri } from '@tauri-apps/api/core';

export default function App() {
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    // 导航计时覆盖模块下载/执行，不记录配置、路径或凭据；正式包不输出。
    const frame = requestAnimationFrame(() => {
      const navigationMs = Math.round(performance.now());
      console.info(`[TermRill startup] 首屏已提交，距页面导航 ${navigationMs} ms`);
      if (isTauri()) {
        const navigation = performance.getEntriesByType('navigation')[0] as
          PerformanceNavigationTiming | undefined;
        const slowResources = (
          performance.getEntriesByType('resource') as PerformanceResourceTiming[]
        )
          .filter((resource) => resource.duration > 200)
          .map((resource) => ({
            // 仅记录本地前端模块名称，不输出查询参数、服务器地址或凭据。
            module:
              new URL(resource.name).origin === location.origin
                ? new URL(resource.name).pathname
                : 'external-resource',
            start: Math.round(resource.startTime),
            duration: Math.round(resource.duration),
          }))
          .slice(0, 30);
        void invoke('startup_ready', {
          navigationMs,
          diagnostics:
            navigationMs > 1000
              ? JSON.stringify({
                  protocol: location.protocol,
                  port: location.port,
                  fetchStart: Math.round(navigation?.fetchStart ?? 0),
                  dnsStart: Math.round(navigation?.domainLookupStart ?? 0),
                  dnsEnd: Math.round(navigation?.domainLookupEnd ?? 0),
                  connectEnd: Math.round(navigation?.connectEnd ?? 0),
                  requestStart: Math.round(navigation?.requestStart ?? 0),
                  responseStart: Math.round(navigation?.responseStart ?? 0),
                  documentResponse: Math.round(navigation?.responseEnd ?? 0),
                  domInteractive: Math.round(navigation?.domInteractive ?? 0),
                  slowResources,
                })
              : null,
        }).catch(console.warn);
      }
    });
    return () => cancelAnimationFrame(frame);
  }, []);
  return (
    <div
      className="app-shell"
      onContextMenu={(event) => {
        // 桌面工作区不提供浏览器导航/打印菜单；文本输入仍保留原生复制粘贴。
        if (!(event.target as HTMLElement).closest('input, textarea, [contenteditable="true"]'))
          event.preventDefault();
      }}
    >
      <WorkspaceTabs>
        {(tab, visible) => <SessionView key={tab.id} tab={tab} visible={visible} />}
      </WorkspaceTabs>
      <AuthPrompts />
    </div>
  );
}

function SessionView({ tab, visible }: { tab: SessionTab; visible: boolean }) {
  const tabs = useWorkspaceTabs();
  const workspace = useTermRillWorkspace({
    initialProfile: tab.initialProfile,
    openDialog: tab.openDialog,
    restoreTransfers: !tab.openDialog,
    visible,
    profiles: tabs.profiles,
    showHidden: tabs.showHidden,
    setShowHidden: tabs.setShowHidden,
    saveProfile: tabs.saveProfile,
    deleteProfile: tabs.deleteProfile,
    openProfile: tabs.open,
  });
  useReportSession(tab, workspace);
  const setDialog = workspace.setDialog;
  useEffect(() => {
    if (tab.dialogRequest > 0) {
      const profile = tabs.profiles.find((item) => item.id === tab.profileId);
      if (profile) setDialog({ profile });
    }
  }, [tab.dialogRequest, setDialog]);
  return (
    <SessionVisibility.Provider value={visible}>
      <div
        className="session-workspace"
        id={'session-' + tab.id}
        role="tabpanel"
        aria-labelledby={'tab-' + tab.id}
        hidden={!visible}
      >
        <ConnectionSidebar workspace={workspace} />
        <main>
          <WorkspaceHeader workspace={workspace} />
          <FileWorkspace workspace={workspace} />
          <TransferQueue workspace={workspace} />
          <WorkspaceFooter workspace={workspace} />
        </main>
        <WorkspaceDialogs workspace={workspace} />
      </div>
    </SessionVisibility.Provider>
  );
}
