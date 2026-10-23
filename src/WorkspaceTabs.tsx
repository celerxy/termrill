/** 标签负责会话生命周期；隐藏的工作空间保持挂载，切换不停止连接或任务。 */
import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  useCallback,
  type ReactNode,
  type Dispatch,
  type SetStateAction,
} from 'react';
import { invoke } from '@tauri-apps/api/core';
import { desktop, type Workspace } from './useTermRillWorkspace';
import type { Profile } from './types';
import { Monitor, Plus, X } from 'lucide-react';
import { useWorkbenchPreferences } from './useWorkbenchPreferences';

export type SessionTab = {
  id: string;
  initialProfile?: Profile;
  openDialog: boolean;
  dialogRequest: number;
  name: string;
  profileId: string | null;
  connectionId: string | null;
  connecting: boolean;
  running: number;
};
type TabsContext = {
  workbench: ReturnType<typeof useWorkbenchPreferences>;
  importProfiles: (profiles: Profile[]) => Promise<void>;
  tabs: SessionTab[];
  selected: string;
  profiles: Profile[];
  saveProfile: (profile: Profile) => Promise<void>;
  deleteProfile: (id: string) => Promise<void>;
  showHidden: boolean;
  setShowHidden: Dispatch<SetStateAction<boolean>>;
  select: (id: string) => void;
  open: (profile?: Profile, edit?: boolean, forceNew?: boolean) => void;
  disconnectProfile: (id: string) => Promise<void>;
  close: (id: string) => void;
  report: (
    id: string,
    state: Pick<SessionTab, 'name' | 'profileId' | 'connectionId' | 'connecting' | 'running'>,
  ) => void;
  register: (id: string, disconnect: () => Promise<void>) => () => void;
};
const Context = createContext<TabsContext | null>(null);
export function useWorkspaceTabs() {
  const value = useContext(Context);
  if (!value) throw new Error('缺少工作空间标签容器');
  return value;
}
function newTab(profile?: Profile, openDialog = true): SessionTab {
  return {
    id: crypto.randomUUID(),
    initialProfile: profile,
    openDialog,
    dialogRequest: 0,
    name: profile?.name ?? '新建连接',
    profileId: profile?.id ?? null,
    connectionId: null,
    connecting: false,
    running: 0,
  };
}
export function WorkspaceTabs({
  children,
}: {
  children: (tab: SessionTab, visible: boolean) => ReactNode;
}) {
  const workbench = useWorkbenchPreferences();
  const [tabs, setTabs] = useState<SessionTab[]>(() => [newTab(undefined, false)]);
  const [selected, setSelected] = useState(tabs[0].id);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [showHidden, setShowHidden] = useState(false);
  const [closing, setClosing] = useState<string | null>(null);
  const [closeBusy, setCloseBusy] = useState(false);
  const [error, setError] = useState('');
  const profileRead = useRef<Promise<void>>(Promise.resolve());
  const profileWrites = useRef<Promise<void>>(Promise.resolve());
  const profilesRef = useRef(profiles);
  profilesRef.current = profiles;
  const writeProfiles = (update: (previous: Profile[]) => Profile[]) => {
    const write = profileWrites.current
      .catch(() => {})
      .then(async () => {
        await profileRead.current;
        const next = update(profilesRef.current);
        await invoke('save_profiles', { profiles: next });
        profilesRef.current = next;
        setProfiles(next);
      });
    profileWrites.current = write;
    return write;
  };
  const saveProfile = (profile: Profile) =>
    writeProfiles((previous) =>
      previous.some((item) => item.id === profile.id)
        ? previous.map((item) => (item.id === profile.id ? profile : item))
        : [...previous, profile],
    );
  const deleteProfile = (id: string) =>
    writeProfiles((previous) => previous.filter((profile) => profile.id !== id));
  const disconnectors = useRef(new Map<string, () => Promise<void>>());
  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    const read = invoke<Profile[]>('load_profiles').then((value) => {
      if (!disposed) {
        profilesRef.current = value;
        setProfiles(value);
      }
    });
    profileRead.current = read;
    void read.catch((e) => {
      if (!disposed) setError(String(e));
    });
    void invoke<boolean>('load_preferences')
      .then((value) => {
        if (!disposed) setShowHidden(value);
      })
      .catch((e) => {
        if (!disposed) setError(String(e));
      });
    return () => {
      disposed = true;
    };
  }, []);
  const report = useCallback<TabsContext['report']>((id, state) => {
    setTabs((previous) => previous.map((tab) => (tab.id === id ? { ...tab, ...state } : tab)));
  }, []);
  const register = useCallback<TabsContext['register']>((id, disconnect) => {
    disconnectors.current.set(id, disconnect);
    return () => {
      disconnectors.current.delete(id);
    };
  }, []);
  const disconnectProfile = async (id: string) => {
    for (const tab of tabs.filter((item) => item.profileId === id && item.connectionId)) {
      await disconnectors.current.get(tab.id)?.();
    }
  };
  const open = (profile?: Profile, edit = false, forceNew = false) => {
    const existing = !forceNew && profile && tabs.find((tab) => tab.profileId === profile.id);
    if (existing) {
      setSelected(existing.id);
      if (edit || (!existing.connectionId && !existing.connecting))
        setTabs((previous) =>
          previous.map((tab) =>
            tab.id === existing.id ? { ...tab, dialogRequest: tab.dialogRequest + 1 } : tab,
          ),
        );
      return;
    }
    const tab = newTab(profile);
    setTabs((previous) => [...previous, tab]);
    setSelected(tab.id);
  };
  const finishClose = async (id: string) => {
    setCloseBusy(true);
    try {
      await disconnectors.current.get(id)?.();
      const remaining = tabs.filter((tab) => tab.id !== id);
      if (!remaining.length) remaining.push(newTab(undefined, false));
      setTabs((previous) => {
        const next = previous.filter((tab) => tab.id !== id);
        return next.length ? next : remaining;
      });
      if (selected === id)
        setSelected(
          remaining[
            Math.min(
              tabs.findIndex((tab) => tab.id === id),
              remaining.length - 1,
            )
          ].id,
        );
      setClosing(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setCloseBusy(false);
    }
  };
  const close = (id: string) => {
    const tab = tabs.find((item) => item.id === id);
    if (!tab || tab.connecting || closeBusy) return;
    // 连接上的目录、权限及终端操作不一定出现在传输队列，因此统一明确确认断开。
    if (tab.connectionId || tab.running) setClosing(id);
    else void finishClose(id);
  };
  return (
    <Context.Provider
      value={{
        workbench,
        importProfiles: (incoming) =>
          writeProfiles((previous) => {
            if (previous.length + incoming.length > 200)
              throw new Error('合并后连接不得超过 200 个');
            return [...previous, ...incoming];
          }),
        tabs,
        selected,
        profiles,
        saveProfile,
        deleteProfile,
        showHidden,
        setShowHidden,
        select: setSelected,
        open,
        disconnectProfile,
        close,
        report,
        register,
      }}
    >
      {tabs.map((tab) => children(tab, tab.id === selected))}
      {closing && (
        <div className="modal-backdrop">
          <section
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="close-tab-title"
          >
            <h2 id="close-tab-title">关闭会话标签</h2>
            <p>
              关闭「{tabs.find((tab) => tab.id === closing)?.name}」将断开该 SSH
              连接，并停止此会话的传输、终端和其他正在执行的操作。其他标签保持连接。
            </p>
            {error && <p role="alert">{error}</p>}
            <div className="modal-actions">
              <button disabled={closeBusy} onClick={() => setClosing(null)}>
                取消
              </button>
              <button
                className="primary"
                disabled={closeBusy}
                onClick={() => void finishClose(closing)}
              >
                {closeBusy ? '关闭中…' : '断开并关闭'}
              </button>
            </div>
          </section>
        </div>
      )}
      {error && !closing && (
        <div className="toast" role="alert">
          {error}
          <button aria-label="关闭提示" onClick={() => setError('')}>
            <X size={15} />
          </button>
        </div>
      )}
    </Context.Provider>
  );
}
export function SessionTabs() {
  const { tabs, selected, select, open, close } = useWorkspaceTabs();
  return (
    <div className="tabs session-tabs" role="tablist" aria-label="服务器会话">
      {tabs.map((tab) => (
        <div key={tab.id} className={tab.id === selected ? 'session-tab active' : 'session-tab'}>
          <button
            role="tab"
            id={'tab-' + tab.id}
            aria-controls={'session-' + tab.id}
            aria-selected={tab.id === selected}
            onClick={() => select(tab.id)}
          >
            <Monitor size={16} />
            <span className="session-tab-name" title={tab.name}>
              {tab.name}
            </span>
            <span className={tab.connectionId ? 'connected-dot' : 'demo-dot'} />
            <span className="tab-demo">
              {tab.connecting ? '连接中' : tab.connectionId ? '已连接' : '未连接'}
            </span>
            {tab.running > 0 && (
              <span className="badge" title="后台传输任务">
                {tab.running}
              </span>
            )}
          </button>
          <button
            className="session-tab-close"
            aria-label={'关闭 ' + tab.name}
            disabled={tab.connecting}
            onClick={() => close(tab.id)}
          >
            <X size={14} strokeWidth={1.8} />
          </button>
        </div>
      ))}
      <button aria-label="新建会话标签" disabled={!desktop} onClick={() => open()}>
        <Plus size={17} />
      </button>
    </div>
  );
}
export function useReportSession(tab: SessionTab, workspace: Workspace) {
  const { report, register } = useWorkspaceTabs();
  const disconnectRef = useRef(workspace.disconnect);
  disconnectRef.current = workspace.disconnect;
  useEffect(() => register(tab.id, () => disconnectRef.current()), [register, tab.id]);
  const name = workspace.profile?.name ?? '新建连接';
  const profileId = workspace.profile?.id ?? null;
  const { connectionId, connecting, running } = workspace;
  useEffect(
    () => report(tab.id, { name, profileId, connectionId, connecting, running }),
    [report, tab.id, name, profileId, connectionId, connecting, running],
  );
}
