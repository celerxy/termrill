/** 日志显示有界且仅文本处理，过滤不改变读取游标或触发远程执行。 */
export function appendLog(previous: string, incoming: string) {
  const text = (previous + incoming).slice(-500000);
  const lines = text.split('\n');
  return lines.slice(-2000).join('\n');
}
export function filterLog(text: string, query: string) {
  return query
    ? text
        .split('\n')
        .filter((line) => line.toLocaleLowerCase().includes(query.toLocaleLowerCase()))
        .join('\n')
    : text;
}
/** 首次从文件末尾开始时丢弃首条残缺记录，也避免从多字节字符中间解码。 */
export function skipLogPrefix(bytes: Uint8Array, encoding: string) {
  const wide = encoding.startsWith('UTF-16');
  const big = encoding.includes('BE');
  for (let i = 0; i < bytes.length; i += wide ? 2 : 1) {
    if (
      (!wide && bytes[i] === 10) ||
      (wide && bytes[i] === (big ? 0 : 10) && bytes[i + 1] === (big ? 10 : 0))
    ) {
      return bytes.slice(i + (wide ? 2 : 1));
    }
  }
  return null;
}
