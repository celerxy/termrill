/** 分类仅作为恢复建议；没有后端结构化错误码时不能据此改变操作权限。 */
export function diagnoseError(message: string) {
  const rules = [
    {
      code: 'installed_cleanup',
      title: '文件已安装，备份清理失败',
      match: /目标文件已安装.*旧备份清理失败/,
      advice: '先刷新并核对目标文件，再处理提示的旧备份；不要直接重复上传或删除备份。',
    },
    {
      code: 'trust',
      title: '主机身份校验问题',
      match: /fingerprint|host key|指纹|主机身份/i,
      advice: '核对服务器指纹和变更原因，再通过原连接流程确认。',
    },
    {
      code: 'auth',
      title: '认证失败',
      match: /authentication|unauthenticated|认证失败|认证被拒|密码错误|验证码/i,
      advice: '检查用户名、认证方式和凭据；需要验证码时使用交互式认证。',
    },
    {
      code: 'space',
      title: '存储空间不足',
      match: /no space|disk full|quota exceeded|空间不足|磁盘已满|配额/i,
      advice: '检查目标磁盘空间或配额，释放空间或选择其他目录后再重试。',
    },
    {
      code: 'permission',
      title: '权限不足',
      match: /permission denied|access denied|权限不足|权限拒绝|拒绝访问|没有权限/i,
      advice: '检查当前账号与目标目录权限；可在属性与权限中查看，或选择可写目录。',
    },
    {
      code: 'conflict',
      title: '文件冲突或内容变化',
      match: /already exists|file exists|同名|已存在|内容.*改变|内容.*变化|revision|摘要.*不匹配/i,
      advice: '核对源文件与目标文件。内容改变时重新创建任务；覆盖仍需明确确认。',
    },
    {
      code: 'network',
      title: '连接中断或超时',
      match:
        /timed? out|timeout|connection.*(?:reset|refused|closed|lost)|broken pipe|not connected|连接.*(?:中断|关闭|失败)|断开|重连|请连接|任务中断|超时/i,
      advice: '检查网络和服务器状态，连接原服务器后再重试；续传仍需核对已有内容。',
    },
    {
      code: 'missing',
      title: '文件或目录不存在',
      match: /no such file|not found|不存在|找不到/i,
      advice: '刷新目录并核对路径，确认文件是否已移动或删除。',
    },
    {
      code: 'format',
      title: '格式不支持或内容无法解析',
      match: /unsupported|decode|parse|invalid json|不支持|解码|解析|格式错误|损坏/i,
      advice: '核对文件格式和内容，可下载后使用相应工具打开；不要直接覆盖原文件。',
    },
    {
      code: 'cancelled',
      title: '操作已取消',
      match: /cancelled|canceled|已取消/i,
      advice: '检查已完成项目和目标目录；取消不一定撤销此前完成的文件操作。',
    },
    {
      code: 'commit',
      title: '远程文件提交失败',
      match: /远程文件提交失败/,
      advice:
        '按错误中的具体步骤核对目标权限、文件占用及改名能力；通用 SFTP FAILURE 不能直接证明原因，确认目标和备份状态后再重试。',
    },
  ];
  const rule = rules.find((item) => item.match.test(message));
  const stages = [
    '关闭上传文件',
    '恢复续传临时文件名',
    '核对上传临时文件',
    '保留目标文件权限',
    '原子替换目标文件',
    '备份原目标文件',
    '安装目标文件',
    '清理旧文件备份',
  ];
  // 只导出固定阶段白名单，不能从服务器错误中摘取路径、账号或任意文本。
  const phase = stages.find((stage) => message.startsWith(`远程文件提交失败（${stage}）：`));
  return {
    ...(rule ?? {
      code: 'unknown',
      title: '未分类错误',
      advice: '查看原始错误和当前操作阶段，核对目标状态后再决定是否重试。',
    }),
    phase,
  };
}

/** 使用固定场景与阶段标识，不导出原始错误、文件内容、地址、路径或凭据。 */
export function diagnosticReport(scope: string, phase: string, code: string, title: string) {
  return [
    'TermRill 诊断摘要',
    `时间：${new Date().toISOString()}`,
    `场景：${scope}`,
    `阶段：${phase}`,
    `分类：${code} / ${title}`,
    '分类依据：客户端错误文本匹配，仅供排查参考',
    '隐私：未包含原始错误、主机地址、文件路径、文件正文或认证凭据',
  ].join('\n');
}
