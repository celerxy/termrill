/** 只收集声明格式的安装包并生成校验文件；缺少任一格式时失败，不发布或安装。 */
import { createHash } from 'node:crypto';
import { constants, createReadStream } from 'node:fs';
import { copyFile, mkdir, readdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const formats = { nsis: '.exe', dmg: '.dmg', deb: '.deb', appimage: '.AppImage' };

export async function collectInstallers(bundleRoot, outputRoot, kinds) {
  const selected = [...new Set(kinds)];
  if (selected.length === 0 || selected.some((kind) => !Object.hasOwn(formats, kind))) {
    throw new Error('安装包格式无效');
  }
  // 先确认所有格式均有产物，再复制；不会把部分平台产物误报为全部格式成功。
  const files = [];
  for (const kind of selected) {
    const directory = join(bundleRoot, kind);
    const entries = await readdir(directory, { withFileTypes: true });
    const installers = entries.filter(
      (entry) => entry.isFile() && entry.name.endsWith(formats[kind]),
    );
    if (installers.length === 0) {
      throw new Error(`缺少 ${kind} 安装包`);
    }
    for (const entry of installers) {
      files.push({ name: entry.name, source: join(directory, entry.name) });
    }
  }
  await mkdir(dirname(outputRoot), { recursive: true });
  await mkdir(outputRoot);
  for (const file of files) {
    const destination = join(outputRoot, file.name);
    // 测试包输出目录必须是新的，禁止复用或覆盖上次输出以掩盖缺失产物。
    await copyFile(file.source, destination, constants.COPYFILE_EXCL);
    const digest = createHash('sha256');
    for await (const bytes of createReadStream(destination)) {
      digest.update(bytes);
    }
    await writeFile(`${destination}.sha256`, `${digest.digest('hex')}  ${file.name}\n`, {
      encoding: 'utf8',
      flag: 'wx',
    });
  }
  return files.map((file) => file.name);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [target, bundles] = process.argv.slice(2);
  if (!/^[a-z0-9_]+(?:-[a-z0-9_]+)+$/.test(target ?? '')) {
    throw new Error('Rust target 无效');
  }
  const root = dirname(dirname(fileURLToPath(import.meta.url)));
  const files = await collectInstallers(
    join(root, 'src-tauri/target', target, 'release/bundle'),
    join(root, 'artifacts', target),
    (bundles ?? '').split(',').filter(Boolean),
  );
  console.log(`已收集 ${files.length} 个安装包及 SHA256 校验文件`);
}
