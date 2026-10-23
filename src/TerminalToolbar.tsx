/** 紧凑工具栏；标签固定靠左，搜索与布局工具独立分组，不继承通用按钮的 auto 外边距。 */
import {
  Plus,
  Columns2,
  FileText,
  X,
  Search,
  Maximize2,
  Minimize2,
  SquareTerminal,
  Circle,
} from 'lucide-react';
import type { TerminalState } from './TerminalSession';
export type TerminalTab = { id: string; name: string };
export default function TerminalToolbar({
  tabs,
  active,
  states,
  log,
  split,
  expanded,
  searchOpen,
  onSelect,
  onAdd,
  onRemove,
  onLog,
  onSearch,
  onSplit,
  onExpand,
  onClose,
}: {
  tabs: TerminalTab[];
  active?: string;
  states: Record<string, TerminalState>;
  log: boolean;
  split: boolean;
  expanded: boolean;
  searchOpen: boolean;
  onSelect: (id: string) => void;
  onAdd: () => void;
  onRemove: (id: string) => void;
  onLog: () => void;
  onSearch: () => void;
  onSplit: () => void;
  onExpand: () => void;
  onClose: () => void;
}) {
  return (
    <header className="terminal-heading">
      <div className="terminal-brand">
        <SquareTerminal size={18} />
        <strong>SSH 终端</strong>
      </div>
      <div className="terminal-tabs" role="tablist" aria-label="SSH 终端会话">
        {tabs.map((tab) => (
          <div
            className={`terminal-tab ${!log && active === tab.id ? 'is-active' : ''}`}
            key={tab.id}
          >
            <button
              role="tab"
              aria-selected={!log && active === tab.id}
              onClick={() => onSelect(tab.id)}
              title={tab.name}
            >
              <Circle
                size={7}
                fill="currentColor"
                strokeWidth={0}
                className={`terminal-state-dot ${states[tab.id]?.phase ?? 'starting'}`}
              />
              {tab.name}
            </button>
            <button
              className="terminal-tab-close"
              aria-label={`关闭 ${tab.name}`}
              title={`关闭 ${tab.name}`}
              onClick={() => onRemove(tab.id)}
            >
              <X size={12} />
            </button>
          </div>
        ))}
      </div>
      <button
        className="terminal-new"
        aria-label="新建终端"
        title="新建终端（每工作区最多 4 个）"
        disabled={tabs.length >= 4}
        onClick={onAdd}
      >
        <Plus size={17} />
      </button>
      <div className="terminal-tools">
        <button aria-label="查看日志" title="查看日志" aria-pressed={log} onClick={onLog}>
          <FileText size={15} />
        </button>
        <button
          aria-label="搜索终端"
          title="搜索终端 · Ctrl/Cmd+F"
          aria-pressed={searchOpen && !log}
          disabled={log || !tabs.length}
          onClick={onSearch}
        >
          <Search size={17} />
        </button>
        <button
          aria-label="切换终端分屏"
          title="切换分屏"
          aria-pressed={split && !log}
          disabled={tabs.length < 2 || log}
          onClick={onSplit}
        >
          <Columns2 size={17} />
        </button>
        <button
          aria-label={expanded ? '还原终端高度' : '展开终端'}
          title={expanded ? '还原高度' : '展开终端'}
          aria-pressed={expanded}
          onClick={onExpand}
        >
          {expanded ? <Minimize2 size={17} /> : <Maximize2 size={17} />}
        </button>
        <button aria-label="关闭终端工作台" title="关闭终端工作台" onClick={onClose}>
          <X size={18} />
        </button>
      </div>
    </header>
  );
}
