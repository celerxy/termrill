import PathBreadcrumbs from './PathBreadcrumbs';
import DirectoryLocationMenu from './DirectoryLocationMenu';
import { DirectoryHistory } from './directoryHistory';
import { useDeferredValue, useEffect, useMemo, useRef, useState } from 'react';
import { useVirtualFileRows } from './useVirtualFileRows';
import { useFileContextMenu } from './useFileContextMenu';
import {
  ArrowUp,
  ArrowLeft,
  ArrowRight,
  Folder,
  Monitor,
  Server,
  Search,
  RefreshCw,
  FolderPlus,
  Pencil,
  Upload,
  Download,
  FolderOpen,
  FileCode2,
  FileText,
  Eye,
  Trash2,
} from 'lucide-react';
import { FileEntry, Listing, formatSize } from './types';
import { fileType } from './fileType';
export type FilePaneProps = {
  locationScope?: string;
  remote?: boolean;
  listing: Listing | null;
  loading: boolean;
  readCount?: number;
  readError?: string;
  selected: string[];
  showHidden: boolean;
  disabled?: boolean;
  transferDisabled?: boolean;
  onSelect: (paths: string[]) => void;
  onNavigate: (path: string) => void;
  onPage: (offset: number, query: string) => void;
  onRefresh: () => void;
  onPick?: () => void;
  onMkdir: () => void;
  onRename: (path?: string) => void;
  onBatchRename?: (entries: FileEntry[]) => void;
  onDelete: (paths: string[]) => void;
  onEdit?: (path: string) => void;
  onArchive?: (path: string) => void;
  onCompress?: (paths: string[]) => void;
  onProperties?: (path: string) => void;
  onBatchPermissions?: (paths: string[]) => void;
  onError: (error: unknown) => void;
  onTransfer: (entries: FileEntry[]) => void;
  onDrop: (paths: string[]) => void;
};
export default function FilePane(p: FilePaneProps) {
  const history = useMemo(() => new DirectoryHistory(), [p.locationScope]);
  const [, updateHistory] = useState(0);
  useEffect(() => {
    if (p.listing?.path) {
      history.visit(p.listing.path);
      updateHistory((version) => version + 1);
    }
  }, [history, p.listing?.path]);
  const [query, setQuery] = useState('');
  const [path, setPath] = useState('');
  const [hover, setHover] = useState(false);
  const filterPending =
    !!p.listing?.page &&
    (p.listing.page.query !== query.toLowerCase() || p.listing.page.showHidden !== p.showHidden);
  const pageCallback = useRef(p.onPage);
  pageCallback.current = p.onPage;
  useEffect(() => {
    const page = p.listing?.page;
    if (!page || (page.query === query.toLowerCase() && page.showHidden === p.showHidden)) return;
    // 搜索全部目录而不是仅筛选当前页；延迟合并连续输入。
    const timer = window.setTimeout(() => pageCallback.current(0, query), 350);
    return () => window.clearTimeout(timer);
  }, [query, p.showHidden, p.listing?.path, p.listing?.page]);
  const { openContext, menu } = useFileContextMenu(p);
  const pane = useRef<HTMLElement>(null);
  const gesture = useRef<{ x: number; y: number; paths: string[]; moved: boolean } | null>(null);
  const suppressClick = useRef(false);
  const drop = useRef(p.onDrop);
  drop.current = p.onDrop;
  useEffect(() => {
    const remotePane = () =>
      pane.current
        ?.closest('.session-workspace')
        ?.querySelector<HTMLElement>('[data-remote="true"]');
    const clear = () => {
      if (!gesture.current) return;
      gesture.current = null;
      document.body.classList.remove('termrill-dragging');
      remotePane()?.classList.remove('drop-hover');
    };
    const move = (event: PointerEvent) => {
      const g = gesture.current;
      if (!g) return;
      if (pane.current?.closest('.session-workspace[hidden]')) {
        clear();
        return;
      }
      if (Math.hypot(event.clientX - g.x, event.clientY - g.y) > 8) g.moved = true;
      if (g.moved) {
        document.body.classList.add('termrill-dragging');
        const target = remotePane();
        const rect = target?.getBoundingClientRect();
        target?.classList.toggle(
          'drop-hover',
          !!rect &&
            event.clientX >= rect.left &&
            event.clientX <= rect.right &&
            event.clientY >= rect.top &&
            event.clientY <= rect.bottom,
        );
      }
    };
    const up = (event: PointerEvent) => {
      const g = gesture.current;
      const rect = remotePane()?.getBoundingClientRect();
      if (g?.moved && !pane.current?.closest('.session-workspace[hidden]')) {
        suppressClick.current = true;
        if (
          rect &&
          event.clientX >= rect.left &&
          event.clientX <= rect.right &&
          event.clientY >= rect.top &&
          event.clientY <= rect.bottom
        )
          drop.current(g.paths);
      }
      clear();
    };
    document.addEventListener('pointermove', move);
    document.addEventListener('pointerup', up);
    document.addEventListener('pointercancel', clear);
    return () => {
      clear();
      document.removeEventListener('pointermove', move);
      document.removeEventListener('pointerup', up);
      document.removeEventListener('pointercancel', clear);
    };
  }, []);
  useEffect(() => {
    setPath(p.listing?.path ?? '');
    setQuery(p.listing?.page?.query ?? '');
  }, [p.listing?.path]);
  const deferredQuery = useDeferredValue(query);
  const entries = useMemo(() => {
    const search = deferredQuery.toLowerCase();
    return (
      p.listing?.entries.filter(
        (f) =>
          (p.showHidden || !f.name.startsWith('.')) &&
          (!search || f.name.toLowerCase().includes(search)),
      ) ?? []
    );
  }, [p.listing, p.showHidden, deferredQuery]);
  const selectedPaths = useMemo(() => new Set(p.selected), [p.selected]);
  const selectedEntries = useMemo(
    () => p.listing?.entries.filter((f) => selectedPaths.has(f.path)) ?? [],
    [p.listing, selectedPaths],
  );
  const virtual = useVirtualFileRows(
    entries.length,
    `${p.listing?.path ?? ''}\n${p.listing?.page?.offset ?? 0}\n${deferredQuery}\n${p.showHidden}`,
  );
  const dateFormatter = useMemo(
    () =>
      new Intl.DateTimeFormat('zh-CN', {
        month: '2-digit',
        day: '2-digit',
        hour: '2-digit',
        minute: '2-digit',
        hour12: false,
      }),
    [],
  );
  const choose = (e: React.MouseEvent | React.KeyboardEvent, f: FileEntry) => {
    if (suppressClick.current) {
      suppressClick.current = false;
      return;
    }
    p.onSelect(
      e.ctrlKey || e.metaKey
        ? selectedPaths.has(f.path)
          ? p.selected.filter((v) => v !== f.path)
          : [...p.selected, f.path]
        : [f.path],
    );
  };
  return (
    <section
      ref={pane}
      className={`file-pane ${hover ? 'drop-hover' : ''}`}
      data-remote={p.remote ? 'true' : 'false'}
      onContextMenu={(event) => {
        // 输入框保留复制/粘贴原生菜单，文件区域使用应用菜单。
        if ((event.target as HTMLElement).closest('input, textarea')) return;
        openContext(event);
      }}
      onDragOver={(e) => {
        if (p.remote) {
          e.preventDefault();
          setHover(true);
        }
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node)) setHover(false);
      }}
      onDrop={(e) => {
        e.preventDefault();
        setHover(false);
        try {
          const value = e.dataTransfer.getData('application/termrill-paths');
          if (value) p.onDrop(JSON.parse(value));
        } catch {
          /* Ignore foreign drag formats. */
        }
      }}
    >
      <div className="pane-heading">
        {p.remote ? <Server size={19} /> : <Monitor size={19} />}
        <h2>{p.remote ? '远程文件' : '本地文件'}</h2>
        <span className="count">
          {p.loading
            ? '读取中…'
            : `${entries.length} 项${p.listing?.page?.hasMore || p.listing?.page?.offset ? ' · 当前页' : ''}`}
        </span>
        {p.onPick && (
          <button title="选择本地文件夹" aria-label="选择本地文件夹" onClick={p.onPick}>
            <FolderOpen size={16} />
          </button>
        )}
        <button
          title="刷新"
          aria-label="刷新"
          disabled={p.loading || p.disabled}
          onClick={p.onRefresh}
        >
          <RefreshCw className={p.loading ? 'spin' : ''} size={15} />
        </button>
      </div>
      <div className="path-bar">
        <button
          aria-label="返回上一目录"
          title="返回上一目录"
          disabled={p.disabled || p.loading || !history.canBack}
          onClick={() => {
            const previous = history.request('back');
            if (previous) p.onNavigate(previous);
          }}
        >
          <ArrowLeft size={16} />
        </button>
        <button
          aria-label="前进到下一目录"
          title="前进到下一目录"
          disabled={p.disabled || p.loading || !history.canForward}
          onClick={() => {
            const next = history.request('forward');
            if (next) p.onNavigate(next);
          }}
        >
          <ArrowRight size={16} />
        </button>
        <button
          aria-label="返回上级目录"
          disabled={p.disabled || !p.listing?.parent || p.loading}
          onClick={() => p.listing?.parent && p.onNavigate(p.listing.parent)}
        >
          <ArrowUp size={17} />
        </button>
        <Folder size={16} />
        <PathBreadcrumbs
          path={path}
          remote={p.remote}
          disabled={p.disabled}
          loading={p.loading}
          onNavigate={p.onNavigate}
          onError={p.onError}
        />
        <DirectoryLocationMenu
          scope={p.remote ? p.locationScope : 'local'}
          path={p.listing?.path ?? ''}
          disabled={!!p.disabled || p.loading}
          onNavigate={p.onNavigate}
          onError={p.onError}
        />
      </div>
      <div className="pane-tools">
        <label className="search">
          <Search size={15} />
          <input
            aria-label="搜索当前目录"
            placeholder="搜索当前目录…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </label>
        <button
          title="新建文件夹"
          aria-label="新建文件夹"
          disabled={p.disabled || !p.listing}
          onClick={p.onMkdir}
        >
          <FolderPlus size={16} />
        </button>
        <button
          title={selectedEntries.length > 1 ? '批量重命名' : '重命名'}
          aria-label={selectedEntries.length > 1 ? '批量重命名' : '重命名'}
          disabled={
            p.disabled ||
            selectedEntries.length === 0 ||
            (selectedEntries.length > 1 &&
              (!p.onBatchRename ||
                selectedEntries.length > 100 ||
                selectedEntries.some((entry) => entry.directory || entry.symlink)))
          }
          onClick={() =>
            selectedEntries.length > 1 ? p.onBatchRename?.(selectedEntries) : p.onRename()
          }
        >
          <Pencil size={15} />
        </button>
        <button
          title="预览 / 编辑所选文件"
          aria-label="预览 / 编辑所选文件"
          disabled={
            p.disabled ||
            !p.onEdit ||
            selectedEntries.length !== 1 ||
            selectedEntries[0]?.directory ||
            selectedEntries[0]?.symlink
          }
          onClick={() => p.onEdit?.(selectedEntries[0].path)}
        >
          <Eye size={16} />
        </button>
        <button
          className="pane-delete"
          title="删除所选项目"
          aria-label="删除所选项目"
          disabled={p.disabled || !selectedEntries.length}
          onClick={() => p.onDelete(selectedEntries.map((entry) => entry.path))}
        >
          <Trash2 size={16} />
        </button>
        <button
          title={p.remote ? '下载所选项目' : '上传所选项目'}
          aria-label={p.remote ? '下载所选项目' : '上传所选项目'}
          disabled={p.disabled || p.transferDisabled || !selectedEntries.length}
          onClick={() => p.onTransfer(selectedEntries)}
        >
          {p.remote ? <Download size={16} /> : <Upload size={16} />}
        </button>
      </div>
      <div className="file-scroll" ref={virtual.scrollRef} aria-busy={p.loading}>
        <table aria-rowcount={entries.length + 1}>
          <thead>
            <tr>
              <th>名称</th>
              <th>类型</th>
              <th>大小</th>
              <th>修改时间</th>
            </tr>
          </thead>
          <tbody>
            {virtual.top > 0 && (
              <tr className="virtual-spacer" aria-hidden="true">
                <td colSpan={4} style={{ height: virtual.top }} />
              </tr>
            )}
            {entries.slice(virtual.start, virtual.end).map((f, index) => (
              <tr
                key={f.path}
                className={`file-entry ${selectedPaths.has(f.path) ? 'selected' : ''}`}
                aria-rowindex={virtual.start + index + 2}
                data-file-index={virtual.start + index}
                onClick={(e) => choose(e, f)}
                onContextMenu={(event) => openContext(event, f)}
                onDoubleClick={() => {
                  if (f.symlink) return;
                  if (f.directory) p.onNavigate(f.path);
                  else p.onEdit?.(f.path);
                }}
                tabIndex={0}
                onKeyDown={(e) => {
                  if (['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) {
                    e.preventDefault();
                    const current = virtual.start + index;
                    const target =
                      e.key === 'Home'
                        ? 0
                        : e.key === 'End'
                          ? entries.length - 1
                          : current + (e.key === 'ArrowUp' ? -1 : 1);
                    virtual.focusRow(Math.max(0, Math.min(entries.length - 1, target)));
                    return;
                  }
                  if (e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10')) {
                    openContext(e, f);
                    return;
                  }
                  if (e.key === 'Enter')
                    f.directory && !f.symlink ? p.onNavigate(f.path) : choose(e, f);
                }}
                onPointerDown={(e) => {
                  if (e.button === 0 && !p.remote && !p.disabled) {
                    suppressClick.current = false;
                    gesture.current = {
                      x: e.clientX,
                      y: e.clientY,
                      paths: selectedPaths.has(f.path) ? p.selected : [f.path],
                      moved: false,
                    };
                  }
                }}
                aria-selected={selectedPaths.has(f.path)}
              >
                <td>
                  {f.directory ? (
                    <Folder className="folder" size={21} fill="currentColor" />
                  ) : f.name.endsWith('.md') ? (
                    <FileText className="document" size={20} />
                  ) : (
                    <FileCode2 className="code" size={20} />
                  )}
                  <span title={f.name}>
                    {f.name}
                    {f.symlink && ' ↗'}
                  </span>
                </td>
                <td className="file-type" title={fileType(f)}>
                  {fileType(f)}
                </td>
                <td>{f.directory ? '—' : formatSize(f.size)}</td>
                <td>{f.modified ? dateFormatter.format(new Date(f.modified * 1000)) : '—'}</td>
              </tr>
            ))}
            {virtual.bottom > 0 && (
              <tr className="virtual-spacer" aria-hidden="true">
                <td colSpan={4} style={{ height: virtual.bottom }} />
              </tr>
            )}
          </tbody>
        </table>
        {!entries.length && (
          <div className="empty-pane">
            <FolderOpen size={30} />
            <p>
              {p.loading
                ? '正在读取目录…'
                : p.readError
                  ? '目录读取未完成'
                  : !p.listing && p.remote
                    ? '选择连接，开始浏览远程文件'
                    : query
                      ? '没有匹配的文件'
                      : '目录为空'}
            </p>
          </div>
        )}
      </div>
      <div className="pane-footer">
        {p.readError
          ? '读取失败，保留上次成功的页面。请刷新重试。'
          : p.loading
            ? '正在更新目录…'
            : p.selected.length
              ? `已选择 ${p.selected.length} 项`
              : '双击文件夹进入 · Ctrl 多选'}
        <span>{p.remote ? 'SFTP' : '本地磁盘'}</span>
      </div>
      {p.listing?.page && (p.listing.page.hasMore || p.listing.page.offset > 0) && (
        <div className="directory-pagination">
          <button
            disabled={p.loading || filterPending || p.disabled || p.listing.page.offset === 0}
            onClick={() =>
              p.onPage(
                Math.max(0, (p.listing?.page?.offset ?? 0) - (p.listing?.page?.size ?? 1000)),
                query,
              )
            }
          >
            上一页
          </button>
          <span>
            第 {Math.floor(p.listing.page.offset / p.listing.page.size) + 1} 页 · 每页最多{' '}
            {p.listing.page.size} 项
          </span>
          <button
            disabled={p.loading || filterPending || p.disabled || !p.listing.page.hasMore}
            onClick={() =>
              p.onPage((p.listing?.page?.offset ?? 0) + (p.listing?.page?.size ?? 1000), query)
            }
          >
            下一页
          </button>
        </div>
      )}
      {menu}
    </section>
  );
}
