import { useEffect, useRef, useState } from 'react';
import {
  normalizeWorkspaceLayout,
  defaultWorkspaceLayout,
  workspaceLayoutKey,
  type WorkspaceLayout,
} from './workspaceLayout';
const eventName = 'termrill-workspace-layout-changed';

/** 拖动期间只更新内存，松开时才落盘；写入失败保留本次布局并明确提示。 */
export function useWorkspaceLayout(onError: (error: unknown) => void) {
  const [layout, setLayout] = useState(defaultWorkspaceLayout);
  const error = useRef(onError);
  error.current = onError;
  useEffect(() => {
    const read = () => {
      try {
        const raw = localStorage.getItem(workspaceLayoutKey);
        setLayout(normalizeWorkspaceLayout(raw ? JSON.parse(raw) : null));
      } catch {
        error.current('无法读取保存的布局，当前使用默认尺寸');
      }
    };
    read();
    window.addEventListener(eventName, read);
    window.addEventListener('storage', read);
    return () => {
      window.removeEventListener(eventName, read);
      window.removeEventListener('storage', read);
    };
  }, []);
  function save(next: WorkspaceLayout) {
    const valid = normalizeWorkspaceLayout(next);
    setLayout(valid);
    try {
      localStorage.setItem(workspaceLayoutKey, JSON.stringify(valid));
      window.dispatchEvent(new Event(eventName));
    } catch {
      onError('布局已调整，但保存失败；重启后可能恢复原尺寸');
    }
  }
  return { layout, preview: setLayout, save, reset: () => save(defaultWorkspaceLayout) };
}
