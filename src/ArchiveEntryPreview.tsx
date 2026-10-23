/** 包内内容只读预览，关闭释放解码数据，不执行 Notebook 或主动 SVG 内容。 */
import { useMemo } from 'react';
import RemoteDocumentDialog from './RemoteDocumentDialog';
import { fileKind } from './fileKind';
import { message } from './types';
export default function ArchiveEntryPreview({
  name,
  bytes,
  onClose,
  embedded = false,
  toolbarTarget,
  statusTarget,
}: {
  name: string;
  bytes: string;
  onClose: () => void;
  embedded?: boolean;
  toolbarTarget?: HTMLElement | null;
  statusTarget?: HTMLElement | null;
}) {
  const decoded = useMemo(() => {
    try {
      const binary = atob(bytes);
      const data = Uint8Array.from(binary, (character) => character.charCodeAt(0));
      const kind = fileKind(name);
      if (kind !== 'text') return { data: data.buffer, content: '', encoding: '', error: '' };
      const encoding =
        data[0] === 255 && data[1] === 254
          ? 'utf-16le'
          : data[0] === 254 && data[1] === 255
            ? 'utf-16be'
            : 'utf-8';
      const content = new TextDecoder(encoding, { fatal: true }).decode(data);
      if (content.includes('\0')) throw new Error('该文件是二进制内容，请解压后使用对应应用打开');
      return { data: data.buffer, content, encoding: encoding.toUpperCase(), error: '' };
    } catch (error) {
      return { data: undefined, content: '', encoding: '', error: message(error) };
    }
  }, [name, bytes]);
  // CodeMirror 按可见区域挂载行，保留完整文档以支持连续行号和全文查找。
  const largeText = decoded.content.length > 1024 * 1024;
  return (
    <RemoteDocumentDialog
      embedded={embedded}
      toolbarTarget={toolbarTarget}
      statusTarget={statusTarget}
      document={{
        path: name,
        connectionId: null,
        content: decoded.content,
        original: decoded.content,
        revision: '',
        encoding: decoded.encoding,
      }}
      kind={fileKind(name)}
      busy={false}
      error={decoded.error}
      sourceOnly={largeText}
      readOnly
      initialBytes={decoded.data}
      onChange={() => {}}
      onSave={() => {}}
      onClose={onClose}
      onDownload={() => {}}
    />
  );
}
