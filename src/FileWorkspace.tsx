import type { Workspace } from './useTermRillWorkspace';
import { desktop } from './useTermRillWorkspace';
import { invoke } from '@tauri-apps/api/core';
import { isArchive } from './archiveTypes';
import { recoveryProfileIdentity } from './transferRecovery';
import FilePane from './FilePane';
import FileActions from './FileActions';
import { FileEntry } from './types';
import { lazy, Suspense, useRef, useState } from 'react';
import ResizeSeparator from './ResizeSeparator';
import type { BatchRenameRequest } from './BatchRenameDialog';
const BatchRenameDialog = lazy(() => import('./BatchRenameDialog'));
const ArchiveDialog = lazy(() => import('./ArchiveDialog'));
const LocalProperties = lazy(() => import('./LocalProperties'));
const RemotePermissions = lazy(() => import('./RemotePermissions'));
const BatchPermissions = lazy(() => import('./BatchPermissions'));
const TerminalPanel = lazy(() => import('./TerminalPanel'));

/** 本地/远程文件面板及按需加载的终端。 */
export default function FileWorkspace({ workspace }: { workspace: Workspace }) {
  const panes = useRef<HTMLDivElement>(null);
  const currentConnection = useRef(workspace.connectionId);
  currentConnection.current = workspace.connectionId;
  const [batchRename, setBatchRename] = useState<BatchRenameRequest | null>(null);
  const { layout, preview, save } = workspace.workspaceLayout;
  const [archive, setArchive] = useState<{
    id: string | null;
    path: string;
    packSources?: string[];
  } | null>(null);
  const [localProperties, setLocalProperties] = useState<string | null>(null);
  const [properties, setProperties] = useState<{ id: string; path: string } | null>(null);
  const [batchPermissions, setBatchPermissions] = useState<{ id: string; paths: string[] } | null>(
    null,
  );
  const {
    connectionId,
    local,
    remote,
    localSelection,
    setLocalSelection,
    remoteSelection,
    setRemoteSelection,
    localBusy,
    localReadCount,
    localReadError,
    remoteBusy,
    remoteReadCount,
    remoteReadError,
    terminal,
    setTerminal,
    showHidden,
    notify,
    loadLocal,
    loadRemote,
    planPaths,
    fileOperation,
  } = workspace;
  return (
    <>
      <FileActions workspace={workspace}>
        {({ busy, requestDelete, openRemote, openLocal }) => (
          <>
            <div
              className="panes adjustable-panes"
              ref={panes}
              style={{
                gridTemplateColumns: `minmax(0, ${layout.panelRatio}fr) 16px minmax(0, ${100 - layout.panelRatio}fr)`,
              }}
            >
              <FilePane
                listing={local}
                loading={localBusy}
                readCount={localReadCount}
                readError={localReadError}
                selected={localSelection}
                showHidden={showHidden}
                disabled={!desktop || busy || !!archive || !!batchRename}
                transferDisabled={!connectionId}
                onDelete={(paths) => requestDelete(paths, false)}
                onEdit={(path) =>
                  isArchive(path) ? setArchive({ id: null, path }) : openLocal(path)
                }
                onArchive={(path) => setArchive({ id: null, path })}
                onCompress={(paths) =>
                  setArchive({
                    id: null,
                    path: `${local?.path}${local?.path.includes('\\') ? '\\' : '/'}archive.zip`,
                    packSources: paths,
                  })
                }
                onProperties={setLocalProperties}
                onError={notify}
                onSelect={setLocalSelection}
                onNavigate={(path) => void loadLocal(path)}
                onPage={(offset, query) => local && void loadLocal(local.path, offset, query)}
                onRefresh={() =>
                  void loadLocal(local?.path, local?.page?.offset ?? 0, local?.page?.query ?? '')
                }
                onPick={() =>
                  void invoke<string[]>('pick_path', { kind: 'folder' })
                    .then((paths) => {
                      if (paths[0]) void loadLocal(paths[0]);
                    })
                    .catch(notify)
                }
                onMkdir={() => fileOperation(false, 'mkdir')}
                onRename={(path) => fileOperation(false, 'rename', path)}
                onBatchRename={(entries) =>
                  local && setBatchRename({ id: null, listing: local, entries })
                }
                onTransfer={(entries: FileEntry[]) =>
                  planPaths(
                    entries.map((f) => f.path),
                    'upload',
                  )
                }
                onDrop={(paths) => planPaths(paths, 'upload')}
              />
              <ResizeSeparator
                axis="x"
                label="左右文件面板宽度"
                value={layout.panelRatio}
                min={30}
                max={70}
                initial={50}
                step={2}
                pixelsPerUnit={() =>
                  Math.max(1, (panes.current?.getBoundingClientRect().width ?? 800) - 64) / 100
                }
                onChange={(panelRatio) => preview({ ...layout, panelRatio })}
                onCommit={(panelRatio) => save({ ...layout, panelRatio })}
              />
              <FilePane
                remote
                locationScope={
                  workspace.profile
                    ? `remote:${recoveryProfileIdentity(workspace.profile)}`
                    : undefined
                }
                listing={remote}
                loading={remoteBusy}
                readCount={remoteReadCount}
                readError={remoteReadError}
                selected={remoteSelection}
                showHidden={showHidden}
                disabled={!connectionId || busy || !!archive || !!batchRename}
                onDelete={(paths) => requestDelete(paths, true)}
                onEdit={(path) =>
                  isArchive(path) ? setArchive({ id: connectionId, path }) : openRemote(path)
                }
                onArchive={(path) => setArchive({ id: connectionId, path })}
                onCompress={(paths) =>
                  setArchive({
                    id: connectionId,
                    path: `${remote?.path}/archive.zip`,
                    packSources: paths,
                  })
                }
                onProperties={(path) => {
                  if (connectionId) setProperties({ id: connectionId, path });
                }}
                onBatchPermissions={(paths) => {
                  if (connectionId) setBatchPermissions({ id: connectionId, paths: [...paths] });
                }}
                onError={notify}
                onSelect={setRemoteSelection}
                onNavigate={(path) => void loadRemote(path, 0, '', true)}
                onPage={(offset, query) =>
                  remote && void loadRemote(remote.path, offset, query, true)
                }
                onRefresh={() =>
                  void loadRemote(
                    remote?.path ?? '.',
                    remote?.page?.offset ?? 0,
                    remote?.page?.query ?? '',
                  )
                }
                onMkdir={() => fileOperation(true, 'mkdir')}
                onRename={(path) => fileOperation(true, 'rename', path)}
                onBatchRename={(entries) =>
                  remote &&
                  connectionId &&
                  setBatchRename({ id: connectionId, listing: remote, entries })
                }
                onTransfer={(entries) =>
                  planPaths(
                    entries.map((f) => f.path),
                    'download',
                  )
                }
                onDrop={(paths) => planPaths(paths, 'upload')}
              />
            </div>
            {terminal && connectionId && (
              <Suspense fallback={<div className="terminal-loading">正在载入终端…</div>}>
                <ResizeSeparator
                  axis="y"
                  label="终端高度"
                  value={layout.terminalHeight}
                  min={100}
                  max={480}
                  initial={180}
                  step={20}
                  direction={-1}
                  onChange={(terminalHeight) => preview({ ...layout, terminalHeight })}
                  onCommit={(terminalHeight) => save({ ...layout, terminalHeight })}
                />
                <TerminalPanel
                  key={connectionId}
                  initialLogPath={remoteSelection[0] ?? ''}
                  connectionId={connectionId}
                  height={layout.terminalHeight}
                  onClose={() => setTerminal(false)}
                />
              </Suspense>
            )}
          </>
        )}
      </FileActions>
      <Suspense fallback={null}>
        {batchRename && (
          <BatchRenameDialog
            request={batchRename}
            currentConnection={() => currentConnection.current}
            onClose={() => setBatchRename(null)}
            onChanged={() => {
              if (batchRename.id && batchRename.id === currentConnection.current)
                void loadRemote(batchRename.listing.path);
              if (!batchRename.id) void loadLocal(batchRename.listing.path);
            }}
          />
        )}
        {archive && (
          <ArchiveDialog
            key={`${archive.id}:${archive.path}:${archive.packSources ? 'pack' : 'browse'}`}
            {...archive}
            localDirectory={local?.path ?? ''}
            onClose={() => setArchive(null)}
            onOpenArchive={(path) => setArchive({ id: archive.id, path })}
            onChanged={() => {
              if (archive.id && archive.id === connectionId && remote) void loadRemote(remote.path);
              if (local) void loadLocal(local.path);
            }}
          />
        )}
        {localProperties && (
          <LocalProperties
            key={localProperties}
            path={localProperties}
            onClose={() => setLocalProperties(null)}
          />
        )}
        {properties && (
          <RemotePermissions
            key={`${properties.id}:${properties.path}`}
            id={properties.id}
            path={properties.path}
            onClose={() => setProperties(null)}
            onChanged={() => {
              if (remote && connectionId === properties.id) void loadRemote(remote.path);
            }}
          />
        )}
        {batchPermissions && (
          <BatchPermissions
            id={batchPermissions.id}
            paths={batchPermissions.paths}
            onClose={() => setBatchPermissions(null)}
            onChanged={() => {
              if (remote && connectionId === batchPermissions.id) void loadRemote(remote.path);
            }}
          />
        )}
      </Suspense>
    </>
  );
}
