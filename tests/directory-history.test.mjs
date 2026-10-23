import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';
const source = await readFile(new URL('../src/directoryHistory.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
});
const { DirectoryHistory } = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`
);

test('失败导航不提前改变历史，成功返回后可前进', () => {
  const history = new DirectoryHistory();
  history.visit('/a');
  history.visit('/b');
  assert.equal(history.request('back'), '/a');
  assert.equal(history.canForward, false);
  assert.equal(history.request('back'), '/a');
  history.visit('/a');
  assert.equal(history.canForward, true);
  assert.equal(history.canBack, false);
  assert.equal(history.request('forward'), '/b');
  history.visit('/b');
  assert.equal(history.canForward, false);
});
test('返回后进入新目录截断前进分支，刷新相同路径不增加记录', () => {
  const history = new DirectoryHistory();
  history.visit('/a');
  history.visit('/b');
  history.visit('/c');
  history.request('back');
  history.visit('/b');
  history.visit('/new');
  history.visit('/new');
  assert.equal(history.canForward, false);
  assert.equal(history.request('back'), '/b');
});
