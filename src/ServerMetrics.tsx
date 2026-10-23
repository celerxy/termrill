/** 默认不采集；仅面板可见且用户开启时串行轮询，失败停止，不影响文件会话。 */
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useSessionVisibility } from './SessionVisibility';
import { formatSize, message } from './types';
type Metrics = {
  cpuTotal: number;
  cpuIdle: number;
  memoryTotal: number;
  memoryAvailable: number;
  diskTotal: number;
  diskAvailable: number;
};
export default function ServerMetrics({ connectionId }: { connectionId: string | null }) {
  const visible = useSessionVisibility();
  const [enabled, setEnabled] = useState(false);
  const [metrics, setMetrics] = useState<Metrics | null>(null);
  const [cpu, setCpu] = useState<number | null>(null);
  const [error, setError] = useState('');
  const [sampledAt, setSampledAt] = useState('');
  const pending = useRef<Promise<Metrics> | null>(null);
  useEffect(() => {
    if (!visible) setEnabled(false);
  }, [visible]);
  useEffect(() => {
    if (!enabled || !visible || !connectionId) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let previous: Metrics | null = null;
    setError('');
    setCpu(null);
    setMetrics(null);
    setSampledAt('');
    async function sample() {
      try {
        // 关闭后重开也先等待上一请求结束，避免同一视图积压通道。
        if (pending.current) await pending.current.catch(() => {});
        if (disposed) return;
        const request = invoke<Metrics>('server_metrics', { connectionId });
        pending.current = request;
        const next = await request;
        if (disposed) return;
        const total = previous ? next.cpuTotal - previous.cpuTotal : 0;
        const idle = previous ? next.cpuIdle - previous.cpuIdle : 0;
        setCpu(total > 0 && idle >= 0 && idle <= total ? (1 - idle / total) * 100 : null);
        previous = next;
        setMetrics(next);
        setSampledAt(new Date().toLocaleTimeString());
        timer = setTimeout(() => void sample(), 5000);
      } catch (cause) {
        if (!disposed) {
          setError(message(cause));
          setEnabled(false);
        }
      }
    }
    void sample();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [enabled, visible, connectionId]);
  return (
    <section className="server-metrics">
      <p>
        Linux CPU、内存与根分区空间。开启后执行固定只读脚本，每次完成后间隔 5
        秒；关闭面板、切换会话或采集失败即停止后续请求。
      </p>
      <button
        className={enabled ? '' : 'primary'}
        disabled={!connectionId}
        onClick={() => setEnabled(!enabled)}
      >
        {enabled ? '停止采集' : '开启状态采集'}
      </button>
      {!connectionId && <p>连接服务器后可采集；SFTP 受限账号无需启用此功能。</p>}
      {error && <p role="alert">{error}</p>}
      <div className="metric-cards">
        <article>
          <span>CPU 使用率</span>
          <strong>{cpu === null ? '—' : `${cpu.toFixed(1)}%`}</strong>
          <small>{enabled && cpu === null ? '等待两次有效采样' : '两次采样间的忙碌时间'}</small>
        </article>
        <article>
          <span>内存使用率</span>
          <strong>
            {metrics
              ? `${((1 - metrics.memoryAvailable / metrics.memoryTotal) * 100).toFixed(1)}%`
              : '—'}
          </strong>
          <small>
            {metrics
              ? `${formatSize(metrics.memoryTotal - metrics.memoryAvailable)} / ${formatSize(metrics.memoryTotal)}`
              : '基于 MemAvailable'}
          </small>
        </article>
        <article>
          <span>根分区使用率</span>
          <strong>
            {metrics
              ? `${((1 - metrics.diskAvailable / metrics.diskTotal) * 100).toFixed(1)}%`
              : '—'}
          </strong>
          <small>
            {metrics
              ? `可用 ${formatSize(metrics.diskAvailable)} / ${formatSize(metrics.diskTotal)}`
              : '仅 / 分区，按可用空间计算'}
          </small>
        </article>
      </div>
      <small>
        {sampledAt
          ? `上次采样 ${sampledAt} · ${enabled ? '采集中' : '已停止，以上为历史数据'}`
          : enabled
            ? '正在采集…'
            : '未开启采集'}
      </small>
    </section>
  );
}
