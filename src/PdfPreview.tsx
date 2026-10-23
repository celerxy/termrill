import ErrorNotice from './ErrorNotice';
/** PDF 连续滚动阅读：附近页面按需渲染，页码跳转定位占位节点。 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { getDocument, GlobalWorkerOptions, type PDFDocumentProxy } from 'pdfjs-dist';
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
import PdfPage from './PdfPage';
import PdfSidebar from './PdfSidebar';
import { usePdfSearch } from './usePdfSearch';
import { usePdfViewport } from './usePdfViewport';
import { usePdfReadingMemory } from './usePdfReadingMemory';
import {
  FileText,
  Minus,
  Plus,
  PanelLeft,
  Search,
  ChevronLeft,
  ChevronRight,
  ChevronsLeft,
  ChevronsRight,
  RotateCw,
  Maximize,
} from 'lucide-react';
import 'pdfjs-dist/web/pdf_viewer.css';
import './pdfReader.css';
GlobalWorkerOptions.workerSrc = workerUrl;
export default function PdfPreview({
  bytes,
  onStatus,
  controlsTarget,
}: {
  bytes: ArrayBuffer;
  onStatus: (status: string) => void;
  controlsTarget?: HTMLElement | null;
}) {
  const [document, setDocument] = useState<PDFDocumentProxy | null>(null);
  const [page, setPage] = useState(1);
  const [pageInput, setPageInput] = useState('1');
  const [zoom, setZoom] = useState(1);
  const [mode, setMode] = useState<'width' | 'page' | 'custom'>('width');
  const [rotation, setRotation] = useState(0);
  const [sidebar, setSidebar] = useState(true);
  const [query, setQuery] = useState('');
  const [matchIndex, setMatchIndex] = useState(0);
  const shell = useRef<HTMLDivElement>(null);
  const [fullscreenActive, setFullscreenActive] = useState(false);
  useEffect(() => {
    // 全屏仅包含阅读器本身，控件必须随之回到阅读器；退出后再合入外层状态栏。
    const update = () => setFullscreenActive(window.document.fullscreenElement === shell.current);
    window.document.addEventListener('fullscreenchange', update);
    return () => window.document.removeEventListener('fullscreenchange', update);
  }, []);
  const [firstCanvas, setFirstCanvas] = useState(false);
  const loadTiming = useRef({ start: performance.now(), painted: false });
  const onRendered = useCallback(() => {
    if (loadTiming.current.painted) return;
    loadTiming.current.painted = true;
    setFirstCanvas(true);
    if (import.meta.env.DEV)
      console.info('[TermRill PDF]', {
        firstCanvasMs: Math.round(performance.now() - loadTiming.current.start),
      });
  }, []);
  const search = usePdfSearch(document, query);
  const selectedMatch = search.matches[matchIndex];
  const pageMatches = useMemo(() => {
    const grouped = new Map<number, typeof search.matches>();
    search.matches.forEach((match) => {
      const list = grouped.get(match.page) ?? [];
      list.push(match);
      grouped.set(match.page, list);
    });
    return grouped;
  }, [search.matches]);
  const [error, setError] = useState('');
  const [readerMessage, setReaderMessage] = useState('');
  const [estimate, setEstimate] = useState({ width: 612, height: 792 });
  const viewport = usePdfViewport(document?.numPages ?? 0, estimate, rotation, mode, zoom);
  const {
    area,
    view: { width, height },
  } = viewport;
  const memoryMessage = usePdfReadingMemory(
    document,
    { page: viewport.page, mode, zoom },
    (saved) => {
      setMode(saved.mode);
      setZoom(saved.zoom);
      viewport.jumpTo(saved.page);
    },
  );
  useEffect(() => {
    setPage(viewport.page);
    setPageInput(String(viewport.page));
  }, [viewport.page]);
  useEffect(() => {
    if (error) onStatus('PDF 无法预览');
    else if (document) onStatus(`第 ${page} 页 / 共 ${document.numPages} 页`);
    else onStatus('正在加载 PDF…');
  }, [document, page, error, onStatus]);
  useEffect(() => {
    let disposed = false;
    loadTiming.current = { start: performance.now(), painted: false };
    setFirstCanvas(false);
    viewport.reset();
    setDocument(null);
    setError('');
    setPage(1);
    setPageInput('1');
    setQuery('');
    const assetRoot = new URL('/pdfjs/', window.location.href).href;
    const task = getDocument({
      // 完整数据就绪后再进入阅读器；翻页和搜索不再触发远程范围读取。
      // 副本避免 PDF.js Worker 转移原缓冲，StrictMode 重建仍能安全加载。
      data: new Uint8Array(bytes.slice(0)),
      cMapUrl: `${assetRoot}cmaps/`,
      standardFontDataUrl: `${assetRoot}standard_fonts/`,
      wasmUrl: `${assetRoot}wasm/`,
      iccUrl: `${assetRoot}iccs/`,
      // 扫描 PDF 常用 JPEG 2000 / JBIG2；本地 WASM 解码避免大型 JS 回退模块阻塞。
      useWasm: true,
      maxImageSize: 32 * 1024 * 1024,
    });
    void task.promise
      .then(async (pdf) => {
        const firstPage = await pdf.getPage(1);
        if (disposed) return;
        const viewport = firstPage.getViewport({ scale: 1 });
        setEstimate({ width: viewport.width, height: viewport.height });
        setDocument(pdf);
        if (import.meta.env.DEV)
          console.info('[TermRill PDF]', {
            parsedMs: Math.round(performance.now() - loadTiming.current.start),
            pages: pdf.numPages,
          });
      })
      .catch((e) => {
        if (!disposed) {
          setError((previous) => previous || String(e));
        }
      });
    return () => {
      disposed = true;
      void task.destroy();
    };
  }, [bytes]);
  const jumpTo = (number: number) => {
    const target = Math.max(1, Math.min(document?.numPages ?? 1, number));
    viewport.jumpTo(target);
    setPage(target);
    setPageInput(String(target));
  };
  useEffect(() => {
    setMatchIndex(0);
  }, [query]);
  useEffect(() => {
    if (selectedMatch) jumpTo(selectedMatch.page);
  }, [selectedMatch]);
  const nextMatch = (direction: number) => {
    if (!search.matches.length) return;
    setMatchIndex((index) => (index + direction + search.matches.length) % search.matches.length);
  };
  const setManualZoom = (value: number) => {
    setMode('custom');
    setZoom(Math.max(0.25, Math.min(3, value)));
  };
  const adjustZoom = (delta: number) => {
    const rendered = Number(
      area.current?.querySelector<HTMLElement>(`[data-pdf-page="${page}"]`)?.dataset.pdfScale,
    );
    setManualZoom((mode === 'custom' || !rendered ? zoom : rendered) + delta);
  };
  const fullScreen = async () => {
    try {
      if (window.document.fullscreenElement) await window.document.exitFullscreen();
      else await shell.current?.requestFullscreen();
    } catch {
      setReaderMessage('无法进入全屏，可使用窗口右上角扩大按钮');
    }
  };
  const footer = (
    <div className="pdf-reader-footer">
      <span>只读预览</span>
      <div className="reader-controls">
        <button aria-label="第一页" disabled={!document || page === 1} onClick={() => jumpTo(1)}>
          <ChevronsLeft size={16} />
        </button>
        <button
          aria-label="上一页"
          disabled={!document || page === 1}
          onClick={() => jumpTo(page - 1)}
        >
          <ChevronLeft size={16} />
        </button>
        <label>
          页码{' '}
          <input
            aria-label="PDF 页码"
            type="number"
            min={1}
            max={document?.numPages ?? 1}
            value={pageInput}
            onChange={(e) => setPageInput(e.target.value)}
            onBlur={() => jumpTo(Number(pageInput) || page)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') jumpTo(Number(pageInput) || page);
            }}
          />{' '}
          / {document?.numPages ?? '…'}
        </label>
        <button
          aria-label="下一页"
          disabled={!document || page === document.numPages}
          onClick={() => jumpTo(page + 1)}
        >
          <ChevronRight size={16} />
        </button>
        <button
          aria-label="最后一页"
          disabled={!document || page === document.numPages}
          onClick={() => jumpTo(document?.numPages ?? 1)}
        >
          <ChevronsRight size={16} />
        </button>
        <span className="reader-divider" />
        <button aria-label="缩小 PDF" onClick={() => adjustZoom(-0.25)}>
          <Minus size={16} />
        </button>
        <select
          aria-label="PDF 缩放比例"
          value={mode === 'custom' ? zoom : mode}
          onChange={(e) => {
            if (e.target.value === 'width' || e.target.value === 'page') setMode(e.target.value);
            else setManualZoom(Number(e.target.value));
          }}
        >
          <option value="width">适应宽度</option>
          <option value="page">适应整页</option>
          {mode === 'custom' && !Number.isInteger(zoom * 4) && (
            <option value={zoom}>{Math.round(zoom * 100)}%</option>
          )}
          {[0.25, 0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 2.25, 2.5, 2.75, 3].map((value) => (
            <option key={value} value={value}>
              {Math.round(value * 100)}%
            </option>
          ))}
        </select>
        <button aria-label="放大 PDF" onClick={() => adjustZoom(0.25)}>
          <Plus size={16} />
        </button>
        <span className="reader-divider" />
        <button
          aria-pressed={mode === 'width'}
          onClick={() => {
            setMode('width');
            setZoom(1);
          }}
        >
          适应宽度
        </button>
        <button aria-pressed={mode === 'page'} onClick={() => setMode('page')}>
          适应整页
        </button>
        <button
          aria-label="顺时针旋转 PDF"
          title="旋转"
          onClick={() => setRotation((value) => (value + 90) % 360)}
        >
          <RotateCw size={16} />
        </button>
        <button aria-label="切换 PDF 全屏" onClick={() => void fullScreen()}>
          <Maximize size={16} />
        </button>
      </div>
    </div>
  );
  return (
    <div className="pdf-reader" ref={shell}>
      <div className="document-toolbar reader-toolbar">
        <button
          aria-label="切换 PDF 侧栏"
          aria-expanded={sidebar}
          onClick={() => setSidebar((value) => !value)}
        >
          <PanelLeft size={17} />
        </button>
        <span className="reader-mode">
          <FileText size={16} />
          PDF 预览
        </span>
        <div className="pdf-search">
          <Search size={16} />
          <input
            aria-label="搜索 PDF 内容"
            placeholder="搜索文档内容"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') nextMatch(event.shiftKey ? -1 : 1);
            }}
          />
          <span role="status">
            {search.searching
              ? '搜索中…'
              : query.trim()
                ? `${search.matches.length ? matchIndex + 1 : 0} / ${search.matches.length}`
                : ''}
          </span>
          <button
            aria-label="上一个匹配"
            disabled={!search.matches.length}
            onClick={() => nextMatch(-1)}
          >
            <ChevronLeft size={16} />
          </button>
          <button
            aria-label="下一个匹配"
            disabled={!search.matches.length}
            onClick={() => nextMatch(1)}
          >
            <ChevronRight size={16} />
          </button>
        </div>
      </div>
      {search.error && (
        <p className="pdf-reader-message" role="status">
          {search.error}
        </p>
      )}
      {readerMessage && (
        <p className="pdf-reader-message" role="status">
          {readerMessage}
        </p>
      )}
      {memoryMessage && (
        <p className="pdf-reader-message" role="status">
          {memoryMessage}
        </p>
      )}
      <div className="pdf-reader-body">
        {document && (
          <PdfSidebar
            document={document}
            current={page}
            jumpTo={jumpTo}
            ready={firstCanvas}
            visible={sidebar}
          />
        )}
        <div className="document-pdf" ref={area}>
          {error && <ErrorNotice error={`PDF 无法预览：${error}`} scope="文件预览" />}
          {!document && !error && <div className="pdf-loading">正在加载 PDF…</div>}
          {document && (
            <div aria-hidden="true" style={{ height: viewport.offsets[viewport.start] }} />
          )}
          {document &&
            Array.from({ length: viewport.end - viewport.start }, (_, offset) => {
              const index = viewport.start + offset;
              return (
                <PdfPage
                  key={index + 1}
                  document={document}
                  number={index + 1}
                  width={width}
                  height={height}
                  zoom={zoom}
                  mode={mode}
                  rotation={rotation}
                  estimate={viewport.sizes.get(index + 1) ?? estimate}
                  onMeasure={viewport.measure}
                  onRendered={onRendered}
                  active
                  matches={pageMatches.get(index + 1)}
                  selectedMatch={selectedMatch}
                />
              );
            })}
          {document && (
            <div
              aria-hidden="true"
              style={{
                height: viewport.offsets[document.numPages] - viewport.offsets[viewport.end],
              }}
            />
          )}
        </div>
      </div>
      {controlsTarget && !fullscreenActive ? createPortal(footer, controlsTarget) : footer}
    </div>
  );
}
