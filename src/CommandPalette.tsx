/** 只调用现有界面动作；不解析或执行远程命令，终端和编辑器输入不被全局快捷键抢占。 */
import { useEffect, useId, useRef, useState } from 'react';
import { Search, X } from 'lucide-react';
import { isTauri } from '@tauri-apps/api/core';
import type { Workspace } from './useTermRillWorkspace';
import './commandPalette.css';

export default function CommandPalette({ workspace }: { workspace: Workspace }) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const dialog = useRef<HTMLElement>(null);
  const returnFocus = useRef<HTMLElement | null>(null);
  const listId = useId();
  const native = isTauri();
  const modalOpen = !!(
    workspace.dialog ||
    workspace.trust ||
    workspace.settings ||
    workspace.syncOpen ||
    workspace.plans ||
    workspace.operation ||
    workspace.deleteProfile
  );
  const focusSearch = (remote: boolean) => {
    document
      .querySelector<HTMLInputElement>(
        `.session-workspace:not([hidden]) [data-remote="${remote}"] input[aria-label="搜索当前目录"]`,
      )
      ?.focus();
  };
  const actions = [
    {
      id: 'local-search',
      label: '搜索本地目录',
      hint: 'Ctrl+Shift+L',
      run: () => focusSearch(false),
    },
    {
      id: 'remote-search',
      label: '搜索远程目录',
      hint: 'Ctrl+Shift+R',
      disabled: !workspace.connectionId,
      run: () => focusSearch(true),
    },
    {
      id: 'local-refresh',
      label: '刷新本地目录',
      disabled: !native || workspace.localBusy,
      run: () => void workspace.loadLocal(workspace.local?.path),
    },
    {
      id: 'remote-refresh',
      label: '刷新远程目录',
      disabled: !workspace.connectionId || !workspace.remote,
      run: () => {
        if (workspace.remote) void workspace.loadRemote(workspace.remote.path);
      },
    },
    {
      id: 'connect',
      label: '打开连接配置',
      disabled: !native || workspace.connecting || workspace.running > 0,
      run: () => workspace.setDialog(workspace.profile ? { profile: workspace.profile } : {}),
    },
    {
      id: 'upload',
      label: '选择文件上传',
      disabled: !workspace.connectionId || !workspace.listenersReady,
      run: () => void workspace.chooseUpload('files'),
    },
    {
      id: 'sync',
      label: '比较与同步目录',
      disabled:
        !workspace.connectionId ||
        !workspace.local ||
        !workspace.remote ||
        !workspace.listenersReady,
      run: () => workspace.setSyncOpen(true),
    },
    {
      id: 'terminal',
      label: workspace.terminal ? '关闭 SSH 终端' : '打开 SSH 终端',
      disabled: !workspace.connectionId,
      run: () => workspace.setTerminal(!workspace.terminal),
    },
    {
      id: 'queue',
      label: workspace.queueOpen ? '收起传输队列' : '展开传输队列',
      run: () => workspace.setQueueOpen(!workspace.queueOpen),
    },
    { id: 'layout', label: '恢复默认面板与终端尺寸', run: workspace.workspaceLayout.reset },
    {
      id: 'preferences',
      label: '打开偏好设置',
      disabled: !native,
      run: () => void workspace.openSettings(),
    },
  ];
  const matches = actions.filter((action) =>
    action.label.toLowerCase().includes(query.trim().toLowerCase()),
  );
  const selected = Math.min(active, Math.max(0, matches.length - 1));
  const close = () => {
    setOpen(false);
    returnFocus.current?.focus();
  };
  useEffect(() => {
    const shortcuts = (event: KeyboardEvent) => {
      if (
        !workspace.visible ||
        modalOpen ||
        open ||
        event.defaultPrevented ||
        event.isComposing ||
        event.altKey
      )
        return;
      const target = event.target as HTMLElement | null;
      if (target?.closest('input,textarea,[contenteditable="true"],.xterm,[role="dialog"]')) return;
      if (!(event.ctrlKey || event.metaKey) || !event.shiftKey) return;
      const key = event.key.toLowerCase();
      if (key === 'p') {
        event.preventDefault();
        returnFocus.current = target;
        setQuery('');
        setActive(0);
        setOpen(true);
      } else if (key === 'l' || (key === 'r' && workspace.connectionId)) {
        event.preventDefault();
        focusSearch(key === 'r');
      }
    };
    document.addEventListener('keydown', shortcuts);
    return () => document.removeEventListener('keydown', shortcuts);
  });
  useEffect(() => {
    if (open) input.current?.focus();
  }, [open]);
  useEffect(() => {
    if (!workspace.visible || modalOpen) setOpen(false);
  }, [workspace.visible, modalOpen]);
  function choose(index: number) {
    const action = matches[index];
    if (!action || action.disabled) return;
    close();
    action.run();
  }
  return (
    <>
      <button
        type="button"
        className="command-palette-trigger"
        title="命令面板（Ctrl+Shift+P）"
        disabled={modalOpen}
        onClick={(event) => {
          returnFocus.current = event.currentTarget;
          setQuery('');
          setActive(0);
          setOpen(true);
        }}
      >
        <Search size={16} />
        <span>命令</span>
      </button>
      {open && (
        <div
          className="modal-backdrop"
          onPointerDown={(event) => {
            if (event.target === event.currentTarget) close();
          }}
        >
          <section
            className="modal command-palette"
            ref={dialog}
            role="dialog"
            aria-modal="true"
            aria-label="命令面板"
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                event.preventDefault();
                event.stopPropagation();
                close();
              } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
                event.preventDefault();
                setActive(
                  (selected + (event.key === 'ArrowDown' ? 1 : -1) + matches.length) %
                    Math.max(1, matches.length),
                );
              } else if (
                event.key === 'Enter' &&
                !(event.target as HTMLElement).closest('.command-palette-search button')
              ) {
                event.preventDefault();
                choose(selected);
              } else if (event.key === 'Tab') {
                const nodes = dialog.current?.querySelectorAll<HTMLElement>(
                  'input,button:not(:disabled)',
                );
                if (!nodes?.length) return;
                const first = nodes[0],
                  last = nodes[nodes.length - 1];
                if (event.shiftKey && document.activeElement === first) {
                  event.preventDefault();
                  last.focus();
                } else if (!event.shiftKey && document.activeElement === last) {
                  event.preventDefault();
                  first.focus();
                }
              }
            }}
          >
            <div className="command-palette-search">
              <Search size={18} />
              <input
                ref={input}
                aria-label="搜索命令"
                role="combobox"
                aria-expanded="true"
                aria-controls={listId}
                aria-activedescendant={matches.length ? `${listId}-${selected}` : undefined}
                value={query}
                placeholder="输入操作名称…"
                onChange={(event) => {
                  setQuery(event.target.value);
                  setActive(0);
                }}
              />
              <button aria-label="关闭命令面板" onClick={close}>
                <X size={17} />
              </button>
            </div>
            <div id={listId} role="listbox" aria-label="可用操作" className="command-palette-list">
              {matches.map((action, index) => (
                <button
                  key={action.id}
                  id={`${listId}-${index}`}
                  role="option"
                  aria-selected={selected === index}
                  disabled={action.disabled}
                  onPointerMove={() => setActive(index)}
                  onClick={() => choose(index)}
                >
                  <span>{action.label}</span>
                  <kbd>{action.hint ?? 'Enter'}</kbd>
                </button>
              ))}
              {matches.length === 0 && <p>没有匹配的操作</p>}
            </div>
            <footer>↑ ↓ 选择 · Enter 执行 · Esc 关闭；终端与编辑器内不接管快捷键</footer>
          </section>
        </div>
      )}
    </>
  );
}
