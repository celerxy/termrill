/** 包内目录模型缓存；虚拟目录选择转为真实条目，不改变后端路径约束。 */
import { useEffect, useMemo, useState } from 'react';
import { ChevronRight, Folder, File } from 'lucide-react';
import type { ArchiveEntry } from './archiveTypes';
import { formatSize } from './types';
export default function ArchiveTree({
  entries,
  query,
  selected,
  busy,
  onSelect,
  onPreview,
  onPrefetch,
}: {
  entries: ArchiveEntry[];
  query: string;
  selected: string[];
  busy: boolean;
  onSelect: (names: string[]) => void;
  onPreview: (name: string) => void;
  onPrefetch: (names: string[]) => void;
}) {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [active, setActive] = useState('');
  const members = useMemo(() => {
    const result = new Map<string, string[]>();
    for (const entry of entries) {
      if (!entry.safe) continue;
      const parts = entry.name.replace(/\/$/, '').split('/');
      for (let index = 1; index <= parts.length; index++) {
        const key = parts.slice(0, index).join('/');
        const names = result.get(key) ?? [];
        names.push(entry.name);
        result.set(key, names);
      }
    }
    return result;
  }, [entries]);
  const rows = useMemo(() => {
    const map = new Map<string, ArchiveEntry>();
    for (const entry of entries) {
      map.set(entry.name.replace(/\/$/, ''), entry);
      if (!entry.safe) continue;
      const parts = entry.name.replace(/\/$/, '').split('/');
      for (let i = 1; i < parts.length; i++) {
        const key = parts.slice(0, i).join('/');
        if (!map.has(key))
          map.set(key, {
            name: key + '/',
            directory: true,
            safe: true,
            encrypted: false,
            size: 0,
            compressed: 0,
          });
      }
    }
    return [...map.entries()].sort(([a, ae], [b, be]) => {
      const ap = a.split('/'),
        bp = b.split('/');
      for (let i = 0; i < Math.min(ap.length, bp.length); i++) {
        if (ap[i] === bp[i]) continue;
        const ad = i < ap.length - 1 || ae.directory,
          bd = i < bp.length - 1 || be.directory;
        if (ad !== bd) return ad ? -1 : 1;
        return ap[i].localeCompare(bp[i], undefined, { numeric: true });
      }
      return ap.length - bp.length;
    });
  }, [entries]);
  const visible = rows.filter(([key]) =>
    query
      ? key.toLowerCase().includes(query.toLowerCase())
      : ![...collapsed].some((folder) => key.startsWith(folder + '/')),
  );
  // 分批显示限制 DOM 数量，导航树滚动到底部时自动增加可见批次。
  const [limit, setLimit] = useState(150);
  const hints = JSON.stringify(
    visible
      .slice(0, limit)
      .filter(([, entry]) => !entry.directory)
      .slice(0, 6)
      .map(([, entry]) => entry.name),
  );
  useEffect(() => {
    onPrefetch(JSON.parse(hints) as string[]);
  }, [hints, onPrefetch]);
  return (
    <>
      <div
        className="archive-tree-scroll"
        onScroll={(event) => {
          const el = event.currentTarget;
          if (el.scrollHeight - el.scrollTop - el.clientHeight < 160)
            setLimit((value) => Math.min(value + 150, visible.length));
        }}
      >
        {visible.slice(0, limit).map(([key, entry]) => {
          const names = members.get(key) ?? [];
          const checked = names.length > 0 && names.every((name) => selected.includes(name));
          return (
            <div
              key={key}
              onMouseEnter={() => onPrefetch([entry.name])}
              onFocus={() => onPrefetch([entry.name])}
              className={`archive-tree-row${active === entry.name ? ' active' : ''}`}
              style={{ paddingLeft: 8 + (query ? 0 : key.split('/').length - 1) * 16 }}
            >
              <button
                className="archive-tree-chevron"
                disabled={busy || !entry.directory}
                aria-label={collapsed.has(key) ? '展开目录' : '折叠目录'}
                onClick={() =>
                  setCollapsed((old) => {
                    const next = new Set(old);
                    if (next.has(key)) next.delete(key);
                    else next.add(key);
                    return next;
                  })
                }
              >
                {entry.directory && (
                  <ChevronRight
                    size={14}
                    style={{ transform: collapsed.has(key) ? undefined : 'rotate(90deg)' }}
                  />
                )}
              </button>
              <input
                type="checkbox"
                aria-label={`选择 ${entry.name}`}
                checked={checked}
                disabled={busy || !entry.safe}
                onChange={() =>
                  onSelect(
                    checked
                      ? selected.filter((name) => !names.includes(name))
                      : [...new Set([...selected, ...names])],
                  )
                }
              />
              {entry.directory ? <Folder size={17} /> : <File size={17} />}
              <button
                className="archive-tree-name"
                title={entry.name}
                disabled={busy || !entry.safe}
                onClick={() => {
                  setActive(entry.name);
                  if (!entry.directory) onPreview(entry.name);
                  else
                    setCollapsed((old) => {
                      const next = new Set(old);
                      if (next.has(key)) next.delete(key);
                      else next.add(key);
                      return next;
                    });
                }}
              >
                {query ? entry.name : key.split('/').pop()}
              </button>
              {!entry.directory && <small>{formatSize(entry.size)}</small>}
            </div>
          );
        })}
        {!visible.length && <p className="archive-empty">没有匹配的文件</p>}
      </div>
      <div className="archive-tree-footer">
        已选择 {selected.length} 项{' '}
        <button disabled={busy || !selected.length} onClick={() => onSelect([])}>
          清除选择
        </button>
      </div>
    </>
  );
}
