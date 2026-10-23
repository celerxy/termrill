import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';
async function load(path) {
  const source = await readFile(new URL(path, import.meta.url), 'utf8');
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
  });
  return import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`);
}
const { archiveWriteFormat, archiveNameForFormat, archiveCanEncrypt } = await load(
  '../src/archiveCapabilities.ts',
);
const { parsePackProgress, updatePackReport } = await load('../src/archivePackMetrics.ts');
test('多后缀与别名识别、格式切换不叠加后缀，只读格式不开放写入', () => {
  for (const [path, expected] of [
    ['a.TAR.GZ', 'tar.gz'],
    ['a.tgz', 'tar.gz'],
    ['a.tbz2', 'tar.bz2'],
    ['a.txz', 'tar.xz'],
    ['a.7z', '7z'],
  ])
    assert.equal(archiveWriteFormat(path), expected);
  for (const path of ['a.rar', 'a.gz', 'a.bz2', 'a.xz'])
    assert.equal(archiveWriteFormat(path), null);
  assert.equal(archiveNameForFormat('中文.tar.gz', '7z'), '中文.7z');
  assert.equal(archiveNameForFormat('中文.7z', 'tar.xz'), '中文.tar.xz');
  assert.equal(archiveCanEncrypt('tar.gz'), false);
  assert.equal(archiveCanEncrypt('7z'), true);
});
test('编码阶段独立计时，不把中间数据当作已安装的压缩包', () => {
  const data = {
    kind: 'packProgress',
    stage: 'compress',
    files: 1,
    directories: 0,
    processedFiles: 1,
    totalBytes: 42,
    processedBytes: 42,
    zipBytes: 0,
    uploadedBytes: 0,
    currentFile: '',
  };
  let report = updatePackReport(null, data, 0);
  const encoding = parsePackProgress(JSON.stringify({ ...data, stage: 'encode' }));
  assert.ok(encoding);
  report = updatePackReport(report, encoding, 100);
  report = updatePackReport(report, { ...encoding, stage: 'upload', zipBytes: 20 }, 400);
  assert.equal(report.durations.encode, 300);
  assert.equal(report.data.uploadedBytes, 0);
});
