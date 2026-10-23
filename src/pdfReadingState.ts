/** 本机阅读偏好只保存 PDF 指纹、页码和缩放；不保存路径、标题、正文或凭据。 */
export type PdfReadingState = { page: number; zoom: number; mode: 'width' | 'page' | 'custom' };
const storageKey = 'termrill.pdfReading.v1';
const recordLimit = 50;
type RecordEntry = { id: string; state: PdfReadingState };

function validState(value: unknown): value is PdfReadingState {
  if (!value || typeof value !== 'object') return false;
  const state = value as Partial<PdfReadingState>;
  return (
    Number.isSafeInteger(state.page) &&
    Number(state.page) >= 1 &&
    typeof state.zoom === 'number' &&
    Number.isFinite(state.zoom) &&
    state.zoom >= 0.25 &&
    state.zoom <= 3 &&
    (state.mode === 'width' || state.mode === 'page' || state.mode === 'custom')
  );
}

function readRecords(storage: Pick<Storage, 'getItem'>): RecordEntry[] {
  const raw = storage.getItem(storageKey);
  if (!raw) return [];
  if (raw.length > 65536) throw new Error('PDF 阅读记录过大，无法读取或保存');
  const records: unknown = JSON.parse(raw);
  if (
    !Array.isArray(records) ||
    !records.every(
      (entry) =>
        entry && typeof entry.id === 'string' && entry.id.length <= 256 && validState(entry.state),
    )
  ) {
    throw new Error('PDF 阅读记录格式异常，未覆盖原记录');
  }
  return records.slice(-recordLimit);
}

export function loadPdfReadingState(storage: Pick<Storage, 'getItem'>, id: string, pages: number) {
  const saved = readRecords(storage).find((entry) => entry.id === id)?.state;
  return saved ? { ...saved, page: Math.min(pages, saved.page) } : null;
}

export function savePdfReadingState(
  storage: Pick<Storage, 'getItem' | 'setItem'>,
  id: string,
  state: PdfReadingState,
) {
  if (!id || id.length > 256 || !validState(state)) return;
  const records = readRecords(storage).filter((entry) => entry.id !== id);
  records.push({ id, state });
  storage.setItem(storageKey, JSON.stringify(records.slice(-recordLimit)));
}
