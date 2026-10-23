import { lazy, Suspense, useEffect, useState, useRef } from 'react';
import DocumentLoadingDialog from './DocumentLoadingDialog';
import type { ReactNode } from 'react';
import { Channel, invoke } from '@tauri-apps/api/core';
import DeleteConfirmationDialog from './DeleteConfirmationDialog';
import type { Workspace } from './useTermRillWorkspace';

import { fileKind } from './fileKind';
const LargeTextPreview = lazy(() => import('./LargeTextPreview'));
const RemoteDocumentDialog = lazy(() => import('./RemoteDocumentDialog'));

type Document = { content: string; revision: string; encoding?: string };
type Editing = Document & { path: string; connectionId: string | null; original: string };

export type FileActionHandlers = {
  busy: boolean;
  requestDelete: (paths: string[], remote: boolean) => void;
  openRemote: (path: string) => void;
  openLocal: (path: string) => void;
};

/** 本地/远程共用文件查看器；远程保存绑定打开文件时的连接，本地无需连接。 */
export default function FileActions({
  workspace: w,
  children,
}: {
  workspace: Workspace;
  children: (actions: FileActionHandlers) => ReactNode;
}) {
  const [deletion, setDeletion] = useState<{ paths: string[]; id: string | null } | null>(null);
  const [largePreview, setLargePreview] = useState<{ path: string; id: string | null } | null>(
    null,
  );
  const [editing, setEditing] = useState<Editing | null>(null);
  const [reading, setReading] = useState<{ path: string; remote: boolean } | null>(null);
  const readGeneration = useRef(0);
  useEffect(
    () => () => {
      readGeneration.current++;
    },
    [],
  );
  const cancelRead = () => {
    readGeneration.current++;
    setReading(null);
    setBusy(false);
  };
  const [busy, setBusy] = useState(false);
  const [recursive, setRecursive] = useState(false);
  const [error, setError] = useState('');
  const [deleteStarted, setDeleteStarted] = useState<number | null>(null);
  const [deleteElapsed, setDeleteElapsed] = useState(0);
  const [deletePhase, setDeletePhase] = useState<'deleting' | 'refreshing' | null>(null);
  const [deleteDetail, setDeleteDetail] = useState('');
  const [deleteStageOffset, setDeleteStageOffset] = useState(0);
  const [deleteHistory, setDeleteHistory] = useState('');
  useEffect(() => {
    if (deleteStarted === null) return;
    // 单调时钟计时，切换标签后仍计算真实经过时间，不累计定时器次数。
    const timer = window.setInterval(
      () => setDeleteElapsed(performance.now() - deleteStarted),
      200,
    );
    return () => window.clearInterval(timer);
  }, [deleteStarted]);
  const open = async (path = w.remoteSelection[0], remote = true, edit = false) => {
    const id = remote ? w.connectionId : null;
    if (!path || (remote && !id)) return;
    const entry = (remote ? w.remote : w.local)?.entries.find((value) => value.path === path);
    if (
      fileKind(path) === 'text' &&
      entry &&
      (entry.size > 2 * 1024 * 1024 || (remote && !edit && entry.size > 64 * 1024))
    ) {
      setLargePreview({ path, id });
      return;
    }
    if (fileKind(path) !== 'text') {
      setEditing({ path, connectionId: id, content: '', original: '', revision: '' });
      setError('');
      return;
    }
    setBusy(true);
    const generation = ++readGeneration.current;
    setReading({ path, remote });
    try {
      const document = await invoke<Document>(remote ? 'open_remote_text' : 'open_local_text', {
        id,
        path,
      });
      if (generation !== readGeneration.current) return;
      setEditing({ ...document, path, connectionId: id, original: document.content });
      setError('');
    } catch (e) {
      if (generation === readGeneration.current) {
        if (String(e).includes('2 MiB')) setLargePreview({ path, id });
        else w.notify(e);
      }
    } finally {
      if (generation === readGeneration.current) {
        setBusy(false);
        setReading(null);
      }
    }
  };
  const save = async () => {
    if (!editing) return;
    setBusy(true);
    try {
      const document = await invoke<Document>(
        editing.connectionId ? 'save_remote_text' : 'save_local_text',
        {
          id: editing.connectionId,
          path: editing.path,
          content: editing.content,
          revision: editing.revision,
        },
      );
      setEditing({ ...editing, ...document, original: document.content });
      setError('');
      if (!editing.connectionId) await w.loadLocal(w.local?.path);
      if (w.remote && editing.connectionId && w.connectionId === editing.connectionId)
        await w.loadRemote(w.remote.path);
    } catch (e) {
      setError(String((e as { message?: string }).message ?? e));
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    if (!deletion) return;
    const started = performance.now();
    setDeleteStarted(started);
    setDeleteElapsed(0);
    setDeletePhase('deleting');
    setDeleteDetail('等待删除任务');
    setDeleteStageOffset(0);
    setDeleteHistory('');
    const onProgress = new Channel<string>();
    let previousPhase = '等待删除任务';
    let phaseStarted = started;
    const phaseTimes: string[] = [];
    let acceptingProgress = true;
    onProgress.onmessage = (phase) => {
      if (!acceptingProgress || phase === previousPhase) return;
      const now = performance.now();
      if (now - phaseStarted >= 100)
        phaseTimes.push(`${previousPhase} ${((now - phaseStarted) / 1000).toFixed(1)} 秒`);
      setDeleteDetail(phase);
      setDeleteHistory(phaseTimes.join(' / '));
      setDeleteStageOffset(now - started);
      setDeleteElapsed(now - started);
      previousPhase = phase;
      phaseStarted = now;
      console.info('[TermRill delete]', { phase, totalMs: Math.round(now - started) });
    };
    setBusy(true);
    setError('');
    let completedCount = 0;
    let deleted = false;
    try {
      // 多选删除可能部分成功；失败后刷新列表并展示错误，不能宣称整批回滚。
      for (const path of deletion.paths) {
        const entry = deletion.id
          ? w.remote?.entries.find((candidate) => candidate.path === path)
          : undefined;
        await invoke('delete_entry', {
          id: deletion.id,
          path,
          recursive,
          fileOnly: entry ? !entry.directory || entry.symlink : false,
          onProgress,
        });
        completedCount += 1;
      }
      deleted = true;
    } catch (e) {
      // 重试只针对剩余项目，避免已删除路径的“不存在”错误阻断后续删除。
      setDeletion({ ...deletion, paths: deletion.paths.slice(completedCount) });
      setError(String((e as { message?: string }).message ?? e));
    } finally {
      acceptingProgress = false;
      const refreshStarted = performance.now();
      if (refreshStarted - phaseStarted >= 100)
        phaseTimes.push(
          `${previousPhase} ${((refreshStarted - phaseStarted) / 1000).toFixed(1)} 秒`,
        );
      if (deleted && deletion.id) {
        const elapsed = refreshStarted - started;
        setDeletion(null);
        setDeleteStarted(null);
        setDeletePhase(null);
        setDeleteElapsed(elapsed);
        setBusy(false);
        w.notify(`删除完成，用时 ${(elapsed / 1000).toFixed(1)} 秒；列表正在后台刷新`);
        // 刷新不再阻塞已完成的删除，也不能在返回后覆盖下一次操作的弹窗状态。
        void w
          .refreshAfterDelete(deletion.id, deletion.paths)
          .then(() =>
            invoke('delete_timing', {
              totalMs: Math.round(performance.now() - started),
              refreshMs: Math.round(performance.now() - refreshStarted),
              success: true,
            }),
          )
          .catch(console.warn);
        return;
      }
      setDeleteHistory(phaseTimes.join(' / '));
      setDeleteStageOffset(refreshStarted - started);
      setDeleteElapsed(refreshStarted - started);
      setDeletePhase('refreshing');
      try {
        if (deletion.id) {
          if (w.connectionId === deletion.id && w.remote) await w.loadRemote(w.remote.path);
        } else await w.loadLocal(w.local?.path);
      } finally {
        const elapsed = performance.now() - started;
        void invoke('delete_timing', {
          totalMs: Math.round(elapsed),
          refreshMs: Math.round(performance.now() - refreshStarted),
          success: deleted,
        }).catch(console.warn);
        console.info('[TermRill delete]', {
          success: deleted,
          totalMs: Math.round(elapsed),
          refreshMs: Math.round(performance.now() - refreshStarted),
          stages: phaseTimes,
        });
        setDeleteElapsed(elapsed);
        setDeleteStarted(null);
        setDeletePhase(null);
        setBusy(false);
        if (deleted) {
          setDeletion(null);
          const refreshElapsed = performance.now() - refreshStarted;
          w.notify(
            `删除完成，总用时 ${(elapsed / 1000).toFixed(1)} 秒；${phaseTimes.join(' / ')} / 刷新列表 ${(refreshElapsed / 1000).toFixed(1)} 秒`,
          );
        }
      }
    }
  };
  const requestDelete = (paths: string[], remote: boolean) => {
    if (busy || !paths.length || (remote && !w.connectionId)) return;
    setError('');
    setRecursive(false);
    setDeleteElapsed(0);
    setDeletion({ paths: [...paths], id: remote ? w.connectionId : null });
  };
  return (
    <>
      {children({
        busy: busy || !!deletion || !!editing || !!largePreview,
        requestDelete,
        openLocal: (path) => {
          if (!busy && !editing && !deletion && !largePreview) void open(path, false);
        },
        openRemote: (path) => {
          if (!busy && !editing && !deletion && !largePreview) void open(path);
        },
      })}
      {largePreview && (
        <Suspense
          fallback={
            <DocumentLoadingDialog
              path={largePreview.path}
              detail="正在准备大文本预览"
              onCancel={() => setLargePreview(null)}
            />
          }
        >
          <LargeTextPreview
            key={`${largePreview.id}:${largePreview.path}`}
            {...largePreview}
            onEdit={
              largePreview.id &&
              w.remote?.entries.some(
                (entry) => entry.path === largePreview.path && entry.size <= 2 * 1024 * 1024,
              )
                ? () => {
                    const path = largePreview.path;
                    setLargePreview(null);
                    void open(path, true, true);
                  }
                : undefined
            }
            onClose={() => setLargePreview(null)}
          />
        </Suspense>
      )}
      {deletion && (
        <DeleteConfirmationDialog
          paths={deletion.paths}
          remote={!!deletion.id}
          entries={(deletion.id ? w.remote : w.local)?.entries ?? []}
          recursive={recursive}
          busy={busy}
          error={error}
          elapsedMs={deleteElapsed}
          phase={deletePhase}
          detail={deleteDetail}
          stageElapsedMs={Math.max(0, deleteElapsed - deleteStageOffset)}
          history={deleteHistory}
          onRecursiveChange={setRecursive}
          onClose={() => setDeletion(null)}
          onConfirm={() => void remove()}
        />
      )}
      {reading && (
        <DocumentLoadingDialog
          path={reading.path}
          detail={`正在获取${reading.remote ? '远程' : '本地'}文件内容，请稍候`}
          onCancel={cancelRead}
        />
      )}
      {editing && (
        <Suspense
          fallback={
            <div className="modal-backdrop">
              <div className="modal">正在加载文件查看器…</div>
            </div>
          }
        >
          <RemoteDocumentDialog
            document={editing}
            kind={fileKind(editing.path)}
            busy={busy}
            error={error}
            onChange={(content) => setEditing({ ...editing, content })}
            onSave={() => void save()}
            onClose={() => setEditing(null)}
            onDownload={() => {
              if (
                !w.connectionId ||
                (editing.connectionId && w.connectionId !== editing.connectionId)
              ) {
                w.notify('请连接目标服务器后传输');
                return;
              }
              const path = editing.path;
              setEditing(null);
              void w.planPaths([path], editing.connectionId ? 'download' : 'upload');
            }}
          />
        </Suspense>
      )}
    </>
  );
}
