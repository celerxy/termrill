/** PDF 页内目录模型：保留稳定序号，搜索显示命中及祖先，折叠不改变条目身份。 */
import type { PDFDocumentProxy } from 'pdfjs-dist';

export type PdfOutlineSource = NonNullable<Awaited<ReturnType<PDFDocumentProxy['getOutline']>>>;
export type PdfOutlineEntry = {
  id: number;
  parent: number | null;
  depth: number;
  title: string;
  dest: unknown;
  hasChildren: boolean;
  page: number | null;
};

export function flattenPdfOutline(items: PdfOutlineSource): PdfOutlineEntry[] {
  const result: PdfOutlineEntry[] = [];
  const stack = items.map((item) => ({ item, parent: null as number | null, depth: 0 })).reverse();
  while (stack.length) {
    const next = stack.pop();
    if (!next) break;
    const id = result.length;
    result.push({
      id,
      parent: next.parent,
      depth: next.depth,
      title: next.item.title.replace(/\s+/g, ' ').trim() || '未命名章节',
      dest: next.item.dest,
      hasChildren: next.item.items.length > 0,
      page: null,
    });
    for (let index = next.item.items.length - 1; index >= 0; index--) {
      stack.push({ item: next.item.items[index], parent: id, depth: next.depth + 1 });
    }
  }
  return result;
}

export function visiblePdfOutline(
  entries: PdfOutlineEntry[],
  collapsed: ReadonlySet<number>,
  query: string,
): PdfOutlineEntry[] {
  const needle = query.trim().toLocaleLowerCase();
  if (needle) {
    const included = new Set<number>();
    // 逆序传播祖先，深层或大量命中时也只遍历一遍目录。
    for (let index = entries.length - 1; index >= 0; index--) {
      const entry = entries[index];
      if (entry.title.toLocaleLowerCase().includes(needle)) included.add(entry.id);
      if (included.has(entry.id) && entry.parent !== null) included.add(entry.parent);
    }
    return entries.filter((entry) => included.has(entry.id));
  }
  let hiddenDepth = Infinity;
  return entries.filter((entry) => {
    if (entry.depth > hiddenDepth) return false;
    hiddenDepth = collapsed.has(entry.id) ? entry.depth : Infinity;
    return true;
  });
}

/** 同页目录选择更深层、后出现的章节；未到首章时不错误高亮未来章节。 */
export function activePdfOutline(entries: PdfOutlineEntry[], current: number): number | null {
  let active: PdfOutlineEntry | null = null;
  for (const entry of entries) {
    if (entry.page === null || entry.page > current) continue;
    if (
      !active ||
      entry.page > (active.page ?? 0) ||
      (entry.page === active.page && entry.depth >= active.depth)
    )
      active = entry;
  }
  return active?.id ?? null;
}

export async function resolvePdfDestination(document: PDFDocumentProxy, dest: unknown) {
  const resolved = typeof dest === 'string' ? await document.getDestination(dest) : dest;
  if (!Array.isArray(resolved) || !resolved.length) return null;
  const index =
    typeof resolved[0] === 'number' ? resolved[0] : await document.getPageIndex(resolved[0]);
  return Number.isInteger(index) && index >= 0 && index < document.numPages ? index + 1 : null;
}
