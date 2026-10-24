import ErrorNotice from './ErrorNotice';
import PreviewLoading from './PreviewLoading';
/** 有界本地/远程媒体预览；关闭时取消读取，Blob URL 只存于当前面板并及时释放。 */
import { lazy, Suspense, useEffect, useRef, useState } from 'react';
import { Channel, invoke } from '@tauri-apps/api/core';
const loadPdfReader = () => import('./PdfPreview');
const PdfPreview = lazy(loadPdfReader);
const loadEpubReader = () => import('./EpubPreview');
const EpubPreview = lazy(loadEpubReader);
import { formatSize, message } from './types';
import { Image as ImageIcon, Minus, Plus } from 'lucide-react';
export default function MediaPreview({
  id,
  path,
  kind,
  onStatus,
  initialBytes,
  pdfControlsTarget,
}: {
  initialBytes?: ArrayBuffer;
  id: string | null;
  path: string;
  kind: 'image' | 'pdf' | 'epub';
  onStatus: (status: string) => void;
  pdfControlsTarget?: HTMLElement | null;
}) {
  const [bytes, setBytes] = useState<ArrayBuffer | null>(null);
  const [url, setUrl] = useState('');
  const [read, setRead] = useState(0);
  const [error, setError] = useState('');
  const [zoom, setZoom] = useState<number | null>(null);
  const [dimensions, setDimensions] = useState({ width: 0, height: 0 });
  const viewport = useRef<HTMLDivElement>(null);
  const drag = useRef<{ x: number; y: number; left: number; top: number } | null>(null);
  useEffect(() => {
    if (error) onStatus('加载失败');
    else if (!bytes) onStatus(`正在读取 · ${formatSize(read)}`);
    else if (kind === 'image' && bytes)
      onStatus(`${dimensions.width} × ${dimensions.height} · ${formatSize(bytes.byteLength)}`);
  }, [bytes, read, error, kind, dimensions, onStatus]);
  useEffect(() => {
    let disposed = false;
    let request: string | undefined;
    let objectUrl: string | undefined;
    const started = performance.now();
    let firstByte = 0;
    setBytes(null);
    setError('');
    setRead(0);
    // 网络读取期间并行准备模块，避免完整文件到达后才开始加载阅读器。
    if (kind === 'pdf') void loadPdfReader().catch(() => {});
    if (kind === 'epub') void loadEpubReader().catch(() => {});
    const progress = new Channel<number>();
    progress.onmessage = (value) => {
      if (!firstByte && value > 0) firstByte = performance.now() - started;
      if (!disposed) setRead(value);
    };
    void (async () => {
      try {
        if (!initialBytes) request = await invoke<string>('begin_preview');
        if (disposed) {
          await invoke('cancel_preview', { request });
          return;
        }
        const data =
          initialBytes ??
          (await invoke<ArrayBuffer>(id ? 'read_remote_preview' : 'read_local_preview', {
            id,
            path,
            request,
            onProgress: progress,
          }));
        if (disposed) return;
        if (import.meta.env.DEV)
          console.info('[TermRill preview]', {
            readMs: Math.round(performance.now() - started),
            firstByteMs: Math.round(firstByte),
            bytes: data.byteLength,
          });
        setBytes(data);
        if (kind === 'image') {
          const header = new Uint8Array(data);
          const mime =
            header[0] === 137
              ? 'image/png'
              : header[0] === 255
                ? 'image/jpeg'
                : header[0] === 71
                  ? 'image/gif'
                  : header[0] === 66
                    ? 'image/bmp'
                    : header[0] === 0 && header[2] === 1
                      ? 'image/x-icon'
                      : String.fromCharCode(...header.slice(4, 8)) === 'ftyp'
                        ? 'image/avif'
                        : 'image/webp';
          objectUrl = URL.createObjectURL(new Blob([data], { type: mime }));
          setUrl(objectUrl);
        }
      } catch (e) {
        if (!disposed) setError(message(e));
      }
    })();
    return () => {
      disposed = true;
      if (request) void invoke('cancel_preview', { request }).catch(() => {});
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [id, path, kind, initialBytes]);
  if (error) return <ErrorNotice error={error} scope="文件预览" />;
  if (!bytes)
    return (
      <PreviewLoading
        title={`正在读取${id ? '远程' : '本地'}文件`}
        detail={`已读取 ${formatSize(read)} · 关闭窗口可取消加载`}
      />
    );
  if (kind === 'pdf')
    return (
      <Suspense fallback={<div className="document-empty">正在加载 PDF 阅读器…</div>}>
        <PdfPreview bytes={bytes} onStatus={onStatus} controlsTarget={pdfControlsTarget} />
      </Suspense>
    );
  if (kind === 'epub') {
    return (
      <Suspense fallback={<div className="document-empty">正在加载 EPUB 阅读器…</div>}>
        <EpubPreview bytes={bytes} onStatus={onStatus} />
      </Suspense>
    );
  }
  return (
    <>
      <div className="document-toolbar reader-toolbar">
        <span className="reader-mode">
          <ImageIcon size={16} />
          图片预览
        </span>
        <div className="reader-controls">
          <button
            aria-label="缩小图片"
            onClick={() => setZoom((z) => Math.max(0.25, (z ?? 1) - 0.25))}
          >
            <Minus size={16} />
          </button>
          <span>{zoom === null ? '适应窗口' : `${Math.round(zoom * 100)}%`}</span>
          <button
            aria-label="放大图片"
            onClick={() => setZoom((z) => Math.min(4, (z ?? 1) + 0.25))}
          >
            <Plus size={16} />
          </button>
          <span className="reader-divider" />
          <button onClick={() => setZoom(null)}>适应窗口</button>
          <button onClick={() => setZoom(1)}>原始尺寸</button>
        </div>
        <span />
      </div>
      <div
        className={`document-image ${zoom === null ? 'fit-image' : ''}`}
        ref={viewport}
        onPointerDown={(e) => {
          if (!viewport.current) return;
          drag.current = {
            x: e.clientX,
            y: e.clientY,
            left: viewport.current.scrollLeft,
            top: viewport.current.scrollTop,
          };
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          if (drag.current && viewport.current) {
            viewport.current.scrollLeft = drag.current.left - e.clientX + drag.current.x;
            viewport.current.scrollTop = drag.current.top - e.clientY + drag.current.y;
          }
        }}
        onPointerUp={() => {
          drag.current = null;
        }}
        onPointerCancel={() => {
          drag.current = null;
        }}
      >
        <img
          src={url}
          alt={path.split('/').pop()}
          draggable={false}
          onError={() => setError('图片解码失败，请下载后打开')}
          onLoad={(e) => {
            setDimensions({
              width: e.currentTarget.naturalWidth,
              height: e.currentTarget.naturalHeight,
            });
            if (zoom !== null)
              e.currentTarget.style.width = `${e.currentTarget.naturalWidth * zoom}px`;
          }}
          style={
            zoom === null
              ? { maxWidth: '100%', maxHeight: '100%', objectFit: 'contain' }
              : { width: `calc(var(--image-width, 1px) * ${zoom})`, maxWidth: 'none' }
          }
          ref={(node) => {
            if (node && zoom !== null) node.style.width = `${node.naturalWidth * zoom}px`;
          }}
        />
      </div>
    </>
  );
}
