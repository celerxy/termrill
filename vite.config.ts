import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { devReadiness } from './scripts/dev-readiness';
import { pdfAssets } from './scripts/pdf-assets';
import { fileURLToPath } from 'node:url';

const projectRoot = fileURLToPath(new URL('.', import.meta.url))
  .replace(/\\/g, '/')
  .replace(/\/$/, '');
const nonFrontendRoots = ['src-tauri', 'tests', 'dist', 'docs'].map(
  (directory) => `${projectRoot}/${directory}`,
);

export default defineConfig({
  plugins: [react(), devReadiness(), pdfAssets()],
  clearScreen: false,
  // Mermaid 的传递依赖含 CommonJS（如 dayjs），必须预构建为浏览器 ESM。
  // 不排除 Mermaid，否则动态加载时会直接请求没有 default 导出的原始 CommonJS 文件。
  optimizeDeps: { include: ['mermaid'] },
  resolve: {
    // 显式定位实际安装的 ESM 文件，绕过开发期残留的包入口解析缓存。
    // 源码保留包名导入，TypeScript 仍使用 Mermaid 官方类型。
    alias: [
      {
        find: /^mermaid$/,
        replacement: fileURLToPath(
          new URL('./node_modules/mermaid/dist/mermaid.core.mjs', import.meta.url),
        ),
      },
    ],
  },
  // 本项目仅使用普通 CSS；不向上查找其他工程或磁盘根目录的 PostCSS 配置。
  css: { postcss: { plugins: [] } },
  server: {
    host: '127.0.0.1',
    port: 15420,
    strictPort: true,
    // 提前转换首屏模块，减少 WebView 首次请求时逐层等待转换的瀑布。
    warmup: {
      clientFiles: [
        './src/main.tsx',
        './src/App.tsx',
        './src/useTermRillWorkspace.ts',
        './src/FilePane.tsx',
      ],
    },
    watch: {
      // 在目录入口就停止扫描。tests 内有虚拟环境和大量故障测试数据，不属于前端 HMR。
      // Rust 源码由 Tauri 自己监视，src 与脚本/配置文件仍保持热更新。
      ignored: [
        (watchedPath: string) => {
          const normalized = watchedPath.replace(/\\/g, '/');
          return nonFrontendRoots.some(
            (directory) => normalized === directory || normalized.startsWith(`${directory}/`),
          );
        },
      ],
    },
  },
});
