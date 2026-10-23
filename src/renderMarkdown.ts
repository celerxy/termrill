/** Markdown 先净化，再仅用文本节点与固定 CSS 类为代码块着色。 */
import { Marked } from 'marked';
import DOMPurify from 'dompurify';
import markedFootnote from 'marked-footnote';
import markedAlert from 'marked-alert';
import { highlightMarkdownCode } from './markdownHighlight';
import { markdownMath, renderMarkdownMath } from './markdownMath';

export function renderMarkdown(source: string): string {
  // 每篇独立解析器，脚注计数与公式状态不能泄漏到其他文档或单元。
  const parser = new Marked(markedFootnote(), markedAlert(), markdownMath());
  parser.use({
    extensions: [
      {
        name: 'termrillMark',
        level: 'inline',
        start: (text) => text.indexOf('=='),
        tokenizer(text) {
          const match = /^==([^=\n]+)==/.exec(text);
          if (match)
            return {
              type: 'termrillMark',
              raw: match[0],
              tokens: this.lexer.inlineTokens(match[1]),
            };
        },
        renderer(token) {
          return `<mark>${this.parser.parseInline(token.tokens ?? [])}</mark>`;
        },
      },
    ],
  });
  const clean = DOMPurify.sanitize(parser.parse(source, { async: false, gfm: true }) as string, {
    FORBID_TAGS: ['img', 'iframe', 'style', 'form'],
    FORBID_ATTR: ['style'],
  });
  const document = new DOMParser().parseFromString(clean, 'text/html');
  // 文档表格独立滚动，序号首列仅按内容占宽，不继承文件列表的 48% 列宽。
  for (const table of document.querySelectorAll('table')) {
    const heading = table.querySelector('tr > th');
    if (heading && /^(#|序号|编号|no\.?|index)$/i.test(heading.textContent?.trim() ?? '')) {
      table.classList.add('md-index-table');
    }
    const wrapper = document.createElement('div');
    wrapper.className = 'md-table-scroll';
    table.replaceWith(wrapper);
    wrapper.append(table);
  }
  // 有界解析，长代码块或大量代码保持可读文本，不阻塞预览窗口。
  const deadline = performance.now() + 150;
  for (const block of document.querySelectorAll('pre > code')) {
    let name = Array.from(block.classList)
      .find((value) => value.startsWith('language-'))
      ?.slice(9)
      .toLowerCase();
    const code = block.textContent ?? '';
    if (
      !name &&
      /^(?:graph|flowchart)\s+(?:TB|TD|BT|RL|LR)\b|^(?:sequenceDiagram|classDiagram|stateDiagram(?:-v2)?|erDiagram|gantt|pie|mindmap|timeline|gitGraph|journey)\b/.test(
        code.trimStart(),
      )
    ) {
      block.classList.add('language-mermaid');
      name = 'mermaid';
    }
    if (name === 'mermaid') continue;
    if (['math', 'latex', 'tex', 'katex'].includes(name ?? '')) {
      renderMarkdownMath(block, code, true);
      continue;
    }
    if (performance.now() < deadline) highlightMarkdownCode(block, name);
    else block.setAttribute('data-highlight-pending', name ?? '');
  }
  for (const block of document.querySelectorAll('code')) {
    if (block.closest('pre')) continue;
    if (performance.now() < deadline) highlightMarkdownCode(block);
    else block.setAttribute('data-highlight-pending', '');
  }
  for (const element of document.querySelectorAll('[data-termrill-math]')) {
    const value = element.getAttribute('data-termrill-math') ?? '';
    try {
      renderMarkdownMath(
        element,
        decodeURIComponent(value),
        element.getAttribute('data-math-display') === 'true',
      );
    } catch {
      element.textContent = value;
    }
    element.removeAttribute('data-termrill-math');
    element.removeAttribute('data-math-display');
  }
  return document.body.innerHTML;
}
