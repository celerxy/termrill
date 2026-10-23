/** 单个串行轮询、流式解码与有界显示；隐藏或停止后不继续派发请求。 */
import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { PreviewTextDecoder } from './previewTextDecoder';
import { appendLog, filterLog, skipLogPrefix } from './logBuffer';
import { message } from './types';

type Cursor = {
  next: number;
  size: number;
  probe: number;
  head: string;
  modified: number | null;
  identity: string;
};
type Batch = {
  bytes: string;
  cursor: Cursor;
  reset: boolean;
  skippedPrefix: boolean;
  encoding: string;
};
export default function LogViewer({
  connectionId,
  initialPath,
  visible,
}: {
  connectionId: string;
  initialPath: string;
  visible: boolean;
}) {
  const [path, setPath] = useState(initialPath);
  const [local, setLocal] = useState(false);
  const [encoding, setEncoding] = useState('');
  const [config, setConfig] = useState<{ path: string; local: boolean; encoding: string } | null>(
    null,
  );
  const [following, setFollowing] = useState(false);
  const [scroll, setScroll] = useState(true);
  const [text, setText] = useState('');
  const [filter, setFilter] = useState('');
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('首次读取末尾 64 KiB；最多保留 2000 行、50 万字符');
  const state = useRef<{
    cursor: Cursor | null;
    decoder: PreviewTextDecoder | null;
    skip: boolean;
  }>({ cursor: null, decoder: null, skip: false });
  const output = useRef<HTMLPreElement>(null);
  const readTail = useRef<Promise<void>>(Promise.resolve());
  const filtered = useMemo(() => filterLog(text, filter), [text, filter]);
  useEffect(() => {
    if (scroll && output.current) output.current.scrollTop = output.current.scrollHeight;
  }, [filtered, scroll]);
  useEffect(() => {
    if (!config || !following || !visible) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function poll() {
      const previous = readTail.current;
      let release!: () => void;
      readTail.current = new Promise<void>((resolve) => {
        release = resolve;
      });
      await previous;
      if (!active) {
        release();
        return;
      }
      try {
        const result = await invoke<Batch>('read_log', {
          id: config!.local ? null : connectionId,
          path: config!.path,
          cursor: state.current.cursor,
        });
        if (!active) return;
        const fresh = !state.current.cursor || result.reset;
        if (fresh) {
          state.current.decoder = new PreviewTextDecoder(config!.encoding || result.encoding);
          state.current.skip = result.skippedPrefix;
          setText('');
          if (result.reset) setNotice('检测到截断或轮转，已从新日志末尾重新读取');
        }
        let bytes = Uint8Array.from(atob(result.bytes), (char) => char.charCodeAt(0));
        if (state.current.skip && bytes.length) {
          const trimmed = skipLogPrefix(bytes, config!.encoding || result.encoding);
          state.current.skip = trimmed === null;
          bytes = trimmed ?? new Uint8Array();
        }
        const chunk = state.current.decoder!.decode(bytes, false);
        setText((previous) => appendLog(previous, chunk));
        state.current.cursor = result.cursor;
        timer = setTimeout(() => void poll(), result.cursor.next < result.cursor.size ? 100 : 1000);
      } catch (e) {
        if (active) {
          setError(message(e));
          setFollowing(false);
        }
      } finally {
        release();
      }
    }
    void poll();
    return () => {
      active = false;
      if (timer) clearTimeout(timer);
    };
  }, [config, following, visible, connectionId]);
  function start() {
    state.current = { cursor: null, decoder: null, skip: false };
    setText('');
    setError('');
    setNotice('首次读取末尾 64 KiB；最多保留 2000 行、50 万字符');
    setConfig({ path: path.trim(), local, encoding });
    setFollowing(true);
  }
  return (
    <section className="log-viewer" aria-label="持续日志查看">
      <div className="log-controls">
        <select
          aria-label="日志来源"
          value={local ? 'local' : 'remote'}
          onChange={(e) => setLocal(e.target.value === 'local')}
        >
          <option value="remote">远程</option>
          <option value="local">本地</option>
        </select>
        <input
          aria-label="日志文件绝对路径"
          placeholder="日志文件绝对路径"
          value={path}
          onChange={(e) => setPath(e.target.value)}
        />
        <select
          aria-label="日志编码"
          value={encoding}
          onChange={(e) => setEncoding(e.target.value)}
        >
          <option value="">自动编码</option>
          <option value="UTF-8">UTF-8</option>
          <option value="GB18030">GB18030</option>
          <option value="UTF-16 LE">UTF-16 LE</option>
          <option value="UTF-16 BE">UTF-16 BE</option>
        </select>
        <button disabled={!path.trim()} onClick={start}>
          开始读取
        </button>
        <button
          disabled={!config}
          onClick={() => {
            setError('');
            setFollowing(!following);
          }}
        >
          {following ? '停止追踪' : '继续追踪'}
        </button>
      </div>
      <div className="log-controls">
        <input
          aria-label="过滤已读取日志"
          placeholder="过滤已读取内容"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <button aria-pressed={!scroll} onClick={() => setScroll(!scroll)}>
          {scroll ? '暂停滚动' : '恢复滚动'}
        </button>
        <button onClick={() => setText('')}>清空显示</button>
        <small role="status">
          {error || notice}
          {following ? ' · 追踪中' : ' · 已停止'}
        </small>
      </div>
      <pre ref={output} tabIndex={0}>
        {filtered || '尚无日志内容'}
      </pre>
    </section>
  );
}
