/** PDF 搜索偏移、缓存淘汰和虚拟布局的浏览器无关回归。 */
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import assert from 'node:assert/strict';
import ts from 'typescript';
async function moduleUrl(name, replacements = {}) {
  let source = await readFile(new URL(`../src/${name}.ts`, import.meta.url), 'utf8');
  for (const [from, to] of Object.entries(replacements)) source = source.replace(from, to);
  const output = ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
  }).outputText;
  return `data:text/javascript;base64,${Buffer.from(output).toString('base64')}`;
}
const matchesUrl = await moduleUrl('pdfSearchMatches');
const { findPdfMatches } = await import(matchesUrl);
const { pdfSearchText, mappedPdfMatches, PdfTextCache } = await import(
  await moduleUrl('pdfTextCache', { "'./pdfSearchMatches'": `'${matchesUrl}'` })
);
const { pageAt, pdfScale, restorePdfTop } = await import(await moduleUrl('pdfLayout'));
test('搜索忽略大小写、转义符号、空查询与命中上限', () => {
  assert.deepEqual(findPdfMatches('Nmap nMAP', 'nmap'), [
    { start: 0, length: 4 },
    { start: 5, length: 4 },
  ]);
  assert.deepEqual(findPdfMatches('a+b aab', 'a+b'), [{ start: 0, length: 3 }]);
  assert.equal(findPdfMatches('aaaa', 'aa', 1).length, 1);
  assert.deepEqual(findPdfMatches('text', '  '), []);
  assert.deepEqual(findPdfMatches('text', 'text', 0), []);
});
test('跨文本片段、换行、重复空白与软连字符仍映射到原始高亮位置', () => {
  const content = pdfSearchText([
    { str: '开头 Nm' },
    { str: 'ap', hasEOL: true },
    { str: '工具   使用' },
  ]);
  assert.deepEqual(mappedPdfMatches(content, 'Nmap 工具 使用', 10), [{ start: 3, length: 11 }]);
  assert.deepEqual(mappedPdfMatches(pdfSearchText([{ str: 'pre\u00adview' }]), 'preview', 10), [
    { start: 0, length: 8 },
  ]);
});
test('文本缓存按最近使用淘汰，并拒绝超过预算的单页', () => {
  const cache = new PdfTextCache();
  const tiny = pdfSearchText([{ str: 'text' }]);
  for (let index = 1; index <= 32; index++) cache.set(index, tiny);
  cache.get(1);
  cache.set(33, tiny);
  assert.equal(cache.get(2), undefined);
  assert.equal(cache.get(1), tiny);
  cache.set(34, { ...tiny, cost: 4 * 1024 * 1024 + 1 });
  assert.equal(cache.get(34), undefined);
  cache.set(35, { ...tiny, cost: 4 * 1024 * 1024 });
  assert.equal(cache.get(1), undefined);
  assert.ok(cache.get(35));
});
test('混合页面高度二分定位，缩放与旋转后保持页内比例', () => {
  const old = [0, 100, 400, 600],
    next = [0, 200, 350, 750];
  assert.equal(pageAt(old, 99), 0);
  assert.equal(pageAt(old, 100), 1);
  assert.equal(pageAt(old, 999), 2);
  assert.equal(restorePdfTop(old, next, 264), 289);
  assert.equal(restorePdfTop(old, next, 0), 0);
  assert.equal(pdfScale({ width: 600, height: 800 }, 648, 440, 'page', 1), 0.5);
  assert.equal(pdfScale({ width: 800, height: 600 }, 648, 440, 'width', 1), 0.75);
});
test('千页文档的可见窗口不会包含整份文档', () => {
  const offsets = Array.from({ length: 1001 }, (_, index) => index * 1000);
  const start = pageAt(offsets, 500000 - 600);
  const end = pageAt(offsets, 500000 + 800 + 600) + 1;
  assert.ok(end - start <= 4);
  assert.equal(pageAt(offsets, 999000), 999);
});
