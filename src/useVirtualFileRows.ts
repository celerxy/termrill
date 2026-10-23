import { useEffect, useRef, useState } from 'react';

// 与 live.css 的文件行高度一致。额外渲染相邻行，快速滚动时减少空白。
export const FILE_ROW_HEIGHT = 46;
const HEADER_HEIGHT = 33;
const OVERSCAN = 8;

/** 列表只挂载视口附近的行；滚动事件按动画帧合并，卸载时清理观察器。 */
export function useVirtualFileRows(
  count: number,
  resetKey: string,
  rowHeight = FILE_ROW_HEIGHT,
  headerHeight = HEADER_HEIGHT,
) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const focusFrame = useRef(0);
  const [viewport, setViewport] = useState({ first: 0, rows: 16 });
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    let frame = 0;
    const update = () => {
      frame = 0;
      if (!element.clientHeight) return;
      const first = Math.max(0, Math.floor((element.scrollTop - headerHeight) / rowHeight));
      const rows = Math.ceil(element.clientHeight / rowHeight) + 1;
      setViewport((previous) =>
        previous.first === first && previous.rows === rows ? previous : { first, rows },
      );
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(update);
    };
    const observer = new ResizeObserver(schedule);
    observer.observe(element);
    element.addEventListener('scroll', schedule, { passive: true });
    update();
    return () => {
      observer.disconnect();
      element.removeEventListener('scroll', schedule);
      cancelAnimationFrame(frame);
      cancelAnimationFrame(focusFrame.current);
    };
  }, [rowHeight, headerHeight]);
  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
    setViewport((previous) => ({ ...previous, first: 0 }));
  }, [resetKey]);
  const start = Math.min(Math.max(0, viewport.first - OVERSCAN), Math.max(0, count - 1));
  const end = Math.min(count, viewport.first + viewport.rows + OVERSCAN);
  const focusRow = (index: number) => {
    const element = scrollRef.current;
    if (!element || index < 0 || index >= count) return;
    element.scrollTop = index * rowHeight;
    setViewport((previous) => ({ ...previous, first: index }));
    cancelAnimationFrame(focusFrame.current);
    focusFrame.current = requestAnimationFrame(() => {
      element
        .querySelector<HTMLElement>(`[data-file-index="${index}"]`)
        ?.focus({ preventScroll: true });
    });
  };
  return {
    scrollRef,
    focusRow,
    start,
    end: Math.max(start, end),
    top: start * rowHeight,
    bottom: Math.max(0, count - Math.max(start, end)) * rowHeight,
  };
}
