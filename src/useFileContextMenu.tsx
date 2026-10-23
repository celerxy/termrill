import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  FolderOpen,
  Download,
  Upload,
  FileText,
  Pencil,
  Trash2,
  Copy,
  FolderPlus,
  RefreshCw,
  ShieldCheck,
  Archive,
} from 'lucide-react';
import { isArchive } from './archiveTypes';
import FileContextMenu from './FileContextMenu';
import type { MenuAction } from './FileContextMenu';
import type { FileEntry } from './types';
import type { FilePaneProps } from './FilePane';

/** 保存右键时的选择快照，按文件类型生成操作；导航/忙碌状态变化时关闭旧菜单。 */
export function useFileContextMenu(p: FilePaneProps) {
  const [context, setContext] = useState<{ x: number; y: number; entries: FileEntry[] } | null>(
    null,
  );
  const closeContext = useCallback(() => setContext(null), []);
  useEffect(closeContext, [closeContext, p.listing, p.disabled, p.loading]);
  const selectedPaths = useMemo(() => new Set(p.selected), [p.selected]);
  const selectedEntries = useMemo(
    () => p.listing?.entries.filter((file) => selectedPaths.has(file.path)) ?? [],
    [p.listing, selectedPaths],
  );
  const openContext = (event: React.MouseEvent | React.KeyboardEvent, entry?: FileEntry) => {
    event.preventDefault();
    event.stopPropagation();
    if (p.disabled || p.loading || !p.listing) return;
    // 右键已选项目保留多选；右键未选项目只操作该项目，避免删除之前选中的文件。
    const targets = entry ? (selectedPaths.has(entry.path) ? selectedEntries : [entry]) : [];
    p.onSelect(targets.map((file) => file.path));
    const rect = event.currentTarget.getBoundingClientRect();
    const pointer = 'clientX' in event && (event.clientX !== 0 || event.clientY !== 0);
    setContext({
      x: pointer ? event.clientX : rect.left + 24,
      y: pointer ? event.clientY : rect.top + 24,
      entries: targets,
    });
  };
  const menuEntries = context?.entries ?? [];
  const single = menuEntries.length === 1 ? menuEntries[0] : undefined;
  const copyPath = async () => {
    const value = menuEntries.length
      ? menuEntries.map((file) => file.path).join('\n')
      : (p.listing?.path ?? '');
    try {
      await navigator.clipboard.writeText(value);
    } catch (error) {
      p.onError(error);
    }
  };
  const openActions: MenuAction[] = [
    ...(single?.directory && !single.symlink
      ? [
          {
            label: '进入目录',
            icon: <FolderOpen size={16} />,
            run: () => p.onNavigate(single.path),
          },
        ]
      : []),
    ...(p.onEdit && single && !single.directory && !single.symlink && !isArchive(single.path)
      ? [
          {
            label: '预览 / 编辑',
            icon: <FileText size={16} />,
            run: () => p.onEdit?.(single.path),
          },
        ]
      : []),
    ...(menuEntries.length
      ? [
          {
            label: p.remote ? '下载到本地' : '上传到远程',
            icon: p.remote ? <Download size={16} /> : <Upload size={16} />,
            disabled: p.transferDisabled,
            run: () => p.onTransfer(menuEntries),
          },
        ]
      : []),
  ];
  const archiveActions: MenuAction[] = [
    ...(p.onArchive && single && !single.directory && !single.symlink && isArchive(single.path)
      ? [
          {
            label: '查看压缩包 / 解压',
            icon: <Archive size={16} />,
            run: () => p.onArchive?.(single.path),
          },
        ]
      : []),
    ...(p.onCompress && menuEntries.length && menuEntries.every((entry) => !entry.symlink)
      ? [
          {
            label: '创建压缩包…',
            icon: <Archive size={16} />,
            run: () => p.onCompress?.(menuEntries.map((entry) => entry.path)),
          },
        ]
      : []),
  ];
  const manageActions: MenuAction[] = [
    ...(menuEntries.length
      ? [
          {
            label: '重命名',
            icon: <Pencil size={16} />,
            disabled: !single,
            run: () => single && p.onRename(single.path),
          },
        ]
      : []),
    {
      label:
        menuEntries.length > 1
          ? '复制所选路径'
          : menuEntries.length
            ? '复制路径'
            : '复制当前目录路径',
      icon: <Copy size={16} />,
      run: copyPath,
    },
  ];
  const permissionActions: MenuAction[] = [
    ...(p.onProperties && menuEntries.length
      ? [
          {
            label: p.remote ? '属性与权限' : '属性',
            icon: <ShieldCheck size={16} />,
            disabled: !single,
            run: () => single && p.onProperties?.(single.path),
          },
        ]
      : []),
    ...(p.onBatchPermissions && menuEntries.length
      ? [
          {
            label: '批量修改权限',
            icon: <ShieldCheck size={16} />,
            run: () => p.onBatchPermissions?.(menuEntries.map((file) => file.path)),
          },
        ]
      : []),
  ];
  const directoryActions: MenuAction[] = [
    { label: '新建文件夹', icon: <FolderPlus size={16} />, run: p.onMkdir },
    { label: '刷新目录', icon: <RefreshCw size={16} />, run: p.onRefresh },
  ];
  if (p.onBatchRename && menuEntries.length > 1) {
    manageActions.push({
      label: '批量重命名',
      icon: <Pencil size={16} />,
      disabled:
        menuEntries.length > 100 || menuEntries.some((entry) => entry.directory || entry.symlink),
      run: () => p.onBatchRename?.(menuEntries),
    });
  }
  const deleteActions: MenuAction[] = menuEntries.length
    ? [
        {
          label: menuEntries.length > 1 ? `删除 ${menuEntries.length} 项` : '删除',
          icon: <Trash2 size={16} />,
          danger: true,
          run: () => p.onDelete(menuEntries.map((file) => file.path)),
        },
      ]
    : [];
  // 先移除空组，避免目录空白区、本地文件或多选菜单出现开头/连续分隔线。
  const menuActions = [
    openActions,
    archiveActions,
    manageActions,
    permissionActions,
    directoryActions,
    deleteActions,
  ]
    .filter((group) => group.length)
    .flatMap((group, groupIndex) =>
      group.map((action, index) => ({
        ...action,
        separator: groupIndex > 0 && index === 0,
      })),
    );
  return {
    openContext,
    menu: context ? (
      <FileContextMenu x={context.x} y={context.y} actions={menuActions} onClose={closeContext} />
    ) : null,
  };
}
