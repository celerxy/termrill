/** CSV 逻辑记录解析：引号中的换行属于单元格，分段读取只交付完整记录。 */
export type CsvData = { rows: string[][]; delimiter: string; warning: string };
export function csvDelimiter(content: string): string {
  const counts = new Map([
    [',', 0],
    ['\t', 0],
    [';', 0],
  ]);
  let quoted = false;
  for (let index = 0; index < Math.min(content.length, 65536); index++) {
    const char = content[index];
    if (char === '"') {
      if (quoted && content[index + 1] === '"') index++;
      else quoted = !quoted;
    } else if (!quoted) {
      if (char === '\r' || char === '\n') break;
      if (counts.has(char)) counts.set(char, (counts.get(char) ?? 0) + 1);
    }
  }
  return [...counts].sort((a, b) => b[1] - a[1])[0][0];
}
export function parseCsv(content: string, delimiter: string, complete = true): CsvData {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = '';
  let mode: 'field' | 'quoted' | 'closed' = 'field';
  let started = false;
  const result = (warning = '') => ({ rows, delimiter, warning });
  const pushField = () => {
    row.push(field);
    field = '';
    mode = 'field';
  };
  for (let index = content.charCodeAt(0) === 0xfeff ? 1 : 0; index < content.length; index++) {
    if (index >= 8 * 1024 * 1024 || rows.length >= 10001)
      return result('表格最多解析 10001 条记录（含表头）及约 800 万字符，可切换原始文本查看。');
    const char = content[index];
    started = true;
    if (mode === 'quoted') {
      if (char === '"') {
        if (content[index + 1] === '"') {
          field += '"';
          index++;
        } else mode = 'closed';
      } else field += char;
      continue;
    }
    if (char === delimiter) {
      pushField();
      if (row.length >= 256) return result('列数超过 256，请切换原始文本查看。');
    } else if (char === '\r' || char === '\n') {
      pushField();
      rows.push(row);
      row = [];
      started = false;
      if (char === '\r' && content[index + 1] === '\n') index++;
    } else if (char === '"' && mode === 'field' && field === '') {
      mode = 'quoted';
    } else if (mode === 'closed') {
      if (char !== ' ' && char !== '\t')
        return result(`第 ${rows.length + 1} 条记录的引号后存在无效内容，请检查原始文本。`);
    } else field += char;
  }
  if (complete && mode === 'quoted') return result('末条记录的引号未闭合，请检查原始文本。');
  // 末批完整文件可没有行结束符；后续内容尚未读取时不显示未完成的末条记录。
  if (complete && started) {
    pushField();
    rows.push(row);
  }
  return result();
}
