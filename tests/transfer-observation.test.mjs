/** 验证停顿观察不会跨暂停/重试累计，也不把过期速度继续当成实时速度。 */
import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';

const source = await readFile(new URL('../src/transferObservation.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
});
const { observeTransfer, transferHealth, transferSnapshot } = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`
);
const task = { status: 'running', bytes: 0, total: 1000, speed: 0 };

test('心跳继续但字节未推进与事件本身停止分开观察', () => {
  const first = observeTransfer(task, undefined, 1000);
  const heartbeat = observeTransfer(task, first, 12000);
  assert.deepEqual(transferHealth(heartbeat, 12000), {
    idleSeconds: 11,
    eventAgeSeconds: 0,
    stale: false,
    waiting: true,
  });
  assert.equal(transferHealth(heartbeat, 17000).stale, true);
  const advanced = observeTransfer({ ...task, bytes: 1 }, heartbeat, 18000);
  assert.equal(transferHealth(advanced, 18000).waiting, false);
});

test('暂停、重试和页面恢复重新观察，旧停顿时间不能触发提示', () => {
  const first = observeTransfer(task, undefined, 0);
  const paused = observeTransfer({ ...task, status: 'paused' }, first, 20000);
  assert.equal(transferHealth(paused, 80000), undefined);
  const resumed = observeTransfer(task, paused, 90000);
  assert.equal(transferHealth(resumed, 90000).idleSeconds, 0);
  const failed = observeTransfer({ ...task, status: 'failed' }, resumed, 95000);
  assert.equal(transferHealth(observeTransfer(task, failed, 100000), 100000).waiting, false);
  const restored = observeTransfer({ ...task, observation: first.observation }, undefined, 110000);
  assert.equal(transferHealth(restored, 110000).idleSeconds, 0);
});

test('收尾不误报正文停顿，回退或重新扫描大小时重置基准', () => {
  const first = observeTransfer({ ...task, bytes: 1000 }, undefined, 0);
  assert.equal(transferHealth(first, 30000).waiting, false);
  const reset = observeTransfer(task, first, 40000);
  assert.equal(transferHealth(reset, 40000).idleSeconds, 0);
  const rescanned = observeTransfer({ ...task, total: 2000 }, reset, 50000);
  assert.equal(transferHealth(rescanned, 50000).idleSeconds, 0);
});

test('观察快照不序列化任务名、连接身份或错误中的私人数据', () => {
  const observed = observeTransfer(
    {
      ...task,
      direction: 'download',
      name: 'private-book.epub',
      connectionId: 'private-account',
      error: '/private/path/secret',
      id: 'private-task',
    },
    undefined,
    0,
  );
  const report = transferSnapshot(observed, 12000);
  assert.equal(report.includes('private'), false);
  assert.match(report, /进度字节：0 \/ 1000/);
  assert.match(report, /距进度事件更新（秒）：12/);
});
