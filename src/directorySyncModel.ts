/** 同步报告仅描述已观测到的任务状态，不将比较结果当作写入成功。 */
export type SyncOptions = { quick: boolean; exclude: string[] };
export type SyncRule = {
  id: string;
  name: string;
  local: string;
  remote: string;
  direction: 'upload' | 'download';
  options: SyncOptions;
};
export type SyncReport = Omit<SyncRule, 'id'> & {
  comparedAt: string;
  selectedCount: number;
  selected: { relative: string; status: string; size: number }[];
};
export function syncReportText(
  report: SyncReport,
  task: {
    id: string;
    status: string;
    bytes: number;
    total: number;
    error: string | null;
  },
) {
  return JSON.stringify(
    {
      reportVersion: 1,
      generatedAt: new Date().toISOString(),
      comparison: report,
      transfer: task,
      note: '任务级结果；清单最多列出前 1000 项，selectedCount 是实际选择总数。失败或取消可能已有部分文件完成。未逐文件确认的项目不标为成功。快速比较不能保证内容一致。',
    },
    null,
    2,
  );
}

export type DiffLine = { kind: 'same' | 'remove' | 'add'; text: string };
/** 有界逐行 LCS；长文件以共同前后缀和中段替换显示，避免平方级内存。 */
export function textDifference(left: string, right: string): DiffLine[] {
  const a = left.replace(/\r\n/g, '\n').split('\n');
  const b = right.replace(/\r\n/g, '\n').split('\n');
  if (a.length + b.length > 6000) throw new Error('文本超过 6000 行，请使用外部差异工具');
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let end = 0;
  while (
    end < a.length - start &&
    end < b.length - start &&
    a[a.length - 1 - end] === b[b.length - 1 - end]
  )
    end++;
  const x = a.slice(start, a.length - end);
  const y = b.slice(start, b.length - end);
  const result: DiffLine[] = a.slice(0, start).map((text) => ({ kind: 'same', text }));
  if (x.length * y.length > 250000) {
    result.push(
      ...x.map((text) => ({ kind: 'remove' as const, text })),
      ...y.map((text) => ({ kind: 'add' as const, text })),
    );
  } else {
    const width = y.length + 1;
    const lcs = new Uint16Array((x.length + 1) * width);
    for (let i = x.length - 1; i >= 0; i--) {
      for (let j = y.length - 1; j >= 0; j--) {
        lcs[i * width + j] =
          x[i] === y[j]
            ? lcs[(i + 1) * width + j + 1] + 1
            : Math.max(lcs[(i + 1) * width + j], lcs[i * width + j + 1]);
      }
    }
    let i = 0,
      j = 0;
    while (i < x.length || j < y.length) {
      if (i < x.length && j < y.length && x[i] === y[j]) {
        result.push({ kind: 'same', text: x[i++] });
        j++;
      } else if (
        i < x.length &&
        (j === y.length || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
      ) {
        result.push({ kind: 'remove', text: x[i++] });
      } else result.push({ kind: 'add', text: y[j++] });
    }
  }
  result.push(...a.slice(a.length - end).map((text) => ({ kind: 'same' as const, text })));
  return result;
}
