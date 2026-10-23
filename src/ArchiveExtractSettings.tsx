/** 解压设置独立于包内列表，密码默认收起且不持久化。 */
import { ChevronRight, LockKeyhole } from 'lucide-react';
import type { useArchiveController } from './useArchiveController';
import { archiveCapabilityText, archiveWriteFormat } from './archiveCapabilities';
import ArchiveWriteSettings from './ArchiveWriteSettings';
type Settings = Pick<
  ReturnType<typeof useArchiveController>,
  | 'busy'
  | 'targetMode'
  | 'target'
  | 'policy'
  | 'password'
  | 'chooseTarget'
  | 'setTargetMode'
  | 'setTarget'
  | 'setPolicy'
  | 'setPassword'
  | 'compression'
  | 'setCompression'
> & { archivePath: string };
const destinations = [
  { value: 'folder', title: '同名文件夹', description: '在当前目录下创建同名文件夹' },
  { value: 'current', title: '当前目录', description: '解压到当前所在位置' },
  { value: 'custom', title: '自定义', description: '输入其他目录的绝对路径' },
];
export default function ArchiveExtractSettings(props: Settings) {
  return (
    <aside className="archive-settings" aria-label="解压设置">
      <p className="archive-settings-hint">{archiveCapabilityText(props.archivePath)}</p>
      <fieldset disabled={props.busy}>
        <legend>解压到</legend>
        <select
          aria-label="解压到"
          value={props.targetMode}
          onChange={(event) => props.chooseTarget(event.target.value)}
        >
          {destinations.map((destination) => (
            <option key={destination.value} value={destination.value}>
              {destination.title}
            </option>
          ))}
        </select>
        <small>
          {destinations.find((destination) => destination.value === props.targetMode)?.description}
        </small>
      </fieldset>
      <label className="archive-setting-field">
        目标路径
        <input
          disabled={props.busy}
          value={props.target}
          title={props.target}
          onChange={(event) => {
            props.setTargetMode('custom');
            props.setTarget(event.target.value);
          }}
        />
      </label>
      <label className="archive-setting-field">
        同名文件
        <select
          disabled={props.busy}
          value={props.policy}
          onChange={(event) => props.setPolicy(event.target.value)}
        >
          <option value="stop">停止并提示</option>
          <option value="skip">跳过现有文件</option>
          <option value="overwrite">覆盖同名文件</option>
        </select>
      </label>
      <details className="archive-password">
        <summary>
          <ChevronRight size={17} />
          <LockKeyhole size={17} />
          加密文件密码
        </summary>
        <input
          aria-label="压缩包密码"
          type="password"
          autoComplete="off"
          disabled={props.busy}
          placeholder="仅加密文件需要"
          value={props.password}
          onChange={(event) => props.setPassword(event.target.value)}
        />
        <small>密码仅用于当前操作，不保存。</small>
      </details>
      {archiveWriteFormat(props.archivePath) && (
        <ArchiveWriteSettings
          format={archiveWriteFormat(props.archivePath)!}
          compression={props.compression}
          onCompressionChange={props.setCompression}
          busy={props.busy}
        />
      )}
      <p className="archive-settings-hint">选择文件可单独解压；未选择时解压全部。</p>
    </aside>
  );
}
