/** 轻量文件类型识别，避免文件列表首屏加载编辑器与语言包。 */
export function fileKind(path: string): 'text' | 'image' | 'pdf' | 'epub' {
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  if (['png', 'jpg', 'jpeg', 'jpe', 'jfif', 'gif', 'webp', 'bmp', 'ico', 'avif'].includes(ext))
    return 'image';
  if (ext === 'pdf') return 'pdf';
  if (ext === 'epub') return 'epub';
  return 'text';
}
