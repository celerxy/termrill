/** 绑定打开时的目录及连接；先枚举完整目录检查冲突，再顺序调用原有不覆盖改名接口。 */
import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { X, Pencil } from 'lucide-react';
import { type FileEntry, type Listing, joinPath, message } from './types';
import { previewBatchRename, runRenamePlan, type RenameResult } from './batchRename';
import ErrorNotice from './ErrorNotice';
import './batchRename.css';

export type BatchRenameRequest = { id: string | null; listing: Listing; entries: FileEntry[] };
const statusText = {
  completed: '已完成',
  failed: '失败／请核对目标',
  unchanged: '未改变',
  'not-started': '未执行',
};
export default function BatchRenameDialog({
  request,
  currentConnection,
  onClose,
  onChanged,
}: {
  request: BatchRenameRequest;
  currentConnection: () => string | null;
  onClose: () => void;
  onChanged: () => void;
}) {
  const [template, setTemplate] = useState('{n}-{stem}{ext}');
  const [start, setStart] = useState(1);
  const [padding, setPadding] = useState(3);
  const [busy, setBusy] = useState(false);
  const [phase, setPhase] = useState('');
  const [error, setError] = useState('');
  const [results, setResults] = useState<RenameResult[] | null>(null);
  const cancel = useRef(false);
  const locked = useRef(false);
  const disposed = useRef(false);
  const panel = useRef<HTMLElement>(null);
  const windows =
    !request.id &&
    (/^[a-z]:/i.test(request.listing.path) || request.listing.path.startsWith('\\\\'));
  const preview = useMemo(
    () =>
      previewBatchRename(
        request.entries,
        template,
        start,
        padding,
        request.listing.entries.map((entry) => entry.name),
        windows,
      ),
    [request, template, start, padding, windows],
  );
  useEffect(() => {
    disposed.current = false;
    cancel.current = false;
    const previous = document.activeElement as HTMLElement | null;
    panel.current?.querySelector<HTMLInputElement>('input')?.focus();
    return () => {
      disposed.current = true;
      cancel.current = true;
      previous?.focus();
    };
  }, []);
  const stopped = () =>
    cancel.current || (request.id !== null && currentConnection() !== request.id);
  async function run() {
    if (locked.current || preview.some((row) => row.error) || !preview.some((row) => row.changed))
      return;
    locked.current = true;
    cancel.current = false;
    setBusy(true);
    setError('');
    try {
      const existing: FileEntry[] = [];
      let offset = 0;
      while (true) {
        if (stopped()) throw new Error('已停止，尚未开始改名');
        setPhase(`检查目录冲突 · 已读取 ${existing.length} 项`);
        const listing = await invoke<Listing>(request.id ? 'list_remote_page' : 'list_local_page', {
          id: request.id,
          path: request.listing.path,
          offset,
          query: '',
          showHidden: true,
        });
        existing.push(...listing.entries);
        if (existing.length > 10000)
          throw new Error('目录超过 10000 项，当前批量改名不执行；请使用单项重命名');
        if (!listing.page?.hasMore) break;
        const next = (listing.page.offset ?? offset) + listing.page.size;
        if (next <= offset) throw new Error('目录分页未前进，已停止检查');
        offset = next;
      }
      for (const entry of request.entries) {
        const fresh = existing.find((item) => item.path === entry.path);
        if (
          !fresh ||
          fresh.directory ||
          fresh.symlink ||
          fresh.size !== entry.size ||
          fresh.modified !== entry.modified
        ) {
          throw new Error(`来源已变化，请刷新后重新选择：${entry.name}`);
        }
      }
      const checked = previewBatchRename(
        request.entries,
        template,
        start,
        padding,
        existing.map((entry) => entry.name),
        windows,
      );
      const conflict = checked.find((row) => row.error);
      if (conflict) throw new Error(`${conflict.entry.name}：${conflict.error}`);
      setPhase('正在逐项改名；停止操作会等待当前请求结束');
      await runRenamePlan(
        checked,
        async (row) => {
          if (stopped()) throw new Error('连接已变化，禁止继续改名');
          await invoke('rename_entry', {
            id: request.id,
            source: row.entry.path,
            destination: joinPath(request.listing.path, row.name, request.id !== null),
          });
        },
        stopped,
        (next) => {
          if (!disposed.current) setResults(next);
        },
      );
      if (!disposed.current) onChanged();
    } catch (e) {
      if (!disposed.current) setError(message(e));
    } finally {
      locked.current = false;
      if (!disposed.current) {
        setBusy(false);
        setPhase('');
      }
    }
  }
  return (
    <div className="modal-backdrop">
      <section
        className="modal batch-rename"
        role="dialog"
        aria-modal="true"
        aria-label="批量重命名"
        ref={panel}
        onKeyDown={(event) => {
          if (event.key === 'Escape' && !busy) {
            event.preventDefault();
            event.stopPropagation();
            onClose();
          }
          if (event.key === 'Tab') {
            const elements = panel.current?.querySelectorAll<HTMLElement>(
              'button:not(:disabled),input:not(:disabled)',
            );
            if (!elements?.length) return;
            const first = elements[0],
              last = elements[elements.length - 1];
            if (event.shiftKey && document.activeElement === first) {
              event.preventDefault();
              last.focus();
            } else if (!event.shiftKey && document.activeElement === last) {
              event.preventDefault();
              first.focus();
            }
          }
        }}
      >
        <header>
          <Pencil size={18} />
          <h2>批量重命名</h2>
          <button aria-label="关闭批量重命名" disabled={busy} onClick={onClose}>
            <X size={18} />
          </button>
        </header>
        <div className="batch-rename-body">
          <p className="batch-rename-path">
            {request.listing.path} · {request.entries.length} 个文件
          </p>
          <fieldset disabled={busy || results !== null}>
            <label>
              名称模板
              <input
                value={template}
                onChange={(event) => setTemplate(event.target.value)}
                maxLength={240}
              />
            </label>
            <small>{'{stem} 原名称（不含扩展名） · {ext} 扩展名（含点） · {n} 编号'}</small>
            <div className="batch-rename-numbers">
              <label>
                起始编号
                <input
                  type="number"
                  min={0}
                  max={1000000}
                  value={start}
                  onChange={(event) => setStart(Number(event.target.value))}
                />
              </label>
              <label>
                编号位数
                <input
                  type="number"
                  min={1}
                  max={8}
                  value={padding}
                  onChange={(event) => setPadding(Number(event.target.value))}
                />
              </label>
            </div>
          </fieldset>
          <p>
            不覆盖同名项目，不交换文件名。执行前检查完整目录；失败或停止后保留已完成结果，不自动撤销。
          </p>
          <div className="batch-rename-table">
            <table>
              <thead>
                <tr>
                  <th>原名称</th>
                  <th>新名称</th>
                  <th>状态</th>
                </tr>
              </thead>
              <tbody>
                {preview.map((row, index) => (
                  <tr key={row.entry.path}>
                    <td title={row.entry.name}>{row.entry.name}</td>
                    <td title={row.name}>{row.name}</td>
                    <td
                      className={
                        row.error || results?.[index].status === 'failed' ? 'error-text' : ''
                      }
                    >
                      {results ? (
                        <>
                          {statusText[results[index].status]}
                          {results[index].error && <small>{results[index].error}</small>}
                        </>
                      ) : (
                        row.error || (row.changed ? '待确认' : '未改变')
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {error && <ErrorNotice error={error} scope="文件编辑" phase="批量改名检查" />}
          {results?.some((item) => item.status === 'failed') && (
            <p role="alert">部分项目失败，目标结果可能未知；请刷新核对后再创建新计划。</p>
          )}
          {phase && <p role="status">{phase}</p>}
        </div>
        <footer>
          <button disabled={busy} onClick={onClose}>
            {results ? '关闭结果' : '取消'}
          </button>
          {busy ? (
            <button
              onClick={() => {
                cancel.current = true;
                setPhase('正在停止，等待当前请求返回…');
              }}
            >
              停止后续操作
            </button>
          ) : (
            !results && (
              <button
                className="primary"
                disabled={preview.some((row) => row.error) || !preview.some((row) => row.changed)}
                onClick={() => void run()}
              >
                确认改名 {preview.filter((row) => row.changed).length} 项
              </button>
            )
          )}
        </footer>
      </section>
    </div>
  );
}
