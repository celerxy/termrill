/** 恢复完成后才允许写入；切换文档和关闭阅读器时提交最后一份有效快照。 */
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { PDFDocumentProxy } from 'pdfjs-dist';
import { loadPdfReadingState, savePdfReadingState, type PdfReadingState } from './pdfReadingState';

export function usePdfReadingMemory(
  document: PDFDocumentProxy | null,
  state: PdfReadingState,
  restore: (state: PdfReadingState) => void,
) {
  const [message, setMessage] = useState('');
  const [restored, setRestored] = useState<PDFDocumentProxy | null>(null);
  const restoreLatest = useRef(restore);
  restoreLatest.current = restore;
  const snapshot = useRef<{ id: string; state: PdfReadingState } | null>(null);
  function saveSnapshot() {
    const value = snapshot.current;
    if (value) savePdfReadingState(window.localStorage, value.id, value.state);
  }
  useLayoutEffect(() => {
    snapshot.current = null;
    setRestored(null);
    setMessage('');
    if (!document) return;
    const id = document.fingerprints.filter(Boolean).join(':');
    try {
      const saved = loadPdfReadingState(window.localStorage, id, document.numPages);
      if (saved) restoreLatest.current(saved);
    } catch {
      setMessage('无法读取本机 PDF 阅读记录，本次仍可正常阅读');
    }
    // 等布局与跳页落定，防止初始第一页覆盖已保存的进度。
    const frame = requestAnimationFrame(() => setRestored(document));
    const flushOnClose = () => {
      try {
        saveSnapshot();
      } catch {
        /* 关闭时的存储失败不阻断文档资源释放。 */
      }
    };
    // 桌面窗口关闭未必触发 React 卸载；页面退出时也提交尚未到防抖期限的快照。
    window.addEventListener('pagehide', flushOnClose);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener('pagehide', flushOnClose);
      flushOnClose();
      snapshot.current = null;
    };
  }, [document]);
  useEffect(() => {
    if (!document || restored !== document) return;
    snapshot.current = { id: document.fingerprints.filter(Boolean).join(':'), state };
    const timer = window.setTimeout(() => {
      try {
        saveSnapshot();
      } catch {
        setMessage('无法保存本机 PDF 阅读进度；请检查本机存储空间或网页数据权限');
      }
    }, 400);
    return () => window.clearTimeout(timer);
  }, [document, restored, state.page, state.zoom, state.mode]);
  return message;
}
