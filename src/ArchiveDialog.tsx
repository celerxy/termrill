import ErrorNotice from './ErrorNotice';
import PreviewLoading from './PreviewLoading';
import ArchiveTree from './ArchiveTree';

/** ZIP 工具弹窗；操作绑定原连接，进度通道在关闭后丢弃，密码仅在面板内存中使用。 */
import { useState } from 'react';
import {
  Archive,
  X,
  Search,
  FilePlus2,
  CircleCheck,
  RefreshCw,
  MoreVertical,
  Maximize2,
  Minimize2,
  ShieldCheck,
  FolderOutput,
  Download,
  Trash2,
  Eye,
  PanelLeft,
  PanelRight,
  FileCode2,
  ChevronLeft,
  ChevronRight,
} from 'lucide-react';
import ArchiveExtractSettings from './ArchiveExtractSettings';
import ArchivePackView from './ArchivePackView';
import ArchivePackProgress from './ArchivePackProgress';
import ArchivePackResult from './ArchivePackResult';
import { basename } from './types';
import { fileLanguage } from './fileLanguage';
import ArchiveEntryPreview from './ArchiveEntryPreview';
import ArchiveEntryTable from './ArchiveEntryTable';
import './archive.css';

import { useArchiveController } from './useArchiveController';
import type { ArchiveDialogProps } from './useArchiveController';
import { archiveWriteFormat, archiveCapabilityText } from './archiveCapabilities';
export default function ArchiveDialog(props: ArchiveDialogProps) {
  const { id, path, packSources, localDirectory, onClose } = props;
  const mutable = !!archiveWriteFormat(path);
  const format = path.match(/\.(tar\.(?:gz|bz2|xz)|[^.]+)$/i)?.[1].toUpperCase() ?? 'ARCHIVE';
  const {
    listing,
    selected,
    pendingPreview,
    query,
    setQuery,
    password,
    setPassword,
    compression,
    setCompression,
    setPackFormat,
    busy,
    phase,
    packReport,
    packResult,
    error,
    notice,
    targetMode,
    target,
    setTarget,
    setTargetMode,
    policy,
    setPolicy,
    sourceText,
    setSourceText,
    adding,
    setAdding,
    packTarget,
    packName,
    setPackName,
    packDirectory,
    setPackDirectory,
    packValid,
    preview,
    setPreview,
    panel,
    supported,
    entries,
    cancel,
    run,
    prefetchEntries,
    chooseTarget,
    chooseSources,
    extract,
    setSelected,
  } = useArchiveController(props);
  const completed = !!packResult && !!packReport && !busy && !error;
  const [statusTarget, setStatusTarget] = useState<HTMLDivElement | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [toolbarTarget, setToolbarTarget] = useState<HTMLDivElement | null>(null);
  const [listView, setListView] = useState(false);
  const [filesCollapsed, setFilesCollapsed] = useState(false);
  const [settingsCollapsed, setSettingsCollapsed] = useState(false);
  const focused = filesCollapsed && settingsCollapsed && !listView;
  const toggleFiles = () => {
    if (!filesCollapsed) setListView(false);
    setFilesCollapsed((value) => !value);
  };
  const toggleFocus = () => {
    setListView(false);
    setFilesCollapsed(!focused);
    setSettingsCollapsed(!focused);
  };
  const openPreview = (name?: string) => {
    setListView(false);
    void run('preview', name ? { selected: [name] } : {});
  };
  const removeSelected = () => {
    if (window.confirm(`确定从压缩包中删除 ${selected.length} 个所选项目及其后代？`))
      void run('remove');
  };
  return (
    <div className="modal-backdrop">
      <section
        ref={panel}
        tabIndex={-1}
        className={`archive-dialog${packSources ? ' archive-pack-dialog' : ''}${completed ? ' archive-pack-completed' : ''}${expanded ? ' expanded' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby="archive-title"
        onKeyDown={(event) => {
          if (event.key === 'Escape' && !preview) {
            event.stopPropagation();
            if (busy) cancel();
            else onClose();
          }
          if (event.key === 'Tab') {
            const nodes = Array.from(
              panel.current?.querySelectorAll<HTMLElement>(
                'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), summary',
              ) ?? [],
            ).filter((node) => node.getClientRects().length);
            if (nodes.length && event.shiftKey && document.activeElement === nodes[0]) {
              event.preventDefault();
              nodes[nodes.length - 1].focus();
            } else if (
              nodes.length &&
              !event.shiftKey &&
              document.activeElement === nodes[nodes.length - 1]
            ) {
              event.preventDefault();
              nodes[0].focus();
            }
          }
        }}
      >
        <header className="archive-heading">
          <Archive size={24} />
          <div>
            <div className="archive-title-line">
              <h2 id="archive-title">
                {completed ? '压缩包已创建' : packSources ? '创建压缩包' : basename(path)}
              </h2>
              <span className="archive-format">
                {packSources ? (archiveWriteFormat(packName)?.toUpperCase() ?? format) : format}
              </span>
            </div>
            <p title={path}>
              {completed && packResult
                ? packResult.path.slice(
                    0,
                    Math.max(packResult.path.lastIndexOf('/'), packResult.path.lastIndexOf('\\')) +
                      1,
                  )
                : packSources
                  ? path.slice(0, Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\')) + 1)
                  : path}
              <span>
                {packSources ? (id ? '远程文件' : '本地文件') : id ? '远程压缩包' : '本地压缩包'}
              </span>
            </p>
          </div>
          <button
            aria-label={expanded ? '恢复窗口' : '展开窗口'}
            onClick={() => setExpanded((value) => !value)}
          >
            {expanded ? <Minimize2 size={19} /> : <Maximize2 size={19} />}
          </button>
          <button disabled={busy} aria-label="关闭压缩包" onClick={onClose}>
            <X size={19} />
          </button>
        </header>
        <div className="archive-body">
          {!supported ? (
            <div className="archive-empty">
              支持 ZIP、RAR、7Z、TAR、GZ、BZ2、XZ 及 TAR 压缩变体。
            </div>
          ) : packSources ? (
            <div className="archive-pack-page">
              {completed && packResult && packReport ? (
                <ArchivePackResult
                  report={packReport}
                  path={packResult.path}
                  sources={packResult.sources}
                  remote={!!id}
                />
              ) : (
                <>
                  {packReport && (
                    <ArchivePackProgress
                      report={packReport}
                      busy={busy}
                      remote={!!id}
                      failed={!!error}
                      name={packName}
                    />
                  )}
                  <ArchivePackView
                    sources={packSources}
                    remote={!!id}
                    busy={busy}
                    name={packName}
                    directory={packDirectory}
                    policy={policy}
                    onNameChange={setPackName}
                    onDirectoryChange={setPackDirectory}
                    onPolicyChange={setPolicy}
                    format={archiveWriteFormat(packName) ?? 'zip'}
                    onFormatChange={setPackFormat}
                    compression={compression}
                    onCompressionChange={setCompression}
                    password={password}
                    onPasswordChange={setPassword}
                  />
                </>
              )}
            </div>
          ) : (
            <>
              <nav className="archive-layout-toolbar" aria-label="预览布局">
                <button
                  aria-expanded={!filesCollapsed}
                  aria-controls="archive-files-panel"
                  onClick={toggleFiles}
                >
                  <PanelLeft size={16} />
                  文件列表
                </button>
                <div className="archive-layout-file" title={preview?.name}>
                  <FileCode2 size={17} />
                  <span className="archive-layout-filename">
                    {preview ? basename(preview.name) : '压缩包预览'}
                  </span>
                  {preview && (
                    <span className="archive-format">{fileLanguage(preview.name).name}</span>
                  )}
                </div>
                <div className="archive-preview-tools" ref={setToolbarTarget} />
                <button
                  aria-expanded={!settingsCollapsed}
                  aria-controls="archive-settings-panel"
                  onClick={() => setSettingsCollapsed((value) => !value)}
                >
                  <PanelRight size={16} />
                  解压设置
                </button>
                <button
                  className={focused ? 'primary' : ''}
                  aria-pressed={focused}
                  onClick={toggleFocus}
                >
                  <Eye size={16} />
                  {focused ? '退出专注' : '专注预览'}
                </button>
              </nav>
              <div
                className={`archive-workspace archive-browser ${listView ? 'is-list' : ''}${filesCollapsed ? ' files-collapsed' : ''}${settingsCollapsed ? ' settings-collapsed' : ''}`}
              >
                {filesCollapsed && (
                  <button
                    className="archive-restore-panel left"
                    aria-label="展开文件列表"
                    title="展开文件列表"
                    onClick={() => setFilesCollapsed(false)}
                  >
                    <ChevronRight size={16} />
                  </button>
                )}
                {settingsCollapsed && (
                  <button
                    className="archive-restore-panel right"
                    aria-label="展开解压设置"
                    onClick={() => setSettingsCollapsed(false)}
                  >
                    <ChevronLeft size={16} />
                  </button>
                )}
                <div className="archive-content" id="archive-files-panel" hidden={filesCollapsed}>
                  <div className="archive-toolbar">
                    <label className="archive-search">
                      <Search size={18} />
                      <input
                        aria-label="搜索包内文件"
                        placeholder="搜索包内文件"
                        value={query}
                        onChange={(event) => setQuery(event.target.value)}
                      />
                    </label>
                    <button
                      disabled={busy || !listing || !mutable}
                      onClick={() => setAdding((value) => !value)}
                    >
                      <FilePlus2 size={18} />
                      添加文件
                    </button>
                    <button disabled={busy || !listing} onClick={() => void run('verify')}>
                      <CircleCheck size={18} />
                      校验
                    </button>
                    <button
                      className="archive-icon-button"
                      disabled={busy}
                      aria-label="刷新压缩包"
                      onClick={() => void run('list', { revision: null })}
                    >
                      <RefreshCw size={18} />
                    </button>
                    <details className="archive-more">
                      <summary aria-label="更多压缩包操作">
                        <MoreVertical size={18} />
                      </summary>
                      <div>
                        <button
                          disabled={
                            busy ||
                            selected.length !== 1 ||
                            listing?.entries.find((entry) => entry.name === selected[0])?.directory
                          }
                          onClick={() => openPreview()}
                        >
                          <Eye size={16} />
                          预览所选
                        </button>
                        <button
                          disabled={busy || !selected.length || !localDirectory}
                          onClick={() => extract(true)}
                        >
                          <Download size={16} />
                          下载所选
                        </button>
                        <button
                          className="danger"
                          disabled={busy || !selected.length || !mutable}
                          onClick={removeSelected}
                        >
                          <Trash2 size={16} />
                          删除包内所选
                        </button>
                      </div>
                    </details>
                  </div>
                  {adding && (
                    <div className="archive-add">
                      <label>
                        添加{id ? '远程' : '本地'}文件或目录：每行一个绝对路径
                        <textarea
                          disabled={busy}
                          value={sourceText}
                          onChange={(event) => setSourceText(event.target.value)}
                          rows={3}
                        />
                      </label>
                      {!id && (
                        <button disabled={busy} onClick={() => void chooseSources()}>
                          选择本地文件
                        </button>
                      )}
                      <button
                        className="primary"
                        disabled={busy || !sourceText.trim()}
                        onClick={() =>
                          void run('add', {
                            sources: sourceText
                              .split('\n')
                              .map((source) => source.trim())
                              .filter(Boolean),
                          })
                        }
                      >
                        确认添加
                      </button>
                      <button disabled={busy} onClick={() => setAdding(false)}>
                        收起
                      </button>
                      <small>同名项目按右侧策略处理；已有加密条目保留，新增条目不加密。</small>
                    </div>
                  )}
                  <div className="archive-list-heading">
                    <button onClick={() => setListView((value) => !value)}>
                      {listView ? '目录树' : '列表视图'}
                    </button>
                    <span>
                      {entries.length} 项{query ? '匹配结果' : ''}
                    </span>
                    <button
                      className="archive-collapse-files"
                      aria-label="收起文件列表"
                      title="收起文件列表"
                      onClick={toggleFiles}
                    >
                      <ChevronLeft size={16} />
                    </button>
                  </div>
                  {busy && !listing ? (
                    <PreviewLoading archive title="正在读取压缩包" detail={phase} />
                  ) : !listView ? (
                    <ArchiveTree
                      entries={listing?.entries ?? []}
                      query={query}
                      selected={selected}
                      busy={busy}
                      onSelect={setSelected}
                      onPreview={openPreview}
                      onPrefetch={prefetchEntries}
                    />
                  ) : (
                    <ArchiveEntryTable
                      compressedSizes={mutable}
                      entries={listing?.entries ?? entries}
                      query={query}
                      onClearSearch={() => setQuery('')}
                      selected={selected}
                      busy={busy}
                      phase={phase}
                      loaded={!!listing}
                      setSelected={setSelected}
                      onPreview={openPreview}
                      onPrefetch={prefetchEntries}
                      onDownload={(names) =>
                        void run('extract', {
                          selected: names,
                          target: localDirectory,
                          download: true,
                        })
                      }
                      canDownload={!!localDirectory}
                    />
                  )}
                </div>
                {!listView && (
                  <div className="archive-preview-pane">
                    {pendingPreview && busy ? (
                      <PreviewLoading title="正在加载文件内容" detail={phase} />
                    ) : preview ? (
                      <ArchiveEntryPreview
                        key={preview.name}
                        {...preview}
                        embedded
                        toolbarTarget={toolbarTarget}
                        statusTarget={statusTarget}
                        onClose={() => setPreview(null)}
                      />
                    ) : (
                      <div className="archive-preview-placeholder">
                        <Eye size={36} />
                        <h3>选择文件以预览</h3>
                        <p>点击左侧文件，直接查看内容</p>
                      </div>
                    )}
                  </div>
                )}
                <div
                  className="archive-settings-panel"
                  id="archive-settings-panel"
                  hidden={settingsCollapsed}
                >
                  <button
                    className="archive-collapse-settings"
                    aria-label="收起解压设置"
                    title="收起解压设置"
                    onClick={() => setSettingsCollapsed(true)}
                  >
                    <ChevronRight size={16} />
                  </button>
                  <ArchiveExtractSettings
                    archivePath={path}
                    busy={busy}
                    targetMode={targetMode}
                    target={target}
                    policy={policy}
                    password={password}
                    chooseTarget={chooseTarget}
                    setTargetMode={setTargetMode}
                    setTarget={setTarget}
                    setPolicy={setPolicy}
                    setPassword={setPassword}
                    compression={compression}
                    setCompression={setCompression}
                  />
                </div>
              </div>
            </>
          )}
          {error && (
            <ErrorNotice
              error={error}
              scope="压缩包"
              disabled={busy}
              recovery={
                !packSources
                  ? {
                      label: '重新读取压缩包',
                      run: () => {
                        void run('list', { revision: null });
                      },
                    }
                  : undefined
              }
            />
          )}
          {notice && !completed && (
            <div className="archive-notice" role="status">
              {notice}
            </div>
          )}
        </div>
        <footer className="archive-footer">
          <div className="archive-preview-status" ref={setStatusTarget} />
          <span role="status">
            <ShieldCheck size={18} />
            {completed ? (
              '原始文件已保留'
            ) : busy ? (
              phase
            ) : packSources ? (
              `将打包 ${packSources.length} 项来源`
            ) : (
              <span title={archiveCapabilityText(path)}>
                安全预览 · {mutable ? '可增删重建' : '只读格式'}
              </span>
            )}
          </span>
          {completed && packResult ? (
            <>
              <button onClick={() => props.onOpenArchive(packResult.path)}>查看压缩包</button>
              <button className="primary" onClick={onClose}>
                完成
              </button>
            </>
          ) : busy ? (
            <>
              <button onClick={cancel}>取消操作</button>
              {packSources && (
                <button className="primary" disabled>
                  正在压缩…
                </button>
              )}
            </>
          ) : (
            <>
              <button onClick={onClose}>{packSources ? '取消' : '关闭'}</button>
              {packSources && supported && (
                <button
                  className="primary"
                  disabled={!packValid}
                  onClick={() =>
                    void run('pack', { path: packTarget, sources: packSources, revision: null })
                  }
                >
                  <Archive size={18} />
                  开始压缩
                </button>
              )}
              {!packSources && supported && (
                <button
                  disabled={!selected.length || !localDirectory}
                  onClick={() => extract(true)}
                >
                  下载所选
                </button>
              )}
              {!packSources && supported && (
                <button
                  className="primary"
                  disabled={!listing || !target}
                  onClick={() => extract(false)}
                >
                  <FolderOutput size={18} />
                  {selected.length ? '解压所选' : '解压全部'}
                </button>
              )}
            </>
          )}
        </footer>
      </section>
    </div>
  );
}
