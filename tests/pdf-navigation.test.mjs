/** 目录搜索/折叠、章节边界与本机阅读记录的行为回归；不连接真实服务器。 */
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import assert from 'node:assert/strict';
import ts from 'typescript';

async function loadModule(name) {
  const source = await readFile(new URL(`../src/${name}.ts`, import.meta.url), 'utf8');
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
  });
  return import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`);
}
const { flattenPdfOutline, visiblePdfOutline, activePdfOutline, resolvePdfDestination } =
  await loadModule('pdfOutline');
const { loadPdfReadingState, savePdfReadingState } = await loadModule('pdfReadingState');
const item = (title, items = [], dest = [0]) => ({ title, items, dest });
const source = [
  item('第一章', [item('LLMs', [item('What is an LLM?')]), item('   ')]),
  item('第二章'),
];

test('多级折叠不隐藏兄弟章节；搜索展示命中及祖先，清空后恢复折叠', () => {
  const entries = flattenPdfOutline(source);
  const collapsed = new Set([0]);
  assert.deepEqual(
    visiblePdfOutline(entries, collapsed, '').map((entry) => entry.title),
    ['第一章', '第二章'],
  );
  assert.deepEqual(
    visiblePdfOutline(entries, collapsed, 'llm?').map((entry) => entry.title),
    ['第一章', 'LLMs', 'What is an LLM?'],
  );
  assert.deepEqual([...collapsed], [0]);
  assert.equal(visiblePdfOutline(entries, collapsed, '未命名章节').at(-1).title, '未命名章节');
  assert.equal(visiblePdfOutline(entries, collapsed, '不存在').length, 0);
  assert.equal(visiblePdfOutline(entries, new Set(), '').length, 5);
});

test('当前章节按页码定位，支持同页子章节、无目标分组和非页码顺序目录', () => {
  const entries = flattenPdfOutline(source);
  [2, 2, 3, null, 10].forEach((page, index) => {
    entries[index].page = page;
  });
  assert.equal(activePdfOutline(entries, 1), null);
  assert.equal(activePdfOutline(entries, 2), 1);
  assert.equal(activePdfOutline(entries, 9), 2);
  assert.equal(activePdfOutline(entries, 10), 4);
  assert.equal(activePdfOutline([entries[4], entries[2], entries[0]], 11), 4);
});

test('长标题不丢失；万项目录和超深层级不使用递归调用栈', () => {
  const title = '很长的章节标题'.repeat(300);
  const entries = flattenPdfOutline(
    Array.from({ length: 10000 }, (_, index) => item(`${index} ${title}`)),
  );
  assert.equal(entries.length, 10000);
  assert.equal(visiblePdfOutline(entries, new Set(), '9999').length, 1);
  assert.equal(entries[0].title, `0 ${title}`);
  let deep = item('最后一层');
  for (let index = 0; index < 12000; index++) deep = item(`层级 ${index}`, [deep]);
  const levels = flattenPdfOutline([deep]);
  assert.equal(visiblePdfOutline(levels, new Set([0]), '').length, 1);
  assert.equal(visiblePdfOutline(levels, new Set([0]), '最后一层').length, 12001);
});

test('页内目标解析覆盖命名目标和页引用，拒绝越界与外部链接', async () => {
  const document = {
    numPages: 20,
    getDestination: async () => [{ num: 4 }],
    getPageIndex: async () => 8,
  };
  assert.equal(await resolvePdfDestination(document, 'chapter'), 9);
  assert.equal(await resolvePdfDestination(document, [0]), 1);
  for (const dest of [null, [], [-1], [20], [1.5], [NaN]])
    assert.equal(await resolvePdfDestination(document, dest), null);
});

function memoryStorage() {
  const values = new Map();
  return {
    values,
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
  };
}
test('同一指纹恢复页码与缩放，不同文档隔离，页数变化时限制跳页范围', () => {
  const storage = memoryStorage();
  const saved = { page: 80, zoom: 1.75, mode: 'custom' };
  savePdfReadingState(storage, 'document-a', saved);
  savePdfReadingState(storage, 'document-b', { page: 2, zoom: 1, mode: 'page' });
  assert.deepEqual(loadPdfReadingState(storage, 'document-a', 100), saved);
  assert.deepEqual(loadPdfReadingState(storage, 'document-a', 30), { ...saved, page: 30 });
  assert.equal(loadPdfReadingState(storage, 'unknown', 100), null);
});
test('最多保留最近 50 份文档，重读文档写入后更新淘汰顺序', () => {
  const storage = memoryStorage();
  const state = { page: 1, zoom: 1, mode: 'width' };
  for (let index = 0; index < 50; index++) savePdfReadingState(storage, `doc-${index}`, state);
  savePdfReadingState(storage, 'doc-0', state);
  savePdfReadingState(storage, 'doc-50', state);
  assert.equal(loadPdfReadingState(storage, 'doc-1', 10), null);
  assert.deepEqual(loadPdfReadingState(storage, 'doc-0', 10), state);
  assert.equal(JSON.parse([...storage.values.values()][0]).length, 50);
});
test('损坏记录不被覆盖；非法页码和缩放不能写入，存储失败返回调用方', () => {
  const storage = memoryStorage();
  for (const state of [
    { page: -1, zoom: 1, mode: 'width' },
    { page: 1, zoom: Infinity, mode: 'custom' },
    { page: 1, zoom: 1, mode: 'bad' },
  ]) {
    savePdfReadingState(storage, 'doc', state);
  }
  assert.equal(storage.values.size, 0);
  storage.setItem('termrill.pdfReading.v1', '{broken');
  assert.throws(() => savePdfReadingState(storage, 'doc', { page: 1, zoom: 1, mode: 'width' }));
  assert.equal(storage.getItem('termrill.pdfReading.v1'), '{broken');
  assert.throws(() =>
    loadPdfReadingState(
      {
        getItem: () => {
          throw new Error('denied');
        },
      },
      'doc',
      10,
    ),
  );
});
