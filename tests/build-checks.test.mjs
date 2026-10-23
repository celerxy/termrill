/** 验证版本漂移会阻止检查/发行入口，不能执行后续构建。 */
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { checkVersions } from '../scripts/check-versions.mjs';

async function fixture(t, versions = ['0.1.0', '0.1.0', '0.1.0']) {
  const root = await mkdtemp(join(tmpdir(), 'termrill-versions-'));
  t.after(async () => {
    assert.equal(dirname(root), resolve(tmpdir()));
    assert.match(root.slice(dirname(root).length + 1), /^termrill-versions-/);
    await rm(root, { recursive: true, force: true });
  });
  await mkdir(join(root, 'src-tauri'));
  await writeFile(join(root, 'package.json'), JSON.stringify({ version: versions[0] }));
  await writeFile(
    join(root, 'src-tauri/tauri.conf.json'),
    JSON.stringify({ version: versions[1] }),
  );
  await writeFile(
    join(root, 'src-tauri/Cargo.toml'),
    `[package]\nname = "fixture"\nversion = "${versions[2]}"\n\n[dependencies]\nother = { version = "99.0.0" }\n`,
  );
  return root;
}

test('三处版本相同可通过，支持预发布版本且不混入依赖版本', async (t) => {
  const root = await fixture(t, Array(3).fill('0.2.0-rc.1'));
  assert.equal(await checkVersions(root), '0.2.0-rc.1');
});

test('任意一处版本漂移均拒绝，检查不改写版本文件', async (t) => {
  for (let index = 0; index < 3; index++) {
    const versions = Array(3).fill('0.1.0');
    versions[index] = '0.2.0';
    const root = await fixture(t, versions);
    const manifest = join(root, 'src-tauri/Cargo.toml');
    const before = await readFile(manifest, 'utf8');
    await assert.rejects(checkVersions(root), /应用版本不一致/);
    assert.equal(await readFile(manifest, 'utf8'), before);
  }
});

test('缺失应用版本不能用 Cargo 其他区段或 workspace 版本冒充', async (t) => {
  const root = await fixture(t);
  for (const content of [
    '[dependencies]\nversion = "0.1.0"\n',
    '[package]\nversion.workspace = true\n[workspace.package]\nversion = "0.1.0"\n',
  ]) {
    await writeFile(join(root, 'src-tauri/Cargo.toml'), content);
    await assert.rejects(checkVersions(root), /Cargo.toml 缺少显式有效的应用版本/);
  }
});

test('命令行版本检查失败返回非零退出码', async (t) => {
  const root = await fixture(t, ['0.1.0', '0.1.0', '0.2.0']);
  const script = fileURLToPath(new URL('../scripts/check-versions.mjs', import.meta.url));
  const result = spawnSync(process.execPath, [script], { cwd: root, encoding: 'utf8' });
  assert.ifError(result.error);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /应用版本不一致/);
});

test(
  'Windows 检查与两个打包入口在版本漂移时均不执行后续阶段',
  { skip: process.platform !== 'win32' },
  async (t) => {
    const root = await fixture(t, ['0.1.0', '0.1.0', '0.2.0']);
    await mkdir(join(root, 'scripts'));
    for (const name of ['check-versions.mjs', 'check-release.ps1', 'release-windows.ps1']) {
      await copyFile(new URL(`../scripts/${name}`, import.meta.url), join(root, 'scripts', name));
    }
    await writeFile(
      join(root, 'package.json'),
      JSON.stringify({
        version: '0.1.0',
        scripts: {
          'check:versions': 'node scripts/check-versions.mjs',
          'format:check': "node -e \"require('node:fs').writeFileSync('unexpected-stage','ran')\"",
        },
      }),
    );
    for (const args of [
      ['check-release.ps1'],
      ['release-windows.ps1'],
      ['check-release.ps1', '-Package'],
    ]) {
      const result = spawnSync(
        'powershell.exe',
        [
          '-NoProfile',
          '-ExecutionPolicy',
          'Bypass',
          '-File',
          join(root, 'scripts', args[0]),
          ...args.slice(1),
        ],
        { cwd: root, encoding: 'utf8', timeout: 30000 },
      );
      assert.ifError(result.error);
      assert.equal(result.status, 1, `${args.join(' ')} 应拒绝版本漂移`);
      await assert.rejects(access(join(root, 'unexpected-stage')));
    }
  },
);

test(
  '前端测试失败时统一检查不得进入构建阶段',
  { skip: process.platform !== 'win32' },
  async (t) => {
    const root = await fixture(t);
    await mkdir(join(root, 'scripts'));
    for (const name of ['check-versions.mjs', 'check-release.ps1']) {
      await copyFile(new URL(`../scripts/${name}`, import.meta.url), join(root, 'scripts', name));
    }
    await writeFile(
      join(root, 'package.json'),
      JSON.stringify({
        version: '0.1.0',
        scripts: {
          'check:versions': 'node scripts/check-versions.mjs',
          'format:check': 'node -e "process.exit(0)"',
          test: 'node -e "process.exit(1)"',
          build: "node -e \"require('node:fs').writeFileSync('unexpected-build','ran')\"",
        },
      }),
    );
    const result = spawnSync(
      'powershell.exe',
      [
        '-NoProfile',
        '-ExecutionPolicy',
        'Bypass',
        '-File',
        join(root, 'scripts/check-release.ps1'),
      ],
      { cwd: root, encoding: 'utf8', timeout: 30000 },
    );
    assert.ifError(result.error);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Frontend tests failed/);
    await assert.rejects(access(join(root, 'unexpected-build')));
  },
);
