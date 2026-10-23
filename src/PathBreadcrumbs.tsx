/** 本地/远程目录面包屑；Windows 盘符与 UNC 根保持完整，编辑路径仍交给原导航回调。 */
import { useEffect, useRef, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import LocalDriveSelect from './LocalDriveSelect';

function segments(path: string, remote: boolean) {
  const windows = !remote && (/^[a-z]:[\\/]/i.test(path) || path.startsWith('\\\\'));
  const separator = windows ? '\\' : '/';
  const normalized = windows ? path.replace(/\//g, '\\') : path;
  let root = '/';
  let rest = normalized.slice(1);
  if (windows) {
    const unc = normalized.match(/^\\\\[^\\]+\\[^\\]+(?:\\|$)/);
    root = unc ? unc[0].replace(/\\?$/, '\\') : normalized.slice(0, 3);
    rest = normalized.slice(root.length);
  } else if (!normalized.startsWith('/')) {
    return [{ label: path, path }];
  }
  const result = [{ label: root, path: root }];
  let current = root;
  for (const part of rest.split(separator).filter(Boolean)) {
    current = `${current.endsWith(separator) ? current : current + separator}${part}`;
    result.push({ label: part, path: current });
  }
  return result;
}

export default function PathBreadcrumbs({
  path,
  remote,
  disabled,
  loading,
  onNavigate,
  onError,
}: {
  path: string;
  remote?: boolean;
  disabled?: boolean;
  loading: boolean;
  onNavigate: (path: string) => void;
  onError: (error: unknown) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(path);
  const input = useRef<HTMLInputElement>(null);
  const nav = useRef<HTMLElement>(null);
  useEffect(() => {
    setDraft(path);
    setEditing(false);
    if (nav.current) nav.current.scrollLeft = nav.current.scrollWidth;
  }, [path]);
  useEffect(() => {
    if (editing) {
      input.current?.focus();
      input.current?.select();
    }
  }, [editing]);
  const edit = () => {
    if (disabled || loading) return;
    setDraft(path);
    setEditing(true);
  };
  return (
    <>
      {editing || !path ? (
        <input
          ref={input}
          aria-label={remote ? '远程路径' : '本地路径'}
          placeholder={remote ? '连接后浏览远程目录' : '输入本地目录，按 Enter'}
          value={draft}
          disabled={disabled || loading}
          onChange={(event) => setDraft(event.target.value)}
          onBlur={() => setEditing(false)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && draft.trim()) {
              setEditing(false);
              onNavigate(draft.trim());
            }
            if (event.key === 'Escape') {
              setDraft(path);
              setEditing(false);
            }
          }}
        />
      ) : (
        <nav
          className="file-path-crumbs"
          ref={nav}
          aria-label={remote ? '远程目录导航' : '本地目录导航'}
          title="双击空白处或当前目录编辑完整路径，聚焦后按 F2 也可编辑"
          tabIndex={disabled || loading ? -1 : 0}
          onDoubleClick={(event) => {
            const target = event.target as HTMLElement;
            if (target.closest('button:not([aria-current="location"])')) return;
            edit();
          }}
          onKeyDown={(event) => {
            if (
              event.key === 'F2' ||
              (event.key === 'Enter' && event.target === event.currentTarget)
            ) {
              event.preventDefault();
              edit();
            }
          }}
        >
          {segments(path, !!remote).map((part, index, parts) => (
            <span key={part.path}>
              {index > 0 && <ChevronRight size={12} aria-hidden="true" />}
              {index === 0 && !remote && /^[a-z]:[\\/]/i.test(path) ? (
                <LocalDriveSelect
                  path={path}
                  disabled={!!disabled || loading}
                  onNavigate={onNavigate}
                  onError={onError}
                />
              ) : (
                <button
                  title={part.path}
                  disabled={disabled || loading}
                  aria-current={index === parts.length - 1 ? 'location' : undefined}
                  onClick={() => {
                    if (part.path !== path) onNavigate(part.path);
                  }}
                >
                  {part.label}
                </button>
              )}
            </span>
          ))}
        </nav>
      )}
    </>
  );
}
