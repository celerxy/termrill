/** 按终端单元格定位，宽字符及组合字符不能用字符串下标当作列号。 */
type BufferLine = {
  length: number;
  getCell: (column: number) => { getChars(): string; getWidth(): number } | undefined;
};
type Buffer = { length: number; getLine: (row: number) => BufferLine | undefined };
export function terminalMatches(buffer: Buffer, query: string) {
  const matches: { row: number; column: number; length: number }[] = [];
  if (!query || query.length > 256) return matches;
  let remaining = 2_000_000;
  for (let row = 0; row < buffer.length && remaining > 0 && matches.length < 10000; row++) {
    const line = buffer.getLine(row);
    if (!line) continue;
    let text = '';
    const columns: number[] = [];
    const ends: number[] = [];
    for (let column = 0; column < line.length && remaining > 0; column++) {
      const cell = line.getCell(column);
      if (!cell || cell.getWidth() === 0) continue;
      const chars = cell.getChars() || ' ';
      text += chars;
      remaining -= chars.length;
      for (let i = 0; i < chars.length; i++) {
        columns.push(column);
        ends.push(column + cell.getWidth());
      }
    }
    let from = 0;
    while (matches.length < 10000) {
      const index = text.indexOf(query, from);
      if (index < 0) break;
      matches.push({
        row,
        column: columns[index],
        length: ends[index + query.length - 1] - columns[index],
      });
      from = index + query.length;
    }
  }
  return matches;
}
