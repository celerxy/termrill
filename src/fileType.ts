/** 根据已有属性和文件名显示类型；不读取文件内容，也不探测链接目标。 */
import type { FileEntry } from './types';

const types: Record<string, string> = {
  xls: 'Excel 表格',
  xlsx: 'Excel 表格',
  xlsm: 'Excel 表格',
  csv: 'CSV 表格',
  doc: 'Word 文档',
  docx: 'Word 文档',
  ppt: 'PowerPoint 演示',
  pptx: 'PowerPoint 演示',
  pdf: 'PDF 文档',
  txt: '文本文件',
  md: 'Markdown 文档',
  log: '日志文件',
  png: 'PNG 图片',
  jpg: 'JPEG 图片',
  jpeg: 'JPEG 图片',
  gif: 'GIF 图片',
  webp: 'WebP 图片',
  svg: 'SVG 图片',
  bmp: 'BMP 图片',
  ico: '图标文件',
  zip: 'ZIP 压缩包',
  rar: 'RAR 压缩包',
  '7z': '7z 压缩包',
  tar: 'TAR 归档',
  gz: 'Gzip 压缩包',
  bz2: 'Bzip2 压缩包',
  xz: 'XZ 压缩包',
  mp3: 'MP3 音频',
  wav: 'WAV 音频',
  flac: 'FLAC 音频',
  mp4: 'MP4 视频',
  mov: 'MOV 视频',
  mkv: 'MKV 视频',
  avi: 'AVI 视频',
  json: 'JSON 文件',
  yaml: 'YAML 配置',
  yml: 'YAML 配置',
  toml: 'TOML 配置',
  ini: 'INI 配置',
  xml: 'XML 文件',
  conf: '配置文件',
  js: 'JavaScript 源码',
  jsx: 'JavaScript 源码',
  ts: 'TypeScript 源码',
  tsx: 'TypeScript 源码',
  html: 'HTML 文件',
  css: 'CSS 样式',
  py: 'Python 源码',
  rs: 'Rust 源码',
  go: 'Go 源码',
  java: 'Java 源码',
  c: 'C 源码',
  cpp: 'C++ 源码',
  h: '头文件',
  sh: 'Shell 脚本',
  ps1: 'PowerShell 脚本',
  bat: '批处理脚本',
  cmd: '批处理脚本',
  exe: '应用程序',
  msi: '安装包',
  dmg: '磁盘映像',
  iso: '磁盘映像',
  dll: '动态库',
  so: '共享库',
  dylib: '动态库',
  sql: 'SQL 文件',
  db: '数据库文件',
};

export function fileType(entry: FileEntry): string {
  // 链接优先：扩展名只能描述链接名称，不能证明目标文件类型。
  if (entry.symlink) return '链接';
  if (entry.directory) return '文件夹';
  const name = entry.name.toLowerCase();
  if (name.endsWith('.tar.gz')) return 'TAR/Gzip 归档';
  if (name.endsWith('.tar.bz2')) return 'TAR/Bzip2 归档';
  if (name.endsWith('.tar.xz')) return 'TAR/XZ 归档';
  const dot = name.lastIndexOf('.');
  // .gitignore 等单段隐藏名称不把整段当作扩展名。
  if (dot <= 0 || dot === name.length - 1) return '文件';
  const extension = name.slice(dot + 1);
  return Object.prototype.hasOwnProperty.call(types, extension)
    ? types[extension]
    : `${extension.toUpperCase()} 文件`;
}
