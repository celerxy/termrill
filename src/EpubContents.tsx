/** EPUB 目录拥有独立滚动与宽度；隐藏时保留位置，事件和尺寸观察在卸载时清理。 */
import { useEffect, useRef, useState } from 'react';
import { ChevronLeft, ChevronRight } from 'lucide-react';
import type { EpubTarget } from './epubBook';
import ResizeSeparator from './ResizeSeparator';

export default function EpubContents({
  contents,
  active,
  hidden,
  onNavigate,
}: {
  contents: EpubTarget[];
  active: number;
  hidden: boolean;
  onNavigate: (target: EpubTarget) => void;
}) {
  const sidebar = useRef<HTMLElement>(null);
  const scroll = useRef<HTMLElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(240);
  const [maximum, setMaximum] = useState(560);
  const [canScroll, setCanScroll] = useState({ left: false, right: false });
  useEffect(() => {
    const parent = sidebar.current?.parentElement;
    if (!parent) return;
    // 窄窗口至少为正文保留一半空间，不把目录标题宽度当作侧栏最小宽度。
    function updateWidth() {
      if (!parent) return;
      const nextMaximum = Math.max(160, Math.min(560, Math.floor(parent.clientWidth / 2)));
      setMaximum(nextMaximum);
      setWidth((current) => Math.min(current, nextMaximum));
    }
    const observer = new ResizeObserver(updateWidth);
    observer.observe(parent);
    updateWidth();
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    const viewport = scroll.current;
    if (!viewport || hidden) return;
    function updateScroll() {
      if (!viewport) return;
      setCanScroll({
        left: viewport.scrollLeft > 1,
        right: viewport.scrollLeft + viewport.clientWidth < viewport.scrollWidth - 1,
      });
    }
    function handleWheel(event: WheelEvent) {
      if (!viewport || !event.shiftKey || event.deltaX || !event.deltaY) return;
      if (viewport.scrollWidth <= viewport.clientWidth) return;
      // 原生非被动监听确保 Shift+滚轮只平移目录，不同时滚动章节或整个弹窗。
      event.preventDefault();
      let unit = 1;
      if (event.deltaMode === 1) unit = 28;
      else if (event.deltaMode === 2) unit = viewport.clientWidth;
      viewport.scrollLeft += event.deltaY * unit;
    }
    const observer = new ResizeObserver(updateScroll);
    observer.observe(viewport);
    if (list.current) observer.observe(list.current);
    viewport.addEventListener('scroll', updateScroll, { passive: true });
    viewport.addEventListener('wheel', handleWheel, { passive: false });
    updateScroll();
    return () => {
      observer.disconnect();
      viewport.removeEventListener('scroll', updateScroll);
      viewport.removeEventListener('wheel', handleWheel);
    };
  }, [contents, hidden]);
  function pan(direction: number) {
    const viewport = scroll.current;
    if (viewport) viewport.scrollLeft += direction * Math.max(80, viewport.clientWidth * 0.75);
  }
  return (
    <aside
      className="epub-contents"
      ref={sidebar}
      hidden={hidden}
      style={{ width, flexBasis: width }}
    >
      <div className="epub-contents-heading">
        <h3>目录</h3>
        <div className="epub-contents-pan">
          <button
            aria-label="向左滚动 EPUB 目录"
            disabled={!canScroll.left}
            onClick={() => pan(-1)}
          >
            <ChevronLeft size={15} />
          </button>
          <button
            aria-label="向右滚动 EPUB 目录"
            disabled={!canScroll.right}
            onClick={() => pan(1)}
          >
            <ChevronRight size={15} />
          </button>
        </div>
      </div>
      <nav className="epub-contents-scroll" aria-label="EPUB 目录" tabIndex={0} ref={scroll}>
        <div className="epub-contents-list" ref={list}>
          {contents.map((item, index) => (
            <button
              key={index}
              aria-current={index === active ? 'location' : undefined}
              title={item.label}
              onClick={() => onNavigate(item)}
            >
              {item.label}
            </button>
          ))}
        </div>
      </nav>
      <ResizeSeparator
        axis="x"
        label="调整 EPUB 目录宽度"
        value={width}
        min={160}
        max={maximum}
        initial={Math.min(240, maximum)}
        step={20}
        onChange={setWidth}
        onCommit={setWidth}
      />
    </aside>
  );
}
