/** CSV 逻辑记录与分段边界回归，避免多行 SQL 被拆成额外表格记录。 */
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import assert from 'node:assert/strict';
import ts from 'typescript';
const source = await readFile(new URL('../src/csvData.ts', import.meta.url), 'utf8');
const output = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
}).outputText;
const { parseCsv, csvDelimiter } = await import(
  `data:text/javascript;base64,${Buffer.from(output).toString('base64')}`
);
test('BOM、CRLF、空单元格及无末尾换行均保留', () => {
  assert.deepEqual(parseCsv('\ufeffID,SQL,User\r\n1,,root\r\n2,a,', ',').rows, [
    ['ID', 'SQL', 'User'],
    ['1', '', 'root'],
    ['2', 'a', ''],
  ]);
  assert.deepEqual(parseCsv('', ',').rows, []);
});
test('截图中的多行 SQL、逗号和转义引号属于同一个字段', () => {
  const content =
    'ID,SQL,User\n1,"SELECT a,b\nFROM t WHERE name=""quoted""",root\n2,"SELECT COUNT(*)",user';
  const data = parseCsv(content, ',');
  assert.equal(data.warning, '');
  assert.deepEqual(data.rows, [
    ['ID', 'SQL', 'User'],
    ['1', 'SELECT a,b\nFROM t WHERE name="quoted"', 'root'],
    ['2', 'SELECT COUNT(*)', 'user'],
  ]);
});
test('自动分隔符识别忽略引号中的逗号', () => {
  assert.equal(csvDelimiter('ID;"SQL,a,b";User\n'), ';');
  assert.equal(csvDelimiter('ID\tSQL\tUser\n'), '\t');
  assert.equal(csvDelimiter('ID,SQL,User\n'), ',');
});
test('在每个字符位置截断，只交付完整逻辑记录', () => {
  const content = 'ID,SQL\r\n1,"a,b\r\nnext ""quote"""\r\n2,end';
  const expected = parseCsv(content, ',').rows;
  for (let index = 0; index < content.length; index++) {
    const partial = parseCsv(content.slice(0, index), ',', false);
    assert.equal(partial.warning, '');
    assert.deepEqual(partial.rows, expected.slice(0, partial.rows.length));
  }
  assert.deepEqual(parseCsv(content, ',', false).rows, expected.slice(0, 2));
});
test('完整文件的未闭合引号和引号后的非法内容明确提示', () => {
  assert.match(parseCsv('ID,SQL\n1,"SELECT', ',').warning, /未闭合/);
  assert.match(parseCsv('ID,SQL\n1,"SELECT"bad', ',').warning, /无效内容/);
});
test('不丢弃空列、不执行公式、不把 HTML 作为结构解释', () => {
  assert.deepEqual(parseCsv('ID,Value\n1,=SUM(A1)\n2,<script>test</script>\n3,,,', ',').rows, [
    ['ID', 'Value'],
    ['1', '=SUM(A1)'],
    ['2', '<script>test</script>'],
    ['3', '', '', ''],
  ]);
});
test('记录和列数预算停止解析并明确提示，而非无界生成表格', () => {
  const data = parseCsv(
    Array.from({ length: 10003 }, (_, index) => `${index},value`).join('\n'),
    ',',
  );
  assert.equal(data.rows.length, 10001);
  assert.match(data.warning, /10001/);
  assert.match(parseCsv(Array(257).fill('column').join(','), ',').warning, /256/);
});
