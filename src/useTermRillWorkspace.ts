/** 协调连接、目录、传输与弹窗状态；协议和文件写入由 Rust 命令执行。 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { RemoteDirectoryCache } from './remoteDirectoryCache';
import { recoveryProfileIdentity, TransferRecoveryIntent } from './transferRecovery';
import { useWorkspaceLayout } from './useWorkspaceLayout';
import { observeTransfer } from './transferObservation';
import {
  Profile,
  Listing,
  FileEntry,
  Transfer,
  TransferPlan,
  ConnectRequest,
  ApiError,
  HostKey,
  message,
  formatSize,
  joinPath,
  basename,
  finished,
} from './types';
export const desktop = isTauri();

/** 扫描包括本地枚举、远程预检及目录准备；恢复页面只能从观察时开始计时。 */
function withTransferTimers(task: Transfer, previous?: Transfer, restored = false): Transfer {
  const now = performance.now();
  task = observeTransfer(task, previous, now);
  const retrying = previous !== undefined && finished(previous) && !finished(task);
  const timerStartedAt = retrying ? now : (previous?.timerStartedAt ?? task.timerStartedAt);
  task = {
    ...task,
    syncReport: previous?.syncReport ?? task.syncReport,
    timerStartedAt: timerStartedAt ?? (finished(task) ? undefined : now),
    timerEndedAt: finished(task) ? (previous?.timerEndedAt ?? now) : undefined,
    timerEstimated: retrying
      ? false
      : (previous?.timerEstimated ?? task.timerEstimated ?? restored),
  };
  if (task.status !== 'scanning') return task;
  return {
    ...task,
    scanStartedAt:
      previous?.status === 'scanning'
        ? (previous.scanStartedAt ?? performance.now())
        : performance.now(),
    scanTimerEstimated:
      previous?.status === 'scanning' ? (previous.scanTimerEstimated ?? restored) : restored,
  };
}
type Operation = {
  remote: boolean;
  kind: 'mkdir' | 'rename';
  name: string;
  source?: string;
};
export type SessionOptions = {
  initialProfile?: Profile;
  openDialog: boolean;
  restoreTransfers: boolean;
  visible: boolean;
  profiles: Profile[];
  showHidden: boolean;
  setShowHidden: React.Dispatch<React.SetStateAction<boolean>>;
  saveProfile: (profile: Profile) => Promise<void>;
  deleteProfile: (id: string) => Promise<void>;
  openProfile: (profile: Profile) => void;
};
export function useTermRillWorkspace(options: SessionOptions) {
  const { profiles, showHidden, setShowHidden } = options;
  const optionsRef = useRef(options);
  optionsRef.current = options;
  const ownedTransfers = useRef(new Set<string>());
  const discardingRef = useRef(new Set<string>());
  const [discardingTransfers, setDiscardingTransfers] = useState<string[]>([]);
  const recoveryIntent = useRef(new TransferRecoveryIntent());
  const retryingRef = useRef(new Set<string>());
  const [retryingTransfers, setRetryingTransfers] = useState<string[]>([]);
  const [recoveryTask, setRecoveryTask] = useState<string | null>(null);
  const [sessionProfile, setSessionProfile] = useState(options.initialProfile);
  const [active, setActive] = useState<string | null>(options.initialProfile?.id ?? null);
  const [connectionId, setConnectionId] = useState<string | null>(null);
  const connectionRef = useRef<string | null>(null);
  const [local, setLocal] = useState<Listing | null>(null);
  const [remote, setRemote] = useState<Listing | null>(null);
  const localRef = useRef(local);
  const remoteRef = useRef(remote);
  localRef.current = local;
  remoteRef.current = remote;
  const [localSelection, setLocalSelection] = useState<string[]>([]);
  const [remoteSelection, setRemoteSelection] = useState<string[]>([]);
  const [localBusy, setLocalBusy] = useState(false);
  const [localReadCount, setLocalReadCount] = useState(0);
  const [localReadError, setLocalReadError] = useState('');
  const [remoteBusy, setRemoteBusy] = useState(false);
  const [remoteReadCount, setRemoteReadCount] = useState(0);
  const [remoteReadError, setRemoteReadError] = useState('');
  const pendingRemote = useRef<{ key: string; sequence: number } | null>(null);
  const remoteReadTail = useRef<Promise<void>>(Promise.resolve());
  const [remoteCache] = useState(() => new RemoteDirectoryCache());
  const [connecting, setConnecting] = useState(false);
  const localSequence = useRef(0);
  const remoteSequence = useRef(0);
  const [notice, setNotice] = useState('');
  const [queueOpen, setQueueOpen] = useState(true);
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [dialog, setDialog] = useState<{
    profile?: Profile;
  } | null>(options.openDialog ? { profile: options.initialProfile } : null);
  const [trust, setTrust] = useState<{
    key: HostKey;
    request: ConnectRequest;
  } | null>(null);
  const [terminal, setTerminal] = useState(false);
  const showHiddenRef = useRef(showHidden);
  showHiddenRef.current = showHidden;
  const [settings, setSettings] = useState(false);
  const [syncOpen, setSyncOpen] = useState(false);
  const [hosts, setHosts] = useState<Record<string, string>>({});
  const [plans, setPlans] = useState<TransferPlan[] | null>(null);
  const [overwrite, setOverwrite] = useState(false);
  const [operation, setOperation] = useState<Operation | null>(null);
  const [operationBusy, setOperationBusy] = useState(false);
  const [deleteProfile, setDeleteProfile] = useState<Profile | null>(null);
  const [listenersReady, setListenersReady] = useState(false);
  const profile = connectionId
    ? sessionProfile
    : (profiles.find((p) => p.id === active) ?? sessionProfile);
  useEffect(() => {
    if (!active && options.restoreTransfers && profiles.length) {
      setActive(profiles[0].id);
      setSessionProfile(profiles[0]);
    }
  }, [active, options.restoreTransfers, profiles]);
  const notify = useCallback((error: unknown) => setNotice(message(error)), []);
  const workspaceLayout = useWorkspaceLayout(notify);
  const cancelRecovery = useCallback(() => {
    recoveryIntent.current.cancel();
    setRecoveryTask(null);
  }, []);
  const closeConnectionDialog = () => {
    cancelRecovery();
    setDialog(null);
  };
  const cancelTrust = () => {
    cancelRecovery();
    setTrust(null);
  };
  // 请求序号防止慢响应覆盖后来选择的目录；远程列表还要匹配当前连接。
  const loadLocal = useCallback(
    async (path?: string, offset = 0, query = '') => {
      const seq = ++localSequence.current;
      setLocalBusy(true);
      setLocalReadCount(0);
      setLocalReadError('');
      try {
        const listing = await invoke<Listing>('list_local_page', {
          path: path ?? null,
          offset,
          query,
          showHidden: showHiddenRef.current,
        });
        if (seq === localSequence.current) {
          setLocal(listing);
          setLocalSelection([]);
        }
      } catch (e) {
        if (seq === localSequence.current) {
          setLocalReadError(message(e));
          notify(e);
        }
      } finally {
        if (seq === localSequence.current) setLocalBusy(false);
      }
    },
    [notify],
  );
  const loadRemote = useCallback(
    async (path: string, offset = 0, query = '', reusePage = false) => {
      const id = connectionRef.current;
      if (!id) return;
      const requestKey = JSON.stringify([id, path, offset, query, showHiddenRef.current]);
      // 只有导航/翻页复用；刷新和写操作后的调用默认清空整个工作区缓存。
      if (!reusePage) remoteCache.clear();
      if (pendingRemote.current?.key === requestKey) return;
      const seq = ++remoteSequence.current;
      const cached = reusePage ? remoteCache.get(requestKey) : null;
      if (cached) {
        setRemote(cached.listing);
        setRemoteSelection([]);
        setRemoteReadError('');
        if (cached.fresh) {
          pendingRemote.current = null;
          setRemoteBusy(false);
          return;
        }
      }
      pendingRemote.current = { key: requestKey, sequence: seq };
      setRemoteBusy(true);
      setRemoteReadCount(0);
      setRemoteReadError('');
      // 每个工作区只发送一个目录请求；等待期间的新选择取代旧选择，不排队扫描过期目录。
      const previous = remoteReadTail.current;
      let release!: () => void;
      remoteReadTail.current = new Promise<void>((resolve) => {
        release = resolve;
      });
      try {
        await previous;
        if (seq !== remoteSequence.current || id !== connectionRef.current) return;
        const listing = await invoke<Listing>('list_remote_page', {
          id,
          path,
          offset,
          query,
          showHidden: showHiddenRef.current,
        });
        if (seq === remoteSequence.current && id === connectionRef.current) {
          remoteCache.set(requestKey, listing);
          setRemote(listing);
          setRemoteSelection([]);
        }
      } catch (e) {
        if (seq === remoteSequence.current && id === connectionRef.current) {
          remoteCache.clear();
          setRemoteReadError(message(e));
          notify(e);
        }
      } finally {
        release();
        if (pendingRemote.current?.sequence === seq) pendingRemote.current = null;
        if (seq === remoteSequence.current) setRemoteBusy(false);
      }
    },
    [notify, remoteCache],
  );
  const refreshAfterDelete = async (id: string, paths: string[]) => {
    if (connectionRef.current !== id || !remoteRef.current) return;
    const listing = remoteRef.current;
    const removed = new Set(paths);
    // 只移除服务器已确认的项目；废弃旧读取，防止迟到的目录页重新显示已删文件。
    remoteSequence.current += 1;
    pendingRemote.current = null;
    remoteCache.clear();
    setRemote((current) =>
      current?.path === listing.path
        ? { ...current, entries: current.entries.filter((entry) => !removed.has(entry.path)) }
        : current,
    );
    setRemoteSelection((selection) => selection.filter((path) => !removed.has(path)));
    await loadRemote(listing.path, listing.page?.offset ?? 0, listing.page?.query ?? '');
  };
  const dropRef = useRef<(paths: string[]) => void>(() => {});
  // 先建立进度订阅再允许启动任务；异步订阅完成时可能已经卸载，需要立即释放。
  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    const stops: (() => void)[] = [];
    void (async () => {
      try {
        const stop = await listen<Transfer>('transfer-progress', ({ payload }) => {
          if (disposed) return;
          if (
            payload.connectionId !== connectionRef.current &&
            !ownedTransfers.current.has(payload.id)
          )
            return;
          ownedTransfers.current.add(payload.id);
          setTransfers((previous) =>
            previous.some((t) => t.id === payload.id)
              ? previous.map((t) => (t.id === payload.id ? withTransferTimers(payload, t) : t))
              : [...previous, withTransferTimers(payload)],
          );
          if (payload.status === 'completed') {
            void loadLocal(localRef.current?.path);
            if (payload.connectionId === connectionRef.current && remoteRef.current)
              void loadRemote(remoteRef.current.path);
          }
        });
        if (disposed) {
          stop();
          return;
        }
        stops.push(stop);
        const stopPersistence = await listen<string>(
          'transfer-persistence-error',
          ({ payload }) => {
            if (!disposed) notify(`传输任务清单保存失败：${payload}`);
          },
        );
        if (disposed) {
          stopPersistence();
          return;
        }
        stops.push(stopPersistence);
        setListenersReady(true);
        const stopDrop = await getCurrentWebview().onDragDropEvent(({ payload }) => {
          if (payload.type !== 'drop' || disposed || !optionsRef.current.visible) return;
          const rect = document
            .querySelector('.session-workspace:not([hidden]) [data-remote="true"]')
            ?.getBoundingClientRect();
          if (!rect) return;
          // 原生拖放返回物理像素，DOM 边界使用 CSS 像素；高 DPI 下必须换算。
          const x = payload.position.x / window.devicePixelRatio;
          const y = payload.position.y / window.devicePixelRatio;
          if (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom)
            dropRef.current(payload.paths);
        });
        if (disposed) {
          stopDrop();
          return;
        }
        stops.push(stopDrop);
      } catch (e) {
        if (!disposed) notify(e);
      }
    })();
    void loadLocal();
    return () => {
      disposed = true;
      stops.forEach((stop) => stop());
    };
  }, [loadLocal, loadRemote, notify]);
  // 不包含显示名称，重命名配置不丢失队列；端点被改动则必须重新隔离归属。
  const transferScope = profile ? JSON.stringify({ ...profile, name: '' }) : null;
  useEffect(() => {
    if (!desktop || !listenersReady) return;
    let disposed = false;
    // 切换服务器时旧任务不得继续靠 ownedTransfers 接收其他服务器的进度。
    ownedTransfers.current.clear();
    cancelRecovery();
    setTransfers([]);
    if (!transferScope) return;
    const restoreProfile = JSON.parse(transferScope) as Profile;
    void invoke<Transfer[]>('pending_transfers', { profile: restoreProfile })
      .then((tasks) => {
        if (!disposed) {
          tasks.forEach((task) => ownedTransfers.current.add(task.id));
          setTransfers((previous) => [
            ...previous.map((task) => {
              const restored = tasks.find((candidate) => candidate.id === task.id);
              // HMR 可能保留上一轮错误的“中断”状态；用后端活动状态修正它。
              return task.status === 'failed' &&
                task.error?.startsWith('任务中断') &&
                restored &&
                restored.status !== 'failed'
                ? withTransferTimers(restored, task, true)
                : !finished(task) && task.timerStartedAt === undefined
                  ? withTransferTimers(task, undefined, true)
                  : task;
            }),
            ...tasks
              .filter((task) => !previous.some((p) => p.id === task.id))
              .map((task) => withTransferTimers(task, undefined, true)),
          ]);
        }
      })
      .catch((error: unknown) => {
        if (!disposed) notify(error);
      });
    return () => {
      disposed = true;
    };
  }, [transferScope, listenersReady, notify, cancelRecovery]);
  const disconnect = async () => {
    const id = connectionRef.current;
    if (!id) return;
    connectionRef.current = null;
    setConnectionId(null);
    setRemote(null);
    remoteCache.clear();
    setRemoteSelection([]);
    setTerminal(false);
    ++remoteSequence.current;
    setRemoteBusy(false);
    setRemoteReadError('');
    setRemoteReadCount(0);
    await invoke('disconnect', { id });
  };
  const selectProfile = async (p: Profile) => {
    if (p.id !== active) {
      options.openProfile(p);
      return;
    }
    if (!connectionRef.current && !connecting) setDialog({ profile: p });
  };
  const save = async (p: Profile) => {
    await options.saveProfile(p);
    if (profile && recoveryProfileIdentity(p) !== recoveryProfileIdentity(profile)) {
      cancelRecovery();
    }
    if (connectionRef.current && p.id === active) await disconnect();
    setSessionProfile(p);
    setActive(p.id);
  };
  const connect = async (request: ConnectRequest) => {
    setConnecting(true);
    setNotice('');
    try {
      await disconnect();
      const result = await invoke<{ id: string }>('connect', { request });
      connectionRef.current = result.id;
      remoteCache.clear();
      setConnectionId(result.id);
      setRemote(null);
      setRemoteSelection([]);
      setSessionProfile(request.profile);
      setActive(request.profile.id);
      setDialog(null);
      setTrust(null);
      setConnecting(false);
      // 目录读取沿用序号/会话保护，失败展示目录错误，不退回认证弹窗。
      void loadRemote('.');
      const retryId = recoveryIntent.current.take(request.profile);
      setRecoveryTask(null);
      if (retryId) {
        // 保留原指纹与交互认证流程；只有用户选中的任务在认证完成后恢复。
        await submitRetry(retryId, result.id);
      }
    } catch (e) {
      const error = e as ApiError;
      if (error.hostKey) {
        setTrust({ key: error.hostKey, request });
        setDialog(null);
      } else throw e;
    } finally {
      setConnecting(false);
    }
  };
  const planPaths = (paths: string[], direction: 'upload' | 'download') => {
    if (!connectionRef.current || !remoteRef.current || !localRef.current) {
      setNotice('请先连接服务器并选择本地目录');
      return;
    }
    setOverwrite(false);
    setPlans(
      paths.map((path) => ({
        name: basename(path),
        direction,
        local: direction === 'upload' ? path : joinPath(localRef.current!.path, basename(path)),
        remote:
          direction === 'upload' ? joinPath(remoteRef.current!.path, basename(path), true) : path,
      })),
    );
  };
  dropRef.current = (paths) => planPaths(paths, 'upload');
  const chooseUpload = async (kind: 'files' | 'folder') => {
    try {
      const paths = await invoke<string[]>('pick_path', { kind });
      if (paths.length) planPaths(paths, 'upload');
    } catch (e) {
      notify(e);
    }
  };
  const startPlans = async () => {
    if (!plans || !connectionId) return;
    const selectedPlans = plans;
    setPlans(null);
    for (const p of selectedPlans) {
      const id = crypto.randomUUID();
      ownedTransfers.current.add(id);
      setTransfers((previous) => [
        ...previous,
        {
          id,
          connectionId,
          name: p.name,
          syncReport: p.syncReport,
          direction: p.direction,
          status: 'queued',
          timerStartedAt: performance.now(),
          timerEstimated: false,
          bytes: 0,
          total: 0,
          speed: 0,
          error: null,
        },
      ]);
      try {
        await invoke('start_transfer', {
          id,
          connectionId,
          local: p.local,
          remote: p.remote,
          direction: p.direction,
          overwrite,
          selection: p.selection ?? null,
        });
      } catch (e) {
        setTransfers((previous) =>
          previous.map((t) =>
            t.id === id ? withTransferTimers({ ...t, status: 'failed', error: message(e) }, t) : t,
          ),
        );
      }
    }
  };
  const discardTransfer = async (id: string) => {
    if (discardingRef.current.has(id)) return;
    discardingRef.current.add(id);
    setDiscardingTransfers([...discardingRef.current]);
    try {
      await invoke('discard_transfer', { id, connectionId: connectionRef.current || null });
      setTransfers((previous) => previous.filter((t) => t.id !== id));
      setNotice('任务临时文件已清理，记录已移除');
    } catch (e) {
      notify(e);
    } finally {
      discardingRef.current.delete(id);
      setDiscardingTransfers([...discardingRef.current]);
    }
  };
  const submitRetry = async (id: string, connectedId: string) => {
    if (retryingRef.current.has(id) || discardingRef.current.has(id)) return;
    retryingRef.current.add(id);
    setRetryingTransfers([...retryingRef.current]);
    try {
      await invoke('retry_transfer', { id, connectionId: connectedId });
    } catch (e) {
      notify(e);
    } finally {
      retryingRef.current.delete(id);
      setRetryingTransfers([...retryingRef.current]);
    }
  };
  const reconnectTransfer = (id: string) => {
    const task = transfers.find((item) => item.id === id);
    if (!profile || !task || task.status !== 'failed' || connecting || !listenersReady) return;
    if (transfers.some((item) => !finished(item))) {
      setNotice('请等待其他任务结束，或取消后再重新连接；重连会断开当前会话');
      return;
    }
    if (discardingRef.current.has(id) || retryingRef.current.has(id)) return;
    if (!recoveryIntent.current.prepare(id, profile)) return;
    setRecoveryTask(task.name);
    setDialog({ profile });
  };
  const retryTransfer = async (id: string) => {
    const task = transfers.find((item) => item.id === id);
    if (!listenersReady || !task || task.status !== 'failed') return;
    const connectedId = connectionRef.current;
    if (!connectedId) {
      reconnectTransfer(id);
      return;
    }
    await submitRetry(id, connectedId);
  };
  const pauseTransfer = async (id: string, paused: boolean) => {
    try {
      await invoke('pause_transfer', { id, paused });
    } catch (e) {
      notify(e);
    }
  };
  const cancel = async (id: string) => {
    try {
      await invoke('cancel_transfer', { id });
      setTransfers((previous) =>
        previous.map((t) => (t.id === id && !finished(t) ? { ...t, status: 'cancelling' } : t)),
      );
    } catch (e) {
      notify(e);
    }
  };
  const fileOperation = (isRemote: boolean, kind: 'mkdir' | 'rename', sourcePath?: string) => {
    const listing = isRemote ? remote : local;
    const selected = isRemote ? remoteSelection : localSelection;
    if (!listing) return;
    const entry = listing.entries.find((f) => f.path === (sourcePath ?? selected[0]));
    setOperation({
      remote: isRemote,
      kind,
      name: kind === 'rename' ? (entry?.name ?? '') : '',
      source: entry?.path,
    });
  };
  const submitOperation = async () => {
    if (!operation) return;
    const name = operation.name.trim();
    if (!name || ['.', '..'].includes(name) || /[\\/]/.test(name)) {
      setNotice('请输入有效的文件名');
      return;
    }
    const listing = operation.remote ? remote : local;
    if (!listing) return;
    setOperationBusy(true);
    try {
      const path = joinPath(listing.path, name, operation.remote);
      const id = operation.remote ? connectionId : null;
      await invoke(
        operation.kind === 'mkdir' ? 'create_directory' : 'rename_entry',
        operation.kind === 'mkdir'
          ? { id, path }
          : { id, source: operation.source, destination: path },
      );
      setOperation(null);
      if (operation.remote) await loadRemote(listing.path);
      else await loadLocal(listing.path);
    } catch (e) {
      notify(e);
    } finally {
      setOperationBusy(false);
    }
  };
  const openSettings = async () => {
    try {
      setHosts(await invoke('known_hosts'));
      setSettings(true);
    } catch (e) {
      notify(e);
    }
  };
  const removeProfile = async () => {
    if (!deleteProfile) return;
    try {
      await invoke('delete_credentials', { profile: deleteProfile });
      await options.deleteProfile(deleteProfile.id);
      setDeleteProfile(null);
    } catch (e) {
      notify(e);
    }
  };
  const statusText = {
    paused: '已暂停',
    queued: '等待中',
    scanning: '扫描目录',
    connecting: '准备上传连接',
    prechecking: '检查远程目标',
    preparing: '创建目标目录',
    running: '传输中',
    completed: '已完成',
    cancelled: '已取消',
    failed: '失败',
    cancelling: '正在取消',
  };
  const running = transfers.filter((t) => !finished(t)).length;
  return {
    workspaceLayout,
    visible: options.visible,
    profiles,
    active,
    connectionId,
    local,
    remote,
    localSelection,
    setLocalSelection,
    remoteSelection,
    setRemoteSelection,
    localBusy,
    localReadCount,
    localReadError,
    remoteBusy,
    remoteReadCount,
    remoteReadError,
    connecting,
    setConnecting,
    notice,
    setNotice,
    queueOpen,
    setQueueOpen,
    transfers,
    setTransfers,
    dialog,
    setDialog,
    closeConnectionDialog,
    cancelTrust,
    recoveryTask,
    trust,
    setTrust,
    terminal,
    setTerminal,
    showHidden,
    setShowHidden,
    settings,
    setSettings,
    syncOpen,
    setSyncOpen,
    hosts,
    setHosts,
    plans,
    setPlans,
    overwrite,
    setOverwrite,
    operation,
    setOperation,
    operationBusy,
    deleteProfile,
    setDeleteProfile,
    listenersReady,
    profile,
    notify,
    loadLocal,
    loadRemote,
    refreshAfterDelete,
    disconnect,
    selectProfile,
    save,
    connect,
    planPaths,
    chooseUpload,
    startPlans,
    cancel,
    pauseTransfer,
    retryTransfer,
    reconnectTransfer,
    retryingTransfers,
    discardTransfer,
    discardingTransfers,
    fileOperation,
    submitOperation,
    openSettings,
    removeProfile,
    statusText,
    running,
  };
}
export type Workspace = ReturnType<typeof useTermRillWorkspace>;
