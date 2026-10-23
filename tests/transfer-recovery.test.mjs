// 独立测试恢复授权的生命周期，无需真实凭据、SSH 服务或 Rust 构建目录。
import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';

const source = await readFile(new URL('../src/transferRecovery.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
});
const { TransferRecoveryIntent } = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`
);
const profile = {
  id: 'server-a',
  name: '原连接',
  target: { host: 'server.example', port: 22, username: 'user', auth: 'password', keyPath: '' },
  jump: null,
};

test('认证成功仅恢复所选任务一次，重命名不改变归属', () => {
  const intent = new TransferRecoveryIntent();
  assert.equal(intent.prepare('task-a', profile), true);
  assert.equal(intent.prepare('task-b', profile), false);
  assert.equal(intent.take({ ...profile, name: '新名称' }), 'task-a');
  assert.equal(intent.take(profile), null);
});

test('取消认证或指纹确认后，随后普通连接不会启动旧任务', () => {
  const intent = new TransferRecoveryIntent();
  intent.prepare('task-a', profile);
  intent.cancel();
  assert.equal(intent.take(profile), null);
  assert.equal(intent.prepare('task-b', profile), true);
  assert.equal(intent.take(profile), 'task-b');
});

test('修改配置 ID、主机、认证方式或跳板都不能沿用恢复授权', () => {
  const changed = [
    { ...profile, id: 'server-b' },
    ...['host', 'port', 'username', 'auth', 'keyPath'].map((key) => ({
      ...profile,
      target: { ...profile.target, [key]: key === 'port' ? 2222 : 'changed' },
    })),
    { ...profile, jump: { ...profile.target } },
  ];
  for (const next of changed) {
    const intent = new TransferRecoveryIntent();
    intent.prepare('task-a', profile);
    assert.equal(intent.take(next), null);
    assert.equal(intent.take(profile), null);
  }
});
