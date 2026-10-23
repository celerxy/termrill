import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';
const source = await readFile(new URL('../src/batchRename.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
});
const { previewBatchRename, runRenamePlan } = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`
);
const entries = ['a.txt', 'b.txt', '.env'].map((name) => ({
  name,
  path: `/work/${name}`,
  directory: false,
  symlink: false,
  size: 5,
  modified: 10,
}));
const preview = (template, names = entries.map((entry) => entry.name)) =>
  previewBatchRename(entries, template, 1, 3, names, true);

test('编号模板保留扩展名和点文件，拒绝越界路径、Windows 保留名和目标冲突', () => {
  assert.deepEqual(
    preview('{n}-{stem}{ext}').map((row) => row.name),
    ['001-a.txt', '002-b.txt', '003-.env'],
  );
  for (const template of ['../{stem}', 'NUL', 'bad.', '{unknown}', 'same.txt']) {
    assert.ok(preview(template).some((row) => row.error));
  }
  assert.ok(preview('{n}-{stem}{ext}', ['001-a.txt']).some((row) => row.error));
  assert.ok(previewBatchRename([entries[0]], 'A.txt', 1, 3, ['a.txt'], false)[0].error);
});

test('预览中任一条冲突时，整个计划不开始写操作', async () => {
  let calls = 0;
  const plan = preview('{n}-{stem}{ext}');
  plan[2].error = 'conflict';
  await assert.rejects(
    runRenamePlan(
      plan,
      async () => {
        calls++;
      },
      () => false,
      () => {},
    ),
  );
  assert.equal(calls, 0);
});

test('失败不重放、不回滚已确认改名，后续项目保持未执行', async () => {
  const calls = [];
  const results = await runRenamePlan(
    preview('{n}-{stem}{ext}'),
    async (row) => {
      calls.push(row.entry.name);
      if (row.entry.name === 'b.txt') throw new Error('connection lost');
    },
    () => false,
    () => {},
  );
  assert.deepEqual(calls, ['a.txt', 'b.txt']);
  assert.deepEqual(
    results.map((row) => row.status),
    ['completed', 'failed', 'not-started'],
  );
  assert.equal(results[1].error, 'connection lost');
});

test('取消只停止尚未发送的项目，当前请求完成后保留其结果', async () => {
  let cancelled = false;
  const results = await runRenamePlan(
    preview('{n}-{stem}{ext}'),
    async () => {
      cancelled = true;
    },
    () => cancelled,
    () => {},
  );
  assert.deepEqual(
    results.map((row) => row.status),
    ['completed', 'not-started', 'not-started'],
  );
});
