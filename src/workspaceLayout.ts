/** 仅持久化布局尺寸，不与连接配置或凭据存储混合。 */
export type WorkspaceLayout = { panelRatio: number; terminalHeight: number };
export const defaultWorkspaceLayout: WorkspaceLayout = { panelRatio: 50, terminalHeight: 180 };
export const workspaceLayoutKey = 'termrill.workspace-layout.v1';
export function normalizeWorkspaceLayout(value: unknown): WorkspaceLayout {
  const candidate = value as Partial<WorkspaceLayout> | null;
  const bounded = (input: unknown, fallback: number, min: number, max: number) =>
    typeof input === 'number' && Number.isFinite(input)
      ? Math.min(max, Math.max(min, input))
      : fallback;
  return {
    panelRatio: bounded(candidate?.panelRatio, 50, 30, 70),
    terminalHeight: bounded(candidate?.terminalHeight, 180, 100, 480),
  };
}
