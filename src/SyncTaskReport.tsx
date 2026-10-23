/** 折叠时不序列化文件清单，避免进度刷新为大批同步反复生成报告。 */
import { useMemo, useState } from 'react';
import type { Transfer } from './types';
import { syncReportText } from './directorySyncModel';

export default function SyncTaskReport({ task, status }: { task: Transfer; status: string }) {
  const [open, setOpen] = useState(false);
  const text = useMemo(
    () =>
      open && task.syncReport
        ? syncReportText(task.syncReport, {
            id: task.id,
            status: task.status,
            bytes: task.bytes,
            total: task.total,
            error: task.error,
          })
        : '',
    [open, task.syncReport, task.id, task.status, task.bytes, task.total, task.error],
  );
  return (
    <details className="sync-task-report" onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>同步结果报告 · {status}</summary>
      {open && <pre>{text}</pre>}
    </details>
  );
}
