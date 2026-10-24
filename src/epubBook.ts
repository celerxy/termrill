/** EPUB 2/3 元数据、阅读顺序及受限正文；文档资源只来自当前内存容器。 */
import DOMPurify from 'dompurify';
import { epubTarget } from './epubPath';

export type EpubChapter = { path: string; label: string };
export type EpubTarget = { path: string; fragment: string; label: string };
export type EpubBook = {
  title: string;
  chapters: EpubChapter[];
  contents: EpubTarget[];
  render: (path: string) => string;
  dispose: () => void;
};

function decode(value: string): Uint8Array<ArrayBuffer> {
  return Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
}

function parseXml(resources: Record<string, string>, path: string): Document {
  if (!Object.hasOwn(resources, path)) throw new Error('EPUB 缺少所需文档资源');
  const text = new TextDecoder('utf-8', { fatal: true }).decode(decode(resources[path]));
  if (/<!ENTITY/i.test(text)) throw new Error('EPUB 不支持自定义 XML 实体');
  const xml = new DOMParser().parseFromString(text, 'application/xml');
  if (xml.getElementsByTagName('parsererror').length) throw new Error('EPUB XML/XHTML 文档损坏');
  return xml;
}

function elements(parent: Document | Element, name: string): Element[] {
  return Array.from(parent.getElementsByTagNameNS('*', name));
}

function label(element: Element | undefined): string {
  return element?.textContent?.trim().slice(0, 300) ?? '';
}

export function createEpubBook(resources: Record<string, string>): EpubBook {
  const container = parseXml(resources, 'META-INF/container.xml');
  const rootfile = elements(container, 'rootfile').find(
    (item) => item.getAttribute('media-type') === 'application/oebps-package+xml',
  );
  const packagePath = epubTarget('', rootfile?.getAttribute('full-path') ?? '')?.path;
  if (!packagePath) throw new Error('EPUB 缺少有效的 OPF 包描述');
  const packageXml = parseXml(resources, packagePath);
  const manifest = new Map(
    elements(packageXml, 'item').map((item) => [item.getAttribute('id'), item]),
  );
  const chapters: EpubChapter[] = [];
  for (const reference of elements(packageXml, 'itemref')) {
    const item = manifest.get(reference.getAttribute('idref'));
    if (!item || item.getAttribute('media-type') !== 'application/xhtml+xml') {
      throw new Error('EPUB 包含缺失或不支持的章节格式');
    }
    const target = epubTarget(packagePath, item.getAttribute('href') ?? '');
    if (!target || !Object.hasOwn(resources, target.path)) throw new Error('EPUB 章节路径无效');
    chapters.push({ path: target.path, label: `第 ${chapters.length + 1} 章` });
  }
  if (!chapters.length) throw new Error('EPUB 没有可阅读章节');
  const chapterPaths = new Set(chapters.map((chapter) => chapter.path));
  const contents: EpubTarget[] = [];
  const addTarget = (base: string, href: string, text: string) => {
    const target = epubTarget(base, href);
    if (target && chapterPaths.has(target.path) && text && contents.length < 4096) {
      contents.push({ ...target, label: text });
    }
  };
  const navItem = [...manifest.values()].find((item) =>
    item.getAttribute('properties')?.split(/\s+/).includes('nav'),
  );
  const ncxItem = manifest.get(elements(packageXml, 'spine')[0]?.getAttribute('toc') ?? '');
  const navigation = navItem ?? ncxItem;
  const navPath =
    navigation && epubTarget(packagePath, navigation.getAttribute('href') ?? '')?.path;
  if (navPath) {
    const nav = parseXml(resources, navPath);
    if (navItem) {
      const toc = elements(nav, 'nav').find((item) =>
        item.getAttributeNS('http://www.idpf.org/2007/ops', 'type')?.split(/\s+/).includes('toc'),
      );
      if (toc) {
        for (const anchor of elements(toc, 'a')) {
          addTarget(navPath, anchor.getAttribute('href') ?? '', label(anchor));
        }
      }
    } else {
      for (const point of elements(nav, 'navPoint')) {
        addTarget(
          navPath,
          elements(point, 'content')[0]?.getAttribute('src') ?? '',
          label(elements(point, 'navLabel')[0]),
        );
      }
    }
  }
  for (const chapter of chapters) {
    chapter.label = contents.find((item) => item.path === chapter.path)?.label ?? chapter.label;
  }
  const urls = new Map<string, string>();
  const imageUrl = (path: string): string | undefined => {
    if (urls.has(path)) return urls.get(path);
    if (!Object.hasOwn(resources, path)) return;
    const bytes = decode(resources[path]);
    let mime = '';
    if (bytes[0] === 137 && bytes[1] === 80 && bytes[2] === 78 && bytes[3] === 71)
      mime = 'image/png';
    else if (bytes[0] === 255 && bytes[1] === 216 && bytes[2] === 255) mime = 'image/jpeg';
    else if (new TextDecoder().decode(bytes.slice(0, 6)).match(/^GIF8[79]a$/)) mime = 'image/gif';
    else if (
      new TextDecoder().decode(bytes.slice(0, 4)) === 'RIFF' &&
      new TextDecoder().decode(bytes.slice(8, 12)) === 'WEBP'
    )
      mime = 'image/webp';
    if (!mime) return;
    const url = URL.createObjectURL(new Blob([bytes], { type: mime }));
    urls.set(path, url);
    return url;
  };
  return {
    title: label(elements(packageXml, 'title')[0]) || 'EPUB 阅读器',
    chapters,
    contents: contents.length
      ? contents
      : chapters.map((chapter) => ({ ...chapter, fragment: '' })),
    render(path) {
      if (!chapterPaths.has(path)) throw new Error('章节不在阅读顺序中');
      const xml = parseXml(resources, path);
      const body = elements(xml, 'body')[0];
      if (!body) throw new Error('EPUB 章节缺少正文');
      // 先移除所有来源属性，再注入验证过的本地资源与内部目标。原书 CSS 不进入应用。
      for (const node of [body, ...body.getElementsByTagName('*')]) {
        const source = node.getAttribute('src');
        const href = node.getAttribute('href');
        for (const attribute of Array.from(node.attributes)) {
          if (!['id', 'alt', 'title'].includes(attribute.name)) node.removeAttributeNode(attribute);
        }
        if (node.hasAttribute('id')) node.setAttribute('id', `epub-${node.getAttribute('id')}`);
        if (node.localName === 'img' && source) {
          const target = epubTarget(path, source);
          const url = target && imageUrl(target.path);
          if (url) node.setAttribute('src', url);
          else node.remove();
        }
        if (node.localName === 'a' && href) {
          const target = epubTarget(path, href);
          if (target && chapterPaths.has(target.path)) {
            node.setAttribute(
              'href',
              `#${encodeURIComponent(target.path)}#${encodeURIComponent(target.fragment)}`,
            );
          }
        }
      }
      return DOMPurify.sanitize(new XMLSerializer().serializeToString(body), {
        ALLOWED_TAGS: [
          'p',
          'div',
          'section',
          'article',
          'span',
          'h1',
          'h2',
          'h3',
          'h4',
          'h5',
          'h6',
          'a',
          'img',
          'br',
          'hr',
          'strong',
          'em',
          'b',
          'i',
          'u',
          's',
          'small',
          'sup',
          'sub',
          'blockquote',
          'pre',
          'code',
          'ul',
          'ol',
          'li',
          'dl',
          'dt',
          'dd',
          'table',
          'thead',
          'tbody',
          'tr',
          'td',
          'th',
          'caption',
          'figure',
          'figcaption',
        ],
        ALLOWED_ATTR: ['id', 'alt', 'title', 'href', 'src'],
        // 资源地址已由上面的内存映射生成；只保留内部锚点与本书 Blob 图片。
        ALLOWED_URI_REGEXP: /^(#|blob:)/,
        ALLOW_DATA_ATTR: false,
        ALLOW_ARIA_ATTR: false,
      });
    },
    dispose() {
      for (const url of urls.values()) URL.revokeObjectURL(url);
      urls.clear();
    },
  };
}
