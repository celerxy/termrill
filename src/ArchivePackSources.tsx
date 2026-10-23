/** 来源路径预处理一次；搜索延后更新，虚拟列表只挂载视口附近的行。 */
import { memo, useDeferredValue, useMemo, useState } from 'react';
import { FileText, File, Search, ShieldCheck } from 'lucide-react';
import { useVirtualFileRows } from './useVirtualFileRows';
const ROW_HEIGHT = 56;
const HEADER_HEIGHT = 42;
function ArchivePackSources({
  sources,
  completed = false,
}: {
  sources: string[];
  completed?: boolean;
}) {
  const [query, setQuery] = useState('');
  const deferredQuery = useDeferredValue(query);
  const rows = useMemo(
    () =>
      sources.map((path) => {
        const offset = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\')) + 1;
        return {
          path,
          name: path.slice(offset),
          directory: path.slice(0, offset),
          search: path.toLowerCase(),
        };
      }),
    [sources],
  );
  const filtered = useMemo(() => {
    const needle = deferredQuery.trim().toLowerCase();
    return needle ? rows.filter((row) => row.search.includes(needle)) : rows;
  }, [rows, deferredQuery]);
  const virtual = useVirtualFileRows(filtered.length, deferredQuery, ROW_HEIGHT, HEADER_HEIGHT);
  return (
    <div className="archive-content archive-pack-content">
      {!completed && (
        <div className="archive-toolbar">
          <label className="archive-search">
            <Search size={18} />
            <input
              aria-label="搜索打包来源"
              placeholder="搜索打包来源"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          </label>
        </div>
      )}
      <div className="archive-list-heading">
        <h3>打包来源</h3>
        <span>
          {filtered.length} 项{deferredQuery.trim() ? '匹配结果' : ''}
        </span>
      </div>
      <div
        className="archive-list archive-pack-list"
        ref={virtual.scrollRef}
        style={
          completed
            ? { height: Math.min(322, 44 + Math.max(1, filtered.length) * ROW_HEIGHT) }
            : undefined
        }
        aria-busy={query !== deferredQuery}
      >
        <table aria-label="打包来源" aria-rowcount={filtered.length + 1}>
          <thead>
            <tr>
              <th>名称</th>
              <th>所在目录</th>
            </tr>
          </thead>
          <tbody>
            {virtual.top > 0 && (
              <tr className="archive-pack-spacer" aria-hidden="true">
                <td colSpan={2} style={{ height: virtual.top }} />
              </tr>
            )}
            {filtered.slice(virtual.start, virtual.end).map((row, index) => (
              <tr key={row.path} aria-rowindex={virtual.start + index + 2}>
                <td title={row.name}>
                  <div className="archive-entry-name">
                    {/\.pdf$/i.test(row.name) ? (
                      <FileText className="archive-pdf-icon" size={21} />
                    ) : (
                      <File size={21} />
                    )}
                    <span>{row.name}</span>
                  </div>
                </td>
                <td title={row.directory}>
                  <span>{row.directory}</span>
                </td>
              </tr>
            ))}
            {virtual.bottom > 0 && (
              <tr className="archive-pack-spacer" aria-hidden="true">
                <td colSpan={2} style={{ height: virtual.bottom }} />
              </tr>
            )}
          </tbody>
        </table>
        {!filtered.length && (
          <div className="archive-empty">
            {sources.length ? '没有匹配的来源' : '未选择打包来源'}
          </div>
        )}
      </div>
      {!completed && (
        <div className="archive-pack-preserve">
          <ShieldCheck size={18} />
          打包后保留原始文件
        </div>
      )}
    </div>
  );
}
export default memo(ArchivePackSources);
