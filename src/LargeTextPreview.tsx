/** 大文本按滚动分块加载；关闭后丢弃在途结果，查找范围为已加载内容。 */
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import RemoteDocumentDialog from './RemoteDocumentDialog';
import DocumentLoadingDialog from './DocumentLoadingDialog';
import { message } from './types';
import { PreviewTextDecoder } from './previewTextDecoder';
type Chunk = {
  bytes: string;
  next: number;
  total: number;
  stamp: string;
  encoding?: string | null;
};
export default function LargeTextPreview({
  path,
  id,
  onClose,
  onEdit,
}: {
  path: string;
  id: string | null;
  onClose: () => void;
  onEdit?: () => void;
}) {
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [selectedEncoding, setSelectedEncoding] = useState('auto');
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState({ next: 0, total: 0 });
  const cursor = useRef({ next: 0, total: Infinity, stamp: null as string | null });
  const active = useRef(false);
  const disposed = useRef(false);
  const decoder = useRef<PreviewTextDecoder | null>(null);
  const encoding = useRef('UTF-8');
  const container = useRef<HTMLDivElement>(null);
  const request = useRef('');
  const release = (token: string) => {
    if (id) void invoke('close_text_reader', { id, request: token }).catch(console.warn);
  };
  const load = async () => {
    if (active.current || disposed.current || cursor.current.next >= cursor.current.total) return;
    // 原始读取总量有界；编辑器仅挂载可见行，但已加载正文仍占用内存。
    if (cursor.current.next >= 128 * 1024 * 1024) {
      setError('已加载 128 MiB，达到当前只读预览上限');
      return;
    }
    active.current = true;
    const token = request.current;
    setBusy(true);
    try {
      const chunk = await invoke<Chunk>('read_text_chunk', {
        id,
        request: token,
        path,
        offset: cursor.current.next,
        stamp: cursor.current.stamp,
      });
      if (disposed.current || request.current !== token) return;
      const binary = atob(chunk.bytes);
      const bytes = Uint8Array.from(binary, (value) => value.charCodeAt(0));
      decoder.current ??= new PreviewTextDecoder(
        selectedEncoding === 'auto' ? (chunk.encoding ?? undefined) : selectedEncoding,
      );
      const text = decoder.current.decode(bytes, chunk.next >= chunk.total);
      encoding.current = decoder.current.encoding;
      cursor.current = { next: chunk.next, total: chunk.total, stamp: chunk.stamp };
      setProgress({ next: chunk.next, total: chunk.total });
      setContent((value) => (value ?? '') + text);
      if (chunk.next >= chunk.total) release(token);
    } catch (reason) {
      if (!disposed.current && request.current === token) {
        setError(message(reason));
        release(token);
      }
    } finally {
      if (request.current === token) {
        active.current = false;
        if (!disposed.current) setBusy(false);
      }
    }
  };
  useEffect(() => {
    disposed.current = false;
    request.current = crypto.randomUUID();
    cursor.current = { next: 0, total: Infinity, stamp: null };
    decoder.current = null;
    encoding.current = 'UTF-8';
    setContent(null);
    setError('');
    setProgress({ next: 0, total: 0 });
    const token = request.current;
    // StrictMode 在开发模式同步执行 setup/cleanup/setup；先让生命周期稳定再发 IPC，
    // 避免第一次读取占锁、被丢弃后第二次又从头打开。真实卸载仍回收在途句柄。
    let started = false;
    const timer = window.setTimeout(() => {
      started = true;
      void load();
    }, 0);
    return () => {
      window.clearTimeout(timer);
      disposed.current = true;
      active.current = false;
      if (started) release(token);
    };
  }, [path, id, selectedEncoding]);
  // 首批内容未填满视口时继续补齐，避免没有垂直滚动条就无法触发加载。
  useEffect(() => {
    if (content === null || busy || error) return;
    const frame = requestAnimationFrame(() => {
      const scroller = container.current?.querySelector('.cm-scroller, .csv-table-scroll');
      if (
        scroller instanceof HTMLElement &&
        scroller.dataset.stopAutoLoading !== 'true' &&
        scroller.scrollHeight <= scroller.clientHeight + 1
      )
        void load();
    });
    return () => cancelAnimationFrame(frame);
  }, [content, busy, error]);
  if (content === null && !error)
    return <DocumentLoadingDialog path={path} detail="正在读取首段文本" onCancel={onClose} />;
  return (
    <div
      ref={container}
      onScrollCapture={(event) => {
        const node = event.target as HTMLElement;
        if (
          (node.classList.contains('cm-scroller') || node.classList.contains('csv-table-scroll')) &&
          node.dataset.stopAutoLoading !== 'true' &&
          node.scrollHeight - node.scrollTop - node.clientHeight < 1000 &&
          !error
        )
          void load();
      }}
    >
      <RemoteDocumentDialog
        document={{
          path,
          connectionId: id,
          content: content ?? '',
          original: content ?? '',
          revision: '',
          encoding: encoding.current,
        }}
        kind="text"
        busy={false}
        error={error}
        readOnly
        sourceOnly
        textComplete={progress.next >= progress.total}
        onChange={() => {}}
        onSave={() => {}}
        onDownload={() => {}}
        onClose={onClose}
        previewNotice={
          <span>
            <label className="text-encoding-control">
              编码
              <select
                aria-label="文本编码"
                value={selectedEncoding}
                onChange={(event) => setSelectedEncoding(event.target.value)}
              >
                <option value="auto">自动识别</option>
                {[
                  'UTF-8',
                  'UTF-16 LE',
                  'UTF-16 BE',
                  'GB18030',
                  'GBK',
                  'Big5',
                  'Shift_JIS',
                  'EUC-JP',
                  'ISO-2022-JP',
                  'EUC-KR',
                  'windows-1252',
                  'windows-1251',
                  'ISO-8859-2',
                  'ISO-8859-7',
                  'ISO-8859-8',
                  'windows-1254',
                  'windows-1256',
                  'windows-874',
                  'KOI8-R',
                  'macintosh',
                ].map((name) => (
                  <option key={name} value={name}>
                    {name}
                  </option>
                ))}
              </select>
            </label>{' '}
            {error
              ? '读取失败'
              : busy
                ? '正在加载后续内容…'
                : progress.next >= progress.total
                  ? '已加载全部内容'
                  : '向下滚动自动加载'}{' '}
            · 已读取 {(progress.next / 1024 / 1024).toFixed(1)} /{' '}
            {(progress.total / 1024 / 1024).toFixed(1)} MiB · 查找范围为已加载内容
            {!error && progress.next < progress.total && (
              <button
                type="button"
                className="button secondary"
                disabled={busy}
                onClick={() => void load()}
              >
                加载更多
              </button>
            )}
            {onEdit && (
              <button type="button" className="button secondary" disabled={busy} onClick={onEdit}>
                加载全文并编辑
              </button>
            )}
          </span>
        }
      />
    </div>
  );
}
