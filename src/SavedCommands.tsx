/** 保存命令仅存草稿；每次执行都展示实际连接与完整命令，绝不在加载时运行。 */
import { useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useWorkspaceTabs } from './WorkspaceTabs';
import { message } from './types';
export default function SavedCommands({
  connectionId,
  target,
}: {
  connectionId: string | null;
  target: string;
}) {
  const { workbench } = useWorkspaceTabs();
  const [name, setName] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [command, setCommand] = useState('');
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const executing = useRef(false);
  const [result, setResult] = useState('');
  const [error, setError] = useState('');
  async function remove(id: string) {
    setBusy(true);
    setError('');
    try {
      await workbench.update((previous) => ({
        ...previous,
        commands: previous.commands.filter((entry) => entry.id !== id),
      }));
      if (selectedId === id) {
        setSelectedId(null);
        setName('');
        setCommand('');
        setConfirm(false);
      }
    } catch (cause) {
      setError(message(cause));
    } finally {
      setBusy(false);
    }
  }
  async function save() {
    setBusy(true);
    setError('');
    try {
      if (!name.trim() || !command.trim()) throw new Error('请填写名称和命令');
      await workbench.update((previous) => ({
        ...previous,
        commands: selectedId
          ? previous.commands.map((item) =>
              item.id === selectedId ? { ...item, name: name.trim(), command } : item,
            )
          : [...previous.commands, { id: crypto.randomUUID(), name: name.trim(), command }],
      }));
      setName('');
      setSelectedId(null);
      setCommand('');
      setConfirm(false);
    } catch (cause) {
      setError(message(cause));
    } finally {
      setBusy(false);
    }
  }
  async function execute() {
    if (!connectionId || executing.current || !confirm) return;
    executing.current = true;
    setBusy(true);
    setConfirm(false);
    setError('');
    setResult('');
    try {
      const reply = await invoke<{ output: string; exitCode: number | null; truncated: boolean }>(
        'workbench_command',
        { connectionId, command },
      );
      setResult(
        `${reply.output}\n\n退出码：${reply.exitCode ?? '未知'}${reply.truncated ? ' · 输出达到 64 KiB，已关闭通道，执行结果可能不完整' : ''}`,
      );
    } catch (cause) {
      setError(message(cause));
    } finally {
      executing.current = false;
      setBusy(false);
    }
  }
  return (
    <section className="saved-commands">
      <p>
        命令以当前账号执行；这里不提供交互输入，最长接收 15 秒、最多 64
        KiB。关闭面板不撤销已发命令，超时关闭通道也不能保证远端进程已停止，结果未知时不要盲目重试。请勿在保存的命令中填写密码、令牌或其他凭据。
      </p>
      <div className="saved-command-list">
        {workbench.value.commands.map((item) => (
          <div key={item.id}>
            <button
              disabled={busy}
              onClick={() => {
                setName(item.name);
                setSelectedId(item.id);
                setCommand(item.command);
                setConfirm(false);
                setResult('');
              }}
            >
              {item.name}
            </button>
            <button
              disabled={busy || !workbench.ready}
              aria-label={`删除命令 ${item.name}`}
              onClick={() => void remove(item.id)}
            >
              删除
            </button>
          </div>
        ))}
      </div>
      <label>
        命令名称
        <input
          value={name}
          maxLength={80}
          disabled={busy}
          onChange={(event) => setName(event.target.value)}
        />
      </label>
      <label>
        命令内容
        <textarea
          value={command}
          maxLength={8192}
          rows={5}
          spellCheck={false}
          disabled={busy}
          onChange={(event) => {
            setCommand(event.target.value);
            setConfirm(false);
          }}
        />
      </label>
      <div className="workbench-actions">
        <button
          disabled={
            busy || !workbench.ready || (!selectedId && workbench.value.commands.length >= 50)
          }
          onClick={() => void save()}
        >
          {selectedId ? '保存修改' : '保存为新命令'}
        </button>
        <button
          disabled={busy}
          onClick={() => {
            setSelectedId(null);
            setName('');
            setCommand('');
            setConfirm(false);
          }}
        >
          新建草稿
        </button>
        <button
          className="primary"
          disabled={busy || !connectionId || !command.trim()}
          onClick={() => setConfirm(true)}
        >
          预览执行
        </button>
      </div>
      {confirm && (
        <div className="command-confirm">
          <strong>执行目标：{target}</strong>
          <pre>{command}</pre>
          <button
            className="primary"
            disabled={busy || !connectionId}
            onClick={() => void execute()}
          >
            确认在此服务器执行
          </button>
          <button onClick={() => setConfirm(false)}>取消</button>
        </div>
      )}
      {busy && <p role="status">正在处理，请勿重复执行…</p>}
      {error && <p role="alert">{error}</p>}
      {result && (
        <pre className="command-result" tabIndex={0}>
          {result}
        </pre>
      )}
    </section>
  );
}
