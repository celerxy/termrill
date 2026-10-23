import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { Workspace } from './useTermRillWorkspace';
import DirectorySyncView from './DirectorySyncView';
import DirectorySyncSettings from './DirectorySyncSettings';
import DirectorySyncDiff from './DirectorySyncDiff';
import { recoveryProfileIdentity } from './transferRecovery';
import type { SyncRule } from './directorySyncModel';

export type Difference = {
  relative: string;
  remote: string;
  status: string;
  size: number;
  directory: boolean;
};
const selectable = (row: Difference) => ['new', 'changed'].includes(row.status);

/** 比较绑定启动时的两端目录；只将明确选择的差异交给既有传输确认流程。 */
export default function DirectorySyncDialog({ workspace: w }: { workspace: Workspace }) {
  const [direction, setDirection] = useState<'upload' | 'download'>('upload');
  const [settings, setSettings] = useState<Omit<SyncRule, 'id' | 'name'>>({
    local: w.local?.path ?? '',
    remote: w.remote?.path ?? '',
    direction: 'upload',
    options: { quick: false, exclude: [] },
  });
  const [diff, setDiff] = useState<Difference | null>(null);
  const [comparedAt, setComparedAt] = useState('');
  const scope = w.profile ? recoveryProfileIdentity(w.profile) : '';
  const [rows, setRows] = useState<Difference[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  const [snapshot, setSnapshot] = useState<{
    local: string;
    remote: string;
    connectionId: string;
    scope: string;
  } | null>(null);
  const request = useRef<string | null>(null);
  const startedAt = useRef(0);
  useEffect(() => {
    const id = request.current;
    request.current = null;
    if (id) void invoke('cancel_transfer', { id }).catch(() => {});
    setBusy(false);
    setSnapshot(null);
    setRows([]);
    setSelected(new Set());
    setDiff(null);
    // 连接身份变化后旧比较与旧规则不能继续写入新服务器。
  }, [scope, w.connectionId]);
  useEffect(() => {
    if (!busy) return;
    const timer = window.setInterval(
      () => setElapsedSeconds(Math.floor((performance.now() - startedAt.current) / 1000)),
      1000,
    );
    return () => window.clearInterval(timer);
  }, [busy]);
  useEffect(
    () => () => {
      const id = request.current;
      request.current = null;
      if (id) void invoke('cancel_transfer', { id }).catch(() => {});
    },
    [],
  );
  async function compare() {
    if (busy || !w.connectionId || !settings.local || !settings.remote) return;
    const context = {
      local: settings.local,
      remote: settings.remote,
      connectionId: w.connectionId,
      scope,
    };
    setDiff(null);
    const id = crypto.randomUUID();
    request.current = id;
    startedAt.current = performance.now();
    setElapsedSeconds(0);
    setBusy(true);
    setRows([]);
    setSelected(new Set());
    setError('');
    setSnapshot(null);
    try {
      const result = await invoke<Difference[]>('compare_directories', {
        ...context,
        direction,
        options: {
          ...settings.options,
          exclude: settings.options.exclude.map((item) => item.trim()).filter(Boolean),
        },
        id,
      });
      // 关闭后的旧请求不能重新显示结果，也不能覆盖新的目录快照。
      if (request.current !== id) return;
      setRows(result);
      setSelected(new Set(result.filter(selectable).map((row) => row.remote)));
      setSnapshot(context);
      setComparedAt(new Date().toISOString());
    } catch (error) {
      if (request.current === id)
        setError(
          String(typeof error === 'object' && error && 'message' in error ? error.message : error),
        );
    } finally {
      if (request.current === id) {
        request.current = null;
        setBusy(false);
      }
    }
  }
  function changeDirection(next: 'upload' | 'download') {
    if (busy || next === direction) return;
    setDirection(next);
    setSettings((previous) => ({ ...previous, direction: next }));
    setDiff(null);
    setRows([]);
    setSnapshot(null);
    setSelected(new Set());
    setError('');
  }
  function toggleSelection(path: string) {
    setSelected((previous) => {
      const next = new Set(previous);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }
  function syncSelected() {
    if (
      !snapshot ||
      busy ||
      !selected.size ||
      w.connectionId !== snapshot.connectionId ||
      scope !== snapshot.scope ||
      !w.listenersReady
    )
      return;
    // 继续走已有覆盖确认；预览中的选择不能直接写入目标目录。
    w.setOverwrite(false);
    w.setPlans([
      {
        ...snapshot,
        direction,
        selection: [...selected],
        name: '目录差异同步',
        syncReport: {
          ...settings,
          name: '目录差异同步',
          comparedAt,
          selectedCount: selected.size,
          selected: rows
            .filter((row) => selected.has(row.remote))
            .slice(0, 1000)
            .map(({ relative, status, size }) => ({ relative, status, size })),
        },
      },
    ]);
    w.setSyncOpen(false);
  }
  return (
    <DirectorySyncView
      localPath={snapshot?.local ?? settings.local}
      remotePath={snapshot?.remote ?? settings.remote}
      settings={
        <DirectorySyncSettings
          key={scope}
          scope={scope}
          value={settings}
          busy={busy}
          onChange={(next) => {
            if (busy) return;
            setSettings(next);
            setDirection(next.direction);
            setSnapshot(null);
            setRows([]);
            setSelected(new Set());
            setDiff(null);
          }}
        />
      }
      quick={settings.options.quick}
      onDiff={(row) => setDiff(row)}
      diff={
        diff && snapshot && w.connectionId === snapshot.connectionId && scope === snapshot.scope ? (
          <DirectorySyncDiff
            local={snapshot.local}
            remote={diff.remote}
            relative={diff.relative}
            connectionId={snapshot.connectionId}
            onClose={() => setDiff(null)}
          />
        ) : null
      }
      direction={direction}
      onDirectionChange={changeDirection}
      rows={rows}
      selected={selected}
      onToggle={toggleSelection}
      onSelectAll={() => setSelected(new Set(rows.filter(selectable).map((row) => row.remote)))}
      onClearSelection={() => setSelected(new Set())}
      busy={busy}
      elapsedSeconds={elapsedSeconds}
      error={error}
      hasCompared={Boolean(snapshot)}
      canCompare={Boolean(w.connectionId && w.local && w.remote)}
      canSync={Boolean(
        !busy &&
        snapshot &&
        selected.size &&
        w.connectionId === snapshot.connectionId &&
        scope === snapshot.scope &&
        w.listenersReady,
      )}
      onCompare={() => void compare()}
      onClose={() => w.setSyncOpen(false)}
      onSync={syncSelected}
    />
  );
}
