/** 编辑器实例由组件持有；外部保存更新不重新创建实例，保留光标与撤销历史。 */
import { useEffect, useRef } from 'react';
import { Compartment, EditorState } from '@codemirror/state';
import {
  Decoration,
  EditorView,
  MatchDecorator,
  ViewPlugin,
  lineNumbers,
  highlightActiveLineGutter,
  highlightSpecialChars,
  drawSelection,
  dropCursor,
  rectangularSelection,
  crosshairCursor,
  highlightActiveLine,
  keymap,
} from '@codemirror/view';
import {
  openSearchPanel,
  highlightSelectionMatches,
  searchKeymap,
  setSearchQuery,
  SearchQuery,
  findNext,
  search as searchExtension,
} from '@codemirror/search';
import {
  foldGutter,
  HighlightStyle,
  syntaxHighlighting,
  indentOnInput,
  defaultHighlightStyle,
  bracketMatching,
  foldKeymap,
} from '@codemirror/language';
import { history, defaultKeymap, historyKeymap } from '@codemirror/commands';
import {
  closeBrackets,
  autocompletion,
  closeBracketsKeymap,
  completionKeymap,
} from '@codemirror/autocomplete';
import { lintKeymap } from '@codemirror/lint';
import { tags } from '@lezer/highlight';
import { fileLanguage } from './fileLanguage';
import { logHighlight } from './logHighlight';

// 显式配置默认编辑能力，避免 basicSetup 内置折叠栏与自定义栏重复注册。
const editorSetup = [
  searchExtension(),
  lineNumbers(),
  highlightActiveLineGutter(),
  highlightSpecialChars(),
  history(),
  foldGutter({ markerDOM: foldMarker }),
  drawSelection(),
  dropCursor(),
  EditorState.allowMultipleSelections.of(true),
  indentOnInput(),
  syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
  bracketMatching(),
  closeBrackets(),
  autocompletion(),
  rectangularSelection(),
  crosshairCursor(),
  highlightActiveLine(),
  highlightSelectionMatches(),
  keymap.of([
    ...closeBracketsKeymap,
    ...defaultKeymap,
    ...searchKeymap,
    ...historyKeymap,
    ...foldKeymap,
    ...completionKeymap,
    ...lintKeymap,
  ]),
];

function foldMarker(open: boolean) {
  const marker = document.createElement('span');
  marker.className = `document-fold-marker ${open ? 'is-open' : 'is-folded'}`;
  marker.setAttribute('aria-label', open ? '折叠代码' : '展开代码');
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 16 16');
  svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  path.setAttribute('d', open ? 'M4 6L8 10L12 6' : 'M6 4L10 8L6 12');
  path.setAttribute('fill', 'none');
  path.setAttribute('stroke', 'currentColor');
  path.setAttribute('stroke-width', '1.5');
  path.setAttribute('stroke-linecap', 'round');
  path.setAttribute('stroke-linejoin', 'round');
  svg.append(path);
  marker.append(svg);
  return marker;
}

// 仅装饰可见行的前导空白，不改写内容；避免大文件生成整篇缩进标记。
const indentationMatcher = new MatchDecorator({
  regexp: /^[ \t]+/g,
  decoration: Decoration.mark({ class: 'cm-indent-guide' }),
});
const indentationGuides = ViewPlugin.fromClass(
  class {
    decorations;
    constructor(view: EditorView) {
      this.decorations = indentationMatcher.createDeco(view);
    }
    update(update: import('@codemirror/view').ViewUpdate) {
      this.decorations = indentationMatcher.updateDeco(update, this.decorations);
    }
  },
  { decorations: (plugin) => plugin.decorations },
);
export default function CodeEditor({
  value,
  path,
  disabled,
  wrap,
  search,
  onChange,
  onCursor,
  embedded = false,
  searchText,
  searchNext = 0,
}: {
  value: string;
  path: string;
  disabled: boolean;
  wrap: boolean;
  search: number;
  onChange: (value: string) => void;
  onCursor: (line: number, column: number) => void;
  embedded?: boolean;
  searchText?: string;
  searchNext?: number;
}) {
  const host = useRef<HTMLDivElement>(null);
  const editor = useRef<EditorView | null>(null);
  const callbacks = useRef({ onChange, onCursor });
  callbacks.current = { onChange, onCursor };
  const options = useRef(new Compartment());
  const settings = () => [
    EditorState.readOnly.of(disabled),
    EditorView.editable.of(!disabled),
    ...(wrap ? [EditorView.lineWrapping] : []),
  ];
  useEffect(() => {
    if (!host.current) return;
    const view = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: value,
        extensions: [
          editorSetup,
          EditorState.tabSize.of(2),
          indentationGuides,
          ...(/\.(?:log|out)$/i.test(path) ? [logHighlight] : []),
          syntaxHighlighting(
            HighlightStyle.define([
              { tag: [tags.propertyName, tags.keyword], color: '#7055ed' },
              { tag: tags.string, color: '#25885b' },
              { tag: [tags.number, tags.bool, tags.null], color: '#c87927' },
              { tag: tags.comment, color: '#9298ac', fontStyle: 'italic' },
            ]),
          ),
          fileLanguage(path).extension,
          options.current.of(settings()),
          EditorView.theme({
            '&': {
              height: embedded ? 'auto' : '100%',
              backgroundColor: '#fcfcff',
              color: '#303650',
              fontSize: '14px',
            },
            '.cm-scroller': {
              overflow: 'auto',
              fontFamily: 'Cascadia Code, Consolas, monospace',
              lineHeight: '1.65',
              maxHeight: embedded ? '400px' : 'none',
            },
            '.cm-content': { padding: '8px 0' },
            '.cm-line': { padding: '0 12px' },
            '.cm-gutters': { backgroundColor: '#f7f8fc', color: '#969bb0', border: 'none' },
            '.cm-lineNumbers .cm-gutterElement': { padding: '0 6px 0 12px' },
            '.cm-foldGutter .cm-gutterElement': { padding: '0 2px', minWidth: '20px' },
            '.document-fold-marker': {
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              width: '16px',
              height: '16px',
              verticalAlign: 'middle',
              borderRadius: '4px',
              color: '#a0a5b8',
              cursor: 'pointer',
            },
            '.document-fold-marker svg': { width: '14px', height: '14px' },
            '.document-fold-marker:hover': { color: '#6b5cff', backgroundColor: '#eae6ff' },
            '.document-fold-marker.is-folded': { color: '#7565ce', backgroundColor: '#efecfb' },
            '.cm-indent-guide': {
              backgroundImage:
                'repeating-linear-gradient(to right, #e5e7f0 0 1px, transparent 1px 2ch)',
            },
            '.cm-activeLine': { backgroundColor: '#eeecff55' },
            '.cm-activeLineGutter': { backgroundColor: '#eeecff' },
            '&.cm-focused': { outline: 'none' },
          }),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) callbacks.current.onChange(update.state.doc.toString());
            if (update.selectionSet || update.docChanged) {
              const pos = update.state.selection.main.head;
              const line = update.state.doc.lineAt(pos);
              callbacks.current.onCursor(line.number, pos - line.from + 1);
            }
          }),
        ],
      }),
    });
    editor.current = view;
    return () => {
      editor.current = null;
      view.destroy();
    };
  }, [path, embedded]);
  useEffect(() => {
    const view = editor.current;
    if (!view) return;
    // CodeMirror 默认把换行规范化为 LF，避免 CRLF 内容每次追加都被误判为全文变化。
    const next = value.replace(/\r\n?/g, '\n');
    const current = view.state.doc.toString();
    if (current === next) return;
    if (disabled && next.startsWith(current)) {
      // 分块只读预览只插入新尾部，保留原行、选择及视口锚点。
      // 使用编辑器自身的滚动快照，避免延迟恢复覆盖用户后续的滚动操作。
      view.dispatch({
        changes: { from: view.state.doc.length, insert: next.slice(current.length) },
        effects: view.scrollSnapshot(),
      });
    } else {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: next } });
    }
  }, [value, disabled]);
  useEffect(() => {
    editor.current?.dispatch({ effects: options.current.reconfigure(settings()) });
  }, [disabled, wrap]);
  useEffect(() => {
    if (search && editor.current) openSearchPanel(editor.current);
  }, [search]);
  useEffect(() => {
    if (searchText === undefined) return;
    const timer = window.setTimeout(() => {
      editor.current?.dispatch({
        effects: setSearchQuery.of(new SearchQuery({ search: searchText })),
      });
    }, 120);
    return () => window.clearTimeout(timer);
  }, [searchText]);
  useEffect(() => {
    if (searchNext && editor.current) {
      editor.current.dispatch({
        effects: setSearchQuery.of(new SearchQuery({ search: searchText ?? '' })),
      });
      findNext(editor.current);
    }
  }, [searchNext]);
  return <div className="document-code" ref={host} />;
}
