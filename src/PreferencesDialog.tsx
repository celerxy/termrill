import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ArrowDownUp, Eye, LockKeyhole, Settings, ShieldCheck, Trash2, X } from 'lucide-react';
import TransferSettingsForm from './TransferSettingsForm';
import './preferences.css';

type Props = {
  showHidden: boolean;
  onShowHiddenChange: (value: boolean) => void;
  hosts: Record<string, string>;
  onHostsChange: (hosts: Record<string, string>) => void;
  notify: (error: unknown) => void;
  onClose: () => void;
};
const sections = [
  { id: 'display', label: '文件显示', Icon: Eye },
  { id: 'transfer', label: '传输设置', Icon: ArrowDownUp },
  { id: 'trust', label: '主机信任', Icon: ShieldCheck },
];

/** 偏好设置只调用现有保存与信任接口；导航定位分区，内容独立滚动。 */
export default function PreferencesDialog({
  showHidden,
  onShowHiddenChange,
  hosts,
  onHostsChange,
  notify,
  onClose,
}: Props) {
  const dialog = useRef<HTMLElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const mounted = useRef(true);
  const [activeSection, setActiveSection] = useState('transfer');
  const [savingDisplay, setSavingDisplay] = useState(false);
  const [removingHosts, setRemovingHosts] = useState<Set<string>>(new Set());
  useEffect(() => {
    mounted.current = true;
    const previousFocus = document.activeElement;
    dialog.current?.focus();
    return () => {
      mounted.current = false;
      if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus();
    };
  }, []);
  async function changeHiddenFiles(value: boolean) {
    if (savingDisplay) return;
    setSavingDisplay(true);
    try {
      await invoke('save_preferences', { showHidden: value });
      // 保存成功后仍更新工作区；关闭弹窗不应让共享状态与磁盘配置不一致。
      onShowHiddenChange(value);
    } catch (error) {
      notify(error);
    } finally {
      if (mounted.current) setSavingDisplay(false);
    }
  }
  async function removeTrust(address: string) {
    if (removingHosts.has(address)) return;
    setRemovingHosts((previous) => new Set(previous).add(address));
    try {
      await invoke('forget_host', { address });
      const updatedHosts = await invoke<Record<string, string>>('known_hosts');
      onHostsChange(updatedHosts);
    } catch (error) {
      notify(error);
    } finally {
      if (mounted.current)
        setRemovingHosts((previous) => {
          const next = new Set(previous);
          next.delete(address);
          return next;
        });
    }
  }
  function navigateToSection(id: string) {
    setActiveSection(id);
    const section = content.current?.querySelector<HTMLElement>(`#preferences-${id}`);
    if (section && content.current) content.current.scrollTo({ top: section.offsetTop - 24 });
  }
  return (
    <div className="modal-backdrop">
      <section
        className="modal preferences-dialog"
        ref={dialog}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby="preferences-title"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation();
            onClose();
          }
          if (event.key === 'Tab') {
            const controls = [
              ...event.currentTarget.querySelectorAll<HTMLElement>(
                'button:not(:disabled), input:not(:disabled), select:not(:disabled)',
              ),
            ];
            const first = controls[0];
            const last = controls.at(-1);
            if (
              event.shiftKey &&
              (document.activeElement === first || document.activeElement === dialog.current)
            ) {
              event.preventDefault();
              last?.focus();
            } else if (!event.shiftKey && document.activeElement === last) {
              event.preventDefault();
              first?.focus();
            }
          }
        }}
      >
        <header className="preferences-heading">
          <span className="preferences-heading-icon">
            <Settings size={29} />
          </span>
          <div>
            <h2 id="preferences-title">偏好设置</h2>
            <p>调整文件显示、传输性能与主机信任</p>
          </div>
          <button className="preferences-close" aria-label="关闭设置" onClick={onClose}>
            <X size={21} />
          </button>
        </header>
        <div className="preferences-body">
          <nav className="preferences-nav" aria-label="设置分区">
            {sections.map(({ id, label, Icon }) => (
              <button
                key={id}
                aria-current={activeSection === id ? 'location' : undefined}
                onClick={() => navigateToSection(id)}
              >
                <Icon size={20} />
                {label}
              </button>
            ))}
          </nav>
          <div className="preferences-content" ref={content}>
            <section id="preferences-display" className="preferences-section">
              <h3>
                <Eye size={22} />
                文件显示
              </h3>
              <label className="preferences-display-row">
                <span>
                  <strong>显示隐藏文件</strong>
                  <small>在文件列表中显示隐藏项目</small>
                </span>
                <input
                  type="checkbox"
                  role="switch"
                  checked={showHidden}
                  disabled={savingDisplay}
                  onChange={(event) => void changeHiddenFiles(event.target.checked)}
                />
                <span className="preferences-switch" aria-hidden="true" />
              </label>
              {savingDisplay && (
                <span className="preferences-inline-status" role="status">
                  正在保存显示设置…
                </span>
              )}
            </section>
            <section id="preferences-transfer" className="preferences-section">
              <TransferSettingsForm notify={notify} />
            </section>
            <section id="preferences-trust" className="preferences-section">
              <h3>
                <ShieldCheck size={22} />
                已信任的主机<span className="preferences-count">{Object.keys(hosts).length}</span>
              </h3>
              <div className="preferences-hosts">
                {Object.entries(hosts).map(([address, fingerprint]) => (
                  <div className="preferences-host" key={address}>
                    <div className="preferences-host-heading">
                      <ShieldCheck size={19} />
                      <strong title={address}>{address}</strong>
                      <button
                        className="preferences-remove"
                        disabled={removingHosts.has(address)}
                        onClick={() => void removeTrust(address)}
                      >
                        <Trash2 size={14} />
                        {removingHosts.has(address) ? '移除中…' : '移除信任'}
                      </button>
                    </div>
                    <small>SHA256 主机指纹</small>
                    <code>{fingerprint}</code>
                  </div>
                ))}
                {!Object.keys(hosts).length && (
                  <div className="preferences-no-hosts">
                    <ShieldCheck size={26} />
                    <strong>尚未信任任何主机</strong>
                    <p>连接服务器并确认指纹后，会显示在这里。</p>
                  </div>
                )}
              </div>
            </section>
          </div>
        </div>
        <footer className="preferences-footer">
          <LockKeyhole size={23} />
          <div>
            <strong>连接配置保存在应用配置目录</strong>
            <small>凭据按所选保存方式处理</small>
          </div>
          <button onClick={onClose}>关闭</button>
        </footer>
      </section>
    </div>
  );
}
