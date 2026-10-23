import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ArrowDownUp, Info } from 'lucide-react';

type Settings = { concurrency: number; bytesPerSecond: number };

/** 传输参数作用于新任务；所有并发线程共享同一任务的限速预算。 */
export default function TransferSettingsForm({ notify }: { notify: (error: unknown) => void }) {
  const [settings, setSettings] = useState<Settings>({
    concurrency: 3,
    bytesPerSecond: 0,
  });
  const [busy, setBusy] = useState(true);
  const [saved, setSaved] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [loadFailed, setLoadFailed] = useState(false);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    let disposed = false;
    setBusy(true);
    setLoadFailed(false);
    setLoaded(false);
    void invoke<Settings>('load_transfer_settings')
      .then((value) => {
        if (!disposed) {
          setSettings(value);
          setLoaded(true);
          setBusy(false);
        }
      })
      .catch((error) => {
        if (!disposed) {
          setLoadFailed(true);
          setBusy(false);
          notify(error);
        }
      });
    return () => {
      disposed = true;
    };
  }, [notify, reload]);
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        // 未读取到已保存设置时，不能将界面默认值误写回配置。
        if (busy || !loaded) return;
        setBusy(true);
        setSaved(false);
        void invoke('save_transfer_settings', { settings })
          .then(() => setSaved(true))
          .catch(notify)
          .finally(() => setBusy(false));
      }}
    >
      <h3>
        <ArrowDownUp size={22} />
        传输设置
      </h3>
      {loadFailed && (
        <div className="preferences-load-error" role="alert">
          传输设置读取失败
          <button type="button" onClick={() => setReload((value) => value + 1)}>
            重新读取
          </button>
        </div>
      )}
      <div className="preferences-transfer-fields">
        <label className="preferences-setting-row">
          <span>
            <strong>下载及大文件并发数</strong>
            <small>小文件上传自动调节并发，最多 32 个，受服务器限制</small>
          </span>
          <select
            aria-label="下载及大文件并发数"
            disabled={busy || !loaded}
            value={settings.concurrency}
            onChange={(event) => {
              setSaved(false);
              setSettings({ ...settings, concurrency: Number(event.target.value) });
            }}
          >
            {[1, 2, 3, 4].map((value) => (
              <option key={value} value={value}>
                {`${value} 个文件`}
              </option>
            ))}
          </select>
        </label>
        <label className="preferences-setting-row">
          <span>
            <strong>单任务合计限速</strong>
            <small>0 表示不限速</small>
          </span>
          <span className="preferences-rate-input">
            <input
              aria-label="单任务合计限速（KiB/s）"
              type="number"
              min="0"
              max="1048576"
              step="1"
              disabled={busy || !loaded}
              value={settings.bytesPerSecond / 1024}
              onChange={(event) => {
                setSaved(false);
                setSettings({ ...settings, bytesPerSecond: Number(event.target.value) * 1024 });
              }}
            />
            <span aria-hidden="true">KiB/s</span>
          </span>
        </label>
      </div>
      <div className="preferences-transfer-actions">
        <p>
          <Info size={16} />
          在新任务开始时生效，交互认证保持串行。
        </p>
        <button className="primary" type="submit" disabled={busy || !loaded}>
          {busy ? '处理中…' : saved ? '已保存' : '保存传输设置'}
        </button>
      </div>
    </form>
  );
}
