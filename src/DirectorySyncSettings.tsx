/** 规则只保存非敏感路径及比较选项，加载规则不会直接开始同步。 */
import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { SyncRule } from './directorySyncModel';
import { message } from './types';

export default function DirectorySyncSettings({
  scope,
  value,
  busy,
  onChange,
}: {
  scope: string;
  value: Omit<SyncRule, 'id' | 'name'>;
  busy: boolean;
  onChange: (next: Omit<SyncRule, 'id' | 'name'>) => void;
}) {
  const [rules, setRules] = useState<SyncRule[]>([]);
  const [selected, setSelected] = useState('');
  const [name, setName] = useState('');
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    let active = true;
    setSelected('');
    setName('');
    setRules([]);
    if (scope)
      void invoke<SyncRule[]>('sync_rules', { scope, rule: null, remove: null })
        .then((next) => {
          if (active) setRules(next);
        })
        .catch((e) => {
          if (active) setError(message(e));
        });
    return () => {
      active = false;
    };
  }, [scope]);
  async function save(remove = false) {
    if (!scope || saving) return;
    setSaving(true);
    setError('');
    try {
      const id = selected || crypto.randomUUID();
      const next = await invoke<SyncRule[]>('sync_rules', {
        scope,
        rule: remove
          ? null
          : {
              ...value,
              options: {
                ...value.options,
                exclude: value.options.exclude.map((item) => item.trim()).filter(Boolean),
              },
              id,
              name: name.trim(),
            },
        remove: remove ? id : null,
      });
      setRules(next);
      setSelected(remove ? '' : id);
      if (remove) setName('');
    } catch (e) {
      setError(message(e));
    } finally {
      setSaving(false);
    }
  }
  return (
    <fieldset className="sync-settings" disabled={busy || saving}>
      <legend>单向同步规则</legend>
      <div className="sync-rule-actions">
        <select
          aria-label="已保存的同步规则"
          value={selected}
          onChange={(event) => {
            const id = event.target.value;
            setSelected(id);
            const rule = rules.find((item) => item.id === id);
            setName(rule?.name ?? '');
            if (rule) onChange(rule);
          }}
        >
          <option value="">新规则</option>
          {rules.map((rule) => (
            <option key={rule.id} value={rule.id}>
              {rule.name}
            </option>
          ))}
        </select>
        <input
          aria-label="规则名称"
          placeholder="规则名称"
          maxLength={80}
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <button disabled={!scope || !name.trim()} onClick={() => void save()}>
          保存规则
        </button>
        <button disabled={!selected} onClick={() => void save(true)}>
          删除规则
        </button>
      </div>
      <div className="sync-rule-paths">
        <label>
          本地目录
          <input
            value={value.local}
            onChange={(e) => onChange({ ...value, local: e.target.value })}
          />
        </label>
        <label>
          远程目录
          <input
            value={value.remote}
            onChange={(e) => onChange({ ...value, remote: e.target.value })}
          />
        </label>
      </div>
      <label>
        比较模式
        <select
          value={value.options.quick ? 'quick' : 'strict'}
          onChange={(e) =>
            onChange({ ...value, options: { ...value.options, quick: e.target.value === 'quick' } })
          }
        >
          <option value="strict">严格 · SHA256 内容校验</option>
          <option value="quick">快速 · 大小与修改时间（不能保证内容一致）</option>
        </select>
      </label>
      <label>
        排除项
        <textarea
          rows={2}
          placeholder={'每行一项，例如 .git、*.log、build/**/cache'}
          value={value.options.exclude.join('\n')}
          onChange={(e) =>
            onChange({
              ...value,
              options: { ...value.options, exclude: e.target.value.split('\n') },
            })
          }
        />
      </label>
      <small>
        相对路径，区分大小写；无斜杠的模式匹配任意层级名称，匹配目录时跳过其子树。最多 32 条。
      </small>
      {error && <p role="alert">{error}</p>}
    </fieldset>
  );
}
