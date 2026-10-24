/** 使用虚构 EPUB 2/3 资源验证生产阅读器与净化规则；不读取配置或连接服务器。 */
import { createRoot } from 'react-dom/client';
import { mockIPC } from '@tauri-apps/api/mocks';
import { createEpubBook } from '../src/epubBook';
import RemoteDocumentDialog from '../src/RemoteDocumentDialog';
import '../src/styles.css';

const encode = (text: string) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));
const wrap = (body: string) =>
  `<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Demo</title></head><body>${body}</body></html>`;
const resources: Record<string, string> = Object.fromEntries(
  Object.entries({
    'META-INF/container.xml':
      '<container><rootfiles><rootfile full-path="EPUB/book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>',
    'EPUB/book.opf':
      '<package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>海岸手记 · EPUB 演示</dc:title></metadata><manifest><item id="one" href="one.xhtml" media-type="application/xhtml+xml"/><item id="two" href="two.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="nav.xhtml" properties="nav" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="one"/><itemref idref="two"/></spine></package>',
    'EPUB/nav.xhtml':
      '<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol><li><a href="one.xhtml">第一章 · 海边的早晨</a></li><li><a href="two.xhtml#harbor">第二章 · 港湾来信</a></li><li><a href="two.xhtml#voyage">2.1 A Long Journey Across the Coast — Reading, Navigation, and Returning Home</a></li></ol></nav></body></html>',
    'EPUB/one.xhtml': wrap(
      '<h1>第一章 · 海边的早晨</h1><p>这是一本使用虚构内容的演示电子书。阅读器支持目录导航、章节跳转和字号调整。</p><p>清晨的风穿过长廊，海面映着柔和的光。</p><a href="two.xhtml#harbor">阅读下一章</a><script>window.epubUnsafe=true</script><style>body{display:none}</style><img src="https://example.invalid/tracker" onerror="alert(1)"/><iframe src="https://example.invalid"/><a href="javascript:alert(1)">不安全链接</a><form><input /></form>',
    ),
    'EPUB/two.xhtml': wrap(
      '<h1 id="harbor">第二章 · 港湾来信</h1><p>午后的信从港湾寄来，记录了一段关于远行与归来的故事。</p><h2 id="voyage">2.1 远行札记</h2><p>这个小节与上一条目录共用章节文件，用于验证独立高亮与单行横向滚动。</p><img src="dot.png" alt="书内示例图片"/><a href="one.xhtml">返回第一章</a>',
    ),
  }).map(([path, text]) => [path, encode(text)]),
);
resources['EPUB/dot.png'] =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a3ioAAAAASUVORK5CYII=';
const results: string[] = [];
function verify(condition: boolean, name: string) {
  if (!condition) throw new Error(name);
  results.push(name);
}
const book = createEpubBook(resources);
verify(
  book.chapters.length === 2 && book.contents[1].fragment === 'harbor',
  'EPUB 3 reading order and navigation',
);
const clean = book.render(book.chapters[0].path);
verify(
  !/script|iframe|onerror|javascript:|https:|<style|<form|<input/i.test(clean),
  'Active content and remote resources removed',
);
verify(clean.includes('#EPUB%2Ftwo.xhtml#harbor'), 'Internal chapter links preserved');
verify(book.render(book.chapters[1].path).includes('blob:'), 'Book image uses local Blob URL');
book.dispose();
const legacy = { ...resources };
legacy['EPUB/book.opf'] = encode(
  '<package><metadata><title>EPUB 2</title></metadata><manifest><item id="one" href="one.xhtml" media-type="application/xhtml+xml"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx"><itemref idref="one"/></spine></package>',
);
legacy['EPUB/toc.ncx'] = encode(
  '<ncx><navMap><navPoint><navLabel><text>旧版目录</text></navLabel><content src="one.xhtml"/></navPoint></navMap></ncx>',
);
const oldBook = createEpubBook(legacy);
verify(oldBook.contents[0].label === '旧版目录', 'EPUB 2 NCX navigation');
oldBook.dispose();
let invalidRejected = false;
try {
  createEpubBook({});
} catch {
  invalidRejected = true;
}
verify(invalidRejected, 'Missing package rejected');
mockIPC((command) => {
  if (command === 'unpack_epub') return resources;
  throw new Error(`Unexpected command ${command}`);
});
createRoot(document.getElementById('root')!).render(
  <>
    <div
      role="status"
      style={{ position: 'fixed', top: 0, zIndex: 10000, background: 'white', padding: 8 }}
    >
      {results.length} EPUB checks passed · fictional test book
    </div>
    <RemoteDocumentDialog
      document={{
        path: 'coastal-notes.epub',
        connectionId: null,
        content: '',
        original: '',
        revision: '',
      }}
      kind="epub"
      busy={false}
      error=""
      readOnly
      initialBytes={new ArrayBuffer(0)}
      onChange={() => {}}
      onSave={() => {}}
      onClose={() => {}}
      onDownload={() => {}}
    />
  </>,
);
