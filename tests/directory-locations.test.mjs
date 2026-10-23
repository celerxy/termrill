import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';

const source = await readFile(new URL('../src/directoryLocations.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
});
const locations = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`
);
function storage() {
  const data = new Map();
  return {
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => data.set(key, value),
    data,
  };
}

test('相同路径的本地和不同服务器记录隔离，其他标签更新不覆盖已有收藏', () => {
  const s = storage();
  locations.toggleDirectoryFavorite(s, 'local', '/work');
  locations.toggleDirectoryFavorite(s, 'remote:a', '/work');
  locations.visitDirectory(s, 'remote:b', '/work');
  locations.toggleDirectoryFavorite(s, 'remote:a', '/logs');
  assert.deepEqual(locations.getDirectoryLocations(s, 'local').favorites, ['/work']);
  assert.deepEqual(locations.getDirectoryLocations(s, 'remote:a').favorites, ['/work', '/logs']);
  assert.deepEqual(locations.getDirectoryLocations(s, 'remote:b'), {
    favorites: [],
    recent: ['/work'],
  });
});

test('最近目录去重并限制数量，刷新或重新创建读取器后仍可读取', () => {
  const s = storage();
  for (let i = 0; i < 30; i++) locations.visitDirectory(s, 'local', `/dir${i}`);
  locations.visitDirectory(s, 'local', '/dir20');
  const recent = locations.getDirectoryLocations(s, 'local').recent;
  assert.equal(recent.length, 20);
  assert.equal(recent[0], '/dir20');
  assert.equal(new Set(recent).size, 20);
  assert.equal(recent.includes('/dir0'), false);
});

test('存储空间不足或记录损坏时失败，不冒充保存成功或覆盖损坏记录', () => {
  const s = storage();
  locations.toggleDirectoryFavorite(s, 'local', '/kept');
  const previous = [...s.data.values()][0];
  assert.throws(
    () =>
      locations.visitDirectory(
        {
          getItem: s.getItem,
          setItem() {
            throw Error('quota');
          },
        },
        'local',
        '/new',
      ),
    /quota/,
  );
  assert.equal([...s.data.values()][0], previous);
  s.setItem([...s.data.keys()][0], '{invalid');
  assert.throws(() => locations.toggleDirectoryFavorite(s, 'local', '/new'));
  assert.equal([...s.data.values()][0], '{invalid');
});

test('收藏上限仍允许移除，不静默挤掉用户收藏', () => {
  const s = storage();
  for (let i = 0; i < 50; i++) locations.toggleDirectoryFavorite(s, 'local', `/dir${i}`);
  assert.throws(() => locations.toggleDirectoryFavorite(s, 'local', '/extra'), /50/);
  locations.toggleDirectoryFavorite(s, 'local', '/dir0');
  locations.toggleDirectoryFavorite(s, 'local', '/extra');
  assert.equal(locations.getDirectoryLocations(s, 'local').favorites.length, 50);
});
