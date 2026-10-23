/** 压缩包操作状态：原连接绑定、取消标记和过期异步结果清理。 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Channel, invoke } from '@tauri-apps/api/core';
import { basename, message } from './types';
import type { ArchiveReply } from './archiveTypes';
import { ArchivePrefetch } from './archivePrefetch';
import { parsePackProgress, updatePackReport, packStageLabels } from './archivePackMetrics';
import type { PackReport } from './archivePackMetrics';
import {
  archiveWriteFormat,
  archiveNameForFormat,
  type ArchiveWriteFormat,
} from './archiveCapabilities';
type Operation = 'list' | 'preview' | 'verify' | 'extract' | 'remove' | 'add' | 'pack';
export type ArchiveDialogProps = {
  id: string | null;
  path: string;
  packSources?: string[];
  localDirectory: string;
  onClose: () => void;
  onChanged: () => void;
  onOpenArchive: (path: string) => void;
};
export function useArchiveController({
  id,
  path,
  packSources,
  localDirectory,
  onChanged,
}: ArchiveDialogProps) {
  const [listing, setListing] = useState<ArchiveReply | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useState('');
  const [password, setPassword] = useState('');
  const [compression, setCompression] = useState('normal');
  const [busy, setBusy] = useState(false);
  const [phase, setPhase] = useState('');
  const [packReport, setPackReport] = useState<PackReport | null>(null);
  // 成功结果绑定本次请求，不能随下一次表单编辑改变保存位置。
  const [packResult, setPackResult] = useState<{ path: string; sources: string[] } | null>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const parent = path.slice(0, Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\')) + 1);
  const defaultTarget = `${parent}${basename(path).replace(/\.(?:tar\.(?:gz|bz2|xz)|zip|tar|tgz|tbz2?|txz|gz|bz2|xz|rar|7z)$/i, '')}`;
  const [targetMode, setTargetMode] = useState('folder');
  const [target, setTarget] = useState(defaultTarget);
  const [policy, setPolicy] = useState('stop');
  const [sourceText, setSourceText] = useState('');
  const [adding, setAdding] = useState(false);
  const [packName, setPackName] = useState(basename(path));
  const [packDirectory, setPackDirectory] = useState(parent);
  const separator = id || !parent.includes('\\') ? '/' : '\\';
  const packTarget = `${packDirectory.replace(/[\\/]+$/, '')}${separator}${packName}`;
  const absoluteDirectory = id
    ? packDirectory.startsWith('/') && !packDirectory.includes('\\')
    : /^(?:[a-z]:[\\/]|\\\\[^\\]+\\[^\\]+|\/)/i.test(packDirectory);
  const packValid =
    !!packSources?.length &&
    absoluteDirectory &&
    !!packName &&
    !!archiveWriteFormat(packName) &&
    !/[\\/:<>"|?*\u0000-\u001f]/.test(packName) &&
    !packName.endsWith(' ') &&
    !packName.endsWith('.');
  const [preview, setPreview] = useState<{ name: string; bytes: string } | null>(null);
  const [pendingPreview, setPendingPreview] = useState<string | null>(null);
  const snapshot = useRef(crypto.randomUUID());
  const backgroundLoad = useRef<
    (name: string) => { name: string; done: Promise<string | null>; cancel: () => void }
  >(() => {
    throw new Error('预取尚未初始化');
  });
  const prefetch = useRef<ArchivePrefetch | null>(null);
  if (!prefetch.current)
    prefetch.current = new ArchivePrefetch((name) => backgroundLoad.current(name));
  const previews = prefetch.current;
  const operationId = useRef<string | null>(null);
  const operationGeneration = useRef(0);
  const disposed = useRef(false);
  const active = useRef(false);
  const panel = useRef<HTMLElement>(null);
  const supported = /\.(?:zip|tar|tgz|tbz2?|txz|gz|bz2|xz|rar|7z)$/i.test(path);
  const entries = useMemo(
    () =>
      listing?.entries.filter((entry) => entry.name.toLowerCase().includes(query.toLowerCase())) ??
      [],
    [listing, query],
  );
  backgroundLoad.current = (name) => {
    let request: string | null = null;
    let cancelled = false;
    const cancel = () => {
      cancelled = true;
      if (request) void invoke('cancel_preview', { request }).catch(() => {});
    };
    const done = (async () => {
      try {
        request = await invoke<string>('begin_preview');
        if (cancelled || disposed.current) return null;
        const progress = new Channel<string>();
        progress.onmessage = () => {};
        const reply = await invoke<ArchiveReply>('archive_operation', {
          id,
          operationId: request,
          onProgress: progress,
          request: {
            operation: 'preview',
            snapshot: snapshot.current,
            path,
            selected: [name],
            password: password || null,
            revision: listing?.revision ?? null,
          },
        });
        return cancelled || disposed.current ? null : reply.bytes;
      } finally {
        if (request) await invoke('cancel_preview', { request }).catch(() => {});
      }
    })();
    return { name, done, cancel };
  };
  useEffect(() => {
    if (listing) {
      previews.configure(
        JSON.stringify([listing.revision, password]),
        id && /\.zip$/i.test(path) ? listing.entries : [],
      );
    }
  }, [listing, password, id, path, previews]);
  const prefetchEntries = useCallback(
    (names: string[]) => {
      const candidates = names.flatMap((name) =>
        name.endsWith('/')
          ? (listing?.entries
              .filter((entry) => entry.name.startsWith(name) && !entry.directory)
              .map((entry) => entry.name) ?? [])
          : [name],
      );
      previews.hint(candidates);
    },
    [listing, previews],
  );
  const cancel = () => {
    operationGeneration.current++;
    previews.reset();
    if (operationId.current)
      void invoke('cancel_preview', { request: operationId.current }).catch(() => {});
  };
  const run = async (operation: Operation, extra: Record<string, unknown> = {}) => {
    if (active.current) return;
    const choices = (extra.selected as string[] | undefined) ?? selected;
    const cached = operation === 'preview' ? previews.get(choices[0]) : undefined;
    if (cached && choices.length === 1) {
      setError('');
      setPreview({ name: choices[0], bytes: cached });
      previews.resume(choices[0]);
      return;
    }
    if (
      operation === 'preview' &&
      listing?.bytes &&
      choices.length === 1 &&
      listing.entries.filter((entry) => !entry.directory).length === 1 &&
      listing.entries.find((entry) => !entry.directory)?.name === choices[0]
    ) {
      setError('');
      setPreview({ name: choices[0], bytes: listing.bytes });
      return;
    }
    active.current = true;
    const generation = operationGeneration.current;
    setBusy(true);
    if (operation === 'preview')
      setPendingPreview(((extra.selected as string[] | undefined) ?? selected)[0]);
    setError('');
    setNotice('');
    setPhase('准备操作');
    if (operation === 'pack') {
      setPackResult(null);
      const data = {
        kind: 'packProgress' as const,
        stage: 'scan' as const,
        files: 0,
        directories: 0,
        processedFiles: 0,
        totalBytes: 0,
        processedBytes: 0,
        zipBytes: 0,
        uploadedBytes: 0,
        currentFile: '',
      };
      setPackReport(updatePackReport(null, data));
    }
    try {
      const prefetched = await previews.foreground(
        operation === 'preview' ? choices[0] : undefined,
      );
      if (disposed.current || generation !== operationGeneration.current) return;
      if (['list', 'pack', 'add', 'remove'].includes(operation)) previews.reset();
      if (prefetched && operation === 'preview' && choices.length === 1) {
        previews.put(choices[0], prefetched);
        setPreview({ name: choices[0], bytes: prefetched });
        return;
      }
      const request = await invoke<string>('begin_preview');
      operationId.current = request;
      if (disposed.current || generation !== operationGeneration.current) {
        await invoke('cancel_preview', { request });
        return;
      }
      const progress = new Channel<string>();
      progress.onmessage = (value) => {
        if (disposed.current || generation !== operationGeneration.current) return;
        const data = parsePackProgress(value);
        if (data) {
          setPhase(packStageLabels[data.stage]);
          if (operation === 'pack') setPackReport((previous) => updatePackReport(previous, data));
        } else setPhase(value);
      };
      const reply = await invoke<ArchiveReply>('archive_operation', {
        id,
        operationId: request,
        onProgress: progress,
        request: {
          operation,
          snapshot: snapshot.current,
          path,
          selected,
          sources: [],
          target: null,
          password: password || null,
          compression,
          revision: listing?.revision ?? null,
          overwrite: policy === 'overwrite',
          skipExisting: policy === 'skip',
          download: false,
          ...extra,
        },
      });
      if (disposed.current || generation !== operationGeneration.current) return;
      if (operation === 'list') {
        setListing(reply);
        setSelected([]);
        const file = reply.entries.find((entry) => !entry.directory);
        setPreview(reply.bytes && file ? { name: file.name, bytes: reply.bytes } : null);
      } else if (operation === 'preview' && reply.bytes) {
        previews.put(choices[0], reply.bytes);
        const previewSelection = (extra.selected as string[] | undefined) ?? selected;
        setPreview({ name: previewSelection[0], bytes: reply.bytes });
      } else {
        setNotice(reply.message);
        if (operation === 'pack') {
          setPackResult({ path: extra.path as string, sources: [...(extra.sources as string[])] });
        }
      }
      if (['pack', 'add', 'remove', 'extract'].includes(operation)) onChanged();
      if (operation === 'add' || operation === 'remove') {
        setListing(null);
        setSelected([]);
        setAdding(false);
        const reloadId = await invoke<string>('begin_preview');
        operationId.current = reloadId;
        if (disposed.current) {
          await invoke('cancel_preview', { request: reloadId });
          return;
        }
        const updated = await invoke<ArchiveReply>('archive_operation', {
          id,
          operationId: reloadId,
          onProgress: progress,
          request: {
            operation: 'list',
            snapshot: snapshot.current,
            path,
            target: null,
            password: password || null,
            revision: null,
          },
        });
        if (!disposed.current) setListing(updated);
      }
    } catch (reason) {
      if (!disposed.current && ['pack', 'add', 'remove', 'extract'].includes(operation))
        onChanged();
      if (!disposed.current) {
        if (operation === 'pack')
          setPackReport((previous) =>
            previous ? { ...previous, finishedAt: previous.finishedAt ?? performance.now() } : null,
          );
        const mutating = ['pack', 'add', 'remove', 'extract'].includes(operation);
        setError(
          message(reason) +
            (mutating
              ? '。失败或取消时已完成的文件可能保留，请刷新目标目录查看。'
              : '。读取失败，可刷新远程目录检查连接；仍失败时重新连接后再读取。'),
        );
      }
    } finally {
      if (operationId.current)
        void invoke('cancel_preview', { request: operationId.current }).catch(() => {});
      operationId.current = null;
      active.current = false;
      if (disposed.current)
        void invoke('release_archive_snapshot', { id, snapshot: snapshot.current, path }).catch(
          () => {},
        );
      if (!disposed.current) {
        setPendingPreview(null);
        if (operation === 'pack')
          setPackReport((previous) =>
            previous ? { ...previous, finishedAt: previous.finishedAt ?? performance.now() } : null,
          );
        setBusy(false);
        setPhase('');
        previews.resume(operation === 'preview' ? choices[0] : undefined);
      }
    }
  };
  useEffect(() => {
    disposed.current = false;
    const mountedSnapshot = crypto.randomUUID();
    snapshot.current = mountedSnapshot;
    const previous = document.activeElement as HTMLElement | null;
    panel.current?.focus();
    // StrictMode 同步执行 setup/cleanup/setup；稳定挂载后才启动 IPC，
    // 否则第一轮取消读取，第二轮又被 active 标记拦住，列表始终为空。
    const timer = !packSources && supported ? window.setTimeout(() => void run('list'), 0) : null;
    if (timer !== null) {
      setBusy(true);
      setPhase('正在读取压缩包');
    }
    return () => {
      if (timer !== null) window.clearTimeout(timer);
      disposed.current = true;
      cancel();
      previews.reset();
      void invoke('release_archive_snapshot', { id, snapshot: mountedSnapshot, path }).catch(
        () => {},
      );
      previous?.focus();
    };
    // 此弹窗以连接和路径为 key，每次打开独立创建状态，避免切换来源后复用结果。
  }, []);
  const toggle = (name: string) =>
    setSelected((values) =>
      values.includes(name) ? values.filter((value) => value !== name) : [...values, name],
    );
  const chooseTarget = (mode: string) => {
    setTargetMode(mode);
    if (mode === 'folder') setTarget(defaultTarget);
    if (mode === 'current') setTarget(parent);
  };
  const chooseSources = async () => {
    try {
      const paths = await invoke<string[]>('pick_path', { kind: 'files' });
      if (paths.length) setSourceText(paths.join('\n'));
    } catch (reason) {
      setError(message(reason));
    }
  };
  const extract = (download: boolean) => {
    if (download && !selected.length) return;
    void run('extract', { target: download ? localDirectory : target, download });
  };
  return {
    compression,
    setCompression,
    setPackFormat: (format: ArchiveWriteFormat) => {
      setPackName((name) => archiveNameForFormat(name, format));
      setCompression('normal');
      if (format !== 'zip' && format !== '7z') setPassword('');
    },
    listing,
    selected,
    query,
    setQuery,
    password,
    setPassword,
    busy,
    phase,
    packReport,
    packResult,
    error,
    notice,
    targetMode,
    target,
    setTarget,
    setTargetMode,
    policy,
    setPolicy,
    sourceText,
    setSourceText,
    adding,
    setAdding,
    packTarget,
    packName,
    setPackName: (name: string) => {
      setPackName(name);
      const format = archiveWriteFormat(name);
      if (format !== 'zip' && format !== '7z') {
        setPassword('');
        setCompression('normal');
      }
    },
    packDirectory,
    setPackDirectory,
    packValid,
    preview,
    pendingPreview,
    setPreview,
    panel,
    supported,
    entries,
    cancel,
    run,
    prefetchEntries,
    toggle,
    chooseTarget,
    chooseSources,
    extract,
    setSelected,
  };
}
