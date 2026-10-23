import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';
const source = await readFile(new URL('../src/terminalSearch.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
});
const { terminalMatches } = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`
);
function line(cells) {
  return {
    length: cells.length,
    getCell: (column) => ({ getChars: () => cells[column][0], getWidth: () => cells[column][1] }),
  };
}
test('终端搜索按显示列选择中文、emoji 与组合字符', () => {
  const row = line([
    ['中', 2],
    ['', 0],
    ['a', 1],
    ['😀', 2],
    ['', 0],
    ['e\u0301', 1],
  ]);
  const buffer = { length: 1, getLine: () => row };
  assert.deepEqual(terminalMatches(buffer, 'a😀'), [{ row: 0, column: 2, length: 3 }]);
  assert.deepEqual(terminalMatches(buffer, 'e\u0301'), [{ row: 0, column: 5, length: 1 }]);
  assert.deepEqual(terminalMatches(buffer, '中'), [{ row: 0, column: 0, length: 2 }]);
});
test('空搜索不匹配，超长搜索和大量命中保持上限', () => {
  const buffer = { length: 20000, getLine: () => line([['x', 1]]) };
  assert.deepEqual(terminalMatches(buffer, ''), []);
  assert.deepEqual(terminalMatches(buffer, 'x'.repeat(257)), []);
  assert.equal(terminalMatches(buffer, 'x').length, 10000);
});
