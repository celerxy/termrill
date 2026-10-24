/** 可展开、可复制的本机进度快照；只复制固定描述与数字，不包含任务身份或来源。 */
import { useEffect, useState } from 'react';
import type { Transfer } from './types';
import { formatSize } from './types';
import { transferHealth, transferSnapshot } from './transferObservation';
import './transferDiagnostics.css';

export default function TransferDiagnostics({ task, now }: { task: Transfer; now: number }) {
  const [report, setReport] = useState('');
  const [copyStatus, setCopyStatus] = useState('');
  useEffect(() => {
    setReport('');
    setCopyStatus('');
  }, [task.status]);
  const health = transferHealth(task, now);
  if (!health) return null;
  function showReport() {
    setCopyStatus('');
    setReport(report ? '' : transferSnapshot(task, now));
  }
  async function copyReport() {
    try {
      await navigator.clipboard.writeText(report);
      setCopyStatus('已复制观察快照');
    } catch {
      setCopyStatus('无法复制，请选中快照手动复制');
    }
  }
  return (
    <div className="transfer-diagnostics">
      {health.stale ? (
        <p>
          已 {health.eventAgeSeconds}{' '}
          秒未收到进度更新，速度与预计剩余时间暂不可用。应用休眠或事件延迟也可能造成此现象。
        </p>
      ) : health.waiting ? (
        <p>
          进度字节已 {health.idleSeconds}{' '}
          秒未变化，仍在接收状态更新。可能仍在等待数据、写入或确认，暂不能判定故障原因。
        </p>
      ) : null}
      <button type="button" onClick={showReport}>
        {report ? '收起传输诊断' : '查看传输诊断'}
      </button>
      <span className="transfer-diagnostics-sample">
        最近速度样本 {health.stale ? '已过期' : `${formatSize(task.speed)}/s`}
      </span>
      {report && (
        <div>
          <pre tabIndex={0}>{report}</pre>
          <button type="button" onClick={() => void copyReport()}>
            复制观察快照
          </button>
          <span role="status">{copyStatus}</span>
        </div>
      )}
    </div>
  );
}
