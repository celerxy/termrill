/** Vite 完成模块转换后才监听开发端口，让 Tauri 的端口探测代表页面资源已经就绪。 */
import type { Plugin, ViteDevServer } from 'vite';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';

async function prepareModules(server: ViteDevServer) {
  const seen = new Set<string>();
  let pending = ['/src/main.tsx'];
  while (pending.length) {
    const batch = pending.filter((url) => !seen.has(url));
    batch.forEach((url) => seen.add(url));
    const children = await Promise.all(
      batch.map(async (url) => {
        const started = performance.now();
        await server.transformRequest(url);
        const elapsed = Math.round(performance.now() - started);
        if (elapsed > 1000) server.config.logger.info(`[TermRill dev] ${url}: ${elapsed} ms`);
        const module = await server.moduleGraph.getModuleByUrl(url);
        return [...(module?.importedModules ?? [])]
          .map((dependency) => dependency.url)
          .filter((dependency) => dependency.startsWith('/src/'));
      }),
    );
    pending = children.flat();
  }
  // HTML 也有独立的转换链；仅预热 TS/CSS 不能保证首次文档响应及时。
  const htmlStarted = performance.now();
  const html = await readFile(join(server.config.root, 'index.html'), 'utf8');
  await server.transformIndexHtml('/', html);
  server.config.logger.info(
    `[TermRill dev] 首页 HTML 已准备，耗时 ${Math.round(performance.now() - htmlStarted)} ms`,
  );
  return seen.size;
}

export function devReadiness(): Plugin {
  return {
    name: 'termrill-dev-readiness',
    apply: 'serve',
    configureServer(server) {
      const httpServer = server.httpServer;
      if (!httpServer) return;
      const listen = httpServer.listen;
      // Vite 在 configureServer 之后包裹 listen 并初始化依赖优化器。
      // 此处接收到调用时，依赖初始化已完成，但端口还未对外开放。
      httpServer.listen = ((...args: unknown[]) => {
        const started = performance.now();
        server.config.logger.info('[TermRill dev] 正在准备页面模块…');
        void prepareModules(server)
          .then((count) => {
            server.config.logger.info(
              `[TermRill dev] ${count} 个页面模块已准备，耗时 ${Math.round(performance.now() - started)} ms`,
            );
            Reflect.apply(listen, httpServer, args);
          })
          .catch((error: unknown) => {
            httpServer.emit('error', error instanceof Error ? error : new Error(String(error)));
          });
        return httpServer;
      }) as typeof httpServer.listen;
    },
  };
}
