/** 压缩包名称识别用于菜单；实际格式、压缩方法与密码由后端校验。 */
export const isArchive = (path: string) =>
  /\.(zip|7z|rar|tar|tgz|tbz2?|txz|gz|bz2|xz)$/i.test(path);
export type ArchiveEntry = {
  name: string;
  size: number;
  compressed: number;
  directory: boolean;
  encrypted: boolean;
  safe: boolean;
};
export type ArchiveReply = {
  entries: ArchiveEntry[];
  revision: string;
  message: string;
  bytes: string | null;
};
