/** 工作区内的成功导航历史；只有目录实际读取成功后才移动历史位置。 */
export class DirectoryHistory {
  private paths: string[] = [];
  private index = -1;
  private pending: number | null = null;

  get canBack() {
    return this.index > 0;
  }
  get canForward() {
    return this.index >= 0 && this.index < this.paths.length - 1;
  }

  visit(path: string) {
    if (this.pending !== null && this.paths[this.pending] === path) {
      this.index = this.pending;
    } else if (this.paths[this.index] !== path) {
      this.paths = [...this.paths.slice(0, this.index + 1), path].slice(-50);
      this.index = this.paths.length - 1;
    }
    this.pending = null;
  }

  request(direction: 'back' | 'forward') {
    const next = this.index + (direction === 'back' ? -1 : 1);
    if (next < 0 || next >= this.paths.length) return null;
    this.pending = next;
    return this.paths[next];
  }
}
