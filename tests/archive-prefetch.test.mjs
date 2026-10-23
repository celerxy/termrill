/** 浏览器无关的队列回归：前台优先、同请求复用、关闭/密码变化丢弃迟到结果。 */
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import assert from 'node:assert/strict';
import ts from 'typescript';
const source = await readFile(new URL('../src/archivePrefetch.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
}).outputText;
const { ArchivePrefetch } = await import(
  `data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`
);
const entry = (name, extra = {}) => ({
  name,
  size: 1024,
  compressed: 1024,
  safe: true,
  encrypted: false,
  directory: false,
  ...extra,
});
const delay = () => new Promise((resolve) => setTimeout(resolve, 150));
function fixture(entries = [entry('1.jpg'), entry('2.jpg')]) {
  const jobs = [];
  const queue = new ArchivePrefetch((name) => {
    let resolve;
    const done = new Promise((finish) => {
      resolve = finish;
    });
    const job = {
      name,
      done,
      cancelled: false,
      resolve,
      cancel() {
        this.cancelled = true;
        resolve(null);
      },
    };
    jobs.push(job);
    return job;
  });
  queue.configure('revision-password', entries);
  return { queue, jobs };
}
test('前台点击同一图片复用预取，不创建第二次加载', async () => {
  const { queue, jobs } = fixture();
  await delay();
  const foreground = queue.foreground('1.jpg');
  jobs[0].resolve('verified-image');
  assert.equal(await foreground, 'verified-image');
  assert.equal(jobs.length, 1);
  assert.equal(jobs[0].cancelled, false);
  assert.equal(queue.get('1.jpg'), 'verified-image');
  queue.reset();
});
test('不同图片取消后台任务，剩余队列不会抢占前台', async () => {
  const { queue, jobs } = fixture();
  await delay();
  assert.equal(await queue.foreground('2.jpg'), null);
  assert.equal(jobs[0].cancelled, true);
  await delay();
  assert.equal(jobs.length, 1);
  queue.reset();
});
test('重置版本后，前台等待的旧结果不能返回或重新写入缓存', async () => {
  const { queue, jobs } = fixture();
  await delay();
  jobs[0].cancel = () => {};
  const foreground = queue.foreground('1.jpg');
  queue.reset();
  jobs[0].resolve('old-image');
  assert.equal(await foreground, null);
  assert.equal(queue.get('1.jpg'), undefined);
});
test('大图、加密或不安全条目不隐式预取；缓存受 32 MiB 限制', async () => {
  const { queue, jobs } = fixture([
    entry('large.jpg', { size: 1024 * 1024 }),
    entry('secret.jpg', { encrypted: true }),
    entry('unsafe.jpg', { safe: false }),
    entry('valid.jpg'),
  ]);
  await delay();
  assert.deepEqual(
    jobs.map((job) => job.name),
    ['valid.jpg'],
  );
  queue.put('first', 'a'.repeat(20 * 1024 * 1024));
  queue.put('second', 'b'.repeat(20 * 1024 * 1024));
  assert.equal(queue.get('first'), undefined);
  assert.ok(queue.get('second'));
  queue.reset();
});
test('重复候选只加载一次；本地归档切换密码同样清除解码缓存', async () => {
  const { queue, jobs } = fixture();
  queue.hint(['1.jpg', '1.jpg', '1.jpg']);
  await delay();
  jobs[0].resolve('verified-image');
  await delay();
  assert.equal(jobs.length, 1);
  queue.put('private.jpg', 'decoded-with-first-password');
  queue.configure('new-password', []);
  assert.equal(queue.get('private.jpg'), undefined);
  queue.reset();
});
