/** 导入只追加新连接，不自动连接；导出仅包含连接元数据和分组。 */
import { useState } from 'react';
import { exportConnections, importConnections } from './connectionBundle';
import { useWorkspaceTabs } from './WorkspaceTabs';
import { message } from './types';
export default function ConnectionExchange() {
  const tabs = useWorkspaceTabs();
  const [preview, setPreview] = useState<ReturnType<typeof importConnections>>([]);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState('');
  const [exportPreview, setExportPreview] = useState('');
  function download() {
    const url = URL.createObjectURL(new Blob([exportPreview], { type: 'application/json' }));
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = 'termrill-connections.json';
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  async function importFile(file: File | undefined) {
    setError('');
    setPreview([]);
    setDone('');
    if (!file) return;
    try {
      if (file.size > 1024 * 1024) throw new Error('文件超过 1 MiB');
      setPreview(importConnections(await file.text()));
    } catch (cause) {
      setError(message(cause));
    }
  }
  async function merge() {
    setBusy(true);
    setError('');
    try {
      await tabs.importProfiles(preview.map((item) => item.profile));
      // 连接已成功落盘即清除预览，分组保存失败也不能再次追加相同批次。
      const imported = preview;
      setPreview([]);
      setDone(`已追加 ${imported.length} 个连接；没有自动连接，也没有覆盖现有连接。`);
      await tabs.workbench.update((previous) => ({
        ...previous,
        groups: {
          ...previous.groups,
          ...Object.fromEntries(
            imported.filter((item) => item.group).map((item) => [item.profile.id, item.group]),
          ),
        },
      }));
    } catch (cause) {
      setError(message(cause));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section>
      <p>
        仅交换名称、地址、账号、认证方式、私钥路径和分组。不会导出密码、私钥内容、系统凭据或主机信任；私钥路径可能包含本地用户名，下载前请检查预览。常用命令不在导出范围。
      </p>
      <div className="workbench-actions">
        <button
          disabled={busy}
          onClick={() =>
            setExportPreview(exportConnections(tabs.profiles, tabs.workbench.value.groups))
          }
        >
          预览导出 {tabs.profiles.length} 个连接
        </button>
        <label className="import-file">
          选择导入 JSON
          <input
            type="file"
            accept=".json,application/json"
            disabled={busy || !tabs.workbench.ready}
            onChange={(event) => {
              void importFile(event.target.files?.[0]);
              event.target.value = '';
            }}
          />
        </label>
      </div>
      {exportPreview && (
        <div>
          <pre className="exchange-preview" tabIndex={0}>
            {exportPreview}
          </pre>
          <button className="primary" onClick={download}>
            下载此配置
          </button>
          <button onClick={() => setExportPreview('')}>收起</button>
        </div>
      )}
      {preview.length > 0 && (
        <div>
          <h3>导入预览 · {preview.length} 个连接</h3>
          <p>
            将追加新配置，同名配置也保留独立副本。私钥路径需在当前设备重新核对，首次连接仍需确认主机指纹。
          </p>
          <ul>
            {preview.map(({ profile, group }) => (
              <li key={profile.id}>
                {profile.name} · {profile.target.username}@{profile.target.host}:
                {profile.target.port}
                {profile.jump ? ` · 经 ${profile.jump.host}` : ''}
                {group ? ` · 分组 ${group}` : ''}
                {profile.target.auth === 'key' ? ` · 私钥路径 ${profile.target.keyPath}` : ''}
              </li>
            ))}
          </ul>
          <button
            className="primary"
            disabled={busy || !tabs.workbench.ready}
            onClick={() => void merge()}
          >
            确认追加
          </button>
          <button disabled={busy} onClick={() => setPreview([])}>
            取消
          </button>
        </div>
      )}
      {error && <p role="alert">{error}</p>}
      {done && <p role="status">{done}</p>}
    </section>
  );
}
