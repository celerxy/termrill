/** 菜单与设置共用归档能力；后端仍独立校验格式与密码，不根据后缀直接信任内容。 */
export const archiveWriteFormats = [
  { id: 'zip', label: 'ZIP', suffix: '.zip', encrypted: true },
  { id: '7z', label: '7Z', suffix: '.7z', encrypted: true },
  { id: 'tar', label: 'TAR', suffix: '.tar', encrypted: false },
  { id: 'tar.gz', label: 'TAR.GZ', suffix: '.tar.gz', encrypted: false },
  { id: 'tar.bz2', label: 'TAR.BZ2', suffix: '.tar.bz2', encrypted: false },
  { id: 'tar.xz', label: 'TAR.XZ', suffix: '.tar.xz', encrypted: false },
] as const;
export type ArchiveWriteFormat = (typeof archiveWriteFormats)[number]['id'];
const suffix = /\.(?:tar\.(?:gz|bz2|xz)|zip|7z|rar|tar|tgz|tbz2?|txz|gz|bz2|xz)$/i;
export function archiveWriteFormat(path: string): ArchiveWriteFormat | null {
  const lower = path.toLowerCase();
  if (lower.endsWith('.tgz')) return 'tar.gz';
  if (/\.tbz2?$/.test(lower)) return 'tar.bz2';
  if (lower.endsWith('.txz')) return 'tar.xz';
  return archiveWriteFormats.find((format) => lower.endsWith(format.suffix))?.id ?? null;
}
export function archiveNameForFormat(name: string, format: ArchiveWriteFormat) {
  return `${name.replace(suffix, '') || 'archive'}.${format}`;
}
export function archiveCanEncrypt(format: ArchiveWriteFormat | null) {
  return format === 'zip' || format === '7z';
}
export function archiveCapabilityText(path: string) {
  const format = archiveWriteFormat(path);
  if (format === 'zip')
    return 'ZIP：浏览、预览、校验、解压、创建和增删；支持 AES-256 内容加密，文件名可见。';
  if (format === '7z')
    return '7Z：浏览、预览、校验、解压、创建和增删重建；支持 AES-256 内容及文件名加密。';
  if (format)
    return 'TAR 系列：浏览、预览、校验、解压、创建和增删重建；不支持密码，不保留原归档的所有者、时间及特殊权限。';
  return /\.rar$/i.test(path)
    ? 'RAR：仅浏览、预览、校验和解压，支持读取密码包；不支持创建、修改或分卷。'
    : '单文件压缩流：仅浏览、预览、校验和解压；创建多文件归档请选择 TAR 系列。';
}
