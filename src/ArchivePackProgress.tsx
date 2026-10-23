/** 显示后端计数和客户端阶段耗时；总量未知时不捏造百分比。 */
import { useEffect, useState } from 'react';
import { CircleCheck, LoaderCircle, Circle, AlertCircle } from 'lucide-react';
import { packStages, packStageLabels } from './archivePackMetrics';
import { archiveWriteFormat } from './archiveCapabilities';
import type { PackReport } from './archivePackMetrics';
import './archivePackProgress.css';
const duration = (milliseconds: number) => `${(milliseconds / 1000).toFixed(1)} 秒`;
const clock = (milliseconds: number) => {
  const seconds = Math.floor(milliseconds / 1000);
  return `${Math.floor(seconds / 60)
    .toString()
    .padStart(2, '0')}:${(seconds % 60).toString().padStart(2, '0')}`;
};
const bytes = (count: number) =>
  count < 1024
    ? `${count} B`
    : count < 1024 * 1024
      ? `${(count / 1024).toFixed(1)} KB`
      : `${(count / (1024 * 1024)).toFixed(1)} MB`;
export default function ArchivePackProgress({
  report,
  busy,
  remote,
  failed,
  name,
}: {
  report: PackReport;
  busy: boolean;
  remote: boolean;
  failed: boolean;
  name: string;
}) {
  const [now, setNow] = useState(performance.now());
  useEffect(() => {
    if (!busy) return;
    const timer = window.setInterval(() => setNow(performance.now()), 250);
    return () => window.clearInterval(timer);
  }, [busy]);
  const { data, startedAt, stageStartedAt, durations } = report;
  const end = report.finishedAt ?? now;
  const elapsed = Math.max(0, end - startedAt);
  const stageElapsed = Math.max(0, end - stageStartedAt);
  const stageIndex = packStages.indexOf(data.stage);
  const uploading = data.stage === 'upload' || data.stage === 'install' || data.stage === 'done';
  const total = uploading ? data.zipBytes : data.totalBytes;
  const processed = uploading ? data.uploadedBytes : data.processedBytes;
  const known = data.stage !== 'scan' && data.stage !== 'encode' && total > 0;
  const stages = packStages.filter(
    (stage) => stage !== 'done' && (stage !== 'encode' || archiveWriteFormat(name) !== 'zip'),
  );
  const percent = known ? Math.min(100, (processed / total) * 100) : null;
  const readingTime = data.stage === 'compress' ? stageElapsed : (durations.compress ?? 0);
  const speed = readingTime > 0 ? data.processedBytes / (readingTime / 1000) : 0;
  const title = failed
    ? '压缩未完成'
    : !busy && data.stage !== 'done'
      ? '操作已停止'
      : data.stage === 'compress'
        ? '正在读取并压缩'
        : packStageLabels[data.stage];
  return (
    <section className="pack-progress" aria-label="压缩进度">
      <div className="pack-progress-heading">
        <h3>
          {failed ? (
            <AlertCircle />
          ) : busy ? (
            <LoaderCircle className="pack-spinning" />
          ) : (
            <CircleCheck />
          )}
          {title}
        </h3>
        <strong>总耗时 {clock(elapsed)}</strong>
      </div>
      <ol
        className="pack-stages"
        style={{ gridTemplateColumns: `repeat(${stages.length}, minmax(0, 1fr))` }}
      >
        {stages.map((stage) => {
          const index = packStages.indexOf(stage);
          const complete = index < stageIndex;
          const active = index === stageIndex;
          const Icon = complete ? CircleCheck : active && busy ? LoaderCircle : Circle;
          const label = stage === 'upload' && !remote ? '写入压缩包' : packStageLabels[stage];
          return (
            <li
              key={stage}
              className={complete ? 'complete' : active ? 'active' : ''}
              aria-current={active ? 'step' : undefined}
            >
              <Icon className={active && busy ? 'pack-spinning' : ''} />
              <strong>{label}</strong>
              <span>
                {complete
                  ? `已完成 · ${duration(durations[stage] ?? 0)}`
                  : active
                    ? `${busy ? '进行中' : '已停止'} · ${duration(stageElapsed)}`
                    : '等待中'}
              </span>
            </li>
          );
        })}
      </ol>
      <div className="pack-progress-label">
        <span>
          {data.stage === 'scan'
            ? '正在确认来源总量'
            : uploading
              ? remote
                ? '压缩包上传进度'
                : '压缩包写入进度'
              : '源文件处理进度'}
          {data.stage !== 'scan' && (
            <em>
              {bytes(processed)} / {bytes(total)}
            </em>
          )}
        </span>
        <strong>{percent === null ? '—' : `${Math.floor(percent)}%`}</strong>
      </div>
      <progress
        max={100}
        value={percent ?? undefined}
        aria-label={uploading ? '压缩包传输进度' : '源文件读取进度'}
      />
      <dl className="pack-metrics">
        <div>
          <dt>{data.stage === 'scan' ? '已发现文件' : '已处理文件'}</dt>
          <dd>{data.stage === 'scan' ? data.files : `${data.processedFiles} / ${data.files}`}</dd>
        </div>
        <div>
          <dt>{data.stage === 'scan' ? '已发现目录' : '平均读取速度'}</dt>
          <dd className="pack-speed">
            {data.stage === 'scan' ? data.directories : readingTime > 0 ? `${bytes(speed)}/s` : '—'}
          </dd>
        </div>
        <div>
          <dt>已生成归档</dt>
          <dd>{data.stage === 'scan' ? '—' : bytes(data.zipBytes)}</dd>
        </div>
      </dl>
      <p className="pack-current" title={data.currentFile}>
        <span>当前文件</span>
        {data.currentFile ||
          (data.stage === 'scan'
            ? '扫描目录中…'
            : data.stage === 'encode'
              ? '正在编码归档，完成后安装目标…'
              : uploading
                ? name
                : '等待下一个文件')}
      </p>
      <p className="pack-summary">
        {data.stage === 'scan'
          ? '扫描期间持续更新已发现项目数，总量确认后显示真实进度。'
          : `扫描完成：${data.files} 个文件 · ${data.directories} 个目录`}
        {data.stage === 'install' && ' · 文件已传完，正在确认并安装目标。'}
      </p>
    </section>
  );
}
