/** EPUB 只读阅读器；异步结果隔离，关闭后释放所有书内图片 URL，不保存书籍数据。 */
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { BookOpen, ChevronLeft, ChevronRight, List, Minus, Plus } from 'lucide-react';
import { createEpubBook, type EpubBook, type EpubTarget } from './epubBook';
import ErrorNotice from './ErrorNotice';
import EpubContents from './EpubContents';
import { message } from './types';
import './epubReader.css';

export default function EpubPreview({
  bytes,
  onStatus,
}: {
  bytes: ArrayBuffer;
  onStatus: (status: string) => void;
}) {
  const [book, setBook] = useState<EpubBook | null>(null);
  const [error, setError] = useState('');
  const [chapter, setChapter] = useState(0);
  const [fragment, setFragment] = useState('');
  const [navigation, setNavigation] = useState(0);
  const [html, setHtml] = useState('');
  const [fontSize, setFontSize] = useState(18);
  const [showContents, setShowContents] = useState(true);
  const body = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let disposed = false;
    let loaded: EpubBook | undefined;
    setBook(null);
    setError('');
    setHtml('');
    setChapter(0);
    setFragment('');
    onStatus('正在解析 EPUB…');
    void invoke<Record<string, string>>('unpack_epub', bytes)
      .then((resources) => {
        if (disposed) return;
        loaded = createEpubBook(resources);
        setBook(loaded);
      })
      .catch((reason) => {
        if (!disposed) setError(message(reason));
      });
    return () => {
      disposed = true;
      loaded?.dispose();
    };
  }, [bytes, onStatus]);
  useEffect(() => {
    if (!book) return;
    try {
      setHtml(book.render(book.chapters[chapter].path));
      setError('');
      onStatus(`第 ${chapter + 1} / ${book.chapters.length} 章 · EPUB 只读`);
    } catch (reason) {
      setHtml('');
      setError(message(reason));
    }
  }, [book, chapter, onStatus]);
  useEffect(() => {
    if (error) onStatus('EPUB 加载失败');
  }, [error, onStatus]);
  useEffect(() => {
    if (!body.current) return;
    body.current.scrollTop = 0;
    if (fragment) {
      body.current.querySelector(`#${CSS.escape(`epub-${fragment}`)}`)?.scrollIntoView();
    }
  }, [html, fragment, navigation]);
  function navigate(target: Pick<EpubTarget, 'path' | 'fragment'>) {
    const index = book?.chapters.findIndex((item) => item.path === target.path) ?? -1;
    if (index >= 0) {
      setChapter(index);
      setFragment(target.fragment);
      setNavigation((value) => value + 1);
    }
  }
  function turnPage(offset: number) {
    if (!book) return;
    setChapter((current) => Math.max(0, Math.min(book.chapters.length - 1, current + offset)));
    setFragment('');
    setNavigation((value) => value + 1);
  }
  if (!book) {
    if (error) return <ErrorNotice error={error} scope="文件预览" />;
    return (
      <div className="document-empty" role="status">
        正在解析 EPUB…
      </div>
    );
  }
  // 同一 XHTML 可有多个目录锚点，只高亮当前目标对应的一行。
  const exactContent = book.contents.findIndex(
    (item) => item.path === book.chapters[chapter].path && item.fragment === fragment,
  );
  const activeContent =
    exactContent >= 0
      ? exactContent
      : book.contents.findIndex((item) => item.path === book.chapters[chapter].path);
  return (
    <div className="epub-reader">
      <div className="document-toolbar epub-toolbar">
        <button
          aria-label="切换 EPUB 目录"
          aria-pressed={showContents}
          onClick={() => setShowContents((value) => !value)}
        >
          <List size={18} />
        </button>
        <BookOpen size={18} />
        <strong title={book.title}>{book.title}</strong>
        <span className="toolbar-spacer" />
        <button
          aria-label="缩小 EPUB 字号"
          disabled={fontSize <= 14}
          onClick={() => setFontSize((value) => value - 2)}
        >
          <Minus size={16} />
        </button>
        <span>{fontSize}px</span>
        <button
          aria-label="放大 EPUB 字号"
          disabled={fontSize >= 32}
          onClick={() => setFontSize((value) => value + 2)}
        >
          <Plus size={16} />
        </button>
        <button disabled={chapter === 0} onClick={() => turnPage(-1)}>
          <ChevronLeft size={16} />
          上一章
        </button>
        <button disabled={chapter === book.chapters.length - 1} onClick={() => turnPage(1)}>
          下一章
          <ChevronRight size={16} />
        </button>
      </div>
      <div className="epub-layout">
        <EpubContents
          contents={book.contents}
          active={activeContent}
          hidden={!showContents}
          onNavigate={navigate}
        />
        <div
          className="epub-page"
          ref={body}
          style={{ fontSize }}
          onClick={(event) => {
            const anchor = (event.target as Element).closest('a');
            if (!anchor) return;
            event.preventDefault();
            const href = anchor.getAttribute('href');
            if (!href?.startsWith('#')) return;
            const [path, fragment = ''] = href.slice(1).split('#');
            navigate({ path: decodeURIComponent(path), fragment: decodeURIComponent(fragment) });
          }}
        >
          {error ? (
            <ErrorNotice error={error} scope="文件预览" />
          ) : (
            <article className="epub-prose" dangerouslySetInnerHTML={{ __html: html }} />
          )}
        </div>
      </div>
    </div>
  );
}
