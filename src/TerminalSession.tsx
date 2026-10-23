import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { X, Search, ChevronUp, ChevronDown } from 'lucide-react';
import { message } from './types';
import '@xterm/xterm/css/xterm.css';
import { terminalMatches } from './terminalSearch';
import { terminalTheme } from './terminalTheme';
export type TerminalState = {
  phase: 'starting' | 'connected' | 'closed' | 'error';
  cols: number;
  rows: number;
};
export default function TerminalSession({
  connectionId,
  onClose,
  searchOpen,
  onSearchClose,
  onSearchRequest,
  onActivate,
  onState,
}: {
  connectionId: string;
  onClose: () => void;
  searchOpen: boolean;
  onSearchClose: () => void;
  onSearchRequest: () => void;
  onActivate: () => void;
  onState: (state: TerminalState) => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const terminal = useRef<Terminal | null>(null);
  // 主题更新直接重绘已挂载的终端，不需要断开 SSH 或丢失当前输出。
  useEffect(() => {
    const term = terminal.current;
    if (!term) return;
    term.options.theme = terminalTheme;
    term.refresh(0, term.rows - 1);
  }, [terminalTheme]);
  const searchInput = useRef<HTMLInputElement>(null);
  const stateCallback = useRef(onState);
  stateCallback.current = onState;
  useEffect(() => {
    if (searchOpen) searchInput.current?.focus();
  }, [searchOpen]);
  function closeSearch() {
    onSearchClose();
    terminal.current?.clearSelection();
    terminal.current?.focus();
  }
  const [query, setQuery] = useState('');
  const [result, setResult] = useState('');
  const matchIndex = useRef(-1);
  function search(back = false) {
    const term = terminal.current;
    if (!term || !query) return;
    const matches = terminalMatches(term.buffer.active, query);
    if (!matches.length) {
      setResult('未找到（最多检索 200 万字符）');
      term.clearSelection();
      return;
    }
    matchIndex.current =
      matchIndex.current < 0
        ? back
          ? matches.length - 1
          : 0
        : (matchIndex.current + (back ? -1 : 1) + matches.length) % matches.length;
    const match = matches[matchIndex.current];
    term.select(match.column, match.row, match.length);
    term.scrollToLine(Math.max(0, match.row - 2));
    setResult(`${matchIndex.current + 1} / ${matches.length}`);
  }
  const closePanel = useRef(onClose);
  closePanel.current = onClose;
  useEffect(() => {
    const id = crypto.randomUUID();
    let disposed = false;
    let started = false;
    let closed = false;
    let unlisten: (() => void) | undefined;
    const term = new Terminal({
      cursorBlink: true,
      fontSize: 14,
      lineHeight: 1.35,
      fontFamily: 'Cascadia Code, Consolas, monospace',
      scrollback: 5000,
      minimumContrastRatio: 4.5,
      theme: terminalTheme,
    });
    terminal.current = term;
    let phase: TerminalState['phase'] = 'starting';
    const report = (next = phase) => {
      phase = next;
      if (!disposed) stateCallback.current({ phase, cols: term.cols, rows: term.rows });
    };
    report();
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(container.current!);
    const fitVisibleArea = () => {
      const host = container.current;
      if (disposed || !host?.clientWidth || !host.clientHeight) return;
      fit.fit();
      // FitAddon 读取 CSS 高度；再以实际渲染高度校正，避免缩放和像素取整裁掉最后一行。
      const screen = host.querySelector<HTMLElement>('.xterm-screen');
      const screenHeight = screen?.getBoundingClientRect().height ?? 0;
      if (screenHeight > 0) {
        const rowHeight = screenHeight / term.rows;
        const visibleRows = Math.max(1, Math.floor((host.clientHeight - 2) / rowHeight));
        if (visibleRows < term.rows) term.resize(term.cols, visibleRows);
      }
      report();
      if (started && !closed)
        void invoke('resize_terminal', { id, cols: term.cols, rows: term.rows }).catch(() => {});
    };
    let fitFrame = 0;
    const scheduleFit = () => {
      cancelAnimationFrame(fitFrame);
      fitFrame = requestAnimationFrame(fitVisibleArea);
    };
    fitVisibleArea();
    term.writeln('\x1b[90m正在建立 SSH 终端…\x1b[0m');
    const input = term.onData((data) => {
      if (started && !closed)
        void invoke('terminal_input', { id, data }).catch((e) => term.writeln(`\r\n${message(e)}`));
    });
    const observer = new ResizeObserver(scheduleFit);
    observer.observe(container.current!);
    window.addEventListener('resize', scheduleFit);
    document.fonts.addEventListener('loadingdone', scheduleFit);
    void document.fonts.ready.then(() => {
      if (!disposed) scheduleFit();
    });
    void (async () => {
      try {
        // 先订阅输出，再启动 PTY，避免遗漏服务器启动时立即输出的数据。
        const stop = await listen<{
          id: string;
          data: number[];
          closed: boolean;
          error: string | null;
        }>('terminal-output', ({ payload }) => {
          if (payload.id !== id || disposed) return;
          // 直接交给 xterm 字节流，避免 UTF-8 字符跨事件分片时被提前解码损坏。
          if (payload.data.length) {
            term.write(new Uint8Array(payload.data), () => {
              if (!disposed)
                void invoke('terminal_ack', { id, bytes: payload.data.length }).catch(() => {});
            });
          }
          if (payload.closed) {
            closed = true;
            report(payload.error ? 'error' : 'closed');
            // 正常退出 shell 后收起面板；异常断开保留错误，方便用户查看原因。
            if (!payload.error) {
              closePanel.current();
              return;
            }
            term.writeln(
              `\r\n\x1b[90m终端已关闭${payload.error ? `：${payload.error}` : ''}\x1b[0m`,
            );
          }
        });
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
        await invoke('start_terminal', { id, connectionId, cols: term.cols, rows: term.rows });
        started = true;
        report('connected');
        // React 卸载可能早于后端启动完成；迟到的会话也必须关闭。
        if (disposed) {
          await invoke('close_terminal', { id });
          return;
        }
        if (container.current?.clientWidth && container.current?.clientHeight) {
          // 建立 PTY 期间布局可能已变化，再同步一次，避免服务器仍按旧行数绘制提示符。
          fitVisibleArea();
          scheduleFit();
          if (!disposed) term.focus();
        }
      } catch (e) {
        if (!disposed) {
          report('error');
          term.writeln(`\r\n\x1b[31m${message(e)}\x1b[0m`);
        }
      }
    })();
    return () => {
      disposed = true;
      cancelAnimationFrame(fitFrame);
      observer.disconnect();
      window.removeEventListener('resize', scheduleFit);
      document.fonts.removeEventListener('loadingdone', scheduleFit);
      input.dispose();
      unlisten?.();
      void invoke('close_terminal', { id }).catch(() => {});
      terminal.current = null;
      term.dispose();
    };
  }, [connectionId]);
  return (
    <section
      className="terminal-session"
      onPointerDown={onActivate}
      onFocusCapture={onActivate}
      onKeyDownCapture={(event) => {
        if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'f') {
          event.preventDefault();
          event.stopPropagation();
          onSearchRequest();
        }
      }}
    >
      {searchOpen && (
        <div
          className="terminal-search"
          role="search"
          aria-label="终端内搜索"
          onKeyDown={(event) => {
            if (event.key === 'Escape') {
              event.stopPropagation();
              closeSearch();
            }
          }}
        >
          <Search size={14} />
          <input
            ref={searchInput}
            aria-label="搜索终端缓存"
            placeholder="搜索（区分大小写）"
            maxLength={256}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              matchIndex.current = -1;
              setResult('');
            }}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                search(event.shiftKey);
              }
            }}
          />
          <span role="status" title="最多搜索 200 万字符及 10000 个匹配">
            {result}
          </span>
          <button
            aria-label="上一处匹配"
            title="上一处 · Shift+Enter"
            disabled={!query}
            onClick={() => search(true)}
          >
            <ChevronUp size={15} />
          </button>
          <button
            aria-label="下一处匹配"
            title="下一处 · Enter"
            disabled={!query}
            onClick={() => search()}
          >
            <ChevronDown size={15} />
          </button>
          <button aria-label="关闭终端搜索" title="关闭搜索 · Esc" onClick={closeSearch}>
            <X size={15} />
          </button>
        </div>
      )}
      <div className="terminal-host" ref={container} />
    </section>
  );
}
