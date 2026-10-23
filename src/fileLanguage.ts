/** 文件类型决定高亮语言；未知类型保持纯文本，打开时不改写内容。 */
import { json } from '@codemirror/lang-json';
import { javascript } from '@codemirror/lang-javascript';
import { python } from '@codemirror/lang-python';
import { yaml } from '@codemirror/lang-yaml';
import { html } from '@codemirror/lang-html';
import { css } from '@codemirror/lang-css';
import { markdown } from '@codemirror/lang-markdown';
import { language, StreamLanguage } from '@codemirror/language';
import { EditorState } from '@codemirror/state';
import type { Extension } from '@codemirror/state';
import { shell } from '@codemirror/legacy-modes/mode/shell';
import { toml } from '@codemirror/legacy-modes/mode/toml';
import { properties } from '@codemirror/legacy-modes/mode/properties';
import { rust } from '@codemirror/legacy-modes/mode/rust';
import { go } from '@codemirror/legacy-modes/mode/go';
import { c, cpp, java, csharp } from '@codemirror/legacy-modes/mode/clike';
import { standardSQL } from '@codemirror/legacy-modes/mode/sql';
import { dockerFile } from '@codemirror/legacy-modes/mode/dockerfile';
import { cmake } from '@codemirror/legacy-modes/mode/cmake';
import { powerShell } from '@codemirror/legacy-modes/mode/powershell';
import { ruby } from '@codemirror/legacy-modes/mode/ruby';
import { perl } from '@codemirror/legacy-modes/mode/perl';
import { lua } from '@codemirror/legacy-modes/mode/lua';
import { diff } from '@codemirror/legacy-modes/mode/diff';
import { xml } from '@codemirror/legacy-modes/mode/xml';
import { r } from '@codemirror/legacy-modes/mode/r';
import { julia } from '@codemirror/legacy-modes/mode/julia';

/** Markdown 围栏语言别名与独立代码块共用；Markdown 本身避免递归创建语言。 */
export function codeBlockLanguage(info: string): Extension {
  const name = info.trim().split(/\s+/)[0].toLowerCase();
  const aliases: Record<string, string> = {
    python: 'py',
    javascript: 'js',
    typescript: 'ts',
    shell: 'sh',
    console: 'sh',
    'c++': 'cpp',
    'c#': 'cs',
    rust: 'rs',
    ruby: 'rb',
    powershell: 'ps1',
    docker: 'dockerfile',
    julia: 'jl',
    plaintext: 'txt',
    text: 'txt',
  };
  if (!name || ['md', 'markdown'].includes(name)) return [];
  return fileLanguage(`snippet.${aliases[name] ?? name}`).extension;
}
export function fileLanguage(path: string): { name: string; extension: Extension } {
  const name = path.split(/[\\/]/).pop()?.toLowerCase() ?? '';
  if (name === 'dockerfile' || name.startsWith('dockerfile.'))
    return { name: 'Dockerfile', extension: StreamLanguage.define(dockerFile) };
  if (name === 'cmakelists.txt') return { name: 'CMake', extension: StreamLanguage.define(cmake) };
  if (['makefile', '.gitignore', '.gitattributes'].includes(name))
    return {
      name: name === 'dockerfile' ? 'Dockerfile' : '配置',
      extension: StreamLanguage.define(shell),
    };
  if (['.bashrc', '.zshrc', '.profile', '.bash_profile'].includes(name))
    return { name: 'Shell', extension: StreamLanguage.define(shell) };
  if (name === '.env' || name.startsWith('.env.'))
    return { name: 'ENV', extension: StreamLanguage.define(properties) };
  const ext = name.split('.').pop() ?? '';
  switch (ext) {
    case 'r':
      return { name: 'R', extension: StreamLanguage.define(r) };
    case 'jl':
      return { name: 'Julia', extension: StreamLanguage.define(julia) };
    case 'ipynb':
      return { name: 'Notebook', extension: json() };
    case 'ps1':
    case 'psm1':
    case 'psd1':
      return { name: 'PowerShell', extension: StreamLanguage.define(powerShell) };
    case 'rb':
      return { name: 'Ruby', extension: StreamLanguage.define(ruby) };
    case 'pl':
    case 'pm':
      return { name: 'Perl', extension: StreamLanguage.define(perl) };
    case 'lua':
      return { name: 'Lua', extension: StreamLanguage.define(lua) };
    case 'patch':
    case 'diff':
      return { name: 'Diff', extension: StreamLanguage.define(diff) };
    case 'cmake':
      return { name: 'CMake', extension: StreamLanguage.define(cmake) };
    case 'csv':
    case 'tsv':
    case 'log':
    case 'txt':
      return { name: ext.toUpperCase(), extension: [] };
    case 'sql':
      return { name: 'SQL', extension: StreamLanguage.define(standardSQL) };
    case 'rs':
      return { name: 'Rust', extension: StreamLanguage.define(rust) };
    case 'go':
      return { name: 'Go', extension: StreamLanguage.define(go) };
    case 'c':
    case 'h':
      return { name: 'C', extension: StreamLanguage.define(c) };
    case 'cpp':
    case 'hpp':
    case 'cc':
      return { name: 'C++', extension: StreamLanguage.define(cpp) };
    case 'java':
      return { name: 'Java', extension: StreamLanguage.define(java) };
    case 'cs':
      return { name: 'C#', extension: StreamLanguage.define(csharp) };
    case 'jsonl':
    case 'ndjson':
      return { name: 'JSON Lines', extension: json() };
    case 'jsonc':
      return { name: 'JSONC', extension: json() };
    case 'json':
      return { name: 'JSON', extension: json() };
    case 'js':
    case 'jsx':
    case 'mjs':
    case 'cjs':
      return { name: 'JavaScript', extension: javascript({ jsx: true }) };
    case 'ts':
    case 'tsx':
      return { name: 'TypeScript', extension: javascript({ typescript: true, jsx: true }) };
    case 'py':
      return { name: 'Python', extension: python() };
    case 'yaml':
    case 'yml':
      return { name: 'YAML', extension: yaml() };
    case 'html':
    case 'htm':
      return { name: 'HTML / XML', extension: html() };
    case 'xml':
    case 'xsd':
    case 'xsl':
    case 'xslt':
    case 'plist':
      return { name: 'XML', extension: StreamLanguage.define(xml) };
    case 'svg':
      return { name: 'SVG', extension: StreamLanguage.define(xml) };
    case 'css':
      return { name: 'CSS', extension: css() };
    case 'md':
    case 'markdown':
      return {
        name: 'Markdown',
        extension: markdown({
          codeLanguages: (info) =>
            EditorState.create({ extensions: codeBlockLanguage(info) }).facet(language),
        }),
      };
    case 'sh':
    case 'bash':
    case 'zsh':
      return { name: 'Shell', extension: StreamLanguage.define(shell) };
    case 'toml':
      return { name: 'TOML', extension: StreamLanguage.define(toml) };
    case 'ini':
    case 'cfg':
    case 'cnf':
    case 'config':
    case 'env':
    case 'conf':
    case 'properties':
      return { name: '配置', extension: StreamLanguage.define(properties) };
    default:
      return { name: '纯文本', extension: [] };
  }
}
