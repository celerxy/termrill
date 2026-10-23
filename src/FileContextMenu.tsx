import { useLayoutEffect, useRef, useState } from 'react';
import { useSessionVisibility } from './SessionVisibility';
import { createPortal } from 'react-dom';
import type { ReactNode } from 'react';

export type MenuAction = {
  label: string;
  icon: ReactNode;
  disabled?: boolean;
  danger?: boolean;
  separator?: boolean;
  run: () => void;
};

/** 使用视口坐标与 portal，避免文件列表的滚动/裁剪影响菜单。 */
export default function FileContextMenu({
  x,
  y,
  actions,
  onClose,
  label = '文件操作',
}: {
  x: number;
  y: number;
  actions: MenuAction[];
  onClose: () => void;
  label?: string;
}) {
  const visible = useSessionVisibility();
  const menu = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ x, y });
  useLayoutEffect(() => {
    if (!visible) {
      onClose();
      return;
    }
    const element = menu.current;
    if (!element) return;
    const previousFocus = document.activeElement;
    const rect = element.getBoundingClientRect();
    setPosition({
      x: Math.max(8, Math.min(x, window.innerWidth - rect.width - 8)),
      y: Math.max(8, Math.min(y, window.innerHeight - rect.height - 8)),
    });
    element.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
    const outside = (event: Event) => {
      if (!element.contains(event.target as Node)) onClose();
    };
    const dismiss = () => onClose();
    const scroll = (event: Event) => {
      if (!element.contains(event.target as Node)) onClose();
    };
    document.addEventListener('pointerdown', outside, true);
    document.addEventListener('contextmenu', outside, true);
    document.addEventListener('scroll', scroll, true);
    window.addEventListener('resize', dismiss);
    window.addEventListener('blur', dismiss);
    return () => {
      document.removeEventListener('pointerdown', outside, true);
      document.removeEventListener('contextmenu', outside, true);
      document.removeEventListener('scroll', scroll, true);
      window.removeEventListener('resize', dismiss);
      window.removeEventListener('blur', dismiss);
      // 只有焦点还在菜单时才恢复，不能抢走随后打开的确认弹窗的焦点。
      if (
        element.contains(document.activeElement) &&
        previousFocus instanceof HTMLElement &&
        previousFocus.isConnected &&
        !previousFocus.closest('.session-workspace[hidden]')
      )
        previousFocus.focus();
    };
  }, [x, y, onClose, visible]);
  if (!visible) return null;
  return createPortal(
    <div
      ref={menu}
      className="file-context-menu"
      role="menu"
      aria-label={label}
      style={{ left: position.x, top: position.y }}
      onContextMenu={(e) => e.preventDefault()}
      onKeyDown={(e) => {
        if (e.key === 'Escape' || e.key === 'Tab') {
          e.preventDefault();
          onClose();
          return;
        }
        if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return;
        e.preventDefault();
        const buttons = Array.from(
          menu.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [],
        );
        const current = buttons.findIndex((button) => button === document.activeElement);
        const next =
          e.key === 'Home'
            ? 0
            : e.key === 'End'
              ? buttons.length - 1
              : (current + (e.key === 'ArrowUp' ? -1 : 1) + buttons.length) % buttons.length;
        buttons[next]?.focus();
      }}
    >
      {actions.map((action) => (
        <div key={action.label}>
          {action.separator && <div role="separator" className="menu-separator" />}
          <button
            type="button"
            role="menuitem"
            className={action.danger ? 'danger' : ''}
            disabled={action.disabled}
            onClick={() => {
              onClose();
              action.run();
            }}
          >
            {action.icon}
            <span>{action.label}</span>
          </button>
        </div>
      ))}
    </div>,
    document.body,
  );
}
