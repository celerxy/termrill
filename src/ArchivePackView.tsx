/** 来源列表只读展示已有选择；设置编辑不触发文件扫描，开始压缩时才访问文件系统。 */
import { Archive, Folder, LockKeyhole } from 'lucide-react';
import ArchivePackSources from './ArchivePackSources';
import ArchiveWriteSettings from './ArchiveWriteSettings';
import type { ArchiveWriteFormat } from './archiveCapabilities';
import './archivePackSettings.css';
type Props = {
  format: ArchiveWriteFormat;
  onFormatChange: (format: ArchiveWriteFormat) => void;
  compression: string;
  onCompressionChange: (value: string) => void;
  password: string;
  onPasswordChange: (value: string) => void;
  sources: string[];
  remote: boolean;
  busy: boolean;
  name: string;
  directory: string;
  policy: string;
  onNameChange: (name: string) => void;
  onDirectoryChange: (directory: string) => void;
  onPolicyChange: (policy: string) => void;
};
export default function ArchivePackView(props: Props) {
  return (
    <div className="archive-workspace archive-pack-workspace">
      <ArchivePackSources sources={props.sources} />
      <aside className="archive-settings archive-pack-settings" aria-label="压缩设置">
        <div className="pack-settings-heading">
          <h3>压缩设置</h3>
          {props.busy && <span className="pack-settings-badge">运行中</span>}
          {props.busy && <LockKeyhole size={16} aria-hidden="true" />}
        </div>
        <p className="pack-settings-subtitle">
          {props.busy ? '本次任务设置' : '设置压缩包名称与保存位置'}
        </p>
        <ArchiveWriteSettings
          format={props.format}
          onFormatChange={props.onFormatChange}
          compression={props.compression}
          onCompressionChange={props.onCompressionChange}
          password={props.password}
          onPasswordChange={props.onPasswordChange}
          busy={props.busy}
        />
        <label className="archive-setting-field">
          压缩包名称
          <span className="pack-setting-input">
            <Archive size={18} aria-hidden="true" />
            <input
              value={props.name}
              readOnly={props.busy}
              aria-readonly={props.busy}
              onChange={(event) => props.onNameChange(event.target.value)}
              placeholder="archive.zip"
              spellCheck={false}
            />
          </span>
        </label>
        <label className="archive-setting-field">
          目标目录
          <span className="pack-setting-input">
            <Folder size={18} aria-hidden="true" />
            <input
              value={props.directory}
              readOnly={props.busy}
              aria-readonly={props.busy}
              onChange={(event) => props.onDirectoryChange(event.target.value)}
              title={props.directory}
              spellCheck={false}
            />
          </span>
          {!props.busy && <small>输入{props.remote ? '远程' : '本地'}目录的绝对路径</small>}
        </label>
        <fieldset disabled={props.busy} className="archive-pack-policy">
          <legend>同名文件</legend>
          <div className="pack-policy-options">
            <label className="archive-destination" title="保留现有压缩包，停止并提示">
              <input
                type="radio"
                name="archive-pack-policy"
                checked={props.policy === 'stop'}
                onChange={() => props.onPolicyChange('stop')}
              />
              <span>
                <strong>停止并提示</strong>
              </span>
            </label>
            <label className="archive-destination" title="替换已有同名压缩包">
              <input
                type="radio"
                name="archive-pack-policy"
                checked={props.policy === 'overwrite'}
                onChange={() => props.onPolicyChange('overwrite')}
              />
              <span>
                <strong>覆盖同名文件</strong>
              </span>
            </label>
          </div>
        </fieldset>
        <div className="pack-settings-footnote">
          <span>
            {props.format.toUpperCase()} ·{' '}
            {props.password && ['zip', '7z'].includes(props.format) ? '加密' : '不加密'}
          </span>
          {props.busy && (
            <span>
              <LockKeyhole size={13} aria-hidden="true" />
              任务进行中，设置已锁定
            </span>
          )}
        </div>
      </aside>
    </div>
  );
}
