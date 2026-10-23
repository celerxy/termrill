import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronLeft,
  ChevronRight,
  File,
  FolderSync,
  LoaderCircle,
  Monitor,
  SearchCheck,
  Server,
  ShieldCheck,
  X,
} from 'lucide-react';
import type { Difference } from './DirectorySyncDialog';
import { formatSize } from './types';
import './directorySync.css';

type Props = {
  settings: ReactNode;
  diff: ReactNode;
  quick: boolean;
  onDiff: (row: Difference) => void;
  localPath: string;
  remotePath: string;
  direction: 'upload' | 'download';
  onDirectionChange: (direction: 'upload' | 'download') => void;
  rows: Difference[];
  selected: Set<string>;
  onToggle: (path: string) => void;
  onSelectAll: () => void;
  onClearSelection: () => void;
  busy: boolean;
  elapsedSeconds: number;
  error: string;
  hasCompared: boolean;
  canCompare: boolean;
  canSync: boolean;
  onCompare: () => void;
  onClose: () => void;
  onSync: () => void;
};
const labels: Record<string, string> = {
  new: '新增',
  changed: '内容不同',
  same: '相同',
  targetOnly: '仅目标存在',
  typeConflict: '类型冲突',
  skipped: '链接或特殊文件 · 跳过',
};
const filters = [
  { value: 'all', label: '全部' },
  { value: 'new', label: '新增' },
  { value: 'changed', label: '内容不同' },
];
const pageSize = 100;

/** 展示独立于 IPC；分页限制 DOM 数量，筛选不清除其他页已经选择的差异。 */
export default function DirectorySyncView(props: Props) {
  const { rows, selected, busy, hasCompared } = props;
  const [filter, setFilter] = useState('all');
  const [page, setPage] = useState(0);
  const dialog = useRef<HTMLElement>(null);
  const headerCheckbox = useRef<HTMLInputElement>(null);
  const filtered = useMemo(
    () => rows.filter((row) => filter === 'all' || row.status === filter),
    [rows, filter],
  );
  const stats = useMemo(
    () =>
      rows.reduce<Record<string, number>>((counts, row) => {
        counts[row.status] = (counts[row.status] ?? 0) + 1;
        return counts;
      }, {}),
    [rows],
  );
  const selectableCount = (stats.new ?? 0) + (stats.changed ?? 0);
  const pageCount = Math.max(1, Math.ceil(filtered.length / pageSize));
  const currentPage = Math.min(page, pageCount - 1);
  useEffect(() => {
    setPage(0);
    setFilter('all');
  }, [rows, props.direction]);
  useEffect(() => {
    if (headerCheckbox.current)
      headerCheckbox.current.indeterminate = selected.size > 0 && selected.size < selectableCount;
  }, [selected.size, selectableCount]);
  useEffect(() => {
    dialog.current?.focus();
  }, []);
  const elapsed = `${Math.floor(props.elapsedSeconds / 60)}:${String(props.elapsedSeconds % 60).padStart(2, '0')}`;
  return (
    <div className="modal-backdrop">
      <section
        className="modal sync-design"
        ref={dialog}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby="sync-title"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation();
            props.onClose();
          }
          // 弹窗内循环焦点，避免键盘操作落到被遮挡的工作区。
          if (event.key === 'Tab') {
            const controls = [
              ...event.currentTarget.querySelectorAll<HTMLElement>(
                'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled)',
              ),
            ];
            const first = controls[0];
            const last = controls.at(-1);
            if (
              event.shiftKey &&
              (document.activeElement === first || document.activeElement === dialog.current)
            ) {
              event.preventDefault();
              last?.focus();
            } else if (!event.shiftKey && document.activeElement === last) {
              event.preventDefault();
              first?.focus();
            }
          }
        }}
      >
        <header className="sync-heading">
          <span className="sync-heading-icon">
            <FolderSync size={28} />
          </span>
          <div>
            <h2 id="sync-title">目录差异与同步</h2>
            <p>查看差异，确认后同步所选项目</p>
          </div>
          <button
            className="sync-icon-button"
            aria-label={busy ? '取消比较并关闭' : '关闭目录同步'}
            onClick={props.onClose}
          >
            <X size={21} />
          </button>
        </header>
        <div className="sync-directory-pair">
          <div className="sync-directory">
            <Monitor size={27} />
            <div>
              <strong>本地目录</strong>
              <span title={props.localPath}>{props.localPath || '未选择目录'}</span>
            </div>
          </div>
          <span className="sync-direction-arrow">
            {props.direction === 'upload' ? <ArrowRight size={22} /> : <ArrowLeft size={22} />}
          </span>
          <div className="sync-directory">
            <Server size={27} />
            <div>
              <strong>远程目录</strong>
              <span title={props.remotePath}>{props.remotePath || '未选择目录'}</span>
            </div>
          </div>
        </div>
        <div className="sync-direction-bar">
          <span>同步方向</span>
          <div className="sync-segments" aria-label="同步方向">
            <button
              aria-pressed={props.direction === 'upload'}
              disabled={busy}
              onClick={() => props.onDirectionChange('upload')}
            >
              本地 → 远程
            </button>
            <button
              aria-pressed={props.direction === 'download'}
              disabled={busy}
              onClick={() => props.onDirectionChange('download')}
            >
              远程 → 本地
            </button>
          </div>
          <span className={`sync-status ${hasCompared ? 'complete' : ''}`} role="status">
            {busy ? (
              <>
                <LoaderCircle className="sync-spin" size={17} /> 比较中
              </>
            ) : hasCompared ? (
              <>
                <Check size={18} /> 比较完成
              </>
            ) : (
              '等待比较'
            )}
          </span>
          {hasCompared && (
            <button
              className="sync-recompare"
              disabled={!props.canCompare}
              onClick={props.onCompare}
            >
              重新比较
            </button>
          )}
        </div>
        {hasCompared && (
          <div className="sync-metrics">
            {['new', 'changed', 'same', 'targetOnly'].map((status) => (
              <div key={status}>
                <i className={`sync-dot ${status}`} />
                <span>
                  <strong>{stats[status] ?? 0}</strong>
                  <small>{labels[status]}</small>
                </span>
              </div>
            ))}
          </div>
        )}
        <details className="sync-options">
          <summary>规则与比较设置 · {props.quick ? '快速元数据比较' : '严格 SHA256 校验'}</summary>
          {props.settings}
        </details>
        <div
          className="sync-policy"
          title={
            props.quick
              ? '仅比较大小及秒级修改时间，不能保证内容一致；时间缺失视为差异。'
              : '同大小文件读取两端内容校验 SHA256。'
          }
        >
          <ShieldCheck size={18} />
          <span>
            {props.quick ? '快速元数据比较（不能保证内容一致）' : 'SHA256 内容校验'} · 不跟随链接 ·
            不删除目标独有文件
          </span>
        </div>
        {props.error && (
          <div className="sync-error" role="alert">
            比较失败：{props.error}
          </div>
        )}
        {busy ? (
          <div className="sync-processing" role="status" aria-live="polite">
            <div className="sync-processing-icon">
              <FolderSync size={34} />
              <span />
            </div>
            <h3>正在比较目录</h3>
            <p>
              {props.quick
                ? '扫描两端目录，比较大小和修改时间'
                : '扫描两端目录，并校验同大小文件的内容'}
            </p>
            <div className="sync-indeterminate" role="progressbar" aria-label="目录比较进行中">
              <span />
            </div>
            <span className="sync-elapsed" aria-live="off">
              已用时间 {elapsed}
            </span>
            <small>文件较多或文件较大时，内容校验可能需要更长时间。</small>
          </div>
        ) : hasCompared ? (
          <div className="sync-results">
            <div className="sync-table-toolbar">
              <h3>差异项目</h3>
              <div className="sync-filters" aria-label="筛选差异">
                {filters.map((item) => (
                  <button
                    key={item.value}
                    aria-pressed={filter === item.value}
                    onClick={() => {
                      setFilter(item.value);
                      setPage(0);
                    }}
                  >
                    {item.label}
                  </button>
                ))}
              </div>
              <div className="sync-selection-actions">
                <button onClick={props.onSelectAll} disabled={!selectableCount}>
                  全选差异
                </button>
                <button onClick={props.onClearSelection} disabled={!selected.size}>
                  清除选择
                </button>
              </div>
            </div>
            {stats.typeConflict || stats.skipped ? (
              <div className="sync-warning">
                {stats.typeConflict ?? 0} 项类型冲突 · {stats.skipped ?? 0}{' '}
                项链接或特殊文件，不能同步；可在「全部」中查看。
              </div>
            ) : null}
            <div className="sync-table-head">
              <input
                ref={headerCheckbox}
                type="checkbox"
                aria-label="选择全部可同步差异"
                disabled={!selectableCount}
                checked={selectableCount > 0 && selected.size === selectableCount}
                onChange={(event) =>
                  event.target.checked ? props.onSelectAll() : props.onClearSelection()
                }
              />
              <span>名称</span>
              <span>差异</span>
              <span>大小</span>
            </div>
            <div className="sync-rows sync-table-body">
              {filtered.slice(currentPage * pageSize, (currentPage + 1) * pageSize).map((row) => (
                <label
                  key={row.remote}
                  className={`sync-table-row ${selected.has(row.remote) ? 'is-selected' : ''}`}
                >
                  <input
                    type="checkbox"
                    aria-label={`选择 ${row.relative}`}
                    disabled={!['new', 'changed'].includes(row.status)}
                    checked={selected.has(row.remote)}
                    onChange={() => props.onToggle(row.remote)}
                  />
                  <span className="sync-file-name" title={row.relative}>
                    <File size={20} />
                    <span>{row.relative}</span>
                    {!row.directory &&
                      ['same', 'changed'].includes(row.status) &&
                      row.size <= 2 * 1024 * 1024 && (
                        <button
                          type="button"
                          title="只读查看两端文本差异（各最多 2 MiB）"
                          onClick={(event) => {
                            event.preventDefault();
                            props.onDiff(row);
                          }}
                        >
                          差异
                        </button>
                      )}
                  </span>
                  <span>
                    <small className={`sync-badge ${row.status}`}>
                      {labels[row.status] ?? row.status}
                    </small>
                  </span>
                  <span className="sync-file-size">{row.size ? formatSize(row.size) : '—'}</span>
                </label>
              ))}
              {!filtered.length && (
                <div className="sync-empty-results">
                  <SearchCheck size={30} />
                  <span>{rows.length ? '没有此类差异' : '没有需要同步的项目'}</span>
                </div>
              )}
            </div>
            <div className="sync-pagination">
              <span>
                共 {rows.length} 项{filter !== 'all' ? ` · 当前筛选 ${filtered.length} 项` : ''}
              </span>
              <div>
                <button
                  aria-label="上一页"
                  disabled={!currentPage}
                  onClick={() => setPage(currentPage - 1)}
                >
                  <ChevronLeft size={17} />
                </button>
                <span>
                  {currentPage + 1} / {pageCount}
                </span>
                <button
                  aria-label="下一页"
                  disabled={currentPage + 1 >= pageCount}
                  onClick={() => setPage(currentPage + 1)}
                >
                  <ChevronRight size={17} />
                </button>
              </div>
            </div>
          </div>
        ) : (
          <div className="sync-processing sync-start">
            <SearchCheck size={38} />
            <h3>比较目录，查看文件差异</h3>
            <p>完成比较后，选择要同步的新增或内容不同项目。</p>
            <button className="primary" disabled={!props.canCompare} onClick={props.onCompare}>
              {props.error ? '重试比较' : '开始比较'}
            </button>
          </div>
        )}
        {props.diff}
        <footer className="sync-footer">
          <div>
            <strong>
              已选择 <b>{selected.size}</b> 项
            </strong>
            <span>同步后将更新{props.direction === 'upload' ? '远程' : '本地'}目录</span>
          </div>
          <div>
            <button onClick={props.onClose}>{busy ? '取消比较' : '关闭'}</button>
            <button className="primary" disabled={!props.canSync} onClick={props.onSync}>
              同步所选 <ArrowRight size={16} />
            </button>
          </div>
        </footer>
      </section>
    </div>
  );
}
