import { useState } from 'react';
import { useWorkspaceTabs } from './WorkspaceTabs';
import { message } from './types';
/** 分组仅改变连接列表组织，不修改活动会话身份。 */
export default function ConnectionGroups() {
  const { profiles, workbench } = useWorkspaceTabs();
  const [draft, setDraft] = useState({ ...workbench.value.groups });
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState('');
  async function save() {
    setBusy(true);
    setStatus('');
    try {
      const groups = Object.fromEntries(
        profiles
          .map((profile) => [profile.id, (draft[profile.id] ?? '').trim()])
          .filter(([, value]) => value),
      );
      await workbench.update((previous) => ({ ...previous, groups }));
      setStatus('分组已保存');
    } catch (cause) {
      setStatus(message(cause));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section>
      <p>相同组名归为一组，留空表示未分组。侧栏可筛选分组；不会断开已有连接。</p>
      <div className="connection-groups">
        {profiles.map((profile) => (
          <label key={profile.id}>
            <span>
              {profile.name}
              <small>
                {profile.target.username}@{profile.target.host}
              </small>
            </span>
            <input
              aria-label={`${profile.name} 的分组`}
              maxLength={40}
              value={draft[profile.id] ?? ''}
              disabled={busy}
              placeholder="未分组"
              onChange={(event) => setDraft({ ...draft, [profile.id]: event.target.value })}
            />
          </label>
        ))}
      </div>
      <button className="primary" disabled={busy || !workbench.ready} onClick={() => void save()}>
        保存分组
      </button>
      {status && <p role="status">{status}</p>}
    </section>
  );
}
