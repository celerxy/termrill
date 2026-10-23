import { useEffect, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
type Challenge = {
  id: string;
  address: string;
  instructions: string;
  prompts: { text: string; echo: boolean }[];
};

/** 独立于连接表单监听多轮认证；后端到期事件移除已失效的提示。 */
export default function AuthPrompts() {
  const [queue, setQueue] = useState<Challenge[]>([]);
  const [answers, setAnswers] = useState<Record<string, string[]>>({});
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    const stops: (() => void)[] = [];
    void (async () => {
      for (const [event, handler] of [
        [
          'auth-challenge',
          (payload: Challenge) => {
            setQueue((q) => [...q, payload]);
            setError('');
          },
        ],
        [
          'auth-challenge-closed',
          (id: string) => {
            setQueue((q) => q.filter((c) => c.id !== id));
            setAnswers((previous) => {
              const next = { ...previous };
              delete next[id];
              return next;
            });
          },
        ],
      ] as const) {
        const stop = await listen(event, ({ payload }) => {
          if (!disposed) (handler as (value: unknown) => void)(payload);
        });
        if (disposed) stop();
        else stops.push(stop);
      }
    })().catch((e) => {
      if (!disposed) setError(String(e));
    });
    return () => {
      disposed = true;
      stops.forEach((stop) => stop());
    };
  }, []);
  const challenge = queue[0];
  if (!challenge) return null;
  const send = async (cancel: boolean) => {
    setBusy(true);
    try {
      await invoke('answer_auth', {
        id: challenge.id,
        answers: cancel ? null : challenge.prompts.map((_, i) => answers[challenge.id]?.[i] ?? ''),
      });
      setQueue((q) => q.filter((c) => c.id !== challenge.id));
      setAnswers((previous) => {
        const next = { ...previous };
        delete next[challenge.id];
        return next;
      });
    } catch (e) {
      setError(String((e as { message?: string }).message ?? e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="modal-backdrop auth-backdrop">
      <form
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label="交互式认证"
        onSubmit={(e) => {
          e.preventDefault();
          void send(false);
        }}
      >
        <h2>交互式认证</h2>
        <p>{challenge.address}</p>
        <p>{challenge.instructions}</p>
        {challenge.prompts.map((prompt, index) => (
          <label key={`${challenge.id}-${index}`}>
            {prompt.text}
            <input
              autoFocus={index === 0}
              autoComplete="off"
              type={prompt.echo ? 'text' : 'password'}
              disabled={busy}
              value={answers[challenge.id]?.[index] ?? ''}
              onChange={(e) =>
                setAnswers((previous) => {
                  const values = [...(previous[challenge.id] ?? [])];
                  values[index] = e.target.value;
                  return { ...previous, [challenge.id]: values };
                })
              }
            />
          </label>
        ))}
        {error && (
          <p role="alert" className="inline-error">
            {error}
          </p>
        )}
        <div className="modal-actions">
          <button type="button" disabled={busy} onClick={() => void send(true)}>
            取消认证
          </button>
          <button className="primary" disabled={busy} type="submit">
            提交
          </button>
        </div>
        <p>提示 120 秒后过期，回答不保存到配置或凭据库。</p>
      </form>
    </div>
  );
}
