/** 只观察进度事件；不把确认字节停顿推断为网络停收、代理故障或磁盘故障。 */
import type { Transfer } from './types';

export function observeTransfer(
  task: Transfer,
  previous: Transfer | undefined,
  now: number,
): Transfer {
  if (task.status !== 'running') return { ...task, observation: undefined };
  const previousObservation = previous?.status === 'running' ? previous.observation : undefined;
  const advanced =
    !previousObservation || task.bytes !== previous?.bytes || task.total !== previous?.total;
  return {
    ...task,
    observation: {
      eventAt: now,
      advancedAt: advanced ? now : (previousObservation?.advancedAt ?? now),
    },
  };
}

export function transferHealth(task: Transfer, now: number) {
  if (task.status !== 'running' || !task.observation) return undefined;
  const idleSeconds = Math.max(0, Math.floor((now - task.observation.advancedAt) / 1000));
  const eventAgeSeconds = Math.max(0, Math.floor((now - task.observation.eventAt) / 1000));
  const finalizing = task.total > 0 && task.bytes >= task.total;
  return {
    idleSeconds,
    eventAgeSeconds,
    stale: eventAgeSeconds >= 5,
    waiting: !finalizing && idleSeconds >= 10,
  };
}

/** 只使用固定枚举与数值生成导出内容，绝不序列化整个任务。 */
export function transferSnapshot(task: Transfer, now: number) {
  const health = transferHealth(task, now);
  return [
    'TermRill 传输观察快照',
    `时间：${new Date().toISOString()}`,
    `方向：${task.direction === 'download' ? '下载' : '上传'}`,
    `进度字节：${task.bytes} / ${task.total}`,
    `最近速度样本（字节/秒）：${task.speed}`,
    `距字节进度变化（秒）：${health?.idleSeconds ?? '无观察样本'}`,
    `距进度事件更新（秒）：${health?.eventAgeSeconds ?? '无观察样本'}`,
    '依据：当前页面观察的进度事件；暂停、重试和重新进入传输阶段会重新计时。',
    '限制：不能据此判断路由、代理、磁盘或服务器瓶颈，不包含连接并发和重连次数。',
    '隐私：不包含任务名、地址、路径、正文或凭据；仅在点击复制后写入本机剪贴板。',
  ].join('\n');
}
