/** 安装包收集不得混入运行文件、漏格式或覆盖旧输出；校验和必须对应实际下载内容。 */
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  access,
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import test from 'node:test';
import { collectInstallers } from '../scripts/collect-installers.mjs';

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'termrill-installers-'));
  t.after(async () => {
    assert.equal(dirname(root), resolve(tmpdir()));
    assert.match(root.slice(dirname(root).length + 1), /^termrill-installers-/);
    await rm(root, { recursive: true, force: true });
  });
  return root;
}

test('多格式安装包保留原字节并生成可复核的 SHA256，不收集无关文件', async (t) => {
  const root = await fixture(t);
  const bundles = join(root, 'bundle');
  const output = join(root, 'artifacts/linux');
  const source = Buffer.from([0, 255, 17, 128, 0, 32]);
  for (const [kind, extension] of [
    ['deb', '.deb'],
    ['appimage', '.AppImage'],
  ]) {
    await mkdir(join(bundles, kind), { recursive: true });
    await writeFile(join(bundles, kind, `TermRill${extension}`), source);
    await writeFile(join(bundles, kind, 'unrelated.txt'), '不上传');
  }
  const files = await collectInstallers(bundles, output, ['deb', 'appimage']);
  assert.deepEqual(files.sort(), ['TermRill.AppImage', 'TermRill.deb']);
  assert.equal((await readdir(output)).length, 4);
  for (const name of files) {
    assert.deepEqual(await readFile(join(output, name)), source);
    assert.equal(
      await readFile(join(output, `${name}.sha256`), 'utf8'),
      `${createHash('sha256').update(source).digest('hex')}  ${name}\n`,
    );
  }
});

test('缺少声明格式时不得生成部分成功输出，拒绝未知格式和路径穿越参数', async (t) => {
  const root = await fixture(t);
  const bundles = join(root, 'bundle');
  const output = join(root, 'output');
  await mkdir(join(bundles, 'deb'), { recursive: true });
  await mkdir(join(bundles, 'appimage'));
  await writeFile(join(bundles, 'deb/TermRill.deb'), 'fixture');
  await assert.rejects(collectInstallers(bundles, output, ['deb', 'appimage']), /缺少 appimage/);
  await assert.rejects(access(output));
  for (const kinds of [[], ['../'], ['zip']]) {
    await assert.rejects(collectInstallers(bundles, output, kinds), /格式无效/);
  }
});

test('已有输出目录必须报错，不能把旧版本安装包混进本次下载', async (t) => {
  const root = await fixture(t);
  const bundles = join(root, 'bundle');
  const output = join(root, 'output');
  await mkdir(join(bundles, 'nsis'), { recursive: true });
  await mkdir(output);
  await writeFile(join(bundles, 'nsis/TermRill.exe'), 'new');
  await writeFile(join(output, 'old.exe'), 'previous');
  await assert.rejects(collectInstallers(bundles, output, ['nsis']), { code: 'EEXIST' });
  assert.equal(await readFile(join(output, 'old.exe'), 'utf8'), 'previous');
  assert.deepEqual(await readdir(output), ['old.exe']);
});

test('命令行入口支持 x86_64 Rust target，并从对应 release 目录收集安装包', async (t) => {
  const root = await fixture(t);
  const target = 'x86_64-pc-windows-msvc';
  const bundle = join(root, 'src-tauri/target', target, 'release/bundle/nsis');
  const scripts = join(root, 'scripts');
  await mkdir(bundle, { recursive: true });
  await mkdir(scripts);
  await writeFile(join(bundle, 'TermRill.exe'), 'fixture installer');
  const entry = join(scripts, 'collect-installers.mjs');
  await copyFile(new URL('../scripts/collect-installers.mjs', import.meta.url), entry);
  const result = spawnSync(process.execPath, [entry, target, 'nsis'], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(
    await readFile(join(root, 'artifacts', target, 'TermRill.exe'), 'utf8'),
    'fixture installer',
  );
});
