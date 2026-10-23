/** 显式列举测试文件，避免 Windows 与 POSIX Shell 的通配符展开差异。 */
import { readdir } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const projectRoot = fileURLToPath(new URL('../', import.meta.url));
const tests = (await readdir(new URL('../tests/', import.meta.url)))
  .filter((name) => name.endsWith('.test.mjs'))
  .sort()
  .map((name) => `tests/${name}`);
if (tests.length === 0) {
  throw new Error('没有找到前端测试文件，拒绝报告检查成功');
}
const result = spawnSync(process.execPath, ['--test', ...tests], {
  cwd: projectRoot,
  stdio: 'inherit',
});
if (result.error) {
  throw result.error;
}
process.exitCode = result.status ?? 1;
