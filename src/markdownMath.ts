/** 公式扩展先生成文本占位节点；净化正文后才生成可信的 KaTeX 排版。 */
import type { MarkedExtension } from 'marked';
import katex from 'katex';
import 'katex/dist/katex.min.css';

export function markdownMath(): MarkedExtension {
  return {
    extensions: [false, true].map((display) => ({
      name: display ? 'termrillBlockMath' : 'termrillInlineMath',
      level: display ? ('block' as const) : ('inline' as const),
      start(source: string) {
        const index = source.search(display ? /\$\$|\\\[/ : /\$|\\\(/);
        return index < 0 ? undefined : index;
      },
      tokenizer(source: string) {
        const match = display
          ? /^(?:\$\$([\s\S]+?)\$\$|\\\[([\s\S]+?)\\\])(?:\n|$)/.exec(source)
          : /^(?:\$([^\s$](?:[^$\n]*?[^\s$])?)\$(?!\d)|\\\(([^\n]+?)\\\))/.exec(source);
        if (!match) return;
        const text = match[1] ?? match[2];
        if (!display && /^\d+(?:[.,]\d+)?$/.test(text)) return;
        return { type: display ? 'termrillBlockMath' : 'termrillInlineMath', raw: match[0], text };
      },
      renderer(token) {
        return `<span data-termrill-math="${encodeURIComponent(token.text)}" data-math-display="${display}"></span>${display ? '\n' : ''}`;
      },
    })),
  };
}

export function renderMarkdownMath(element: Element, source: string, display: boolean) {
  if (source.length > 10000) {
    element.textContent = source;
    return;
  }
  try {
    element.innerHTML = katex.renderToString(source, {
      displayMode: display,
      trust: false,
      throwOnError: true,
      strict: 'ignore',
      maxExpand: 200,
      maxSize: 20,
    });
    element.classList.add('md-math');
  } catch {
    element.textContent = source;
    element.classList.add('md-math-error');
    element.setAttribute('title', '公式无法解析，已保留原文');
  }
}
