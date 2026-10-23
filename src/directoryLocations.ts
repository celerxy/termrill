/** 本机目录快捷入口；只保存路径，不保存凭据。远程记录由配置及端点身份隔离。 */
export type DirectoryLocations = { favorites: string[]; recent: string[] };
type StoredLocations = { version: 1; scopes: Record<string, DirectoryLocations> };
type LocationStorage = Pick<Storage, 'getItem' | 'setItem'>;
const key = 'termrill.directory-locations.v1';
const validPath = (path: unknown): path is string =>
  typeof path === 'string' && path.length > 0 && path.length <= 4096 && !path.includes('\0');

function read(storage: LocationStorage): StoredLocations {
  const raw = storage.getItem(key);
  if (!raw) return { version: 1, scopes: {} };
  const value = JSON.parse(raw) as StoredLocations;
  if (
    !value ||
    value.version !== 1 ||
    !value.scopes ||
    typeof value.scopes !== 'object' ||
    Array.isArray(value.scopes)
  ) {
    throw new Error('目录收藏与历史记录无法读取，请清除本站目录记录后重试');
  }
  for (const entry of Object.values(value.scopes)) {
    if (
      !entry ||
      !Array.isArray(entry.favorites) ||
      !Array.isArray(entry.recent) ||
      entry.favorites.length > 50 ||
      entry.recent.length > 20 ||
      !entry.favorites.every(validPath) ||
      !entry.recent.every(validPath)
    ) {
      throw new Error('目录收藏与历史记录格式异常，未覆盖原记录');
    }
  }
  return value;
}

export function getDirectoryLocations(storage: LocationStorage, scope: string): DirectoryLocations {
  const scopes = read(storage).scopes;
  return Object.hasOwn(scopes, scope) ? scopes[scope] : { favorites: [], recent: [] };
}

export function updateDirectoryLocations(
  storage: LocationStorage,
  scope: string,
  change: (locations: DirectoryLocations) => DirectoryLocations,
) {
  const stored = read(storage);
  if (!Object.hasOwn(stored.scopes, scope) && Object.keys(stored.scopes).length >= 100) {
    throw new Error('目录记录已达到 100 个连接，请先清理不再使用的连接目录记录');
  }
  const previous = Object.hasOwn(stored.scopes, scope)
    ? stored.scopes[scope]
    : { favorites: [], recent: [] };
  const next = change({ favorites: [...previous.favorites], recent: [...previous.recent] });
  if (next.favorites.length > 50)
    throw new Error('当前连接最多收藏 50 个目录，请先移除不常用的收藏');
  // 不缓存整份旧数据，多个标签修改不同连接时保留刚保存的其他连接记录。
  if (next.favorites.length === 0 && next.recent.length === 0) {
    delete stored.scopes[scope];
  } else {
    stored.scopes[scope] = next;
  }
  storage.setItem(key, JSON.stringify(stored));
  return next;
}

export function visitDirectory(storage: LocationStorage, scope: string, path: string) {
  if (!validPath(path)) throw new Error('目录路径为空或过长，无法记录');
  return updateDirectoryLocations(storage, scope, (previous) => ({
    ...previous,
    recent: [path, ...previous.recent.filter((item) => item !== path)].slice(0, 20),
  }));
}

export function toggleDirectoryFavorite(storage: LocationStorage, scope: string, path: string) {
  if (!validPath(path)) throw new Error('目录路径为空或过长，无法收藏');
  return updateDirectoryLocations(storage, scope, (previous) => ({
    ...previous,
    favorites: previous.favorites.includes(path)
      ? previous.favorites.filter((item) => item !== path)
      : [...previous.favorites, path],
  }));
}
