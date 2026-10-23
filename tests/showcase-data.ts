/** 公开截图专用的虚构数据；域名、目录、终端输出和进度均不来自真实主机。 */
import type { FileEntry, Listing, Profile, Transfer } from '../src/types';

export const showcaseProfiles: Profile[] = [
  {
    id: 'showcase-app',
    name: 'Demo · 应用服务器',
    target: { host: 'app.example.com', port: 22, username: 'demo', auth: 'agent', keyPath: '' },
    jump: null,
  },
  {
    id: 'showcase-staging',
    name: 'Staging · SSH 跳板',
    target: { host: 'staging.example.com', port: 22, username: 'demo', auth: 'agent', keyPath: '' },
    jump: { host: 'bastion.example.com', port: 22, username: 'demo', auth: 'agent', keyPath: '' },
  },
  {
    id: 'showcase-backup',
    name: 'Demo · 文件归档',
    target: { host: 'backup.example.com', port: 22, username: 'demo', auth: 'agent', keyPath: '' },
    jump: null,
  },
];

type SampleEntry = [name: string, directory: boolean, size: number];
const localEntries: SampleEntry[] = [
  ['assets', true, 0],
  ['documents', true, 0],
  ['release-notes.md', false, 2840],
  ['app-config.json', false, 1260],
  ['deployment.csv', false, 8432],
  ['design-tokens.css', false, 6200],
  ['sample-project.zip', false, 18454937],
  ['checksums.sha256', false, 360],
];
const remoteEntries: SampleEntry[] = [
  ['assets', true, 0],
  ['documents', true, 0],
  ['release-notes.md', false, 2840],
  ['app-config.json', false, 1260],
  ['deployment.csv', false, 8432],
  ['application.log', false, 482400],
  ['sample-project.zip', false, 18454937],
  ['checksums.sha256', false, 360],
];

export function showcaseListing(remote: boolean): Listing {
  const path = remote ? '/srv/demo/releases' : 'C:\\Demo\\releases';
  const separator = remote ? '/' : '\\';
  const entries: FileEntry[] = (remote ? remoteEntries : localEntries).map(
    ([name, directory, size]) => ({
      name,
      path: `${path}${separator}${name}`,
      directory,
      symlink: false,
      size,
      modified: 1792756800,
    }),
  );
  return {
    path,
    parent: remote ? '/srv/demo' : 'C:\\Demo',
    entries,
    page: { offset: 0, size: 100, hasMore: false, query: '', showHidden: false },
  };
}

export function showcaseTransfers(): Transfer[] {
  return [
    {
      id: 'showcase-upload',
      connectionId: 'showcase-session',
      name: 'sample-project.zip',
      direction: 'upload',
      status: 'paused',
      bytes: 11072962,
      total: 18454937,
      speed: 0,
      error: null,
    },
    {
      id: 'showcase-download',
      connectionId: 'showcase-session',
      name: 'release-notes.md',
      direction: 'download',
      status: 'completed',
      bytes: 2840,
      total: 2840,
      speed: 0,
      error: null,
    },
  ];
}

export const showcaseMarkdown = `# Release workspace

> 一处管理文件、终端与发布文档。

这是用于产品截图的示例文档。连接信息和文件内容均为虚构数据。

## 发布清单

| 项目 | 用途 | 状态 |
| --- | --- | --- |
| assets/ | 静态资源 | 已准备 |
| app-config.json | 应用配置示例 | 已检查 |
| checksums.sha256 | 文件完整性核对 | 已生成 |

## 在熟悉的工作流中操作

- 双栏浏览本地与远程文件
- 预览文档并查看源代码
- 在 SSH 终端中继续工作

\`\`\`json
{
  "application": "demo-workspace",
  "environment": "preview",
  "release": "0.1.0"
}
\`\`\`

所有内容仅用于界面展示，不代表一次真实发布或性能测试。
`;

export const showcaseTerminal = [
  [
    '\x1b[2J\x1b[H\x1b[35mTermRill demo workspace\x1b[0m',
    '\x1b[90mFictional terminal output · no remote commands executed\x1b[0m',
    '',
    '\x1b[32mdemo@app.example.com\x1b[0m:\x1b[34m/srv/demo/releases\x1b[0m $ ls -lh',
    'drwxr-xr-x  demo  demo       assets/',
    'drwxr-xr-x  demo  demo       documents/',
    '-rw-r--r--  demo  demo  2.8K release-notes.md',
    '-rw-r--r--  demo  demo  1.2K app-config.json',
    '-rw-r--r--  demo  demo   18M sample-project.zip',
    '',
    '\x1b[32mdemo@app.example.com\x1b[0m:\x1b[34m/srv/demo/releases\x1b[0m $ ',
  ].join('\r\n'),
  [
    '\x1b[2J\x1b[H\x1b[35mApplication log · example output\x1b[0m',
    '\x1b[90mFictional terminal output · no remote commands executed\x1b[0m',
    '',
    '\x1b[32mdemo@app.example.com\x1b[0m:\x1b[34m/srv/demo\x1b[0m $ tail application.log',
    '\x1b[90m10:24:01\x1b[0m  \x1b[32mINFO\x1b[0m  Application started',
    '\x1b[90m10:24:02\x1b[0m  \x1b[32mINFO\x1b[0m  Static assets ready',
    '\x1b[90m10:24:03\x1b[0m  \x1b[32mINFO\x1b[0m  Configuration loaded',
    '\x1b[90m10:24:04\x1b[0m  \x1b[32mINFO\x1b[0m  Health check passed',
    '\x1b[90m10:24:05\x1b[0m  \x1b[32mINFO\x1b[0m  Ready to serve requests',
  ].join('\r\n'),
];
