/** 两端各至多 2 MiB 的只读文本；沿用后端普通文件/编码检查，不写入目标。 */
import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { textDifference, type DiffLine } from './directorySyncModel';
import { joinPath, message } from './types';

export default function DirectorySyncDiff({
  local,
  remote,
  relative,
  connectionId,
  onClose,
}: {
  local: string;
  remote: string;
  relative: string;
  connectionId: string;
  onClose: () => void;
}) {
  const [lines, setLines] = useState<DiffLine[] | null>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    let active = true;
    setLines(null);
    setError('');
    void Promise.all([
      invoke<{ content: string }>('open_local_text', { path: joinPath(local, relative) }),
      invoke<{ content: string }>('open_remote_text', { id: connectionId, path: remote }),
    ])
      .then(([left, right]) => {
        if (active) setLines(textDifference(left.content, right.content));
      })
      .catch((e) => {
        if (active) setError(message(e));
      });
    return () => {
      active = false;
    };
  }, [local, remote, relative, connectionId]);
  return (
    <section className="sync-diff" aria-label="文本差异">
      <header>
        <strong>{relative}</strong>
        <button onClick={onClose}>关闭差异</button>
      </header>
      <p>− 本地　+ 远程；仅为当前读取快照，比较后文件可能变化。</p>
      {error ? (
        <p role="alert">{error}</p>
      ) : !lines ? (
        <p role="status">正在读取两端文本…</p>
      ) : (
        <pre>
          {lines.map((line, index) => (
            <span key={index} className={`diff-${line.kind}`}>
              {line.kind === 'remove' ? '−' : line.kind === 'add' ? '+' : ' '} {line.text}
              {'\n'}
            </span>
          ))}
        </pre>
      )}
    </section>
  );
}
