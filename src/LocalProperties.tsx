/** 本地属性只展示系统实际提供的信息，不把 Windows 只读属性当作 POSIX 权限。 */
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { File, Folder, Link2, ShieldCheck, X } from 'lucide-react';
import { basename, formatSize, message } from './types';
import { fileType } from './fileType';
import './localProperties.css';

type Properties = {
  path: string;
  kind: string;
  size: number;
  readonly: boolean;
  modified: number | null;
  created: number | null;
  accessed: number | null;
};
export default function LocalProperties({ path, onClose }: { path: string; onClose: () => void }) {
  const [properties, setProperties] = useState<Properties | null>(null);
  const [error, setError] = useState('');
  const panel = useRef<HTMLElement>(null);
  useEffect(() => {
    let disposed = false;
    void invoke<Properties>('local_properties', { path })
      .then((value) => {
        if (!disposed) setProperties(value);
      })
      .catch((reason) => {
        if (!disposed) setError(message(reason));
      });
    return () => {
      disposed = true;
    };
  }, [path]);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    panel.current?.focus();
    return () => {
      previous?.focus();
    };
  }, []);
  const date = (value: number | null) =>
    value === null ? '不可用' : new Date(value * 1000).toLocaleString();
  const name = basename(path);
  const directory = path.slice(0, Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\')) + 1);
  const extension = name.lastIndexOf('.') > 0 ? name.slice(name.lastIndexOf('.') + 1) : '';
  const badge =
    properties?.kind === '文件夹'
      ? '文件夹'
      : properties?.kind === '链接'
        ? '链接'
        : extension.toUpperCase() || '文件';
  const Icon = properties?.kind === '文件夹' ? Folder : properties?.kind === '链接' ? Link2 : File;
  const type =
    properties?.kind === '文件'
      ? fileType({
          name,
          path,
          directory: false,
          symlink: false,
          size: properties.size,
          modified: properties.modified,
        })
      : properties?.kind;
  return (
    <div className="modal-backdrop">
      <section
        ref={panel}
        tabIndex={-1}
        className="modal local-properties"
        role="dialog"
        aria-modal="true"
        aria-labelledby="local-properties-title"
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.stopPropagation();
            onClose();
          }
          if (event.key === 'Tab') {
            const buttons = panel.current?.querySelectorAll<HTMLButtonElement>('button');
            if (!buttons?.length) return;
            if (event.shiftKey && document.activeElement === buttons[0]) {
              event.preventDefault();
              buttons[buttons.length - 1].focus();
            } else if (!event.shiftKey && document.activeElement === buttons[buttons.length - 1]) {
              event.preventDefault();
              buttons[0].focus();
            }
          }
        }}
      >
        <header>
          <File size={22} />
          <h2 id="local-properties-title">本地属性</h2>
          <button aria-label="关闭属性" onClick={onClose}>
            <X size={18} />
          </button>
        </header>
        <div className="local-properties-body">
          <div className="local-property-summary">
            <div className="local-property-icon">
              <Icon size={36} />
              <span title={badge}>{badge}</span>
            </div>
            <div className="local-property-identity">
              <h3 title={name}>{name}</h3>
              <p title={directory}>{directory}</p>
            </div>
          </div>
          {error ? (
            <p className="local-property-error" role="alert">
              {error}
            </p>
          ) : !properties ? (
            <p className="local-property-loading" role="status">
              正在读取属性…
            </p>
          ) : (
            <>
              <section className="local-property-section" aria-labelledby="local-basic-title">
                <h3 id="local-basic-title">基本信息</h3>
                <dl>
                  <dt>类型</dt>
                  <dd>
                    {type}
                    {properties.kind === '文件' && extension && (
                      <span className="local-type-extension"> (.{extension.toLowerCase()})</span>
                    )}
                  </dd>
                  <dt>大小</dt>
                  <dd>
                    {properties.kind === '文件夹' ? (
                      '未统计目录内容'
                    ) : (
                      <>
                        {formatSize(properties.size)}{' '}
                        <span className="local-property-secondary">
                          （{properties.size.toLocaleString()} 字节）
                        </span>
                      </>
                    )}
                  </dd>
                  <dt>只读属性</dt>
                  <dd>{properties.readonly ? '是' : '否'}</dd>
                </dl>
              </section>
              <section className="local-property-section" aria-labelledby="local-time-title">
                <h3 id="local-time-title">时间信息</h3>
                <dl>
                  <dt>创建时间</dt>
                  <dd>{date(properties.created)}</dd>
                  <dt>修改时间</dt>
                  <dd>{date(properties.modified)}</dd>
                  <dt>访问时间</dt>
                  <dd>{date(properties.accessed)}</dd>
                </dl>
              </section>
            </>
          )}
        </div>
        <footer>
          <span>
            <ShieldCheck size={19} />
            权限由本地操作系统管理
          </span>
          <button className="primary" onClick={onClose}>
            关闭
          </button>
        </footer>
      </section>
    </div>
  );
}
