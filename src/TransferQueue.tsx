import SyncTaskReport from './SyncTaskReport';
import ErrorNotice from './ErrorNotice';
import { diagnoseError } from './errorDiagnosis';
import { useEffect, useState } from 'react';
import type { Workspace } from './useTermRillWorkspace';
import {
  ChevronDown,
  RotateCcw,
  Upload,
  X,
  Check,
  Clock,
  ArrowUp,
  ArrowDown,
  LoaderCircle,
  Pause,
  Play,
} from 'lucide-react';
import { formatSize, finished } from './types';

function scanDuration(milliseconds: number) {
  const seconds = Math.max(0, Math.floor(milliseconds / 1000));
  return `${Math.floor(seconds / 60)
    .toString()
    .padStart(2, '0')}:${(seconds % 60).toString().padStart(2, '0')}`;
}

/** 传输进度、结果与取消入口。 */
export default function TransferQueue({ workspace }: { workspace: Workspace }) {
  const {
    queueOpen,
    setQueueOpen,
    transfers,
    setTransfers,
    pauseTransfer,
    retryTransfer,
    reconnectTransfer,
    retryingTransfers,
    connecting,
    listenersReady,
    connectionId,
    discardTransfer,
    discardingTransfers,
    cancel,
    statusText,
    running,
  } = workspace;
  const [now, setNow] = useState(() => performance.now());
  const timing = transfers.some((task) => !finished(task));
  useEffect(() => {
    if (!timing) return;
    setNow(performance.now());
    const timer = window.setInterval(() => setNow(performance.now()), 1000);
    return () => window.clearInterval(timer);
  }, [timing]);
  return (
    <section className="transfer">
      <div className="transfer-top">
        <button className="transfer-heading" onClick={() => setQueueOpen(!queueOpen)}>
          <ChevronDown size={17} style={{ transform: queueOpen ? '' : 'rotate(-90deg)' }} />
          <strong>传输队列</strong>
          <span className="badge">{transfers.length}</span>
          <small>{running ? `${running} 个任务进行中` : '文件安全传输'}</small>
        </button>
        <button
          className="clear-queue"
          onClick={() =>
            setTransfers((t) => t.filter((v) => !['completed', 'cancelled'].includes(v.status)))
          }
        >
          清除已结束
        </button>
      </div>
      {queueOpen && (
        <div className="transfer-table">
          {!transfers.length && (
            <div className="empty-queue">
              <Upload size={18} />
              选择本地文件上传，或把文件拖到远程面板
            </div>
          )}
          {transfers.map((t) => {
            const discarding = discardingTransfers.includes(t.id);
            const retrying = retryingTransfers.includes(t.id);
            const recoveryBusy = discarding || retrying || connecting || !listenersReady;
            const finalizing = t.status === 'running' && t.total > 0 && t.bytes >= t.total;
            const roundedEqual =
              t.status === 'running' &&
              t.bytes < t.total &&
              formatSize(t.bytes) === formatSize(t.total);
            const progress =
              t.status === 'completed'
                ? 100
                : t.total
                  ? Math.min(99, Math.round((t.bytes / t.total) * 100))
                  : 0;
            let remainingSeconds: number | undefined;
            let speedLabel = statusText[t.status];
            if (finalizing) {
              speedLabel = '正在收尾';
            } else if (t.status === 'running') {
              speedLabel = `${formatSize(t.speed)}/s`;
            }
            if (t.status === 'running' && !finalizing) {
              if (t.direction === 'upload') {
                remainingSeconds = t.remainingSeconds ?? undefined;
              } else if (t.speed > 0) {
                remainingSeconds = Math.ceil(Math.max(0, t.total - t.bytes) / t.speed);
              }
            }
            return (
              <div key={t.id} className="transfer-item">
                {t.syncReport && <SyncTaskReport task={t} status={statusText[t.status]} />}
                <div className="transfer-row">
                  <span className="code">
                    {t.direction === 'upload' ? <ArrowUp size={18} /> : <ArrowDown size={18} />}
                  </span>
                  <div className="transfer-file">
                    <strong title={t.name}>{t.name}</strong>
                    <span className="direction">
                      {t.direction === 'upload' ? '本地 → 远程' : '远程 → 本地'}
                    </span>
                  </div>
                  <div className="transfer-progress-group">
                    <div className="transfer-progress-meta">
                      <span
                        title={`${t.bytes.toLocaleString()} / ${t.total.toLocaleString()} 字节`}
                      >
                        {formatSize(t.bytes)} / {formatSize(t.total)}
                      </span>
                      <b>{progress}%</b>
                    </div>
                    <div className={`progress ${t.status === 'completed' ? 'done' : ''}`}>
                      <div style={{ width: `${progress}%` }} />
                    </div>
                    <div className="transfer-progress-meta transfer-progress-caption">
                      <span className="transfer-speed">{speedLabel}</span>
                      <span>
                        {remainingSeconds !== undefined
                          ? `预计剩余 ${scanDuration(remainingSeconds * 1000)}`
                          : roundedEqual
                            ? `剩余 ${(t.total - t.bytes).toLocaleString()} 字节`
                            : ''}
                      </span>
                    </div>
                  </div>
                  <span
                    data-transfer-status
                    className={
                      t.status === 'completed'
                        ? 'success'
                        : t.status === 'failed'
                          ? 'error-text'
                          : 'accent'
                    }
                  >
                    {t.status === 'completed' ? (
                      <Check size={14} />
                    ) : [
                        'running',
                        'scanning',
                        'connecting',
                        'prechecking',
                        'preparing',
                        'cancelling',
                      ].includes(t.status) ? (
                      <LoaderCircle
                        size={14}
                        className="transfer-status-spinner"
                        aria-hidden="true"
                      />
                    ) : (
                      <Clock size={14} />
                    )}{' '}
                    <span>
                      {finalizing ? '正在收尾' : statusText[t.status]}
                      {t.timerStartedAt !== undefined && (
                        <small
                          className="transfer-scan-time"
                          title="包含排队、扫描、连接准备、传输、暂停及收尾；重试重新计时"
                        >
                          {t.timerEstimated ? '观察用时' : '总用时'}{' '}
                          {scanDuration(
                            (t.timerEndedAt ?? Math.max(now, performance.now())) - t.timerStartedAt,
                          )}
                        </small>
                      )}
                      {t.status === 'scanning' && t.scanStartedAt !== undefined && (
                        <small
                          className="transfer-scan-time"
                          title={
                            t.scanTimerEstimated
                              ? '页面恢复后开始计时；包括远程预检、建连和目标目录准备'
                              : '包括本地扫描、远程预检、建连和目标目录准备'
                          }
                        >
                          扫描 {scanDuration(Math.max(now, performance.now()) - t.scanStartedAt)}
                        </small>
                      )}
                    </span>
                  </span>
                  <div className="transfer-controls">
                    {['running', 'paused'].includes(t.status) && (
                      <button
                        className="transfer-toggle"
                        title={t.status === 'paused' ? '继续' : '暂停'}
                        aria-label={`${t.status === 'paused' ? '继续' : '暂停'} ${t.name}`}
                        onClick={() => void pauseTransfer(t.id, t.status !== 'paused')}
                      >
                        {t.status === 'paused' ? <Play size={17} /> : <Pause size={17} />}
                      </button>
                    )}
                    {t.status === 'failed' && (
                      <>
                        <button disabled={recoveryBusy} onClick={() => void retryTransfer(t.id)}>
                          <RotateCcw size={14} />
                          {retrying ? '正在恢复…' : connectionId ? '重试/续传' : '连接后续传'}
                        </button>
                        <button
                          disabled={discarding || retrying}
                          onClick={() => void discardTransfer(t.id)}
                        >
                          <X size={14} /> {discarding ? '清理中…' : '移除记录'}
                        </button>
                      </>
                    )}
                    {!finished(t) && (
                      <button
                        aria-label={`取消 ${t.name}`}
                        disabled={finished(t) || t.status === 'cancelling'}
                        onClick={() => void cancel(t.id)}
                      >
                        <X size={15} />
                      </button>
                    )}
                  </div>
                </div>
                {t.error && !discarding && (
                  <ErrorNotice
                    error={t.error}
                    scope="传输"
                    phase={t.status}
                    disabled={recoveryBusy || running > 0}
                    recovery={
                      t.status === 'failed' &&
                      connectionId &&
                      ['network', 'auth', 'trust'].includes(diagnoseError(t.error).code)
                        ? { label: '重新连接后续传', run: () => reconnectTransfer(t.id) }
                        : undefined
                    }
                  />
                )}
                {(discarding || (!t.error && finalizing)) && (
                  <div className={t.error ? 'transfer-detail error-text' : 'transfer-detail'}>
                    {discarding
                      ? '正在清理任务临时文件，完成后移除记录'
                      : '数据已传完，正在完成文件安装及任务记录保存'}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}
