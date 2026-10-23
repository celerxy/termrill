/** 目录仅解析文档内页目标；缩略图离开可见区域后释放画布。 */
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { PDFDocumentProxy } from 'pdfjs-dist';
import PdfPage from './PdfPage';
import ResizeSeparator from './ResizeSeparator';
import PdfOutlineView from './PdfOutlineView';
type SidebarTab = 'thumbs' | 'outline';

export default function PdfSidebar({
  document,
  current,
  jumpTo,
  ready,
  visible,
}: {
  document: PDFDocumentProxy;
  current: number;
  jumpTo: (page: number) => void;
  ready: boolean;
  visible: boolean;
}) {
  const [tab, setTab] = useState<SidebarTab>('thumbs');
  const [outlineOpened, setOutlineOpened] = useState(false);
  const [thumbView, setThumbView] = useState({ top: 0, height: 600 });
  const [sidebarWidth, setSidebarWidth] = useState(220);
  const [maxSidebarWidth, setMaxSidebarWidth] = useState(560);
  const sidebar = useRef<HTMLElement>(null);
  useEffect(() => {
    const parent = sidebar.current?.parentElement;
    if (!parent) return;
    // 阅读窗口缩小时限制目录宽度，始终为正文保留至少一半空间。
    const update = () => {
      const maximum = Math.max(150, Math.min(560, Math.floor(parent.clientWidth / 2)));
      setMaxSidebarWidth(maximum);
      setSidebarWidth((width) => Math.min(width, maximum));
    };
    const observer = new ResizeObserver(update);
    observer.observe(parent);
    update();
    return () => observer.disconnect();
  }, []);
  // 固定行高让横向、超长页面也不会改变整份缩略图列表的定位。
  const rowHeight = 246;
  const start = Math.max(0, Math.floor(thumbView.top / rowHeight) - 1);
  const end = Math.min(
    document.numPages,
    Math.ceil((thumbView.top + thumbView.height) / rowHeight) + 1,
  );
  const scroll = useRef<HTMLDivElement>(null);
  const thumbPosition = useRef({ top: 0, left: 0 });
  function changeTab(next: SidebarTab) {
    if (next === tab) return;
    if (next === 'outline') setOutlineOpened(true);
    setTab(next);
  }
  // 隐藏不卸载侧栏；再次显示时恢复独立的缩略图偏移，目录由自身保存位置。
  useLayoutEffect(() => {
    if (scroll.current && visible && tab === 'thumbs') {
      scroll.current.scrollTop = thumbPosition.current.top;
      scroll.current.scrollLeft = thumbPosition.current.left;
    }
  }, [tab, visible]);
  useEffect(() => {
    const root = scroll.current;
    if (!root || tab !== 'thumbs' || !visible) return;
    const update = () => {
      if (!root.clientHeight) return;
      thumbPosition.current = { top: root.scrollTop, left: root.scrollLeft };
      setThumbView({ top: root.scrollTop, height: root.clientHeight });
    };
    const observer = new ResizeObserver(update);
    observer.observe(root);
    root.addEventListener('scroll', update, { passive: true });
    update();
    return () => {
      observer.disconnect();
      root.removeEventListener('scroll', update);
    };
  }, [document, tab, visible]);
  useEffect(() => {
    const root = scroll.current;
    if (!root || tab !== 'thumbs' || !visible) return;
    const top = (current - 1) * rowHeight;
    if (top < root.scrollTop) root.scrollTop = top;
    else if (top + rowHeight > root.scrollTop + root.clientHeight)
      root.scrollTop = top + rowHeight - root.clientHeight;
    setThumbView({ top: root.scrollTop, height: root.clientHeight });
  }, [current, tab]);
  return (
    <aside
      className="pdf-sidebar"
      aria-label="PDF 页面导航"
      ref={sidebar}
      hidden={!visible}
      style={{ width: sidebarWidth, flexBasis: sidebarWidth }}
    >
      <div className="pdf-sidebar-tabs">
        <button aria-pressed={tab === 'thumbs'} onClick={() => changeTab('thumbs')}>
          缩略图
        </button>
        <button aria-pressed={tab === 'outline'} onClick={() => changeTab('outline')}>
          目录
        </button>
      </div>
      <div
        className="pdf-sidebar-scroll pdf-thumbnails-scroll"
        ref={scroll}
        hidden={tab !== 'thumbs'}
      >
        {tab === 'thumbs' ? (
          <>
            <div aria-hidden="true" style={{ height: start * rowHeight }} />
            {Array.from({ length: end - start }, (_, offset) => {
              const index = start + offset;
              return (
                <button
                  className={`pdf-thumbnail${current === index + 1 ? ' selected' : ''}`}
                  key={index}
                  data-thumb={index + 1}
                  aria-label={`跳转到第 ${index + 1} 页`}
                  aria-current={current === index + 1 ? 'page' : undefined}
                  onClick={() => jumpTo(index + 1)}
                >
                  <PdfPage
                    document={document}
                    number={index + 1}
                    width={172}
                    height={208}
                    mode="page"
                    zoom={1}
                    estimate={{ width: 612, height: 792 }}
                    active={ready && visible && tab === 'thumbs'}
                    thumbnail
                  />
                  <span>{index + 1}</span>
                </button>
              );
            })}
            <div aria-hidden="true" style={{ height: (document.numPages - end) * rowHeight }} />
          </>
        ) : null}
      </div>
      {outlineOpened && (
        <PdfOutlineView
          document={document}
          current={current}
          jumpTo={jumpTo}
          visible={visible && tab === 'outline'}
        />
      )}
      <ResizeSeparator
        axis="x"
        label="调整 PDF 目录宽度"
        value={sidebarWidth}
        min={150}
        max={maxSidebarWidth}
        initial={Math.min(220, maxSidebarWidth)}
        step={20}
        onChange={setSidebarWidth}
        onCommit={setSidebarWidth}
      />
    </aside>
  );
}
