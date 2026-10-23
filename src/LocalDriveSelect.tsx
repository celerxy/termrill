/** Windows 盘符导航；仅枚举盘符，实际可访问性由原目录读取流程检查。 */
import { invoke, isTauri } from '@tauri-apps/api/core';
import { useEffect, useId, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, HardDrive } from 'lucide-react';

export default function LocalDriveSelect({
  path,
  disabled,
  onNavigate,
  onError,
}: {
  path: string;
  disabled: boolean;
  onNavigate: (path: string) => void;
  onError: (error: unknown) => void;
}) {
  const [drives, setDrives] = useState<string[]>([]);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const close = () => {
    setPosition(null);
    trigger.current?.focus();
  };
  useEffect(() => {
    let active = true;
    const refresh = () => {
      if (!isTauri()) return;
      void invoke<string[]>('list_local_drives')
        .then((values) => {
          if (active) setDrives(values);
        })
        .catch((error: unknown) => {
          if (active) onError(error);
        });
    };
    refresh();
    // 切回窗口时更新，插拔移动磁盘后不必重启应用。
    window.addEventListener('focus', refresh);
    return () => {
      active = false;
      window.removeEventListener('focus', refresh);
    };
  }, [onError]);
  useEffect(() => {
    setPosition(null);
  }, [path, disabled]);
  useEffect(() => {
    if (!position) return;
    menu.current?.querySelector<HTMLButtonElement>('[aria-checked="true"]')?.focus();
    const outside = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !menu.current?.contains(event.target) &&
        !trigger.current?.contains(event.target)
      ) {
        setPosition(null);
      }
    };
    const hide = (event: Event) => {
      if (event.target instanceof Node && menu.current?.contains(event.target)) return;
      setPosition(null);
    };
    document.addEventListener('pointerdown', outside);
    window.addEventListener('resize', hide);
    // 菜单通过 portal 避开面包屑的横向裁剪，滚动后关闭以免与锚点错位。
    window.addEventListener('scroll', hide, true);
    return () => {
      document.removeEventListener('pointerdown', outside);
      window.removeEventListener('resize', hide);
      window.removeEventListener('scroll', hide, true);
    };
  }, [position]);
  const current = path.match(/^([a-z]):[\\/]/i)?.[1].toUpperCase();
  const root = current ? `${current}:\\` : '';
  const options = root && !drives.includes(root) ? [...drives, root].sort() : drives;
  return (
    <>
      <button
        ref={trigger}
        className="local-drive-trigger"
        aria-label="切换本地磁盘"
        aria-haspopup="menu"
        aria-expanded={!!position}
        aria-controls={position ? menuId : undefined}
        disabled={disabled}
        onClick={() => {
          if (position) {
            setPosition(null);
            return;
          }
          const rect = trigger.current?.getBoundingClientRect();
          if (rect)
            setPosition({
              left: Math.max(8, Math.min(rect.left, window.innerWidth - 168)),
              top: rect.bottom + 6,
            });
        }}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown') {
            event.preventDefault();
            trigger.current?.click();
          }
        }}
      >
        {root}
      </button>
      {position &&
        createPortal(
          <div
            id={menuId}
            ref={menu}
            className="local-drive-menu"
            role="menu"
            aria-label="本地磁盘"
            style={position}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                event.preventDefault();
                event.stopPropagation();
                close();
              }
              if (event.key === 'Tab') setPosition(null);
              const keys = ['ArrowDown', 'ArrowUp', 'Home', 'End'];
              if (!keys.includes(event.key)) return;
              event.preventDefault();
              const buttons = Array.from(
                menu.current?.querySelectorAll<HTMLButtonElement>('button') ?? [],
              );
              const currentIndex = buttons.findIndex((button) => button === document.activeElement);
              let next = 0;
              if (event.key === 'End') {
                next = buttons.length - 1;
              } else if (event.key !== 'Home') {
                const step = event.key === 'ArrowDown' ? 1 : -1;
                next = (currentIndex + step + buttons.length) % buttons.length;
              }
              buttons[next]?.focus();
            }}
          >
            {options.map((drive) => (
              <button
                key={drive}
                role="menuitemradio"
                aria-checked={drive === root}
                onClick={() => {
                  close();
                  onNavigate(drive);
                }}
              >
                <HardDrive size={15} aria-hidden="true" />
                <span>{drive}</span>
                {drive === root && <Check size={14} aria-hidden="true" />}
              </button>
            ))}
          </div>,
          document.body,
        )}
    </>
  );
}
