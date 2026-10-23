/** 合并目录批次，在每个动画帧最多更新一次列表；完成后忽略迟到批次。 */
import type { Dispatch, SetStateAction } from 'react';
import type { Listing } from './types';

export type ListingBatch = Listing & { count: number; reset: boolean };
export function directoryFeed(
  current: () => boolean,
  setListing: Dispatch<SetStateAction<Listing | null>>,
  setCount: (count: number) => void,
  resetSelection: () => void,
) {
  let finished = false;
  let frame = 0;
  let batches: ListingBatch[] = [];
  const flush = () => {
    frame = 0;
    const pending = batches;
    batches = [];
    if (finished || !current() || !pending.length) return;
    const last = pending[pending.length - 1];
    const reset = pending.some((batch) => batch.reset);
    const incoming = pending.flatMap((batch) => batch.entries);
    if (reset) resetSelection();
    setCount(last.count);
    setListing((previous) => {
      if (finished || !current()) return previous;
      return {
        path: last.path,
        parent: last.parent,
        entries:
          reset || previous?.path !== last.path ? incoming : [...previous.entries, ...incoming],
      };
    });
  };
  return {
    onmessage(batch: ListingBatch) {
      if (finished || !current()) return;
      if (batch.reset) batches = [];
      batches.push(batch);
      if (!frame) frame = requestAnimationFrame(flush);
    },
    finish() {
      finished = true;
      cancelAnimationFrame(frame);
      batches = [];
    },
  };
}
