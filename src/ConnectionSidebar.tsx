import type { Workspace } from './useTermRillWorkspace';
import { useCallback, useEffect, useState } from 'react';
import type { Profile } from './types';
import FileContextMenu from './FileContextMenu';
import { desktop } from './useTermRillWorkspace';
import { Plus, Monitor, Server, Settings, ShieldCheck, Pencil, Trash2 } from 'lucide-react';

// 生产 CSP 仅允许本地图片 URL；禁止将小 SVG 内联成 data: URL。
import termrillLogo from './assets/termrill.svg?no-inline';
import packageInfo from '../package.json';
import { useWorkspaceTabs } from './WorkspaceTabs';

/** 连接配置列表与连接入口。 */
export default function ConnectionSidebar({ workspace }: { workspace: Workspace }) {
  const tabs = useWorkspaceTabs();
  const {
    profiles,
    active,
    connectionId,
    connecting,
    setDeleteProfile,
    profile,
    selectProfile,
    openSettings,
    notify,
  } = workspace;
  const [context, setContext] = useState<{ x: number; y: number; profile: Profile } | null>(null);
  const [group, setGroup] = useState('*');
  const groups = [
    ...new Set(profiles.map((item) => tabs.workbench.value.groups[item.id] ?? '').filter(Boolean)),
  ].sort();
  const filteredProfiles = profiles.filter(
    (item) => group === '*' || JSON.stringify(tabs.workbench.value.groups[item.id] ?? '') === group,
  );
  useEffect(() => {
    if (
      group !== '*' &&
      group !== JSON.stringify('') &&
      !groups.some((name) => JSON.stringify(name) === group)
    )
      setGroup('*');
  }, [group, groups.join('\0')]);
  const closeContext = useCallback(() => setContext(null), []);
  useEffect(closeContext, [closeContext, profiles, connecting, active, connectionId]);
  const showMenu = (event: React.MouseEvent | React.KeyboardEvent, target: Profile) => {
    event.preventDefault();
    event.stopPropagation();
    if (!desktop || connecting) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const pointer = 'clientX' in event && (event.clientX !== 0 || event.clientY !== 0);
    setContext({
      x: pointer ? event.clientX : rect.left + 24,
      y: pointer ? event.clientY : rect.bottom,
      profile: target,
    });
  };
  const isConnected =
    !!context &&
    tabs.tabs.some((tab) => tab.profileId === context.profile.id && !!tab.connectionId);
  return (
    <aside>
      <div className="brand">
        <img src={termrillLogo} alt="" width={29} height={29} />
        <strong>TermRill</strong>
        <span>{packageInfo.version}</span>
      </div>
      <button
        className="new-connection"
        disabled={!desktop || connecting}
        onClick={() => tabs.open()}
      >
        <Plus size={18} />
        新建连接
      </button>
      <div className="section-label">
        工作空间 <span>{profiles.length}</span>
      </div>
      <nav>
        {groups.length > 0 && (
          <select
            className="sidebar-group-filter"
            aria-label="筛选连接分组"
            value={group}
            onChange={(event) => setGroup(event.target.value)}
          >
            <option value="*">全部连接</option>
            <option value={JSON.stringify('')}>未分组</option>
            {groups.map((name) => (
              <option key={name} value={JSON.stringify(name)}>
                {name}
              </option>
            ))}
          </select>
        )}
        {filteredProfiles.map((p) => (
          <div
            className={`host-wrap ${active === p.id ? 'current' : ''}`}
            key={p.id}
            onContextMenu={(event) => showMenu(event, p)}
          >
            <button
              className={`host ${active === p.id ? 'active' : ''}`}
              disabled={connecting}
              onClick={() => void selectProfile(p)}
              onKeyDown={(event) => {
                if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10'))
                  showMenu(event, p);
              }}
            >
              {p.jump ? <Server size={21} /> : <Monitor size={21} />}
              <span>
                <strong>{p.name}</strong>
                <small>
                  {p.target.username}@{p.target.host}
                </small>
              </span>
              {tabs.tabs.some((tab) => tab.profileId === p.id && tab.connectionId) && (
                <i className="connected-dot" />
              )}
            </button>
            <div className="host-actions">
              <button
                aria-label={`编辑 ${p.name}`}
                disabled={connecting}
                onClick={() => tabs.open(p, true)}
              >
                <Pencil size={13} />
              </button>
              <button
                aria-label={`删除 ${p.name}`}
                disabled={connecting}
                onClick={() => setDeleteProfile(p)}
              >
                <Trash2 size={13} />
              </button>
            </div>
          </div>
        ))}
      </nav>
      {!profiles.length && (
        <div className="sidebar-empty">
          添加你的第一台服务器
          <br />
          支持直连和 SSH 堡垒机
        </div>
      )}
      <div className="sidebar-bottom">
        <div className="security-note">
          <ShieldCheck size={19} />
          <span>
            已启用主机指纹验证<small>凭据默认仅用于当前会话</small>
          </span>
        </div>
        <button disabled={!desktop} onClick={() => void openSettings()}>
          <Settings size={17} />
          偏好设置<span>{packageInfo.version}</span>
        </button>
      </div>
      {context && (
        <FileContextMenu
          x={context.x}
          y={context.y}
          onClose={closeContext}
          label="连接操作"
          actions={[
            {
              label: isConnected ? '断开连接' : '连接服务器',
              icon: <Monitor size={16} />,
              run: () => {
                if (isConnected) void tabs.disconnectProfile(context.profile.id).catch(notify);
                else void selectProfile(context.profile);
              },
            },
            {
              label: '在新标签中连接…',
              icon: <Plus size={16} />,
              run: () => tabs.open(context.profile, false, true),
            },
            {
              label: '编辑连接…',
              icon: <Pencil size={16} />,
              run: () => tabs.open(context.profile, true),
            },
            {
              label: '删除连接…',
              icon: <Trash2 size={16} />,
              danger: true,
              separator: true,
              run: () => setDeleteProfile(context.profile),
            },
          ]}
        />
      )}
    </aside>
  );
}
