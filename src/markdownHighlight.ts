/** 预览代码使用完整语言库；未指定语言只在常用语言中做有界识别。 */
import hljs from 'highlight.js';
import DOMPurify from 'dompurify';

const candidates = [
  'bash',
  'python',
  'javascript',
  'typescript',
  'json',
  'yaml',
  'xml',
  'css',
  'sql',
  'java',
  'cpp',
  'csharp',
  'go',
  'rust',
  'ruby',
  'php',
  'ini',
  'powershell',
];
const aliases: Record<string, string> = {
  shell: 'bash',
  sh: 'bash',
  zsh: 'bash',
  console: 'shell',
  terminal: 'shell',
  env: 'ini',
  conf: 'ini',
  config: 'ini',
  docker: 'dockerfile',
  jsonc: 'json',
  jsx: 'javascript',
  tsx: 'typescript',
  vue: 'xml',
  svelte: 'xml',
};
export function highlightMarkdownCode(element: Element, name?: string) {
  const code = element.textContent ?? '';
  if (!code || code.length > 50000 || ['text', 'txt', 'plain', 'plaintext'].includes(name ?? ''))
    return;
  const command =
    /^(?:\s*[$>]\s*)?(?:git|pip3?|python3?|npm|pnpm|yarn|cargo|docker|kubectl|curl|wget|cd|source|export|sudo)\s/m.test(
      code,
    );
  const language = name ? (aliases[name] ?? name) : command ? 'bash' : undefined;
  try {
    const result = language
      ? hljs.getLanguage(language)
        ? hljs.highlight(code, { language, ignoreIllegals: true })
        : null
      : code.length <= 10000
        ? hljs.highlightAuto(code, candidates)
        : null;
    if (!result || (!language && result.relevance < 3)) return;
    // 着色器输出只允许 span 与 class，源文件中的 HTML 仍然是转义文本。
    element.innerHTML = DOMPurify.sanitize(result.value, {
      ALLOWED_TAGS: ['span'],
      ALLOWED_ATTR: ['class'],
    });
    element.classList.add('hljs');
    if (result.language) element.setAttribute('data-highlight-language', result.language);
    if (['bash', 'shell', 'powershell'].includes(language ?? result.language ?? '')) {
      highlightShellDetails(element);
    }
  } catch {
    // 未识别或不合法的语言保留原文，不能阻断整篇预览。
  }
}

function highlightShellDetails(element: Element) {
  const document = element.ownerDocument;
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  while (walker.nextNode() && nodes.length < 3000) {
    const node = walker.currentNode as Text;
    if (!node.parentElement?.closest('.hljs-string, .hljs-comment')) nodes.push(node);
  }
  for (const node of nodes) {
    const pattern =
      /(^|\n)([ \t]*(?:[$>]\s*)?)(git|pip3?|python3?|npm|pnpm|yarn|cargo|docker|kubectl|curl|wget|cd|source|export|sudo|cp|ls|mkdir|jupyter)\b|(?<!\w)(--?[a-zA-Z][\w-]*)\b|(?:\.{1,2}\/|~\/|[A-Za-z]:\\)[^\s"'<>]+/g;
    const fragment = document.createDocumentFragment();
    let position = 0;
    for (const match of node.data.matchAll(pattern)) {
      const start = match.index;
      const prefix = match[3] ? `${match[1]}${match[2]}` : '';
      fragment.append(document.createTextNode(node.data.slice(position, start) + prefix));
      const span = document.createElement('span');
      span.className = match[3] ? 'hljs-built_in' : match[4] ? 'hljs-attr' : 'hljs-string';
      span.textContent = match[3] ?? match[4] ?? match[0];
      fragment.append(span);
      position = start + match[0].length;
    }
    if (!position) continue;
    fragment.append(document.createTextNode(node.data.slice(position)));
    node.replaceWith(fragment);
  }
}
