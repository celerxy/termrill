/** 工作区内的目录页缓存：3 秒内复用，30 秒内可先展示再校验；不持久化。 */
import type { Listing } from './types';

export class RemoteDirectoryCache {
  private pages = new Map<string, { listing: Listing; readAt: number }>();

  get(key: string) {
    const page = this.pages.get(key);
    if (!page) return null;
    const age = performance.now() - page.readAt;
    if (age > 30_000) {
      this.pages.delete(key);
      return null;
    }
    this.pages.delete(key);
    this.pages.set(key, page);
    return { listing: page.listing, fresh: age < 3_000 };
  }

  set(key: string, listing: Listing) {
    this.pages.delete(key);
    this.pages.set(key, { listing, readAt: performance.now() });
    while (this.pages.size > 8) {
      const oldest = this.pages.keys().next().value;
      if (oldest === undefined) break;
      this.pages.delete(oldest);
    }
  }

  clear() {
    this.pages.clear();
  }
}
