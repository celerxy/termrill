/** 只对已恢复且完成布局的章节保存；图片或字体变化后仍按相对位置恢复。 */
import { useEffect, useRef, useState, type RefObject } from 'react';
import type { EpubBook } from './epubBook';
import {
  loadEpubReadingState,
  saveEpubReadingState,
  type EpubReadingState,
} from './epubReadingState';

export function useEpubReadingMemory(
  bytes: ArrayBuffer,
  book: EpubBook | null,
  body: RefObject<HTMLDivElement | null>,
  state: { chapter: number; fontSize: number; html: string },
  restore: (state: EpubReadingState) => void,
) {
  const [notice, setNotice] = useState('');
  const [identity, setIdentity] = useState<{ book: EpubBook; id: string } | null>(null);
  const restoreLatest = useRef(restore);
  restoreLatest.current = restore;
  const pending = useRef<EpubReadingState | null>(null);
  const interacted = useRef(false);
  const snapshot = useRef<{ id: string; state: EpubReadingState } | null>(null);
  useEffect(() => {
    let disposed = false;
    setIdentity(null);
    setNotice('');
    pending.current = null;
    interacted.current = false;
    if (!book) return;
    void crypto.subtle
      .digest('SHA-256', bytes)
      .then((digest) => {
        if (disposed) return;
        const id = Array.from(new Uint8Array(digest), (value) =>
          value.toString(16).padStart(2, '0'),
        ).join('');
        try {
          const saved = loadEpubReadingState(localStorage, id, book.chapters.length);
          pending.current = interacted.current ? null : saved;
          if (saved && !interacted.current) restoreLatest.current(saved);
        } catch {
          setNotice('无法读取 EPUB 阅读记录，本次仍可阅读；原记录未覆盖');
        }
        setIdentity({ book, id });
      })
      .catch(() => {
        if (!disposed) setNotice('无法生成书籍指纹，本次不保存阅读进度');
      });
    return () => {
      disposed = true;
    };
  }, [bytes, book]);

  useEffect(() => {
    const element = body.current;
    if (!identity || identity.book !== book || !element || !state.html) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const position = () =>
      element.scrollTop / Math.max(1, element.scrollHeight - element.clientHeight);
    const capture = () => {
      if (pending.current) return;
      snapshot.current = {
        id: identity.id,
        state: {
          chapter: state.chapter,
          fontSize: state.fontSize,
          progress: Math.max(0, Math.min(1, position())),
        },
      };
    };
    const flush = () => {
      if (!snapshot.current) return;
      try {
        saveEpubReadingState(localStorage, snapshot.current.id, snapshot.current.state);
      } catch {
        setNotice('无法保存 EPUB 阅读进度，请检查本机存储空间或数据权限');
      }
    };
    const apply = () => {
      if (pending.current)
        element.scrollTop =
          pending.current.progress * Math.max(0, element.scrollHeight - element.clientHeight);
    };
    const stopRestoring = () => {
      interacted.current = true;
      pending.current = null;
      capture();
    };
    // 图片加载会改变章节高度；用户主动滚动或导航后立即停止旧位置补偿。
    const resize = new ResizeObserver(apply);
    if (element.firstElementChild) resize.observe(element.firstElementChild);
    const frame = requestAnimationFrame(() => {
      apply();
      if (!pending.current) {
        capture();
        timer = setTimeout(flush, 400);
      }
    });
    const scroll = () => {
      capture();
      clearTimeout(timer);
      timer = setTimeout(flush, 400);
    };
    const exit = () => {
      capture();
      flush();
    };
    element.addEventListener('scroll', scroll);
    element.addEventListener('wheel', stopRestoring, { passive: true });
    element.addEventListener('touchstart', stopRestoring, { passive: true });
    element.addEventListener('pointerdown', stopRestoring);
    element.addEventListener('keydown', stopRestoring);
    window.addEventListener('pagehide', exit);
    return () => {
      // 不在切章清理中重新读取 DOM：此时正文可能已经换成下一章。
      flush();
      clearTimeout(timer);
      cancelAnimationFrame(frame);
      resize.disconnect();
      element.removeEventListener('scroll', scroll);
      element.removeEventListener('wheel', stopRestoring);
      element.removeEventListener('touchstart', stopRestoring);
      element.removeEventListener('pointerdown', stopRestoring);
      element.removeEventListener('keydown', stopRestoring);
      window.removeEventListener('pagehide', exit);
      snapshot.current = null;
    };
  }, [identity, book, body, state.chapter, state.fontSize, state.html]);
  function navigationStarted() {
    interacted.current = true;
    pending.current = null;
  }
  return { notice, navigationStarted };
}
