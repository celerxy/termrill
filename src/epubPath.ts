/** 按包内 URL 解析相对路径；拒绝协议、查询、反斜杠和逃逸容器的路径。 */
export function epubTarget(base: string, href: string): { path: string; fragment: string } | null {
  try {
    if (/^[\s/]|[\\:?\u0000-\u001f]/.test(href)) return null;
    const [rawPath, ...hash] = href.split('#');
    const path = decodeURIComponent(rawPath);
    if (/[\\:?#\u0000-\u001f]/.test(path) || path.startsWith('/')) return null;
    const parts = rawPath ? base.split('/').slice(0, -1) : [];
    if (!rawPath) return { path: base, fragment: decodeURIComponent(hash.join('#')) };
    for (const part of path.split('/')) {
      if (part === '..') {
        if (!parts.length) return null;
        parts.pop();
      } else if (part && part !== '.') parts.push(part);
    }
    return { path: parts.join('/'), fragment: decodeURIComponent(hash.join('#')) };
  } catch {
    return null;
  }
}
