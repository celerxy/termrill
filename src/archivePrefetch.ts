/** 小图后台队列：单个解码任务、范围请求内部并发，前台复用或取消当前预取。 */
import type { ArchiveEntry } from './archiveTypes';

type Task = { name: string; done: Promise<string | null>; cancel: () => void };
const LIMIT = 32 * 1024 * 1024;
export class ArchivePrefetch {
  private cache = new Map<string, string>();
  private queue: string[] = [];
  private task: Task | null = null;
  private paused = false;
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private entries: ArchiveEntry[] = [];
  private identity = '';
  private last = -1;
  private sequential = 0;

  constructor(private load: (name: string) => Task) {}

  configure(identity: string, entries: ArchiveEntry[]) {
    if (this.identity === identity) return;
    this.reset();
    this.identity = identity;
    this.entries = entries.filter(
      (entry) =>
        entry.safe &&
        !entry.directory &&
        !entry.encrypted &&
        entry.size <= 512 * 1024 &&
        entry.compressed <= 512 * 1024 &&
        /\.(?:png|jpe?g|webp|gif|bmp|avif)$/i.test(entry.name),
    );
    this.hint(this.entries.map((entry) => entry.name));
  }

  get(name: string) {
    const bytes = this.cache.get(name);
    if (bytes) {
      this.cache.delete(name);
      this.cache.set(name, bytes);
    }
    return bytes;
  }

  put(name: string, bytes: string) {
    if (bytes.length > LIMIT) return;
    this.cache.delete(name);
    this.cache.set(name, bytes);
    while ([...this.cache.values()].reduce((sum, value) => sum + value.length, 0) > LIMIT) {
      const oldest = this.cache.keys().next().value;
      if (oldest === undefined) break;
      this.cache.delete(oldest);
    }
  }

  hint(names: string[]) {
    let budget = 0;
    this.queue = [...new Set(names)]
      .filter((name) => {
        const entry = this.entries.find((entry) => entry.name === name);
        if (!entry || this.cache.has(name) || name === this.task?.name) return false;
        budget += entry.compressed;
        return budget <= 2 * 1024 * 1024;
      })
      .slice(0, 6);
    this.schedule();
  }

  async foreground(name?: string): Promise<string | null> {
    const generation = this.generation;
    this.paused = true;
    this.clearTimer();
    const task = this.task;
    if (!task) return null;
    if (task.name !== name) task.cancel();
    // 等待当前请求退出后再复用面板句柄；不等待剩余预取队列。
    const bytes = await task.done.catch(() => null);
    return generation === this.generation && task.name === name ? bytes : null;
  }

  resume(name?: string) {
    this.paused = false;
    if (name) {
      const index = this.entries.findIndex((entry) => entry.name === name);
      this.sequential = index >= 0 && index === this.last + 1 ? this.sequential + 1 : 0;
      this.last = index;
      if (index >= 0) {
        // 连续浏览时增加邻居，随机点击仅预取两张，控制带宽和内存。
        this.hint(
          this.entries
            .slice(index + 1, index + 3 + Math.min(4, this.sequential))
            .map((entry) => entry.name),
        );
      }
    }
    this.schedule();
  }

  reset() {
    this.generation++;
    this.clearTimer();
    this.task?.cancel();
    this.queue = [];
    this.cache.clear();
    this.entries = [];
    this.identity = '';
    this.last = -1;
    this.sequential = 0;
  }

  private clearTimer() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }

  private schedule() {
    if (this.paused || this.task || this.timer || !this.queue.length) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      if (this.paused || this.task) return;
      const name = this.queue.shift();
      if (!name) return;
      const generation = this.generation;
      const task = this.load(name);
      this.task = task;
      void task.done
        .then((bytes) => {
          if (bytes && generation === this.generation) this.put(name, bytes);
        })
        .catch(() => {})
        .finally(() => {
          if (this.task === task) this.task = null;
          this.schedule();
        });
    }, 120);
  }
}
