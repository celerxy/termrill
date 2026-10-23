/** 查询缓存仅在当前文档内保留：最多 32 页、约 4 MiB 字符与偏移数据。 */
import { findPdfMatches } from './pdfSearchMatches';
type TextItem = { str: string; hasEOL?: boolean };
export function pdfSearchText(items: TextItem[]) {
  let text = '';
  const offsets: number[] = [];
  const ends: number[] = [];
  let original = 0;
  const append = (character: string, offset: number, length = 1) => {
    if (character === '\u00ad') return;
    const value = /\s/u.test(character) ? ' ' : character;
    if (value === ' ' && text.endsWith(' ')) return;
    text += value;
    offsets.push(offset);
    ends.push(offset + length);
  };
  for (const item of items) {
    for (let index = 0; index < item.str.length; index++) append(item.str[index], original + index);
    original += item.str.length;
    if (item.hasEOL) append(' ', original, 0);
  }
  return { text, offsets, ends, cost: text.length * 18 };
}
export type SearchText = ReturnType<typeof pdfSearchText>;
export function mappedPdfMatches(content: SearchText, query: string, limit: number) {
  return findPdfMatches(content.text, query.replace(/\s+/gu, ' '), limit).map((match) => {
    const start = content.offsets[match.start];
    const end = content.ends[match.start + match.length - 1];
    return { start, length: end - start };
  });
}
export class PdfTextCache {
  private entries = new Map<number, SearchText>();
  private cost = 0;
  get(page: number) {
    const entry = this.entries.get(page);
    if (entry) {
      this.entries.delete(page);
      this.entries.set(page, entry);
    }
    return entry;
  }
  set(page: number, entry: SearchText) {
    const old = this.entries.get(page);
    if (old) {
      this.cost -= old.cost;
      this.entries.delete(page);
    }
    if (entry.cost > 4 * 1024 * 1024) return;
    this.entries.set(page, entry);
    this.cost += entry.cost;
    while (this.entries.size > 32 || this.cost > 4 * 1024 * 1024) {
      const first = this.entries.keys().next().value;
      if (first === undefined) break;
      const oldest = this.entries.get(first);
      if (oldest) this.cost -= oldest.cost;
      this.entries.delete(first);
    }
  }
}
