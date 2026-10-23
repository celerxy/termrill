import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';
const source = await readFile(new URL('../src/connectionBundle.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
});
const { exportConnections, importConnections } = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`
);
const endpoint = {
  host: '127.0.0.1',
  port: 22,
  username: 'fixture',
  auth: 'key',
  keyPath: 'C:/keys/fixture',
  password: 'must-not-export',
};
const profile = {
  id: 'original',
  name: '测试',
  target: endpoint,
  jump: { ...endpoint, auth: 'agent' },
  password: 'also-private',
};

test('导出字段白名单、保留跳板与分组，导入分配新身份', () => {
  const encoded = exportConnections([profile], { original: '生产' });
  assert.equal(encoded.includes('must-not-export'), false);
  assert.equal(encoded.includes('also-private'), false);
  const first = importConnections(encoded)[0];
  const second = importConnections(encoded)[0];
  assert.notEqual(first.profile.id, 'original');
  assert.notEqual(first.profile.id, second.profile.id);
  assert.equal(first.group, '生产');
  assert.equal(first.profile.jump.auth, 'agent');
});
test('拒绝凭据、信任字段、异常端口与超量导入', () => {
  const valid = JSON.parse(exportConnections([profile], {}));
  for (const property of ['password', 'knownHosts', '__proto__']) {
    const invalid = JSON.parse(JSON.stringify(valid));
    Object.defineProperty(invalid, property, { value: 'x', enumerable: true });
    assert.throws(() => importConnections(JSON.stringify(invalid)));
  }
  valid.profiles[0].target.port = 0;
  assert.throws(() => importConnections(JSON.stringify(valid)));
  valid.profiles = new Array(201).fill({});
  assert.throws(() => importConnections(JSON.stringify(valid)));
  assert.throws(() => importConnections('x'.repeat(1024 * 1024 + 1)));
});
