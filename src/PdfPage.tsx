/** 可见页面单独绘制；离开预加载区域就释放画布，保留页面占位高度。 */
import { useEffect, useRef, useState } from 'react';
import { TextLayer, type PDFDocumentProxy } from 'pdfjs-dist';
import type { PdfMatch } from './usePdfSearch';
export default function PdfPage({
  document,
  number,
  width,
  zoom,
  estimate,
  active,
  rotation = 0,
  mode = 'width',
  height = 800,
  thumbnail = false,
  matches = [],
  selectedMatch,
  onMeasure,
  onRendered,
}: {
  document: PDFDocumentProxy;
  number: number;
  width: number;
  zoom: number;
  estimate: { width: number; height: number };
  active: boolean;
  rotation?: number;
  mode?: 'width' | 'page' | 'custom';
  height?: number;
  thumbnail?: boolean;
  matches?: PdfMatch[];
  selectedMatch?: PdfMatch;
  onMeasure?: (number: number, size: { width: number; height: number }) => void;
  onRendered?: () => void;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const textContainer = useRef<HTMLDivElement>(null);
  const [textVersion, setTextVersion] = useState(0);
  const [size, setSize] = useState(estimate);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [textError, setTextError] = useState(false);
  const rotated = rotation % 180 !== 0;
  const pageWidth = rotated ? size.height : size.width;
  const pageHeight = rotated ? size.width : size.height;
  const getScale = (w: number, h: number) => {
    if (mode === 'custom') return zoom;
    const fit = (width - (thumbnail ? 8 : 48)) / w;
    return Math.max(0.1, mode === 'page' ? Math.min(fit, (height - 40) / h) : fit);
  };
  const scale = getScale(pageWidth, pageHeight);
  const displayWidth = pageWidth * scale;
  const displayHeight = pageHeight * scale;
  useEffect(() => {
    if (!active) return;
    let disposed = false;
    let cancel: (() => void) | undefined;
    let layer: TextLayer | undefined;
    setLoading(true);
    setError('');
    setTextError(false);
    void document
      .getPage(number)
      .then(async (page) => {
        if (disposed || !canvas.current) return;
        const natural = page.getViewport({ scale: 1 });
        setSize((previous) =>
          previous.width === natural.width && previous.height === natural.height
            ? previous
            : { width: natural.width, height: natural.height },
        );
        onMeasure?.(number, { width: natural.width, height: natural.height });
        const viewport = page.getViewport({
          scale: getScale(
            rotated ? natural.height : natural.width,
            rotated ? natural.width : natural.height,
          ),
          rotation: page.rotate + rotation,
        });
        const target = canvas.current;
        // 高 DPI 和大页面仍保持有界画布，避免缩放时分配数百 MB。
        const ratio = Math.min(
          window.devicePixelRatio || 1,
          2,
          Math.sqrt(8_000_000 / (viewport.width * viewport.height)),
        );
        target.width = Math.max(1, Math.floor(viewport.width * ratio));
        target.height = Math.max(1, Math.floor(viewport.height * ratio));
        const context = target.getContext('2d');
        if (!context) throw new Error('无法创建 PDF 画布');
        const task = page.render({
          canvas: target,
          canvasContext: context,
          viewport,
          transform: [ratio, 0, 0, ratio, 0, 0],
        });
        cancel = () => task.cancel();
        await task.promise;
        if (!disposed) setLoading(false);
        if (!disposed) onRendered?.();
        if (disposed || thumbnail || !textContainer.current) return;
        try {
          const content = await page.getTextContent();
          if (disposed || !textContainer.current) return;
          textContainer.current.replaceChildren();
          textContainer.current.style.setProperty('--total-scale-factor', String(viewport.scale));
          layer = new TextLayer({
            textContentSource: content,
            container: textContainer.current,
            viewport,
          });
          await layer.render();
          if (!disposed) setTextVersion((value) => value + 1);
        } catch {
          if (!disposed) {
            textContainer.current?.replaceChildren();
            setTextError(true);
          }
        }
      })
      .then(() => {
        if (!disposed) setLoading(false);
      })
      .catch((e) => {
        if (!disposed) {
          setError(String(e));
          setLoading(false);
        }
      });
    const target = canvas.current;
    return () => {
      disposed = true;
      cancel?.();
      layer?.cancel();
      textContainer.current?.replaceChildren();
      // 清理当前渲染占用；新的 render 会重新设置尺寸。
      if (target) {
        target.width = 0;
        target.height = 0;
      }
    };
  }, [
    document,
    number,
    width,
    height,
    zoom,
    active,
    rotation,
    mode,
    thumbnail,
    onMeasure,
    onRendered,
  ]);
  useEffect(() => {
    const root = textContainer.current;
    if (!root) return;
    let offset = 0;
    for (const span of root.querySelectorAll<HTMLElement>('span[role="presentation"]')) {
      const text = span.textContent || '';
      span.replaceChildren();
      let cursor = 0;
      for (const match of matches) {
        const start = Math.max(0, match.start - offset);
        const end = Math.min(text.length, match.start + match.length - offset);
        if (end <= start) continue;
        span.append(text.slice(cursor, start));
        const mark = window.document.createElement('mark');
        mark.textContent = text.slice(start, end);
        if (selectedMatch?.start === match.start && selectedMatch.page === number)
          mark.className = 'current-match';
        span.append(mark);
        cursor = end;
      }
      span.append(text.slice(cursor));
      offset += text.length;
    }
  }, [matches, selectedMatch, number, textVersion]);
  useEffect(() => {
    textContainer.current
      ?.querySelector('.current-match')
      ?.scrollIntoView({ block: 'center', inline: 'nearest' });
  }, [selectedMatch, textVersion]);
  return (
    <section
      className="pdf-page"
      data-pdf-page={thumbnail ? undefined : number}
      data-pdf-scale={scale}
      aria-label={`PDF 第 ${number} 页`}
      style={{ width: displayWidth, height: displayHeight }}
    >
      {active && (
        <canvas
          ref={canvas}
          style={{
            width: displayWidth,
            height: displayHeight,
            visibility: loading || error ? 'hidden' : 'visible',
          }}
        />
      )}
      {active && !thumbnail && <div className="textLayer" ref={textContainer} />}
      {(loading || !active) && !error && (
        <span className="pdf-page-placeholder">
          第 {number} 页{active ? ' · 正在渲染…' : ''}
        </span>
      )}
      {textError && (
        <span className="pdf-text-warning" role="status">
          本页文字暂时无法选择或高亮
        </span>
      )}
      {error && (
        <p className="pdf-page-error" role="alert">
          第 {number} 页无法预览：{error}
        </p>
      )}
    </section>
  );
}
