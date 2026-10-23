/** 日志装饰只扫描可见文本片段；不建立全文 token 索引或修改原内容。 */
import {
  Decoration,
  MatchDecorator,
  ViewPlugin,
  type EditorView,
  type ViewUpdate,
} from '@codemirror/view';

const matcher = new MatchDecorator({
  regexp:
    /\b(?:TRACE|DEBUG|INFO|WARN(?:ING)?|ERROR|FATAL|SUCCESS|FAILURE)\b|\b\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?|\[(?:main|[^\]\r\n]*(?:thread|scheduling|pool|exec)[^\]\r\n]*)\]|\b(?:[\w$]+\.)*[\w$]*(?:Exception|Error)\b/g,
  decoration: (match) => {
    const value = match[0];
    let category = 'exception';
    if (/^\d/.test(value)) category = 'time';
    else if (value.startsWith('[')) category = 'thread';
    else if (/^(INFO|SUCCESS)$/.test(value)) category = 'info';
    else if (/^WARN/.test(value)) category = 'warn';
    else if (/^(ERROR|FATAL|FAILURE)$/.test(value)) category = 'error';
    else if (value === 'DEBUG') category = 'debug';
    else if (value === 'TRACE') category = 'trace';
    return Decoration.mark({ class: `cm-log-${category}` });
  },
});

export const logHighlight = ViewPlugin.fromClass(
  class {
    decorations;
    constructor(view: EditorView) {
      this.decorations = matcher.createDeco(view);
    }
    update(update: ViewUpdate) {
      this.decorations = matcher.updateDeco(update, this.decorations);
    }
  },
  { decorations: (plugin) => plugin.decorations },
);
