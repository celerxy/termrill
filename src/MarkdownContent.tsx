/** Mermaid 按可见范围加载，过期结果不写回已关闭面板。 */
import { useEffect, useMemo, useRef } from 'react';
import { renderMarkdown } from './renderMarkdown';
import { highlightMarkdownCode } from './markdownHighlight';

export default function MarkdownContent({
  source,
  className,
}: {
  source: string;
  className: string;
}) {
  const html = useMemo(() => renderMarkdown(source), [source]);
  const host = useRef<HTMLElement>(null);
  useEffect(() => {
    const pending = Array.from(host.current?.querySelectorAll('[data-highlight-pending]') ?? []);
    let timer: ReturnType<typeof setTimeout>;
    const next = () => {
      const block = pending.shift();
      if (!block) return;
      if (host.current?.contains(block)) {
        highlightMarkdownCode(block, block.getAttribute('data-highlight-pending') || undefined);
        block.removeAttribute('data-highlight-pending');
      }
      timer = setTimeout(next, 16);
    };
    timer = setTimeout(next, 16);
    return () => clearTimeout(timer);
  }, [html]);
  useEffect(() => {
    const root = host.current;
    if (!root) return;
    let disposed = false;
    const pending = new Set<Element>();
    const statuses: HTMLElement[] = [];
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting || pending.has(entry.target)) continue;
          pending.add(entry.target);
          observer.unobserve(entry.target);
          const code = entry.target as HTMLElement;
          const pre = code.parentElement;
          if (!pre) continue;
          const source = code.textContent ?? '';
          const status = document.createElement('div');
          status.className = 'markdown-diagram-status';
          status.textContent = '正在渲染图表…';
          statuses.push(status);
          pre.before(status);
          void import('./renderMermaid')
            .then((module) => module.renderMermaid(source))
            .then((svg) => {
              if (disposed || !root.contains(pre)) return;
              const panel = document.createElement('figure');
              panel.className = 'markdown-diagram';
              panel.innerHTML = svg;
              const details = document.createElement('details');
              details.className = 'markdown-diagram-source';
              const summary = document.createElement('summary');
              summary.textContent = '查看图表源码';
              details.append(summary);
              pre.replaceWith(panel, details);
              details.append(pre);
              status.remove();
            })
            .catch((error: unknown) => {
              if (!disposed && root.contains(pre)) {
                const reason = error instanceof Error ? error.message : String(error);
                status.textContent = `图表渲染失败：${reason.slice(0, 500)}`;
                status.classList.add('is-error');
              }
            });
        }
      },
      { root: root.closest('.document-notebook') ?? root, rootMargin: '300px' },
    );
    root.querySelectorAll('pre > code.language-mermaid').forEach((code) => observer.observe(code));
    return () => {
      disposed = true;
      observer.disconnect();
      statuses.forEach((status) => status.remove());
    };
  }, [html]);
  return (
    <article
      ref={host}
      className={className}
      dangerouslySetInnerHTML={{ __html: html }}
      onClick={(event) => {
        const link = (event.target as Element).closest('a');
        if (!link) return;
        event.preventDefault();
        const href = link.getAttribute('href');
        if (href?.startsWith('#')) {
          try {
            host.current
              ?.querySelector(`#${CSS.escape(decodeURIComponent(href.slice(1)))}`)
              ?.scrollIntoView({ block: 'nearest' });
          } catch {
            // 无效锚点不影响阅读。
          }
        }
      }}
    />
  );
}
