/** PDF 字体、CMap 和解码资源随应用打包；开发时也只提供同一份本地资源。 */
import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { Plugin } from 'vite';
export function pdfAssets(): Plugin {
  const assets = new Map<string, Buffer>();
  for (const folder of ['cmaps', 'standard_fonts', 'wasm', 'iccs']) {
    const root = new URL(`../node_modules/pdfjs-dist/${folder}/`, import.meta.url);
    for (const file of readdirSync(root)) {
      if (
        file.endsWith('.bcmap') ||
        file.endsWith('.ttf') ||
        file.endsWith('.pfb') ||
        file.endsWith('.wasm') ||
        file.endsWith('.js') ||
        file.endsWith('.icc')
      ) {
        assets.set(`pdfjs/${folder}/${file}`, readFileSync(fileURLToPath(new URL(file, root))));
      }
    }
  }
  return {
    name: 'termrill-pdf-assets',
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const path = (request.url ?? '').split('?')[0].replace(/^\//, '');
        const bytes = assets.get(path);
        if (!bytes) return next();
        response.setHeader(
          'Content-Type',
          path.endsWith('.wasm')
            ? 'application/wasm'
            : path.endsWith('.js')
              ? 'text/javascript'
              : 'application/octet-stream',
        );
        response.end(bytes);
      });
    },
    generateBundle() {
      for (const [fileName, source] of assets) this.emitFile({ type: 'asset', fileName, source });
    },
  };
}
