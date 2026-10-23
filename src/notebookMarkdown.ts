/** 仅为缺失换行的 Notebook 说明单元提供预览兼容排版，不修改源文件。 */
export function notebookMarkdown(source: string): { source: string; recovered: boolean } {
  let recovered = false;
  let fence: string | null = null;
  const preview = source.split(/\r?\n/).map((line) => {
    const marker = /^\s{0,3}(`{3,}|~{3,})/.exec(line)?.[1];
    if (marker) {
      if (!fence) fence = marker;
      else if (marker[0] === fence[0] && marker.length >= fence.length) fence = null;
      return line;
    }
    if (fence) return line;
    const result = recoverLine(line);
    recovered ||= result !== line;
    return result;
  });
  return { source: recovered ? preview.join('\n') : source, recovered };
}

function recoverLine(source: string): string {
  const collapsed =
    source.length > 200 &&
    /^\s*(?:---)?#{1,6}\s/.test(source) &&
    /\S#{2,6}\s|\)[A-Z]|[a-z][A-Z][a-z]|```|>\s|\|[- :]+\|/.test(source);
  if (!collapsed) return source;

  // 单行围栏的正文保持原样；仅补上围栏边界，不能把代码交给说明文字的修复规则。
  if (/```[^`]*```/.test(source)) {
    const parts = source.split(/(```[^`]*```)/g);
    return parts
      .map((part) => {
        if (!part.startsWith('```')) return recoverProse(part);
        return `\n\n\`\`\`\n${part.slice(3, -3)}\n\`\`\`\n\n`;
      })
      .join('');
  }
  return recoverProse(source);
}

function recoverProse(source: string): string {
  let preview = source
    .replace(/^\s*---(?=#{1,6}\s)/, '---\n\n')
    .replace(/([^\s#])(#{2,6}\s)/g, '$1\n\n$2')
    .replace(/\s*---(?=\s*(?:#{1,6}\s|$))/g, '\n\n---\n\n')
    .replace(/([^\\])>\s/g, '$1\n\n> ')
    .replace(/([^\n])(-\s+\*\*)/g, '$1\n$2')
    // 只拆开紧贴前文的标签；空格可能属于列表的「- **标签**」，不能拆开。
    .replace(/([^\s])(\*\*[\w ]+:\*\*)/g, '$1\n\n$2');

  preview = preview
    // 带页码的标题结束后直接接正文，例如「(p. 72)Use」。
    .replace(/^(#{1,6} [^\n]*?\))(?=[A-Z])/gm, '$1\n\n')
    // 例如 ChapterPrompt / AgentsPrompts：丢失换行处留下的大小写边界。
    .replace(/^(#{1,6} [^\n]*?[a-z])([A-Z][a-z])/gm, '$1\n\n$2')
    .replace(/^(#{1,6} [^\n|>]+)(?=[|>])/gm, '$1\n\n')
    // 单行 GFM 表格的相邻行边界为 ||；只处理有分隔行的段落。
    .replace(/\|[^\n]*\|[ :\-]+\|[^\n]*/g, (table) => table.replace(/\|\|/g, '|\n|'));
  return preview;
}
