/** 正文只挂载视口附近的页面；尺寸变化按页内比例恢复阅读位置。 */
import { useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { pageAt, pdfScale, restorePdfTop, type PdfSize } from './pdfLayout';
export function usePdfViewport(
  count: number,
  estimate: PdfSize,
  rotation: number,
  mode: 'width' | 'page' | 'custom',
  zoom: number,
) {
  const area = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ width: 800, height: 800, top: 0 });
  const [sizes, setSizes] = useState(new Map<number, PdfSize>());
  const offsets = useMemo(() => {
    const result = [0];
    for (let index = 0; index < count; index++) {
      const natural = sizes.get(index + 1) ?? estimate;
      const size = rotation % 180 ? { width: natural.height, height: natural.width } : natural;
      result.push(
        result[index] + size.height * pdfScale(size, view.width, view.height, mode, zoom) + 24,
      );
    }
    return result;
  }, [count, sizes, estimate, rotation, view.width, view.height, mode, zoom]);
  const previous = useRef(offsets);
  useLayoutEffect(() => {
    const root = area.current;
    if (!root) return;
    const old = previous.current;
    if (old !== offsets && old.length > 1 && old.length === offsets.length) {
      // 使用更新布局前的滚动位置，避免缩小后浏览器先截断 scrollTop。
      root.scrollTop = restorePdfTop(old, offsets, view.top);
    }
    previous.current = offsets;
    setView((value) => ({ ...value, top: root.scrollTop }));
  }, [offsets]);
  useLayoutEffect(() => {
    const root = area.current;
    if (!root) return;
    let frame = 0;
    const update = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() =>
        setView({ width: root.clientWidth, height: root.clientHeight, top: root.scrollTop }),
      );
    };
    const observer = new ResizeObserver(update);
    observer.observe(root);
    root.addEventListener('scroll', update, { passive: true });
    update();
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      root.removeEventListener('scroll', update);
    };
  }, []);
  const measure = useCallback((number: number, size: PdfSize) => {
    setSizes((old) => {
      const existing = old.get(number);
      if (existing?.width === size.width && existing.height === size.height) return old;
      const next = new Map(old);
      next.set(number, size);
      return next;
    });
  }, []);
  const reset = useCallback(() => {
    setSizes(new Map());
    if (area.current) area.current.scrollTop = 0;
  }, []);
  const jumpTo = useCallback(
    (number: number) => {
      const index = Math.max(0, Math.min(count - 1, number - 1));
      if (area.current) {
        area.current.scrollTop = offsets[index] ?? 0;
        setView((value) => ({ ...value, top: area.current?.scrollTop ?? 0 }));
      }
    },
    [count, offsets],
  );
  const start = pageAt(offsets, Math.max(0, view.top - 600));
  const end = Math.min(count, pageAt(offsets, view.top + view.height + 600) + 1);
  return {
    area,
    view,
    offsets,
    sizes,
    measure,
    reset,
    jumpTo,
    start,
    end,
    page: count ? pageAt(offsets, view.top + 26) + 1 : 1,
  };
}
