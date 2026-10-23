import { readFile } from 'node:fs/promises';
import test from 'node:test';
import assert from 'node:assert/strict';
import ts from 'typescript';
const source = await readFile(new URL('../src/previewTextDecoder.ts', import.meta.url), 'utf8');
const output = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
}).outputText;
const { PreviewTextDecoder } = await import(
  `data:text/javascript;base64,${Buffer.from(output).toString('base64')}`
);
test('GBK 中文 CSV 在每个分段边界正确解码', () => {
  const bytes = Uint8Array.from([73, 68, 44, 195, 251, 179, 198, 10, 49, 44, 179, 181]);
  for (let i = 1; i < bytes.length; i++) {
    const decoder = new PreviewTextDecoder();
    assert.equal(
      decoder.decode(bytes.slice(0, i), false) + decoder.decode(bytes.slice(i), true),
      'ID,名称\n1,车',
    );
    assert.equal(decoder.encoding, 'GB18030');
  }
});
test('UTF-8 中文在每个分段边界正确解码', () => {
  const bytes = new TextEncoder().encode('ID,名称\n1,车');
  for (let i = 1; i < bytes.length; i++) {
    const decoder = new PreviewTextDecoder();
    assert.equal(
      decoder.decode(bytes.slice(0, i), false) + decoder.decode(bytes.slice(i), true),
      'ID,名称\n1,车',
    );
    assert.equal(decoder.encoding, 'UTF-8');
  }
});
test('UTF-16 BOM 与二进制、损坏的 BOM 文本', () => {
  const decoder = new PreviewTextDecoder();
  assert.equal(decoder.decode(Uint8Array.from([255, 254, 65, 0]), true), 'A');
  assert.throws(() => new PreviewTextDecoder().decode(Uint8Array.from([65, 0]), true), /二进制/);
  assert.throws(
    () => new PreviewTextDecoder().decode(Uint8Array.from([239, 187, 191, 255]), true),
    /编码/,
  );
});
test('手动选择西文、Big5 与 UTF-16 编码后保持分块状态', () => {
  for (const [name, bytes, text] of [
    [
      'windows-1252',
      [105, 100, 44, 110, 97, 109, 101, 10, 49, 44, 99, 97, 102, 233],
      'id,name\n1,café',
    ],
    ['Big5', [73, 68, 44, 166, 87, 186, 217], 'ID,名稱'],
    ['UTF-16 LE', [255, 254, 65, 0, 44, 0, 66, 0], 'A,B'],
    ['UTF-16 BE', [254, 255, 0, 65, 0, 44, 0, 66], 'A,B'],
  ]) {
    const data = Uint8Array.from(bytes);
    for (let i = 1; i < data.length; i++) {
      const decoder = new PreviewTextDecoder(name);
      assert.equal(
        decoder.decode(data.slice(0, i), false) + decoder.decode(data.slice(i), true),
        text,
      );
      assert.equal(decoder.encoding, name);
    }
  }
});
