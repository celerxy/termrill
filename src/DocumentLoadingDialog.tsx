/** 读取前即展示预览外壳，取消由请求所有者处理。 */
import { useEffect, useRef } from 'react';
import { FileCode2, X, Search, LoaderCircle, Maximize2, Minimize2 } from 'lucide-react';
import { useState } from 'react';
import PreviewLoading from './PreviewLoading';
import { basename } from './types';
import './remoteDocument.css';
export default function DocumentLoadingDialog({
  path,
  detail,
  onCancel,
}: {
  path: string;
  detail: string;
  onCancel: () => void;
}) {
  const button = useRef<HTMLButtonElement>(null);
  const [expanded, setExpanded] = useState(false);
  const format = basename(path).split('.').pop()?.toUpperCase() ?? 'FILE';
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    button.current?.focus();
    return () => previous?.focus();
  }, []);
  return (
    <div className="modal-backdrop">
      <section
        className={`remote-document text-document document-loading-dialog${expanded ? ' expanded' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-label="正在读取文件"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation();
            onCancel();
          }
          if (event.key === 'Tab') {
            const buttons = Array.from(
              event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'),
            );
            const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
            if (
              (!event.shiftKey && index === buttons.length - 1) ||
              (event.shiftKey && index <= 0)
            ) {
              event.preventDefault();
              buttons[event.shiftKey ? buttons.length - 1 : 0]?.focus();
            }
          }
        }}
      >
        <header className="document-heading">
          <FileCode2 size={28} />
          <div className="document-title-group">
            <div className="document-loading-title">
              <h2>{basename(path)}</h2>
              <span className="document-badge">{format}</span>
            </div>
            <p title={path}>{path}</p>
          </div>
          <button
            aria-label={expanded ? '还原窗口' : '扩大窗口'}
            onClick={() => setExpanded((value) => !value)}
          >
            {expanded ? <Minimize2 size={18} /> : <Maximize2 size={18} />}
          </button>
          <button aria-label="取消读取" onClick={onCancel}>
            <X size={20} />
          </button>
        </header>
        <div className="document-toolbar">
          <button disabled>
            <Search size={16} />
            查找
          </button>
          <span className="toolbar-spacer" />
          <span className="document-loading-pill">读取中</span>
        </div>
        <PreviewLoading title="正在读取文件内容" detail={detail} editor />
        <footer className="document-footer">
          <span className="document-loading-status">
            <LoaderCircle size={16} />
            正在读取文件
          </span>
          <button ref={button} onClick={onCancel}>
            取消读取
          </button>
        </footer>
      </section>
    </div>
  );
}
