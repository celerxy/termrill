import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';
const source = await readFile(new URL('../src/workspaceLayout.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
});
const { normalizeWorkspaceLayout } = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`
);
test('损坏或越界布局不能隐藏面板或产生无限终端高度', () => {
  for (const input of [
    null,
    {},
    { panelRatio: NaN, terminalHeight: Infinity },
    { panelRatio: '100%', terminalHeight: '-1px' },
  ]) {
    assert.deepEqual(normalizeWorkspaceLayout(input), { panelRatio: 50, terminalHeight: 180 });
  }
  assert.deepEqual(normalizeWorkspaceLayout({ panelRatio: -500, terminalHeight: 50000 }), {
    panelRatio: 30,
    terminalHeight: 480,
  });
});
