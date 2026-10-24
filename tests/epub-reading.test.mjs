/** EPUB 进度记录的隐私、损坏保护与淘汰边界。 */
import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';
const source = await readFile(new URL('../src/epubReadingState.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
});
const { loadEpubReadingState, saveEpubReadingState } = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`
);
function storage(initial = null) {
  let raw = initial;
  return {
    getItem: () => raw,
    setItem: (_, value) => {
      raw = value;
    },
    raw: () => raw,
  };
}
const id = 'a'.repeat(64);
const state = { chapter: 2, progress: 0.63, fontSize: 22 };
test('恢复章节、比例和字号，不保存调用方额外字段', () => {
  const value = storage();
  saveEpubReadingState(value, id, { ...state, title: 'private title', path: '/private/book.epub' });
  assert.deepEqual(loadEpubReadingState(value, id, 3), state);
  assert.doesNotMatch(value.raw(), /private|path|title/);
  assert.equal(loadEpubReadingState(value, 'b'.repeat(64), 3), null);
  assert.equal(loadEpubReadingState(value, id, 2), null);
});
test('无效数字和摘要不能保存，损坏记录不被覆盖', () => {
  for (const invalid of [
    { ...state, chapter: -1 },
    { ...state, progress: NaN },
    { ...state, progress: 1.1 },
    { ...state, fontSize: 19 },
  ]) {
    const value = storage();
    saveEpubReadingState(value, id, invalid);
    assert.equal(value.raw(), null);
  }
  const value = storage('broken');
  assert.throws(() => saveEpubReadingState(value, id, state));
  assert.equal(value.raw(), 'broken');
  saveEpubReadingState(storage(), '/private/path', state);
});
test('最多保留最近使用的 50 本，更新已有记录不重复', () => {
  const value = storage();
  for (let index = 0; index < 51; index++)
    saveEpubReadingState(value, index.toString(16).padStart(64, '0'), state);
  assert.equal(JSON.parse(value.raw()).length, 50);
  assert.equal(loadEpubReadingState(value, '0'.repeat(64), 3), null);
  saveEpubReadingState(value, id, state);
  saveEpubReadingState(value, id, { ...state, chapter: 0 });
  assert.equal(JSON.parse(value.raw()).length, 50);
  assert.equal(loadEpubReadingState(value, id, 3).chapter, 0);
});
test('存储不可用时向界面报告失败', () => {
  const unavailable = {
    getItem: () => {
      throw new Error('denied');
    },
    setItem: () => {},
  };
  assert.throws(() => loadEpubReadingState(unavailable, id, 3), /denied/);
  assert.throws(() => saveEpubReadingState(unavailable, id, state), /denied/);
});
