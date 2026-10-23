/** 压缩通道仍接受普通文本；结构化事件校验后才用于真实进度。 */
export const packStages = ['scan', 'compress', 'encode', 'upload', 'install', 'done'] as const;
export type PackStage = (typeof packStages)[number];
export const packStageLabels: Record<PackStage, string> = {
  scan: '扫描目录',
  compress: '读取并压缩',
  encode: '编码归档格式',
  upload: '上传压缩包',
  install: '写入目标',
  done: '压缩完成',
};
export type PackProgress = {
  kind: 'packProgress';
  stage: PackStage;
  files: number;
  directories: number;
  processedFiles: number;
  totalBytes: number;
  processedBytes: number;
  zipBytes: number;
  uploadedBytes: number;
  currentFile: string;
};
export type PackReport = {
  data: PackProgress;
  startedAt: number;
  stageStartedAt: number;
  finishedAt?: number;
  durations: Partial<Record<PackStage, number>>;
};
export function parsePackProgress(message: string): PackProgress | null {
  if (!message.startsWith('{')) return null;
  try {
    const data = JSON.parse(message) as Record<string, unknown>;
    if (
      data.kind !== 'packProgress' ||
      !packStages.includes(data.stage as PackStage) ||
      typeof data.currentFile !== 'string'
    )
      return null;
    for (const key of [
      'files',
      'directories',
      'processedFiles',
      'totalBytes',
      'processedBytes',
      'zipBytes',
      'uploadedBytes',
    ]) {
      if (
        typeof data[key] !== 'number' ||
        !Number.isSafeInteger(data[key]) ||
        (data[key] as number) < 0
      )
        return null;
    }
    return data as PackProgress;
  } catch {
    return null;
  }
}
export function updatePackReport(
  previous: PackReport | null,
  data: PackProgress,
  now = performance.now(),
): PackReport {
  if (!previous) return { data, startedAt: now, stageStartedAt: now, durations: {} };
  const changed = previous.data.stage !== data.stage;
  return {
    ...previous,
    data,
    stageStartedAt: changed ? now : previous.stageStartedAt,
    durations: changed
      ? { ...previous.durations, [previous.data.stage]: now - previous.stageStartedAt }
      : previous.durations,
    finishedAt: data.stage === 'done' ? now : previous.finishedAt,
  };
}
