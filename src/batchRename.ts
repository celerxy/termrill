/** 预览仅生成当前目录的新名称，不生成跨目录路径，也不支持交换或覆盖。 */
import type { FileEntry } from './types';
export type RenamePreview = { entry: FileEntry; name: string; error: string; changed: boolean };
const identity = (name: string) => name.toLowerCase();

export function previewBatchRename(
  entries: FileEntry[],
  template: string,
  start: number,
  padding: number,
  existingNames: string[],
  windows: boolean,
): RenamePreview[] {
  const existing = new Set(existingNames.map(identity));
  const used = new Set<string>();
  return entries.map((entry, index) => {
    const dot = entry.name.lastIndexOf('.');
    const stem = dot > 0 ? entry.name.slice(0, dot) : entry.name;
    const ext = dot > 0 ? entry.name.slice(dot) : '';
    const parts: Record<string, string> = {
      stem,
      ext,
      n: String(start + index).padStart(Math.min(8, Math.max(1, padding)), '0'),
    };
    const name = template.replace(/\{(stem|ext|n)\}/g, (_, part: string) => parts[part]);
    const changed = name !== entry.name;
    let error = '';
    if (
      !Number.isInteger(start) ||
      start < 0 ||
      start > 1_000_000 ||
      !Number.isInteger(padding) ||
      padding < 1 ||
      padding > 8
    )
      error = '编号必须是 0～1000000 的整数，位数为 1～8';
    else if (entry.directory || entry.symlink) error = '仅支持普通文件';
    else if (
      !name ||
      ['.', '..'].includes(name) ||
      /[\\/\u0000-\u001f\u007f]/.test(name) ||
      name.length > 240 ||
      /\{[^}]*\}/.test(name)
    )
      error = '名称为空、过长或包含非法字符/未知占位符';
    else if (
      windows &&
      (/[<>:"|?*]/.test(name) ||
        /[. ]$/.test(name) ||
        /^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)/i.test(name))
    )
      error = '不符合 Windows 文件名规则';
    else if (changed && identity(name) === identity(entry.name)) error = '暂不支持仅修改大小写';
    else if (used.has(identity(name))) error = '与本批其他新名称冲突';
    else if (changed && existing.has(identity(name)))
      error = '当前目录已存在同名项目，不能覆盖或交换名称';
    used.add(identity(name));
    return { entry, name, error, changed };
  });
}

export type RenameResult = {
  source: string;
  destination: string;
  status: 'completed' | 'failed' | 'unchanged' | 'not-started';
  error?: string;
};
/** 任一写入结果未知或失败即停止，不重放、不反向回滚已经确认的改名。 */
export async function runRenamePlan(
  plan: RenamePreview[],
  rename: (row: RenamePreview) => Promise<void>,
  cancelled: () => boolean,
  report: (results: RenameResult[]) => void,
) {
  if (plan.some((row) => row.error)) throw new Error('计划包含冲突，禁止执行');
  const results: RenameResult[] = plan.map((row) => ({
    source: row.entry.name,
    destination: row.name,
    status: row.changed ? 'not-started' : 'unchanged',
  }));
  for (let index = 0; index < plan.length; index++) {
    const row = plan[index];
    if (cancelled()) break;
    if (!row.changed) continue;
    try {
      await rename(row);
      results[index] = { ...results[index], status: 'completed' };
    } catch (error) {
      results[index] = {
        ...results[index],
        status: 'failed',
        error:
          error instanceof Error
            ? error.message
            : String((error as { message?: string })?.message ?? error),
      };
      report([...results]);
      return results;
    }
    report([...results]);
  }
  report([...results]);
  return results;
}
