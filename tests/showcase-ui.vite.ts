/** 仅在本机运行的产品截图入口；不改变应用端口，不加载真实配置或服务器。 */
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { pdfAssets } from '../scripts/pdf-assets';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  plugins: [react(), pdfAssets()],
  optimizeDeps: { include: ['mermaid'] },
  resolve: {
    alias: [
      {
        find: /^mermaid$/,
        replacement: fileURLToPath(
          new URL('../node_modules/mermaid/dist/mermaid.core.mjs', import.meta.url),
        ),
      },
    ],
  },
  css: { postcss: { plugins: [] } },
  server: {
    host: '127.0.0.1',
    port: 15422,
    strictPort: true,
    watch: {
      // 测试目录可包含 Python 环境和大量运行数据，只监视展示源码。
      ignored: ['**/tests/.runtime*/**', '**/tests/.venv/**', '**/src-tauri/**', '**/docs/**'],
    },
  },
});
