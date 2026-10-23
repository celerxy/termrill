/** 本地严格模式渲染排队，避免跨单元配置和 DOM 冲突。 */
import DOMPurify from 'dompurify';
let queue: Promise<unknown> = Promise.resolve();
let initialized = false;
export function renderMermaid(source: string): Promise<string> {
  const result = queue.then(async () => {
    if (source.length > 30000) throw new Error('图表过大');
    const mermaid = (await import('mermaid')).default;
    if (!initialized) {
      mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'strict',
        suppressErrorRendering: true,
        maxTextSize: 30000,
        maxEdges: 500,
        htmlLabels: false,
        flowchart: { htmlLabels: false },
        theme: 'base',
        themeVariables: {
          primaryColor: '#f0ecff',
          primaryTextColor: '#343b56',
          primaryBorderColor: '#aa9ce8',
          lineColor: '#8c84ad',
          secondaryColor: '#f7f8fc',
          tertiaryColor: '#fff',
          fontFamily: 'sans-serif',
        },
      });
      initialized = true;
    }
    // 文档自带配置不能改变应用的渲染与安全策略。
    const text = source
      .replace(/^\s*---\s*\r?\n[\s\S]*?\r?\n---\s*\r?\n/, '')
      .replace(/%%\{[\s\S]*?\}%%/g, '');
    // 保持连接到 DOM 才能测量文字；固定定位且不可见，避免临时 SVG 撑大正文造成窗口抖动。
    const staging = document.createElement('div');
    staging.style.cssText =
      'position:fixed;inset:0 auto auto 0;width:1200px;visibility:hidden;pointer-events:none;contain:layout style;z-index:-1';
    staging.setAttribute('aria-hidden', 'true');
    document.body.append(staging);
    let svg: string;
    try {
      ({ svg } = await mermaid.render(`termrill-diagram-${crypto.randomUUID()}`, text, staging));
    } finally {
      // 包括语法错误与取消后丢弃结果的情况，均不能遗留测量容器。
      staging.remove();
    }
    const clean = DOMPurify.sanitize(svg, {
      USE_PROFILES: { svg: true, svgFilters: true },
      FORBID_TAGS: ['script', 'foreignObject', 'image', 'a', 'animate', 'set'],
    });
    const parsed = new DOMParser().parseFromString(clean, 'image/svg+xml');
    if (parsed.querySelector('parsererror')) throw new Error('图表输出无效');
    for (const element of parsed.querySelectorAll('*')) {
      for (const attribute of Array.from(element.attributes)) {
        if (/href$/i.test(attribute.name) && !attribute.value.startsWith('#'))
          element.removeAttributeNode(attribute);
        if (/url\s*\(/i.test(attribute.value) && !/^url\(\s*#[\w-]+\s*\)$/i.test(attribute.value))
          element.removeAttributeNode(attribute);
      }
      if (element.localName === 'style') {
        // Mermaid 的样式含 url(#marker) 等本地引用；不能因出现 url 就删除整张样式表。
        // 仅移除外部加载，保留节点填色、文字与连线的原有规则。
        element.textContent = (element.textContent ?? '')
          .replace(/@import\s+[^;{}]*(?:;|$)/gi, '')
          .replace(/url\s*\(\s*(['"]?)(.*?)\1\s*\)/gi, (_match, _quote, target: string) => {
            return /^#[\w-]+$/.test(target.trim()) ? `url(${target.trim()})` : 'none';
          });
      }
    }
    return new XMLSerializer().serializeToString(parsed.documentElement);
  });
  queue = result.catch(() => {});
  return result;
}
