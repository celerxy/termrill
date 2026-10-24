/** 虚构长章节与模拟 IPC，用于验证生产 EPUB 阅读器的关闭重开和图片布局恢复。 */
import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { mockIPC } from '@tauri-apps/api/mocks';
import EpubPreview from '../src/EpubPreview';
import '../src/styles.css';
const encode = (text: string) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));
const resources = Object.fromEntries(
  Object.entries({
    'META-INF/container.xml':
      '<container><rootfiles><rootfile full-path="book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>',
    'book.opf':
      '<package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>Reading memory fixture</dc:title></metadata><manifest><item id="one" href="one.xhtml" media-type="application/xhtml+xml"/><item id="two" href="two.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="one"/><itemref idref="two"/></spine></package>',
    'one.xhtml':
      '<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>First chapter</h1>' +
      '<p>Fictional coastal reading memory test paragraph.</p>'.repeat(100) +
      '</body></html>',
    'two.xhtml':
      '<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Second chapter</h1>' +
      '<p>Fictional harbor reading memory test paragraph.</p>'.repeat(100) +
      '</body></html>',
  }).map(([path, text]) => [path, encode(text)]),
);
mockIPC((command) => {
  if (command === 'unpack_epub') return resources;
  throw new Error('Unexpected fixture command');
});
const bytes = new TextEncoder().encode('termrill-reading-memory-fictional-book-v1').buffer;
function Fixture() {
  const [opened, setOpened] = useState(true);
  const [status, setStatus] = useState('');
  return (
    <div style={{ height: '100vh', display: 'flex', flexDirection: 'column' }}>
      <div>
        <button onClick={() => setOpened((value) => !value)}>
          {opened ? 'Close fixture' : 'Open fixture'}
        </button>
        <span role="status">{status}</span>
      </div>
      {opened && <EpubPreview bytes={bytes} onStatus={setStatus} />}
    </div>
  );
}
createRoot(document.getElementById('root')!).render(<Fixture />);
