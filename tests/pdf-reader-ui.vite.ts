/** 独立本机界面回归入口，不修改应用开发端口，也不连接真实服务器。 */
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { pdfAssets } from '../scripts/pdf-assets';
export default defineConfig({
  plugins: [react(), pdfAssets()],
  css: { postcss: { plugins: [] } },
  server: { host: '127.0.0.1', port: 15421, strictPort: true },
});
