/** 按后端识别或用户指定的编码保持分段解码状态；旧后端无标签时兼容 UTF-8/GB18030。 */
export class PreviewTextDecoder {
  private decoder: TextDecoder | null = null;
  private decided = false;
  encoding = 'UTF-8';
  private pending = new Uint8Array(0);

  constructor(name?: string) {
    if (name) {
      const label = name.startsWith('UTF-16 LE')
        ? 'utf-16le'
        : name.startsWith('UTF-16 BE')
          ? 'utf-16be'
          : name === 'UTF-8 BOM'
            ? 'utf-8'
            : name;
      this.decoder = new TextDecoder(label, { fatal: true });
      this.decided = true;
      this.encoding = name;
    }
  }

  decode(bytes: Uint8Array, complete: boolean): string {
    if (!this.decoder) {
      const name =
        bytes[0] === 255 && bytes[1] === 254
          ? 'utf-16le'
          : bytes[0] === 254 && bytes[1] === 255
            ? 'utf-16be'
            : 'utf-8';
      this.decided = name !== 'utf-8' || (bytes[0] === 239 && bytes[1] === 187 && bytes[2] === 191);
      this.decoder = new TextDecoder(name, { fatal: true });
      this.encoding = name.toUpperCase();
    }
    if (!this.decided && this.pending.length) {
      const joined = new Uint8Array(this.pending.length + bytes.length);
      joined.set(this.pending);
      joined.set(bytes, this.pending.length);
      bytes = joined;
      this.decoder = new TextDecoder('utf-8', { fatal: true });
    }
    let text: string;
    try {
      text = this.decoder.decode(bytes, { stream: !complete });
    } catch {
      if (this.decided) throw new Error('文本编码无法解析，请确认文件编码；原文件未修改');
      // 已交付的前缀仅含 ASCII，在 UTF-8 与 GB18030 中一致，切换不会改变其内容。
      this.decoder = new TextDecoder('gb18030', { fatal: true });
      this.encoding = 'GB18030';
      this.decided = true;
      try {
        text = this.decoder.decode(bytes, { stream: !complete });
      } catch {
        throw new Error('文件既不是有效 UTF-8，也无法按 GB18030 解析；原文件未修改');
      }
    }
    if (!this.decided && bytes.some((byte) => byte >= 128)) {
      if (!complete && !/[^\x00-\x7f]/.test(text)) {
        this.pending = bytes.slice();
        return '';
      }
      this.decided = true;
    }
    this.pending = new Uint8Array(0);
    if (text.includes('\0')) throw new Error('文件包含二进制内容，无法作为文本预览');
    return text;
  }
}
