import type { SyncReport } from './directorySyncModel';
export type Endpoint = {
  host: string;
  port: number;
  username: string;
  auth: 'password' | 'key' | 'agent' | 'interactive';
  keyPath: string;
};
export type Profile = {
  id: string;
  name: string;
  target: Endpoint;
  jump: Endpoint | null;
};
export type Secrets = {
  password: string;
  passphrase: string;
};
export type ConnectRequest = {
  profile: Profile;
  targetSecrets: Secrets;
  jumpSecrets: Secrets;
};
export type HostKey = {
  address: string;
  fingerprint: string;
  previous: string | null;
};
export type ApiError = {
  message: string;
  hostKey?: HostKey;
};
export type FileEntry = {
  name: string;
  path: string;
  directory: boolean;
  symlink: boolean;
  size: number;
  modified: number | null;
};
export type Listing = {
  path: string;
  parent: string | null;
  entries: FileEntry[];
  page?: { offset: number; size: number; hasMore: boolean; query: string; showHidden: boolean };
};
export type Transfer = {
  syncReport?: SyncReport;
  id: string;
  connectionId: string;
  name: string;
  direction: 'upload' | 'download';
  status:
    | 'queued'
    | 'scanning'
    | 'connecting'
    | 'prechecking'
    | 'preparing'
    | 'running'
    | 'completed'
    | 'cancelled'
    | 'failed'
    | 'cancelling'
    | 'paused';
  bytes: number;
  total: number;
  speed: number;
  remainingSeconds?: number | null;
  error: string | null;
  /** 仅界面观察的单调时间；不通过 IPC 发送，不写入恢复记录。 */
  observation?: { eventAt: number; advancedAt: number };
  /** 本次尝试的全程计时，包含排队和暂停；结束后冻结，重试重新开始。 */
  timerStartedAt?: number;
  timerEndedAt?: number;
  timerEstimated?: boolean;
  /** 仅界面使用的单调时钟；连续扫描进度不能重置计时。 */
  scanStartedAt?: number;
  scanTimerEstimated?: boolean;
};
export type TransferPlan = {
  syncReport?: SyncReport;
  selection?: string[];
  local: string;
  remote: string;
  direction: 'upload' | 'download';
  name: string;
};
export const emptyEndpoint = (): Endpoint => ({
  host: '',
  port: 22,
  username: '',
  auth: 'password',
  keyPath: '',
});
export const emptySecrets = (): Secrets => ({ password: '', passphrase: '' });
export const message = (e: unknown) =>
  typeof e === 'object' && e !== null && 'message' in e ? String(e.message) : String(e);
export const formatSize = (bytes: number) => {
  if (bytes < 1024) return `${bytes} B`;
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), 4);
  return `${(bytes / 1024 ** i).toFixed(1)} ${['B', 'KB', 'MB', 'GB', 'TB'][i]}`;
};
export const joinPath = (base: string, name: string, remote = false) =>
  `${base.replace(/[\\/]$/, '')}${remote ? '/' : base.includes('\\') ? '\\' : '/'}${name}`;
export const basename = (path: string) => path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
export const finished = (t: Transfer) => ['completed', 'cancelled', 'failed'].includes(t.status);
