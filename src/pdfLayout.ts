/** 页面布局使用原始尺寸；二分定位与阅读锚点不依赖全部页面 DOM。 */
export type PdfSize = { width: number; height: number };
export function pdfScale(
  size: PdfSize,
  width: number,
  height: number,
  mode: 'width' | 'page' | 'custom',
  zoom: number,
) {
  if (mode === 'custom') return zoom;
  const fit = (width - 48) / size.width;
  return Math.max(0.1, mode === 'page' ? Math.min(fit, (height - 40) / size.height) : fit);
}
export function pageAt(offsets: number[], top: number) {
  let low = 0;
  let high = Math.max(0, offsets.length - 2);
  while (low < high) {
    const middle = Math.floor((low + high + 1) / 2);
    if (offsets[middle] <= top) low = middle;
    else high = middle - 1;
  }
  return low;
}
export function restorePdfTop(old: number[], next: number[], top: number) {
  if (top <= 14) return top;
  const index = pageAt(old, top - 14);
  const fraction = Math.max(0, (top - 14 - old[index]) / (old[index + 1] - old[index]));
  return 14 + next[index] + fraction * (next[index + 1] - next[index]);
}
