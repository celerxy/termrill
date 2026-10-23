/** 成功页展示本次任务的固定结果；阶段细节按需展开。 */
import { useState } from 'react';
import { Archive, CircleCheck, Copy, ChevronRight } from 'lucide-react';
import ArchivePackSources from './ArchivePackSources';
import { packStages, packStageLabels } from './archivePackMetrics';
import type { PackReport } from './archivePackMetrics';
import { basename } from './types';
import './archivePackResult.css';
import { archiveWriteFormat } from './archiveCapabilities';

export default function ArchivePackResult({
  report,
  path,
  sources,
  remote,
}: {
  report: PackReport;
  path: string;
  sources: string[];
  remote: boolean;
}) {
  const [copyStatus, setCopyStatus] = useState('');
  const stages = packStages.filter(
    (stage) => stage !== 'done' && (stage !== 'encode' || archiveWriteFormat(path) !== 'zip'),
  );
  const elapsed = Math.max(0, (report.finishedAt ?? report.startedAt) - report.startedAt);
  const copyPath = async () => {
    try {
      await navigator.clipboard.writeText(path);
      setCopyStatus('保存路径已复制');
    } catch {
      setCopyStatus('复制失败，请选中路径手动复制');
    }
  };
  return (
    <div className="pack-result">
      <div className="pack-result-heading" role="status">
        <CircleCheck aria-hidden="true" />
        <div>
          <h3>{remote ? '创建并上传成功' : '压缩包创建成功'}</h3>
          <p>压缩包已保存到{remote ? '远程' : '本地'}目录，原始文件已保留。</p>
        </div>
      </div>
      <section className="pack-result-card" aria-label="压缩包结果">
        <div className="pack-result-file">
          <Archive aria-hidden="true" />
          <div>
            <h4 title={basename(path)}>{basename(path)}</h4>
            <span className="archive-format">
              {archiveWriteFormat(path)?.toUpperCase() ?? '归档'} 压缩包
            </span>
          </div>
          <div className="pack-result-path">
            <span>保存位置</span>
            <p>{path}</p>
          </div>
          <button onClick={() => void copyPath()} aria-label="复制保存路径" title="复制保存路径">
            <Copy size={20} />
          </button>
        </div>
        <dl className="pack-result-metrics">
          <div>
            <dt>文件大小</dt>
            <dd>
              {report.data.zipBytes < 1024 * 1024
                ? `${(report.data.zipBytes / 1024).toFixed(1)} KB`
                : `${(report.data.zipBytes / (1024 * 1024)).toFixed(1)} MB`}
            </dd>
          </div>
          <div>
            <dt>包含文件</dt>
            <dd>{report.data.files} 个</dd>
          </div>
          <div>
            <dt>总耗时</dt>
            <dd>{(elapsed / 1000).toFixed(1)} 秒</dd>
          </div>
        </dl>
        {copyStatus && (
          <p className="pack-copy-status" role="status">
            {copyStatus}
          </p>
        )}
      </section>
      <details className="pack-result-details">
        <summary>
          <ChevronRight size={19} />
          <strong>处理详情</strong>
          <span>
            {stages
              .map(
                (stage) =>
                  `${stage === 'upload' && !remote ? '写入压缩包' : packStageLabels[stage]} ${((report.durations[stage] ?? 0) / 1000).toFixed(1)} 秒`,
              )
              .join(' · ')}
          </span>
        </summary>
        <dl>
          {stages.map((stage) => (
            <div key={stage}>
              <dt>{stage === 'upload' && !remote ? '写入压缩包' : packStageLabels[stage]}</dt>
              <dd>已完成 · {((report.durations[stage] ?? 0) / 1000).toFixed(1)} 秒</dd>
            </div>
          ))}
        </dl>
        <p>
          共处理 {report.data.files} 个文件 · {report.data.directories} 个目录
        </p>
      </details>
      <ArchivePackSources sources={sources} completed />
    </div>
  );
}
