/** 标签与分屏仅改变可见性；已挂载终端保留通道，关闭时各自清理。 */
import { useEffect, useState } from 'react';
import { Circle } from 'lucide-react';
import TerminalToolbar from './TerminalToolbar';
import TerminalSession, { type TerminalState } from './TerminalSession';
import LogViewer from './LogViewer';
import './terminalWorkspace.css';

export default function TerminalPanel({
  connectionId,
  height,
  onClose,
  initialLogPath = '',
}: {
  connectionId: string;
  height?: number;
  onClose: () => void;
  initialLogPath?: string;
}) {
  const [tabs, setTabs] = useState(() => [{ id: crypto.randomUUID(), name: '终端 1' }]);
  const [active, setActive] = useState('');
  const [split, setSplit] = useState(false);
  const [log, setLog] = useState(false);
  const [serial, setSerial] = useState(1);
  const [expanded, setExpanded] = useState(false);
  const [searchId, setSearchId] = useState<string | null>(null);
  const [states, setStates] = useState<Record<string, TerminalState>>({});
  const selected = tabs.some((tab) => tab.id === active) ? active : tabs[0]?.id;
  const other = tabs.find((tab) => tab.id !== selected)?.id;
  function add() {
    if (tabs.length >= 4) return;
    const id = crypto.randomUUID();
    setTabs((previous) =>
      previous.length >= 4 ? previous : [...previous, { id, name: `终端 ${serial + 1}` }],
    );
    setSerial(serial + 1);
    setActive(id);
    setLog(false);
  }
  function close(id: string) {
    setTabs((previous) => previous.filter((tab) => tab.id !== id));
    setStates((previous) => {
      const next = { ...previous };
      delete next[id];
      return next;
    });
  }
  useEffect(() => {
    if (!tabs.length) {
      setLog(true);
      setSplit(false);
    }
  }, [tabs.length]);
  return (
    <section className="terminal-panel terminal-workspace">
      <TerminalToolbar
        tabs={tabs}
        active={selected}
        states={states}
        log={log}
        split={split}
        expanded={expanded}
        searchOpen={searchId === selected}
        onSelect={(id) => {
          setActive(id);
          setLog(false);
        }}
        onAdd={add}
        onRemove={close}
        onLog={() => setLog(!log)}
        onSearch={() => setSearchId(searchId === selected ? null : (selected ?? null))}
        onSplit={() => setSplit(!split)}
        onExpand={() => setExpanded(!expanded)}
        onClose={onClose}
      />
      <div
        className="terminal-workspace-body"
        style={{
          height: expanded ? 'min(560px, 65vh)' : `min(${Math.max(130, height ?? 240)}px, 40vh)`,
        }}
      >
        <div className={`terminal-grid ${split && other ? 'split' : ''}`} hidden={log}>
          {tabs.map((tab) => (
            <div
              key={tab.id}
              className={`terminal-cell ${tab.id === selected ? 'is-active' : ''}`}
              hidden={tab.id !== selected && (!split || tab.id !== other)}
            >
              <TerminalSession
                connectionId={connectionId}
                onClose={() => close(tab.id)}
                searchOpen={searchId === tab.id && !log}
                onSearchClose={() => setSearchId(null)}
                onSearchRequest={() => {
                  setActive(tab.id);
                  setSearchId(tab.id);
                }}
                onActivate={() => setActive(tab.id)}
                onState={(state) =>
                  setStates((previous) => {
                    const old = previous[tab.id];
                    if (
                      old?.phase === state.phase &&
                      old.cols === state.cols &&
                      old.rows === state.rows
                    )
                      return previous;
                    return { ...previous, [tab.id]: state };
                  })
                }
              />
            </div>
          ))}
        </div>
        <div className="terminal-log-cell" hidden={!log}>
          <LogViewer connectionId={connectionId} initialPath={initialLogPath} visible={log} />
        </div>
      </div>
      <footer className="terminal-status">
        <span>
          <Circle
            size={8}
            strokeWidth={0}
            fill="currentColor"
            className={`terminal-state-dot ${log ? 'starting' : (states[selected ?? '']?.phase ?? 'starting')}`}
          />
          {log
            ? '只读日志'
            : { starting: '正在连接…', connected: '已连接', closed: '已关闭', error: '连接失败' }[
                states[selected ?? '']?.phase ?? 'starting'
              ]}
        </span>
        <span>
          {log
            ? '有界缓存 · 仅显示已读取内容'
            : `UTF-8${states[selected ?? ''] ? ` · ${states[selected ?? ''].cols} × ${states[selected ?? ''].rows}` : ''}`}
        </span>
      </footer>
    </section>
  );
}
