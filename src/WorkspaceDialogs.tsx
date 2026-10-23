import type { Workspace } from './useTermRillWorkspace';
import { X, ShieldCheck, FolderPlus, Pencil, Trash2 } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { lazy, Suspense } from 'react';
// 首屏不需要弹窗业务模块；只在用户打开时下载和执行。
const ConnectionDialog = lazy(() => import('./ConnectionDialog'));
const PreferencesDialog = lazy(() => import('./PreferencesDialog'));
const DirectorySyncDialog = lazy(() => import('./DirectorySyncDialog'));
const TransferConfirmDialog = lazy(() => import('./TransferConfirmDialog'));

/** 连接、主机指纹、传输计划和偏好设置的交互入口。 */
export default function WorkspaceDialogs({ workspace }: { workspace: Workspace }) {
  const {
    connectionId,
    local,
    remote,
    connecting,
    setConnecting,
    notice,
    setNotice,
    dialog,
    setDialog,
    trust,
    setTrust,
    showHidden,
    setShowHidden,
    settings,
    setSettings,
    hosts,
    setHosts,
    plans,
    setPlans,
    overwrite,
    setOverwrite,
    operation,
    setOperation,
    operationBusy,
    deleteProfile,
    setDeleteProfile,
    listenersReady,
    profile,
    notify,
    save,
    connect,
    startPlans,
    submitOperation,
    removeProfile,
  } = workspace;
  return (
    <Suspense fallback={null}>
      {workspace.syncOpen && <DirectorySyncDialog workspace={workspace} />}
      {notice && (
        <div className="toast" role="status">
          {notice}
          <button aria-label="关闭提示" onClick={() => setNotice('')}>
            <X size={15} />
          </button>
        </div>
      )}
      {dialog && (
        <ConnectionDialog
          initial={dialog.profile}
          recoveryTask={workspace.recoveryTask}
          busy={connecting}
          onClose={workspace.closeConnectionDialog}
          onSave={save}
          onConnect={connect}
        />
      )}
      {trust && (
        <div className="modal-backdrop">
          <section className="modal trust-modal" role="dialog" aria-modal="true">
            <div className="modal-icon">
              <ShieldCheck size={25} />
            </div>
            <h2>{trust.key.previous ? '主机指纹发生变化' : '确认服务器身份'}</h2>
            <p>
              {trust.key.previous
                ? '请先向服务器管理员核实新指纹。'
                : '首次连接此主机，请核对 SHA256 指纹。'}
            </p>
            <strong>{trust.key.address}</strong>
            <code className="fingerprint">{trust.key.fingerprint}</code>
            {trust.key.previous && (
              <>
                <small>之前的指纹</small>
                <code className="fingerprint previous">{trust.key.previous}</code>
              </>
            )}
            <div className="modal-actions">
              <button disabled={connecting} onClick={workspace.cancelTrust}>
                取消
              </button>
              <button
                disabled={connecting}
                className="primary"
                onClick={() => {
                  const current = trust;
                  setConnecting(true);
                  void invoke('trust_host', { hostKey: current.key })
                    .then(() => {
                      setTrust(null);
                      return connect(current.request);
                    })
                    .catch((e) => {
                      notify(e);
                      setConnecting(false);
                      setDialog({ profile: current.request.profile });
                    });
                }}
              >
                {connecting ? '正在连接…' : '信任并继续'}
              </button>
            </div>
          </section>
        </div>
      )}
      {plans && (
        <TransferConfirmDialog
          plans={plans}
          overwrite={overwrite}
          onOverwriteChange={setOverwrite}
          canStart={Boolean(connectionId && listenersReady)}
          onClose={() => setPlans(null)}
          onStart={() => void startPlans()}
        />
      )}
      {operation && (
        <div className="modal-backdrop">
          <section
            className="modal compact-operation"
            role="dialog"
            aria-modal="true"
            aria-labelledby="file-operation-title"
          >
            <header className="compact-operation-heading">
              <span className="compact-operation-icon">
                {operation.kind === 'mkdir' ? <FolderPlus size={22} /> : <Pencil size={22} />}
              </span>
              <h2 id="file-operation-title">
                {operation.kind === 'mkdir' ? '新建文件夹' : '重命名'}
              </h2>
              <button aria-label="关闭" disabled={operationBusy} onClick={() => setOperation(null)}>
                <X size={18} />
              </button>
            </header>
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void submitOperation();
              }}
            >
              <label>
                名称
                <input
                  autoFocus
                  required
                  value={operation.name}
                  onChange={(e) => setOperation({ ...operation, name: e.target.value })}
                />
              </label>
              <div className="modal-actions">
                <button type="button" disabled={operationBusy} onClick={() => setOperation(null)}>
                  取消
                </button>
                <button type="submit" className="primary" disabled={operationBusy}>
                  {operationBusy ? '处理中…' : '确定'}
                </button>
              </div>
            </form>
          </section>
        </div>
      )}
      {settings && (
        <PreferencesDialog
          showHidden={showHidden}
          onShowHiddenChange={setShowHidden}
          hosts={hosts}
          onHostsChange={setHosts}
          notify={notify}
          onClose={() => setSettings(false)}
        />
      )}
      {deleteProfile && (
        <div className="modal-backdrop">
          <section
            className="modal compact-operation"
            role="dialog"
            aria-modal="true"
            aria-labelledby="delete-profile-title"
          >
            <header className="compact-operation-heading">
              <span className="compact-operation-icon danger">
                <Trash2 size={22} />
              </span>
              <h2 id="delete-profile-title">删除连接配置</h2>
              <button aria-label="关闭" onClick={() => setDeleteProfile(null)}>
                <X size={18} />
              </button>
            </header>
            <p>删除「{deleteProfile.name}」的已保存配置；打开的会话仍保留，可通过标签关闭。</p>
            <div className="modal-actions">
              <button onClick={() => setDeleteProfile(null)}>取消</button>
              <button className="primary danger-action" onClick={() => void removeProfile()}>
                删除配置
              </button>
            </div>
          </section>
        </div>
      )}
    </Suspense>
  );
}
