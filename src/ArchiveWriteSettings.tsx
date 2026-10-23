/** 新建和重建共用写入选项；密码只保存在当前弹窗内存，不进入任务报告。 */
import {
  archiveCanEncrypt,
  archiveWriteFormats,
  type ArchiveWriteFormat,
} from './archiveCapabilities';
export default function ArchiveWriteSettings({
  format,
  onFormatChange,
  compression,
  onCompressionChange,
  password,
  onPasswordChange,
  busy,
}: {
  format: ArchiveWriteFormat;
  onFormatChange?: (format: ArchiveWriteFormat) => void;
  compression: string;
  onCompressionChange: (value: string) => void;
  password?: string;
  onPasswordChange?: (value: string) => void;
  busy: boolean;
}) {
  const store = format === 'zip' || format === '7z';
  return (
    <fieldset disabled={busy} className="archive-write-settings">
      {onFormatChange && (
        <label className="archive-setting-field">
          归档格式
          <select
            aria-label="归档格式"
            value={format}
            onChange={(event) => onFormatChange(event.target.value as ArchiveWriteFormat)}
          >
            {archiveWriteFormats.map((item) => (
              <option key={item.id} value={item.id}>
                {item.label}
              </option>
            ))}
          </select>
        </label>
      )}
      <label className="archive-setting-field">
        {onFormatChange ? '压缩等级' : '新增 / 重建压缩等级'}
        <select
          aria-label="压缩等级"
          value={format === 'tar' ? 'normal' : compression}
          disabled={busy || format === 'tar'}
          onChange={(event) => onCompressionChange(event.target.value)}
        >
          {store && <option value="store">仅存储，不压缩</option>}
          <option value="fast">快速压缩</option>
          <option value="normal">标准压缩</option>
        </select>
      </label>
      {onPasswordChange && (
        <label className="archive-setting-field">
          {archiveCanEncrypt(format) ? '加密密码（留空不加密）' : '此格式不支持密码'}
          <input
            aria-label="创建归档密码"
            type="password"
            autoComplete="new-password"
            maxLength={1024}
            disabled={busy || !archiveCanEncrypt(format)}
            value={archiveCanEncrypt(format) ? password : ''}
            onChange={(event) => onPasswordChange(event.target.value)}
          />
        </label>
      )}
      <small>
        {format === 'zip'
          ? 'ZIP AES-256：保护文件内容，文件名仍可见；部分系统自带解压器不支持 AES。'
          : format === '7z'
            ? '7Z 使用非固实 LZMA2；设置密码时同时加密文件名。'
            : 'TAR 系列不加密；重建保留内容与目录，时间、所有者和权限使用标准值。'}
      </small>
    </fieldset>
  );
}
