/** 使用生产 App 和官方 IPC mocks 截图；此入口不进入产品构建，不访问真实配置、磁盘或网络。 */
import { createRoot } from 'react-dom/client';
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import { emit } from '@tauri-apps/api/event';
import type { InvokeArgs } from '@tauri-apps/api/core';
import {
  showcaseListing,
  showcaseMarkdown,
  showcaseProfiles,
  showcaseTerminal,
  showcaseTransfers,
} from './showcase-data';
import '../src/styles.css';
import '../src/live.css';
import '../src/serverWorkbench.css';
import './showcase-ui.css';

const scenes = {
  workspace: {
    number: '01',
    label: 'FILE WORKSPACE',
    title: '两端文件，一处掌控。',
    description: '双栏浏览 · 会话管理 · 可暂停的传输队列',
  },
  terminal: {
    number: '02',
    label: 'SSH TERMINAL',
    title: '文件之外，继续你的工作流。',
    description: '多标签 · 分屏终端 · 缓存搜索',
  },
  preview: {
    number: '03',
    label: 'DOCUMENT PREVIEW',
    title: '打开即读，随手编辑。',
    description: 'Markdown · 代码高亮 · 本地与远程文档',
  },
  connection: {
    number: '04',
    label: 'SSH CONNECTIONS',
    title: '直连与跳板，清楚配置。',
    description: '标准 SSH 跳板 · SSH Agent · 凭据按需保存',
  },
};
const sceneName = new URLSearchParams(location.search).get('scene') ?? 'workspace';
const embedded = new URLSearchParams(location.search).get('embedded') === '1';
const scene = scenes[sceneName as keyof typeof scenes] ?? scenes.workspace;
document.title = `TermRill · ${scene.label}`;
let terminalCount = 0;

function commandArguments(payload?: InvokeArgs): Record<string, unknown> {
  if (
    !payload ||
    Array.isArray(payload) ||
    payload instanceof ArrayBuffer ||
    payload instanceof Uint8Array
  ) {
    return {};
  }
  return payload;
}

function mockCommand(command: string, payload?: InvokeArgs): unknown {
  const args = commandArguments(payload);
  switch (command) {
    case 'load_profiles':
    case 'save_profiles':
      return showcaseProfiles;
    case 'load_preferences':
      return false;
    case 'workbench_preferences':
      return {
        groups: { 'showcase-app': 'Demo', 'showcase-staging': 'Demo', 'showcase-backup': 'Demo' },
        commands: [],
      };
    case 'list_local_page':
      return showcaseListing(false);
    case 'list_remote_page':
      return showcaseListing(true);
    case 'list_local_drives':
      return ['C:\\'];
    case 'connect':
      return { id: 'showcase-session' };
    case 'pending_transfers':
      return sceneName === 'workspace' ? showcaseTransfers() : [];
    case 'credential_support':
      return true;
    case 'known_hosts':
      return {};
    case 'load_credentials':
      return null;
    case 'open_remote_text':
    case 'open_local_text':
      return { content: showcaseMarkdown, revision: 'showcase-only', encoding: 'UTF-8' };
    case 'start_terminal': {
      const id = String(args.id);
      const output = showcaseTerminal[Math.min(terminalCount, showcaseTerminal.length - 1)];
      terminalCount += 1;
      queueMicrotask(() => {
        void emit('terminal-output', {
          id,
          data: Array.from(new TextEncoder().encode(output)),
          closed: false,
          error: null,
        });
      });
      return null;
    }
    case 'startup_ready':
    case 'disconnect':
    case 'resize_terminal':
    case 'terminal_ack':
    case 'terminal_input':
    case 'close_terminal':
      return null;
    default:
      throw new Error(`Showcase fixture does not implement command: ${command}`);
  }
}

// 必须在导入 App 之前设置环境：desktop 在模块求值时确定；不改生产入口或认证行为。
Object.defineProperty(window, 'isTauri', { value: true, configurable: true });
mockWindows('main');
mockIPC(mockCommand, { shouldMockEvents: true });

const { default: App } = await import('../src/App');
const root = document.getElementById('root');
if (!root) throw new Error('Showcase root is missing.');
if (embedded) {
  createRoot(root).render(<App />);
} else {
  createRoot(root).render(
    <div className="showcase-stage">
      <div className="showcase-caption">
        <div>
          <div className="showcase-eyebrow">TERMRILL / {scene.label}</div>
          <h1>{scene.title}</h1>
          <p>{scene.description}</p>
        </div>
        <div className="showcase-number">{scene.number}</div>
      </div>
      <div className="showcase-window">
        <div className="showcase-titlebar">
          <div className="showcase-lights" aria-hidden="true">
            <i />
            <i />
            <i />
          </div>
          <span>TermRill · SSH / SFTP workspace</span>
          <span className="showcase-demo">DEMO</span>
        </div>
        <iframe
          className="showcase-app"
          title="TermRill 演示工作台"
          src={`/tests/showcase-ui.html?scene=${encodeURIComponent(sceneName)}&embedded=1`}
        />
      </div>
      <div className="showcase-footnote">
        <span>LOCAL-FIRST. YOUR WORKSPACE.</span>
        <span>实际产品组件 · 虚构演示数据 · 不代表性能测试</span>
      </div>
    </div>,
  );
}
