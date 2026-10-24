import ErrorNotice from './ErrorNotice';
/** 已选设计稿的统一文件弹窗；保存始终由原有摘要校验流程处理。 */
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { FileCode2, X, Maximize2, Minimize2, Search, Braces, Save, Download } from 'lucide-react';
import { createPortal } from 'react-dom';
import MarkdownContent from './MarkdownContent';
import CodeEditor from './CodeEditor';
import SvgPreview from './SvgPreview';
import NotebookPreview from './NotebookPreview';
import MediaPreview from './MediaPreview';
import CsvPreview from './CsvPreview';
import { fileLanguage } from './fileLanguage';
import './remoteDocument.css';
export type RemoteDocument = {
  path: string;
  connectionId: string | null;
  content: string;
  original: string;
  revision: string;
  encoding?: string;
};
export default function RemoteDocumentDialog({
  document,
  kind,
  busy,
  error,
  onChange,
  onSave,
  onClose,
  onDownload,
  readOnly = false,
  initialBytes,
  previewNotice,
  sourceOnly = false,
  embedded = false,
  toolbarTarget,
  statusTarget,
  textComplete = true,
}: {
  document: RemoteDocument;
  kind: 'text' | 'image' | 'pdf' | 'epub';
  busy: boolean;
  error: string;
  onChange: (content: string) => void;
  onSave: () => void;
  onClose: () => void;
  onDownload: () => void;
  readOnly?: boolean;
  initialBytes?: ArrayBuffer;
  previewNotice?: ReactNode;
  sourceOnly?: boolean;
  embedded?: boolean;
  toolbarTarget?: HTMLElement | null;
  statusTarget?: HTMLElement | null;
  textComplete?: boolean;
}) {
  const [expanded, setExpanded] = useState(false);
  const [wrap, setWrap] = useState(true);
  const [search, setSearch] = useState(0);
  const [searchText, setSearchText] = useState('');
  const [searchNext, setSearchNext] = useState(0);
  const isSvg = document.path.toLowerCase().endsWith('.svg');
  const isNotebook = document.path.toLowerCase().endsWith('.ipynb');
  const isCsv = /\.(csv|tsv)$/i.test(document.path);
  const [preview, setPreview] = useState(isCsv || (!sourceOnly && (isSvg || isNotebook)));
  const [cursor, setCursor] = useState([1, 1]);
  const [formatError, setFormatError] = useState('');
  const [mediaStatus, setMediaStatus] = useState('正在加载…');
  const [pdfControlsTarget, setPdfControlsTarget] = useState<HTMLDivElement | null>(null);
  const panel = useRef<HTMLElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  const modified = document.content !== document.original;
  const language = fileLanguage(document.path).name;
  const closeSafely = () => {
    if (busy) return;
    if (!modified || window.confirm('文件有未保存修改，确定放弃并关闭？')) close.current();
  };
  const safeClose = useRef(closeSafely);
  safeClose.current = closeSafely;
  useEffect(() => {
    if (embedded) return;
    const previous = window.document.activeElement as HTMLElement | null;
    panel.current?.focus();
    const listener = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        safeClose.current();
      }
      if (event.key === 'Tab' && panel.current) {
        const nodes = Array.from(
          panel.current.querySelectorAll<HTMLElement>(
            'button:not(:disabled), input, [tabindex="0"], [contenteditable="true"]',
          ),
        ).filter((node) => node.getClientRects().length);
        const index = nodes.indexOf(window.document.activeElement as HTMLElement);
        if (
          nodes.length &&
          ((event.shiftKey && index <= 0) || (!event.shiftKey && index === nodes.length - 1))
        ) {
          event.preventDefault();
          nodes[event.shiftKey ? nodes.length - 1 : 0].focus();
        }
      }
    };
    panel.current?.addEventListener('keydown', listener);
    const current = panel.current;
    return () => {
      current?.removeEventListener('keydown', listener);
      previous?.focus();
    };
  }, []);
  const format = () => {
    try {
      onChange(JSON.stringify(JSON.parse(document.content), null, 2));
      setFormatError('');
    } catch (e) {
      setFormatError(`JSON 格式错误：${String(e)}`);
    }
  };
  const toolbar = (
    <div className="document-toolbar">
      {language === 'JSON' && (
        <button disabled={busy || readOnly} onClick={format}>
          <Braces size={18} />
          格式化
        </button>
      )}
      {embedded ? (
        <label className="archive-content-search">
          <Search size={15} />
          <input
            aria-label="在文件中查找"
            placeholder="在文件中查找"
            disabled={preview || kind !== 'text'}
            value={searchText}
            onChange={(event) => setSearchText(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                setSearchNext((value) => value + 1);
              }
            }}
          />
          <kbd>Ctrl F</kbd>
        </label>
      ) : (
        <button disabled={preview} onClick={() => setSearch((n) => n + 1)}>
          <Search size={17} />
          查找
        </button>
      )}
      {(isCsv || (!sourceOnly && (language === 'Markdown' || isSvg || isNotebook))) && (
        <button onClick={() => setPreview((v) => !v)}>
          {preview
            ? isCsv
              ? '原始文本'
              : '查看源码'
            : isCsv
              ? '表格预览'
              : isSvg
                ? '预览 SVG'
                : isNotebook
                  ? '预览 Notebook'
                  : '预览 Markdown'}
        </button>
      )}
      <span className="toolbar-spacer" />
      <label>
        自动换行
        <input
          type="checkbox"
          role="switch"
          checked={wrap}
          onChange={(e) => setWrap(e.target.checked)}
        />
      </label>
    </div>
  );
  const status =
    kind === 'text' ? (
      <footer className="document-footer">
        <div className="document-status" role="status">
          <span className={modified ? 'unsaved' : 'saved'} />
          {readOnly ? '只读预览' : modified ? '未保存修改' : '已保存'}
          <span>{document.encoding ?? 'UTF-8'}</span>
          <span>{language}</span>
          <span className="document-cursor" hidden={isCsv && preview}>
            行 {cursor[0]}，列 {cursor[1]}
          </span>
        </div>
        <div className="document-footer-actions">
          <button disabled={busy} onClick={closeSafely}>
            关闭
          </button>
          {!readOnly && (
            <button className="primary" disabled={busy || !modified} onClick={onSave}>
              <Save size={17} />
              {busy ? '保存中…' : document.connectionId ? '保存到远程' : '保存到本地'}
            </button>
          )}
        </div>
      </footer>
    ) : (
      <footer className={`document-reader-status${kind === 'pdf' ? ' pdf-reader-status' : ''}`}>
        <span>
          <i />
          只读预览
        </span>
        {kind === 'pdf' && <div className="pdf-status-controls" ref={setPdfControlsTarget} />}
        <span>{mediaStatus}</span>
      </footer>
    );
  return (
    <div className={embedded ? 'archive-inline-document' : 'modal-backdrop'}>
      <section
        ref={panel}
        tabIndex={-1}
        className={`remote-document ${kind === 'text' ? 'text-document' : 'media-document'} ${expanded ? 'expanded' : ''}`}
        role={embedded ? undefined : 'dialog'}
        aria-modal={embedded ? undefined : true}
        aria-labelledby="remote-document-title"
      >
        <header className="document-heading">
          <FileCode2 size={26} />
          <div className="document-title-group">
            <h2 id="remote-document-title">{document.path.split(/[\\/]/).pop()}</h2>
            <p title={document.path}>
              {document.path.slice(
                0,
                Math.max(document.path.lastIndexOf('/'), document.path.lastIndexOf('\\')) + 1,
              )}
            </p>
          </div>
          {kind === 'text' && <span className="document-badge">{language}</span>}
          <span className="toolbar-spacer" />
          {kind !== 'text' && !readOnly && (
            <button className="document-download" onClick={onDownload}>
              <Download size={17} />
              {document.connectionId ? '下载' : '上传'}
            </button>
          )}
          <button
            aria-label={expanded ? '还原窗口' : '扩大窗口'}
            onClick={() => setExpanded((v) => !v)}
          >
            {expanded ? <Minimize2 size={18} /> : <Maximize2 size={18} />}
          </button>
          <button disabled={busy} aria-label="关闭文件" onClick={closeSafely}>
            <X size={20} />
          </button>
        </header>
        {kind === 'text' ? (
          <>
            {toolbarTarget ? createPortal(toolbar, toolbarTarget) : toolbar}
            {preview && isCsv ? (
              <CsvPreview content={document.content} path={document.path} complete={textComplete} />
            ) : preview && isNotebook ? (
              <NotebookPreview content={document.content} wrap={wrap} />
            ) : preview && isSvg ? (
              <SvgPreview content={document.content} />
            ) : preview ? (
              <MarkdownContent className="document-markdown" source={document.content} />
            ) : (
              <CodeEditor
                value={document.content}
                path={document.path}
                disabled={busy || readOnly}
                wrap={wrap}
                search={search}
                searchText={embedded ? searchText : undefined}
                searchNext={searchNext}
                onChange={onChange}
                onCursor={(line, column) => setCursor([line, column])}
              />
            )}
          </>
        ) : (
          <MediaPreview
            initialBytes={initialBytes}
            id={document.connectionId}
            path={document.path}
            kind={kind}
            onStatus={setMediaStatus}
            pdfControlsTarget={pdfControlsTarget}
          />
        )}
        {previewNotice && (
          <div className="document-preview-notice" role="status">
            {previewNotice}
          </div>
        )}
        {(error || formatError) && <ErrorNotice error={error || formatError} scope="文件编辑" />}
        {statusTarget ? createPortal(status, statusTarget) : status}
      </section>
    </div>
  );
}
