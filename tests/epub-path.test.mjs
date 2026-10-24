/** EPUB 包内链接必须规范化且不能访问外部 URL、本地文件或逃逸容器。 */
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import assert from 'node:assert/strict';
import ts from 'typescript';
const source = await readFile(new URL('../src/epubPath.ts', import.meta.url), 'utf8');
const output = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
}).outputText;
const { epubTarget } = await import(
  `data:text/javascript;base64,${Buffer.from(output).toString('base64')}`
);
test('EPUB links resolve Unicode, parent folders, and chapter fragments', () => {
  assert.deepEqual(epubTarget('EPUB/text/one.xhtml', '../two.xhtml#part'), {
    path: 'EPUB/two.xhtml',
    fragment: 'part',
  });
  assert.deepEqual(epubTarget('EPUB/one.xhtml', '#中文'), {
    path: 'EPUB/one.xhtml',
    fragment: '中文',
  });
  assert.deepEqual(epubTarget('EPUB/one.xhtml', '%E4%B8%AD%E6%96%87.xhtml'), {
    path: 'EPUB/中文.xhtml',
    fragment: '',
  });
});
test('EPUB links reject remote resources, encoded protocols, queries, and traversal', () => {
  for (const href of [
    'https://example.com/a',
    '//example.com/a',
    'javascript:alert(1)',
    'data:text/html,a',
    'file:///a',
    '/absolute',
    '..%2f..%2fescape',
    '%2foutside',
    '%5coutside',
    'http%3a//example.com',
    'a?query=x',
    'a%3fquery',
    '\\server',
    '%',
    '\u0000bad',
  ]) {
    assert.equal(epubTarget('EPUB/one.xhtml', href), null, href);
  }
});
