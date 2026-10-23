/** 当前面板的目录收藏和最近访问；导航仍复用原目录读取和权限校验。 */
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Bookmark, Clock, Star, X } from 'lucide-react';
import {
  type DirectoryLocations as Locations,
  getDirectoryLocations,
  visitDirectory,
  toggleDirectoryFavorite,
  updateDirectoryLocations,
} from './directoryLocations';
import './directoryLocations.css';

const changed = 'termrill-directory-locations-changed';
export default function DirectoryLocationMenu({
  scope,
  path,
  disabled,
  onNavigate,
  onError,
}: {
  scope?: string;
  path: string;
  disabled: boolean;
  onNavigate: (path: string) => void;
  onError: (error: unknown) => void;
}) {
  const [locations, setLocations] = useState<Locations>({ favorites: [], recent: [] });
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const popover = useRef<HTMLElement>(null);
  const [position, setPosition] = useState({ left: 0, top: 0 });
  const error = useRef(onError);
  error.current = onError;
  useEffect(() => {
    setOpen(false);
    if (!scope) {
      setLocations({ favorites: [], recent: [] });
      return;
    }
    const reload = () => {
      try {
        setLocations(getDirectoryLocations(localStorage, scope));
      } catch (e) {
        error.current(e);
      }
    };
    try {
      if (path) {
        visitDirectory(localStorage, scope, path);
        window.dispatchEvent(new Event(changed));
      }
    } catch (e) {
      error.current(e);
    }
    reload();
    window.addEventListener(changed, reload);
    window.addEventListener('storage', reload);
    return () => {
      window.removeEventListener(changed, reload);
      window.removeEventListener('storage', reload);
    };
  }, [scope, path]);
  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => {
      if (
        !root.current?.contains(event.target as Node) &&
        !popover.current?.contains(event.target as Node)
      )
        setOpen(false);
    };
    document.addEventListener('pointerdown', close);
    return () => document.removeEventListener('pointerdown', close);
  }, [open]);
  useLayoutEffect(() => {
    if (!open || !trigger.current) return;
    const bounds = trigger.current.getBoundingClientRect();
    const width = Math.min(350, window.innerWidth * 0.7);
    setPosition({
      left: Math.max(8, Math.min(bounds.right - width, window.innerWidth - width - 8)),
      top: Math.max(8, Math.min(bounds.bottom + 8, window.innerHeight - 350)),
    });
    const close = (event: Event) => {
      if (!popover.current?.contains(event.target as Node)) setOpen(false);
    };
    window.addEventListener('resize', close);
    window.addEventListener('scroll', close, true);
    return () => {
      window.removeEventListener('resize', close);
      window.removeEventListener('scroll', close, true);
    };
  }, [open]);
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);
  function change(update: () => Locations) {
    try {
      setLocations(update());
      window.dispatchEvent(new Event(changed));
    } catch (e) {
      onError(e);
    }
  }
  const favorite = locations.favorites.includes(path);
  return (
    <div
      className="directory-locations"
      ref={root}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && open) {
          event.preventDefault();
          event.stopPropagation();
          setOpen(false);
          trigger.current?.focus();
        }
      }}
    >
      <button
        type="button"
        disabled={disabled || !scope || !path}
        aria-pressed={favorite}
        aria-label={favorite ? '取消收藏当前目录' : '收藏当前目录'}
        title={favorite ? '取消收藏当前目录' : '收藏当前目录'}
        onClick={() => scope && change(() => toggleDirectoryFavorite(localStorage, scope, path))}
      >
        <Star size={15} fill={favorite ? 'currentColor' : 'none'} />
      </button>
      <button
        type="button"
        ref={trigger}
        aria-label="收藏与最近目录"
        title="收藏与最近目录"
        aria-expanded={open}
        disabled={disabled || !scope}
        onClick={() => setOpen(!open)}
      >
        <Clock size={15} />
      </button>
      {open &&
        scope &&
        createPortal(
          <section
            ref={popover}
            className="directory-location-popover"
            style={position}
            aria-label="收藏与最近目录"
          >
            {(['favorites', 'recent'] as const).map((kind) => (
              <div key={kind}>
                <div className="directory-location-heading">
                  {kind === 'favorites' ? <Bookmark size={13} /> : <Clock size={13} />}
                  <strong>{kind === 'favorites' ? '收藏目录' : '最近访问'}</strong>
                  {kind === 'recent' && locations.recent.length > 0 && (
                    <button
                      type="button"
                      onClick={() =>
                        change(() =>
                          updateDirectoryLocations(localStorage, scope, (value) => ({
                            ...value,
                            recent: [],
                          })),
                        )
                      }
                    >
                      清空
                    </button>
                  )}
                </div>
                {locations[kind].length === 0 && <p>暂无记录</p>}
                {locations[kind].map((location) => (
                  <div className="directory-location-row" key={location}>
                    <button
                      type="button"
                      title={location}
                      onClick={() => {
                        setOpen(false);
                        onNavigate(location);
                      }}
                    >
                      {location}
                    </button>
                    {kind === 'favorites' && (
                      <button
                        type="button"
                        aria-label={`取消收藏 ${location}`}
                        onClick={() =>
                          change(() => toggleDirectoryFavorite(localStorage, scope, location))
                        }
                      >
                        <X size={13} />
                      </button>
                    )}
                  </div>
                ))}
              </div>
            ))}
            <small>路径仅保存在此设备；远程记录按连接隔离</small>
          </section>,
          document.body,
        )}
    </div>
  );
}
