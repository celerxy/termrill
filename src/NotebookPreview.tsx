/** Notebook 只读展示存储的单元及输出，不执行代码或信任 HTML/脚本输出。 */
import { useEffect, useMemo, useRef, useState } from 'react';
import MarkdownContent from './MarkdownContent';
import CodeEditor from './CodeEditor';
import { notebookMarkdown } from './notebookMarkdown';

const ignoreChanges = () => {};
// 先保留单元高度，接近可见区后创建编辑器，避免打开长 Notebook 时同时初始化数百实例。
function NotebookCode({ source, path, wrap }: { source: string; path: string; wrap: boolean }) {
  const host = useRef<HTMLDivElement>(null);
  const [ready, setReady] = useState(false);
  useEffect(() => {
    if (!host.current) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setReady(true);
          observer.disconnect();
        }
      },
      { root: host.current.closest('.document-notebook'), rootMargin: '400px' },
    );
    observer.observe(host.current);
    return () => observer.disconnect();
  }, []);
  return (
    <div className="notebook-code" ref={host}>
      {ready ? (
        <CodeEditor
          value={source}
          path={path}
          disabled
          wrap={wrap}
          search={0}
          embedded
          onChange={ignoreChanges}
          onCursor={ignoreChanges}
        />
      ) : (
        <div style={{ height: Math.min(400, source.split('\n').length * 23 + 16) }} />
      )}
    </div>
  );
}

function notebookLanguage(metadata: unknown) {
  const record =
    metadata && typeof metadata === 'object' ? (metadata as Record<string, unknown>) : {};
  const info =
    record.language_info && typeof record.language_info === 'object'
      ? (record.language_info as Record<string, unknown>)
      : {};
  const kernel =
    record.kernelspec && typeof record.kernelspec === 'object'
      ? (record.kernelspec as Record<string, unknown>)
      : {};
  const name =
    typeof info.name === 'string'
      ? info.name
      : typeof kernel.language === 'string'
        ? kernel.language
        : 'python';
  const extensions: Record<string, string> = {
    python: 'py',
    javascript: 'js',
    typescript: 'ts',
    r: 'r',
    julia: 'jl',
    bash: 'sh',
    shell: 'sh',
    ruby: 'rb',
    lua: 'lua',
    java: 'java',
    c: 'c',
    'c++': 'cpp',
    cpp: 'cpp',
    sql: 'sql',
    rust: 'rs',
    go: 'go',
    powershell: 'ps1',
  };
  return { name, path: `cell.${extensions[name.toLowerCase()] ?? 'txt'}` };
}

type Cell = {
  cell_type?: string;
  source?: unknown;
  execution_count?: unknown;
  outputs?: Array<{
    output_type?: string;
    text?: unknown;
    data?: Record<string, unknown>;
    traceback?: unknown;
  }>;
};
function text(value: unknown): string {
  if (typeof value === 'string') return value;
  return Array.isArray(value) ? value.filter((part) => typeof part === 'string').join('') : '';
}

function NotebookMarkdown({ source }: { source: string }) {
  const preview = notebookMarkdown(source);
  if (preview.recovered) {
    return (
      <div className="notebook-markdown">
        <div className="notebook-markdown-warning" role="status">
          此单元缺少换行，正在使用兼容排版；原文件未修改，段落边界可能与原稿不同。
        </div>
        <MarkdownContent className="notebook-markdown" source={preview.source} />
      </div>
    );
  }
  return <MarkdownContent className="notebook-markdown" source={source} />;
}
function outputImage(data: Record<string, unknown>) {
  for (const mime of ['image/png', 'image/jpeg']) {
    const base64 = text(data[mime]).replace(/\s/g, '');
    if (!base64 || !/^[A-Za-z0-9+/]*={0,2}$/.test(base64)) continue;
    try {
      const binary = atob(base64);
      const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
      const valid =
        mime === 'image/png'
          ? bytes[0] === 137 && bytes[1] === 80 && bytes[2] === 78 && bytes[3] === 71
          : bytes[0] === 255 && bytes[1] === 216 && bytes[2] === 255;
      if (valid) return URL.createObjectURL(new Blob([bytes], { type: mime }));
    } catch {
      /* 无效输出保留文字表示。 */
    }
  }
  return null;
}

function NotebookOutput({ output }: { output: NonNullable<Cell['outputs']>[number] }) {
  const [image, setImage] = useState<string | null>(null);
  useEffect(() => {
    const url = output.data ? outputImage(output.data) : null;
    setImage(url);
    return () => {
      if (url) URL.revokeObjectURL(url);
    };
  }, [output]);
  const value = text(output.text) || text(output.data?.['text/plain']) || text(output.traceback);
  return (
    <div className={`notebook-output ${output.output_type === 'error' ? 'is-error' : ''}`}>
      {image && <img src={image} alt="单元图片输出" loading="lazy" />}
      {value && <pre>{value.replace(/\x1b\[[0-9;]*m/g, '')}</pre>}
      {!value && !image && <small>该输出类型暂不展示</small>}
    </div>
  );
}

export default function NotebookPreview({ content, wrap }: { content: string; wrap: boolean }) {
  const notebook = useMemo(() => {
    try {
      const parsed = JSON.parse(content);
      if (!parsed || !Array.isArray(parsed.cells)) throw new Error('缺少 cells 数组');
      return {
        cells: parsed.cells as Cell[],
        language: notebookLanguage(parsed.metadata),
        error: '',
      };
    } catch (error) {
      return {
        cells: [],
        language: notebookLanguage(null),
        error: `Notebook 格式无效：${String(error)}`,
      };
    }
  }, [content]);
  if (notebook.error)
    return (
      <div className="document-empty" role="alert">
        {notebook.error}，可切换到源码修正
      </div>
    );
  return (
    <div className="document-notebook">
      <div className="notebook-summary">
        {notebook.cells.length} 个单元 · {notebook.language.name} · 只读预览，不执行代码
      </div>
      {notebook.cells.map(
        (cell, index) =>
          cell &&
          typeof cell === 'object' && (
            <section className="notebook-cell" key={index}>
              <span className="notebook-cell-label">
                {cell.cell_type === 'code'
                  ? `In [${typeof cell.execution_count === 'number' ? cell.execution_count : ' '}]`
                  : cell.cell_type === 'markdown'
                    ? 'Markdown'
                    : 'Raw'}
              </span>
              <div className="notebook-cell-content">
                {cell.cell_type === 'markdown' ? (
                  <NotebookMarkdown source={text(cell.source)} />
                ) : cell.cell_type === 'code' ? (
                  <NotebookCode
                    source={text(cell.source)}
                    path={notebook.language.path}
                    wrap={wrap}
                  />
                ) : (
                  <pre className="notebook-source">{text(cell.source)}</pre>
                )}
                {Array.isArray(cell.outputs) &&
                  cell.outputs.map(
                    (output, outputIndex) =>
                      output &&
                      typeof output === 'object' && (
                        <NotebookOutput key={outputIndex} output={output} />
                      ),
                  )}
              </div>
            </section>
          ),
      )}
      {!notebook.cells.length && <div className="document-empty">Notebook 没有单元</div>}
    </div>
  );
}
