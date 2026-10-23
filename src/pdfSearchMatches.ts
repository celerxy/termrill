/** 使用原始文本偏移，避免大小写转换改变字符长度而使高亮错位。 */
export function findPdfMatches(text: string, query: string, limit = 10000) {
  const needle = query.trim();
  if (!needle || limit <= 0) return [];
  const pattern = new RegExp(needle.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'giu');
  const matches: { start: number; length: number }[] = [];
  for (const match of text.matchAll(pattern)) {
    matches.push({ start: match.index, length: match[0].length });
    if (matches.length >= limit) break;
  }
  return matches;
}
