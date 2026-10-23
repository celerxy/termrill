/** 共享工作台偏好串行保存；先完成读取，失败不回退默认值覆盖磁盘。 */
import { useEffect, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
export type SavedCommand = { id: string; name: string; command: string };
export type WorkbenchPreferences = { groups: Record<string, string>; commands: SavedCommand[] };
export function useWorkbenchPreferences() {
  const [value, setValue] = useState<WorkbenchPreferences>({ groups: {}, commands: [] });
  const [ready, setReady] = useState(!isTauri());
  const [error, setError] = useState('');
  const current = useRef(value);
  const loading = useRef<Promise<void>>(Promise.resolve());
  const writes = useRef<Promise<void>>(Promise.resolve());
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    loading.current = invoke<WorkbenchPreferences>('workbench_preferences', { update: null }).then(
      (next) => {
        if (!disposed) {
          current.current = next;
          setValue(next);
          setReady(true);
        }
      },
    );
    void loading.current.catch(() => {
      if (!disposed) setError('工作台偏好读取失败，未覆盖原配置');
    });
    return () => {
      disposed = true;
    };
  }, []);
  function update(change: (previous: WorkbenchPreferences) => WorkbenchPreferences) {
    const write = writes.current
      .catch(() => {})
      .then(async () => {
        await loading.current;
        const next = await invoke<WorkbenchPreferences>('workbench_preferences', {
          update: change(current.current),
        });
        current.current = next;
        setValue(next);
      });
    writes.current = write;
    return write;
  }
  return { value, ready, error, update };
}
