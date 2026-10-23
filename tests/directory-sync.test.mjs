import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';

const source = await readFile(new URL('../src/directorySyncModel.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
});
const { textDifference, syncReportText } = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`
);

test('文本差异可还原两端，兼容中文、空行、增加与删除', () => {
  for (const [left, right] of [
    ['甲\n乙\n\n丙', '甲\n丁\n丙'],
    ['', '新增'],
    ['删除', ''],
    ['一样', '一样'],
  ]) {
    const lines = textDifference(left, right);
    assert.equal(
      lines
        .filter((line) => line.kind !== 'add')
        .map((line) => line.text)
        .join('\n'),
      left,
    );
    assert.equal(
      lines
        .filter((line) => line.kind !== 'remove')
        .map((line) => line.text)
        .join('\n'),
      right,
    );
  }
});
test('长中段使用有界替换，超行数明确拒绝', () => {
  const left = Array(700).fill('a').join('\n');
  const right = Array(700).fill('b').join('\n');
  assert.equal(textDifference(left, right).length, 1400);
  assert.throws(() => textDifference('\n'.repeat(6000), ''), /6000/);
});
test('失败同步报告保留实测状态，不将已选择文件标为成功', () => {
  const report = JSON.parse(
    syncReportText(
      { comparedAt: 'now', selected: [{ relative: 'a', status: 'changed', size: 100 }] },
      { id: 'task', status: 'failed', bytes: 40, total: 100, error: '连接中断' },
    ),
  );
  assert.equal(report.transfer.status, 'failed');
  assert.equal(report.transfer.bytes, 40);
  assert.equal(report.comparison.selected[0].status, 'changed');
  assert.match(report.note, /未逐文件确认/);
});
