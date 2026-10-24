/** 收尾失败保留固定阶段，通用 SFTP 错误不能被推断为权限或空间不足。 */
import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';

const source = await readFile(new URL('../src/errorDiagnosis.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
});
const { diagnoseError, diagnosticReport } = await import(
  `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`
);

test('通用 SFTP FAILURE 不猜测具体原因，已知收尾步骤可单独分类', () => {
  assert.equal(diagnoseError('Failure: sftp: "Failure" (SSH_FX_FAILURE)').code, 'unknown');
  const diagnosis = diagnoseError('远程文件提交失败（保留目标文件权限）：Failure');
  assert.equal(diagnosis.code, 'commit');
  assert.equal(diagnosis.phase, '保留目标文件权限');
  assert.equal(
    diagnoseError('远程文件提交失败（关闭上传文件）：connection closed').code,
    'network',
  );
});

test('下载数据连接断开归入连接中断，不再显示为未分类错误', () => {
  assert.equal(
    diagnoseError(
      '下载数据连接已断开，没有可继续读取的连接：服务器长时间无响应，连接已断开（keepalive 超时）',
    ).code,
    'network_timeout',
  );
});

test('目标已安装的备份清理失败优先提示核对，不能建议直接重新上传', () => {
  const diagnosis = diagnoseError(
    '远程文件提交失败（清理旧文件备份）：目标文件已安装，但旧备份清理失败；Permission denied',
  );
  assert.equal(diagnosis.code, 'installed_cleanup');
  assert.match(diagnosis.advice, /不要直接重复上传/);
});

test('诊断摘要只带固定阶段，原始错误中的地址和路径不进入摘要', () => {
  const diagnosis = diagnoseError('远程文件提交失败（安装目标文件）：private.example/path Failure');
  const report = diagnosticReport('传输', diagnosis.phase, diagnosis.code, diagnosis.title);
  assert.match(report, /阶段：安装目标文件/);
  assert.equal(report.includes('private.example'), false);
  assert.equal(diagnoseError('远程文件提交失败（private.example/path）：Failure').phase, undefined);
});

test('连接阶段区分路由、解析、拒绝和超时，不从孤立系统错误码推断平台', () => {
  for (const [message, code] of [
    ['No route to host (os error 65)', 'network_route'],
    ['Network is unreachable (os error 101)', 'network_route'],
    ['failed to lookup address information: nodename nor servname provided', 'network_dns'],
    ['不知道这样的主机。 (os error 11001)', 'network_dns'],
    ['Connection refused (os error 61)', 'network_refused'],
    ['Connection timed out (os error 60)', 'network_timeout'],
  ]) {
    assert.equal(diagnoseError(message, '连接').code, code);
  }
  assert.equal(diagnoseError('os error 65').code, 'unknown');
  assert.match(diagnoseError('No route to host', '连接').advice, /本地网络权限/);
});

test('SSH 认证拒绝不误报为目录权限，协议协商失败不建议跳过校验', () => {
  assert.equal(diagnoseError('Permission denied (publickey,password).', '连接').code, 'auth');
  assert.equal(diagnoseError('Permission denied', '连接').code, 'permission');
  assert.match(diagnoseError('Permission denied', '连接').advice, /私钥/);
  assert.match(diagnoseError('Permission denied', '文件编辑').advice, /目标目录/);
  assert.equal(diagnoseError('no matching key exchange method found').code, 'ssh_negotiation');
  assert.equal(diagnoseError('No such file or directory').code, 'missing');
});
