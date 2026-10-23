/** 搜索只保留命中位置，逐页解析；输入变化/关闭后丢弃过期结果。 */
import { useEffect, useMemo, useState } from 'react';
import type { PDFDocumentProxy } from 'pdfjs-dist';
import { PdfTextCache, mappedPdfMatches, pdfSearchText } from './pdfTextCache';
export type PdfMatch = { page: number; start: number; length: number };
export function usePdfSearch(document: PDFDocumentProxy | null, query: string) {
  const [matches, setMatches] = useState<PdfMatch[]>([]);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState('');
  const cache = useMemo(() => new PdfTextCache(), [document]);
  useEffect(() => {
    let stopped = false;
    setMatches([]);
    setError('');
    setSearching(false);
    const needle = query.trim();
    if (!document || !needle) return;
    const timer = window.setTimeout(() => {
      setSearching(true);
      void (async () => {
        const found: PdfMatch[] = [];
        let lastUpdate = 0;
        for (let number = 1; number <= document.numPages; number++) {
          if (stopped) return;
          let content = cache.get(number);
          if (!content) {
            try {
              const page = await document.getPage(number);
              const extracted = await page.getTextContent();
              if (stopped) return;
              content = pdfSearchText(extracted.items.filter((item) => 'str' in item));
              cache.set(number, content);
            } catch {
              if (!stopped) setError('部分页面无法搜索，其余页面继续搜索');
              continue;
            }
          }
          if (stopped) return;
          found.push(
            ...mappedPdfMatches(content, needle, 10000 - found.length).map((match) => ({
              ...match,
              page: number,
            })),
          );
          if (
            performance.now() - lastUpdate > 100 ||
            number === document.numPages ||
            found.length >= 10000
          ) {
            setMatches([...found]);
            lastUpdate = performance.now();
            await new Promise((resolve) => window.setTimeout(resolve, 0));
          }
          if (found.length >= 10000) break;
        }
        if (!stopped) setMatches([...found]);
      })()
        .catch(() => {
          if (!stopped) setError('部分页面无法搜索');
        })
        .finally(() => {
          if (!stopped) setSearching(false);
        });
    }, 250);
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [document, query, cache]);
  return { matches, searching, error };
}
