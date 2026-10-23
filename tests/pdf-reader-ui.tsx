/** 使用生产侧栏、布局 Hook 和进度 Hook；页画布以占位块代替，不验证 PDF 解码或服务器。 */
import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import type { PDFDocumentProxy } from 'pdfjs-dist';
import PdfSidebar from '../src/PdfSidebar';
import PdfPreview from '../src/PdfPreview';
import { pdfReaderSample } from './pdf-reader-sample';
import { usePdfViewport } from '../src/usePdfViewport';
import { usePdfReadingMemory } from '../src/usePdfReadingMemory';
import '../src/styles.css';
import '../src/live.css';
import '../src/workspace.css';
import '../src/remoteDocument.css';
import '../src/pdfReader.css';

type FixtureOutline = { title: string; dest: number[]; items: FixtureOutline[] };
function outlineItem(title: string, page: number, items: FixtureOutline[] = []): FixtureOutline {
  return { title, dest: [page - 1], items };
}
function outlineLeaf(title: string, page: number): FixtureOutline {
  return { title, dest: [page - 1], items: [] };
}
const outlines = [
  outlineItem('第一章 · 长标题测试 ' + 'Long title without wrapping '.repeat(8), 1, [
    outlineItem('第二层 · LLMs', 2, [outlineLeaf('第三层 · What is an LLM?', 3)]),
  ]),
  outlineLeaf('   ', 5),
  ...Array.from({ length: 9998 }, (_, index) =>
    outlineLeaf(`章节 ${index + 1}`, Math.min(1000, index + 6)),
  ),
];
const documents = ['a', 'b'].map(
  (id) =>
    ({
      fingerprints: [`termrill-pdf-ui-regression-${id}`],
      numPages: 1000,
      getOutline: async () => outlines,
    }) as unknown as PDFDocumentProxy,
);
const pageSize = { width: 612, height: 792 };

function Reader({ document }: { document: PDFDocumentProxy }) {
  const [visible, setVisible] = useState(true);
  const [zoom, setZoom] = useState(1);
  const [mode, setMode] = useState<'width' | 'page' | 'custom'>('width');
  const viewport = usePdfViewport(1000, pageSize, 0, mode, zoom);
  const message = usePdfReadingMemory(document, { page: viewport.page, zoom, mode }, (saved) => {
    setMode(saved.mode);
    setZoom(saved.zoom);
    viewport.jumpTo(saved.page);
  });
  return (
    <div
      className="remote-document media-document"
      style={{ width: '95vw', height: '85vh', margin: 12 }}
    >
      <div className="pdf-reader">
        <div className="document-toolbar reader-toolbar">
          <button aria-label="切换 PDF 侧栏" onClick={() => setVisible((value) => !value)}>
            侧栏
          </button>
          <button onClick={() => viewport.jumpTo(viewport.page + 1)}>下一页</button>
          <button onClick={() => viewport.jumpTo(500)}>跳转第500页</button>
          <select
            aria-label="PDF 缩放比例"
            value={mode === 'custom' ? String(zoom) : mode}
            onChange={(event) => {
              const value = event.target.value;
              if (value === 'width' || value === 'page') setMode(value);
              else {
                setMode('custom');
                setZoom(Number(value));
              }
            }}
          >
            <option value="width">适应宽度</option>
            <option value="page">适应整页</option>
            <option value="1">100%</option>
            <option value="1.75">175%</option>
          </select>
          <span role="status">第 {viewport.page} 页 / 共 1000 页</span>
        </div>
        {message && <p role="alert">{message}</p>}
        <div className="pdf-reader-body">
          <PdfSidebar
            document={document}
            current={viewport.page}
            jumpTo={viewport.jumpTo}
            ready={false}
            visible={visible}
          />
          <div className="document-pdf" ref={viewport.area}>
            <div style={{ height: viewport.offsets[1000] }}>
              <div style={{ position: 'sticky', top: 0 }}>
                千页正文占位；生产布局 Hook 根据滚动定位当前页。
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
function App() {
  const [open, setOpen] = useState(true);
  const [documentIndex, setDocumentIndex] = useState(0);
  const [realPdf, setRealPdf] = useState(false);
  const [controlsTarget, setControlsTarget] = useState<HTMLDivElement | null>(null);
  const [status, setStatus] = useState('正在加载…');
  return (
    <>
      <p>隔离回归：万项目录、千页正文、长标题、多级与空标题；不连接服务器。</p>
      <button onClick={() => setOpen((value) => !value)}>{open ? '关闭阅读器' : '重新打开'}</button>
      <button onClick={() => setDocumentIndex((value) => 1 - value)}>切换文档</button>
      <button onClick={() => setRealPdf((value) => !value)}>
        {realPdf ? '返回隔离样本' : '真实 PDF.js 样本'}
      </button>
      <span>当前文档 {documentIndex === 0 ? 'A' : 'B'}</span>
      {open &&
        (realPdf ? (
          <div
            className="remote-document media-document"
            style={{ width: '95vw', height: '85vh', margin: 12 }}
          >
            <PdfPreview
              bytes={pdfReaderSample}
              onStatus={setStatus}
              controlsTarget={controlsTarget}
            />
            <footer className="document-reader-status pdf-reader-status">
              <span>
                <i />
                只读预览
              </span>
              <div className="pdf-status-controls" ref={setControlsTarget} />
              <span>{status}</span>
            </footer>
          </div>
        ) : (
          <Reader key={documentIndex} document={documents[documentIndex]} />
        ))}
    </>
  );
}
const root = window.document.getElementById('root');
if (root) createRoot(root).render(<App />);
