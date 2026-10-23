/** 当前目录分页展示，搜索覆盖全包；虚拟目录选择映射为真实条目，跨目录保留选择。 */
import { useEffect, useMemo, useRef, useState } from 'react';
import {
  Folder,
  FolderOpen,
  File,
  FileImage,
  FileCode2,
  FileText,
  LockKeyhole,
  ShieldAlert,
  Eye,
  Download,
  ArrowUp,
  ChevronLeft,
  ChevronRight,
} from 'lucide-react';
import { formatSize } from './types';
import type { ArchiveEntry } from './archiveTypes';
export default function ArchiveEntryTable({
  entries,
  selected,
  busy,
  phase,
  loaded,
  setSelected,
  onPreview,
  onPrefetch,
  onDownload,
  canDownload,
  query,
  onClearSearch,
  compressedSizes = true,
}: {
  entries: ArchiveEntry[];
  selected: string[];
  busy: boolean;
  phase: string;
  loaded: boolean;
  setSelected: (values: string[]) => void;
  onPreview: (name: string) => void;
  onPrefetch: (names: string[]) => void;
  onDownload: (names: string[]) => void;
  canDownload: boolean;
  query: string;
  onClearSearch: () => void;
  compressedSizes?: boolean;
}) {
  const [page, setPage] = useState(0);
  const scroll = useRef<HTMLDivElement>(null);
  const [directory, setDirectory] = useState('');
  const search = query.trim().toLowerCase();
  const model = useMemo(() => {
    const rows = new Map<string, ArchiveEntry>();
    const children = new Map<string, string[]>();
    for (const entry of entries) {
      rows.set(entry.name, entry);
      if (!entry.safe) continue;
      const parts = entry.name.replace(/\/$/, '').split('/');
      let parent = '';
      for (let index = 0; index < parts.length - 1; index++) {
        parent += `${parts[index]}/`;
        if (!rows.has(parent))
          rows.set(parent, {
            name: parent,
            directory: true,
            safe: true,
            encrypted: false,
            size: 0,
            compressed: 0,
          });
        const names = children.get(parent) ?? [];
        names.push(entry.name);
        children.set(parent, names);
      }
    }
    const compare = new Intl.Collator('zh-CN', { numeric: true, sensitivity: 'base' });
    const sorted = [...rows.values()].sort(
      (a, b) => Number(b.directory) - Number(a.directory) || compare.compare(a.name, b.name),
    );
    const originals = new Set(entries.map((entry) => entry.name));
    const targets = new Map(
      sorted.map((entry) => [
        entry.name,
        entry.directory
          ? [
              ...new Set([
                ...(originals.has(entry.name) ? [entry.name] : []),
                ...(children.get(entry.name) ?? []),
              ]),
            ]
          : [entry.name],
      ]),
    );
    return { sorted, targets, rows };
  }, [entries]);
  const hierarchy = useMemo(
    () =>
      model.sorted.filter((entry) => {
        if (search) return entry.name.toLowerCase().includes(search);
        if (!entry.safe) return !directory;
        const path = entry.name.replace(/\/$/, '');
        const parent = path.slice(0, path.lastIndexOf('/') + 1);
        return parent === directory;
      }),
    [model, search, directory],
  );
  const namesFor = (entry: ArchiveEntry) => model.targets.get(entry.name) ?? [];
  useEffect(() => {
    if (directory && !model.rows.has(directory)) setDirectory('');
  }, [model, directory]);
  const rowSelected = (entry: ArchiveEntry) => {
    const names = namesFor(entry);
    return names.length > 0 && names.every((name) => selectedSet.has(name));
  };
  const selectRow = (entry: ArchiveEntry) => {
    const names = new Set(namesFor(entry));
    setSelected(
      rowSelected(entry)
        ? selected.filter((name) => !names.has(name))
        : [...new Set([...selected, ...names])],
    );
  };
  const navigate = (path: string) => {
    setDirectory(path);
    onClearSearch();
    setPage(0);
  };
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const pages = Math.max(1, Math.ceil(hierarchy.length / 150));
  const visible = hierarchy.slice(
    Math.min(page, pages - 1) * 150,
    (Math.min(page, pages - 1) + 1) * 150,
  );
  const hints = JSON.stringify(
    visible
      .filter((entry) => !entry.directory)
      .slice(0, 6)
      .map((entry) => entry.name),
  );
  useEffect(() => {
    onPrefetch(JSON.parse(hints) as string[]);
  }, [hints, onPrefetch]);
  useEffect(() => {
    setPage(0);
  }, [entries, directory, search]);
  useEffect(() => {
    if (scroll.current) scroll.current.scrollTop = 0;
  }, [page, directory, search]);
  return (
    <>
      <nav className="archive-breadcrumbs" aria-label="压缩包目录">
        <button
          aria-label="返回上级目录"
          disabled={busy || !directory}
          onClick={() => navigate(directory.replace(/[^/]+\/$/, ''))}
        >
          <ArrowUp size={16} />
        </button>
        <button disabled={busy} onClick={() => navigate('')}>
          压缩包根目录
        </button>
        {directory
          .split('/')
          .filter(Boolean)
          .map((part, index, parts) => (
            <span key={index}>
              <ChevronRight size={14} />
              <button
                disabled={busy}
                onClick={() => navigate(parts.slice(0, index + 1).join('/') + '/')}
              >
                {part}
              </button>
            </span>
          ))}
        {search && <strong>全包搜索结果</strong>}
      </nav>
      <div className="archive-list" aria-busy={busy} ref={scroll}>
        <table>
          <thead>
            <tr>
              <th>
                <input
                  type="checkbox"
                  aria-label="选择当前页"
                  disabled={busy || !entries.length}
                  checked={
                    !!visible.length &&
                    visible.filter((entry) => entry.safe).length > 0 &&
                    visible.filter((entry) => entry.safe).every(rowSelected)
                  }
                  onChange={(event) =>
                    setSelected(
                      event.target.checked
                        ? [
                            ...new Set([
                              ...selected,
                              ...visible.filter((entry) => entry.safe).flatMap(namesFor),
                            ]),
                          ]
                        : selected.filter(
                            (name) =>
                              !visible
                                .filter((entry) => entry.safe)
                                .flatMap(namesFor)
                                .includes(name),
                          ),
                    )
                  }
                />
              </th>
              <th>名称</th>
              <th>原始大小</th>
              <th>压缩大小</th>
              <th aria-label="文件操作" />
            </tr>
          </thead>
          <tbody>
            {visible.map((entry) => (
              <tr
                onMouseEnter={() => onPrefetch([entry.name])}
                onFocus={() => onPrefetch([entry.name])}
                key={entry.name}
                className={rowSelected(entry) ? 'selected' : ''}
                onDoubleClick={() => {
                  if (busy || !entry.safe) return;
                  if (entry.directory) navigate(entry.name);
                  else onPreview(entry.name);
                }}
              >
                <td>
                  <input
                    type="checkbox"
                    aria-label={`选择 ${entry.name}`}
                    disabled={busy || !entry.safe}
                    checked={rowSelected(entry)}
                    onChange={() => selectRow(entry)}
                  />
                </td>
                <td title={entry.name}>
                  <div className="archive-entry-name">
                    {entry.directory ? (
                      <Folder size={19} />
                    ) : /\.(png|jpe?g|gif|webp|svg|bmp)$/i.test(entry.name) ? (
                      <FileImage size={19} />
                    ) : /\.(json|ini|ipynb|py|js|ts|tsx|html|css)$/i.test(entry.name) ? (
                      <FileCode2 size={19} />
                    ) : /\.(md|txt|pdf)$/i.test(entry.name) ? (
                      <FileText size={19} />
                    ) : (
                      <File size={19} />
                    )}
                    <span className="archive-name-label">
                      {entry.directory && entry.safe ? (
                        <button
                          className="archive-folder-link"
                          disabled={busy}
                          onClick={() => navigate(entry.name)}
                        >
                          {entry.name.replace(/\/$/, '').split('/').pop()}
                        </button>
                      ) : (
                        <span>{entry.safe ? entry.name.split('/').pop() : entry.name}</span>
                      )}
                      {search && <small>{entry.name}</small>}
                    </span>
                    {!entry.safe ? (
                      <ShieldAlert className="unsafe" size={16} aria-label="不安全路径" />
                    ) : entry.encrypted ? (
                      <LockKeyhole size={15} aria-label="加密文件" />
                    ) : null}
                  </div>
                </td>
                <td>{entry.directory ? '—' : formatSize(entry.size)}</td>
                <td>{entry.directory || !compressedSizes ? '—' : formatSize(entry.compressed)}</td>
                <td>
                  <div className="archive-row-actions">
                    {search && entry.safe && (
                      <button
                        aria-label={`定位 ${entry.name}`}
                        title="打开所在目录"
                        disabled={busy}
                        onClick={() => {
                          const path = entry.name.replace(/\/$/, '');
                          navigate(path.slice(0, path.lastIndexOf('/') + 1));
                        }}
                      >
                        <FolderOpen size={18} />
                      </button>
                    )}
                    {!entry.directory && (
                      <button
                        aria-label={`预览 ${entry.name}`}
                        title="预览文件"
                        disabled={busy || entry.directory || !entry.safe}
                        onClick={() => onPreview(entry.name)}
                      >
                        <Eye size={18} />
                      </button>
                    )}
                    <button
                      aria-label={`下载 ${entry.name}`}
                      title="下载到本地目录"
                      disabled={busy || !canDownload || !entry.safe || !namesFor(entry).length}
                      onClick={() => onDownload(namesFor(entry))}
                    >
                      <Download size={18} />
                    </button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {!hierarchy.length && (
          <div className="archive-empty">
            {busy
              ? phase || '正在读取'
              : loaded
                ? search
                  ? '没有匹配的项目'
                  : '当前目录为空'
                : '等待读取'}
          </div>
        )}
      </div>
      <div className="archive-pagination">
        <span>
          已选择 {selected.length} 个包内条目 · {search ? '搜索结果' : '当前目录'}{' '}
          {hierarchy.length} 项
        </span>
        <button disabled={busy || !selected.length} onClick={() => setSelected([])}>
          清除选择
        </button>
        <button
          aria-label="上一页"
          disabled={page <= 0}
          onClick={() => setPage((value) => value - 1)}
        >
          <ChevronLeft size={18} />
        </button>
        <span className="archive-page-number">
          {Math.min(page, pages - 1) + 1} / {pages}
        </span>
        <button
          aria-label="下一页"
          disabled={page >= pages - 1}
          onClick={() => setPage((value) => value + 1)}
        >
          <ChevronRight size={18} />
        </button>
      </div>
    </>
  );
}
