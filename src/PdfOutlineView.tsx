/** 目录标题搜索与可见行渲染；目标解析只使用已加载 PDF，旧文档结果在卸载后丢弃。 */
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { ChevronDown, ChevronRight, LocateFixed } from 'lucide-react';
import type { PDFDocumentProxy } from 'pdfjs-dist';
import {
  activePdfOutline,
  flattenPdfOutline,
  resolvePdfDestination,
  visiblePdfOutline,
  type PdfOutlineEntry,
} from './pdfOutline';

export default function PdfOutlineView({
  document,
  current,
  jumpTo,
  visible,
}: {
  document: PDFDocumentProxy;
  current: number;
  jumpTo: (page: number) => void;
  visible: boolean;
}) {
  const [entries, setEntries] = useState<PdfOutlineEntry[] | null>(null);
  const [collapsed, setCollapsed] = useState<Set<number>>(new Set());
  const [query, setQuery] = useState('');
  const [error, setError] = useState('');
  const [view, setView] = useState({ top: 0, height: 600 });
  const [contentWidth, setContentWidth] = useState(0);
  const scroll = useRef<HTMLDivElement>(null);
  const position = useRef({ top: 0, left: 0 });
  const alive = useRef(true);
  const pendingFollow = useRef(false);
  useEffect(() => {
    let stopped = false;
    alive.current = true;
    void (async () => {
      try {
        const items = flattenPdfOutline((await document.getOutline()) ?? []);
        if (stopped) return;
        setCollapsed(
          new Set(
            items.filter((item) => item.depth > 0 && item.hasChildren).map((item) => item.id),
          ),
        );
        setEntries(items.map((item) => ({ ...item })));
        // 有界解析，避免长目录一次向 PDF Worker 派发全部目标请求。
        for (let start = 0; start < items.length && !stopped; start += 8) {
          await Promise.all(
            items.slice(start, start + 8).map(async (item) => {
              try {
                item.page = await resolvePdfDestination(document, item.dest);
              } catch {
                item.page = null;
              }
            }),
          );
        }
        if (!stopped) setEntries(items.map((item) => ({ ...item })));
      } catch {
        if (!stopped) setError('目录读取失败');
      }
    })();
    return () => {
      stopped = true;
      alive.current = false;
    };
  }, [document]);
  const active = useMemo(() => activePdfOutline(entries ?? [], current), [entries, current]);
  // 跟随阅读时只打开当前章节的祖先；用户随后仍可手动折叠。
  useEffect(() => {
    if (active === null || !entries) return;
    setCollapsed((previous) => {
      const next = new Set(previous);
      let parent = entries[active].parent;
      while (parent !== null) {
        next.delete(parent);
        parent = entries[parent].parent;
      }
      return next;
    });
  }, [active, entries]);
  const rows = useMemo(
    () => visiblePdfOutline(entries ?? [], collapsed, query),
    [entries, collapsed, query],
  );
  useLayoutEffect(() => {
    const root = scroll.current;
    if (!root || !visible) return;
    root.scrollTop = position.current.top;
    root.scrollLeft = position.current.left;
    const update = () => {
      if (root.clientHeight) setView({ top: root.scrollTop, height: root.clientHeight });
    };
    const observer = new ResizeObserver(update);
    observer.observe(root);
    update();
    return () => observer.disconnect();
  }, [visible]);
  useLayoutEffect(() => {
    const root = scroll.current;
    if (!root || !visible) return;
    const context = window.document.createElement('canvas').getContext('2d');
    if (!context) return;
    context.font = getComputedStyle(root).font;
    setContentWidth(
      rows.reduce(
        (width, row) =>
          Math.max(width, context.measureText(row.title).width + Math.min(row.depth, 20) * 12 + 36),
        0,
      ),
    );
  }, [rows, visible]);
  useEffect(() => {
    const root = scroll.current;
    if (!visible || !root || query.trim() || active === null || !pendingFollow.current) return;
    const index = rows.findIndex((row) => row.id === active);
    if (index < 0) return;
    pendingFollow.current = false;
    const top = index * 26;
    if (top < root.scrollTop) root.scrollTop = top;
    else if (top + 26 > root.scrollTop + root.clientHeight)
      root.scrollTop = top + 26 - root.clientHeight;
    position.current.top = root.scrollTop;
    setView({ top: root.scrollTop, height: root.clientHeight });
  }, [active, rows, visible, query]);
  function toggle(id: number) {
    setCollapsed((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }
  function locateCurrent() {
    if (active === null || !entries) return;
    pendingFollow.current = true;
    setCollapsed((previous) => {
      const next = new Set(previous);
      let parent = entries[active].parent;
      while (parent !== null) {
        next.delete(parent);
        parent = entries[parent].parent;
      }
      return next;
    });
  }
  async function navigate(entry: PdfOutlineEntry) {
    setError('');
    try {
      const page = entry.page ?? (await resolvePdfDestination(document, entry.dest));
      if (!alive.current) return;
      if (page === null) setError('无法定位该目录项');
      else jumpTo(page);
    } catch {
      if (alive.current) setError('无法定位该目录项');
    }
  }
  const start = Math.max(0, Math.floor(view.top / 26) - 5);
  const end = Math.min(rows.length, Math.ceil((view.top + view.height) / 26) + 5);
  return (
    <div className="pdf-outline-panel" hidden={!visible}>
      <div className="pdf-outline-tools">
        <input
          aria-label="搜索 PDF 目录标题"
          placeholder="搜索目录标题"
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            position.current = { top: 0, left: 0 };
            if (scroll.current) {
              scroll.current.scrollTop = 0;
              scroll.current.scrollLeft = 0;
            }
            setView((previous) => ({ ...previous, top: 0 }));
          }}
        />
        <div>
          <button
            disabled={!entries?.length || !!query.trim()}
            onClick={() => setCollapsed(new Set())}
          >
            全部展开
          </button>
          <button
            disabled={!entries?.length || !!query.trim()}
            onClick={() =>
              setCollapsed(
                new Set(entries?.filter((item) => item.hasChildren).map((item) => item.id)),
              )
            }
          >
            全部折叠
          </button>
          <button
            aria-label="定位当前章节"
            title="定位当前章节"
            disabled={active === null || !!query.trim()}
            onClick={locateCurrent}
          >
            <LocateFixed size={14} />
          </button>
        </div>
      </div>
      {error && (
        <p className="pdf-outline-message" role="status">
          {error}
        </p>
      )}
      <div
        className="pdf-sidebar-scroll"
        ref={scroll}
        onScroll={(event) => {
          if (!visible) return;
          const root = event.currentTarget;
          position.current = { top: root.scrollTop, left: root.scrollLeft };
          setView({ top: root.scrollTop, height: root.clientHeight });
        }}
      >
        {!entries && !error && <p className="pdf-outline-message">正在读取目录…</p>}
        {entries && !rows.length && (
          <p className="pdf-outline-message" role="status">
            {entries.length ? '没有匹配的目录标题' : '文档没有内置目录'}
          </p>
        )}
        <div
          className="pdf-outline-virtual"
          role="list"
          aria-label="PDF 目录"
          style={{ height: rows.length * 26, minWidth: contentWidth }}
        >
          {rows.slice(start, end).map((entry, offset) => {
            const expanded = !!query.trim() || !collapsed.has(entry.id);
            return (
              <div
                key={entry.id}
                role="listitem"
                aria-posinset={start + offset + 1}
                aria-setsize={rows.length}
                className="pdf-outline-row"
                data-outline-id={entry.id}
                style={{ top: (start + offset) * 26, paddingLeft: Math.min(entry.depth, 20) * 12 }}
              >
                {entry.hasChildren ? (
                  <button
                    className="pdf-outline-toggle"
                    aria-label={`${expanded ? '折叠' : '展开'} ${entry.title}`}
                    aria-expanded={expanded}
                    disabled={!!query.trim()}
                    onClick={() => toggle(entry.id)}
                  >
                    {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                  </button>
                ) : (
                  <span className="pdf-outline-indent" aria-hidden="true" />
                )}
                <button
                  className="pdf-outline-title"
                  title={entry.title}
                  aria-current={active === entry.id ? 'location' : undefined}
                  disabled={!entry.dest}
                  onClick={() => void navigate(entry)}
                >
                  {entry.title}
                </button>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
