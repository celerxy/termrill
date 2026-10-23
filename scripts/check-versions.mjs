/** 只读取三处应用版本；检查失败不得改写配置或继续打包。 */
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export async function checkVersions(projectRoot) {
  const packageJson = JSON.parse(await readFile(resolve(projectRoot, 'package.json'), 'utf8'));
  const tauriConfig = JSON.parse(
    await readFile(resolve(projectRoot, 'src-tauri/tauri.conf.json'), 'utf8'),
  );
  const cargoManifest = await readFile(resolve(projectRoot, 'src-tauri/Cargo.toml'), 'utf8');
  // 本项目使用显式 [package].version；不把依赖版本或 workspace 继承当作应用版本。
  const packageSection = cargoManifest
    .split(/^\s*\[package\][ \t]*(?:#.*)?$/m)[1]
    ?.split(/^\s*\[/m)[0];
  const cargoVersion = packageSection?.match(
    /^version[ \t]*=[ \t]*["']([^"']+)["'][ \t]*(?:#.*)?$/m,
  )?.[1];
  const versions = {
    'package.json': packageJson.version,
    'tauri.conf.json': tauriConfig.version,
    'Cargo.toml': cargoVersion,
  };
  for (const [name, version] of Object.entries(versions)) {
    if (typeof version !== 'string' || !version.trim() || version !== version.trim()) {
      throw new Error(`${name} 缺少显式有效的应用版本`);
    }
  }
  if (new Set(Object.values(versions)).size !== 1) {
    throw new Error(
      `应用版本不一致：${Object.entries(versions)
        .map(([name, value]) => `${name}=${value}`)
        .join('，')}`,
    );
  }
  return packageJson.version;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    console.log(`应用版本一致：${await checkVersions(process.cwd())}`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : '版本检查失败');
    process.exitCode = 1;
  }
}
