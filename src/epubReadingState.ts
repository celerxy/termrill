/** 本机 EPUB 记录仅保存内容摘要与数字偏好，不保存书名、路径、锚点或正文。 */
export type EpubReadingState = { chapter: number; progress: number; fontSize: number };
const key = 'termrill.epubReading.v1';
type Entry = { id: string; state: EpubReadingState };

function valid(value: unknown): value is EpubReadingState {
  if (!value || typeof value !== 'object') return false;
  const state = value as Partial<EpubReadingState>;
  return (
    Number.isSafeInteger(state.chapter) &&
    Number(state.chapter) >= 0 &&
    typeof state.progress === 'number' &&
    Number.isFinite(state.progress) &&
    state.progress >= 0 &&
    state.progress <= 1 &&
    typeof state.fontSize === 'number' &&
    Number.isInteger(state.fontSize) &&
    state.fontSize >= 14 &&
    state.fontSize <= 32 &&
    state.fontSize % 2 === 0
  );
}

function records(storage: Pick<Storage, 'getItem'>): Entry[] {
  const raw = storage.getItem(key);
  if (!raw) return [];
  if (raw.length > 16384) throw new Error('EPUB 阅读记录过大');
  const value: unknown = JSON.parse(raw);
  if (
    !Array.isArray(value) ||
    value.length > 50 ||
    !value.every(
      (entry) =>
        entry &&
        typeof entry.id === 'string' &&
        /^[a-f0-9]{64}$/.test(entry.id) &&
        valid(entry.state),
    )
  ) {
    throw new Error('EPUB 阅读记录异常，未覆盖原记录');
  }
  return value.map((entry) => ({
    id: entry.id,
    state: {
      chapter: entry.state.chapter,
      progress: entry.state.progress,
      fontSize: entry.state.fontSize,
    },
  }));
}

export function loadEpubReadingState(
  storage: Pick<Storage, 'getItem'>,
  id: string,
  chapters: number,
) {
  const state = records(storage).find((entry) => entry.id === id)?.state;
  if (!state || state.chapter >= chapters) return null;
  return { ...state };
}

export function saveEpubReadingState(
  storage: Pick<Storage, 'getItem' | 'setItem'>,
  id: string,
  state: EpubReadingState,
) {
  if (!/^[a-f0-9]{64}$/.test(id) || !valid(state)) return;
  const entries = records(storage).filter((entry) => entry.id !== id);
  // 明确重建白名单，调用方多余字段不能进入持久化记录。
  entries.push({
    id,
    state: { chapter: state.chapter, progress: state.progress, fontSize: state.fontSize },
  });
  storage.setItem(key, JSON.stringify(entries.slice(-50)));
}
