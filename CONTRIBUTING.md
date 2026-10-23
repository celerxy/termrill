# 贡献指南

欢迎通过 [Issues](https://github.com/celerxy/termrill/issues) 反馈问题、讨论功能，并通过 Pull Request 贡献改进。安全漏洞请按 [安全反馈说明](SECURITY.md) 联系维护者。

## 提交问题

先搜索已有 Issue。报告问题时提供应用版本或提交号、操作系统、复现步骤、预期与实际结果。连接问题可补充认证类型和是否使用标准 SSH 跳板或代理。

截图、配置和日志必须脱敏：不要公开密码、令牌、私钥、真实服务器地址、主机指纹、私人文件名或含本机用户名的路径。使用 `example.com`、虚构目录和测试数据描述问题。

## 开发与提交

1. 阅读 [README](README.md) 和 [开发规范](AGENTS.md)。涉及认证、传输、终端或数据格式时，同时阅读 [架构说明](docs/ARCHITECTURE.md)。
2. 从 `main` 创建工作分支，每个 Pull Request 聚焦一个可审阅的问题。较大行为变更建议先在 Issue 中讨论范围。
3. 按 README 准备独立开发与测试环境。默认使用本机临时 SSH 服务；真实服务器验证必须获得服务器所有者授权。
4. 使用英文 Conventional Commits，例如 `fix(transfer): preserve files when cancellation fails`。提交前确认作者信息和 GitHub 隐私邮箱正确，不提交依赖目录、构建产物、个人配置或运行日志。
5. 按改动范围运行检查，并在 Pull Request 中说明问题、变更行为、实际验证和未验证范围。界面变更附上使用虚构数据的截图。

## 验证要求

纯文档修改检查格式和链接。前端修改至少运行 `npm run format:check` 和 `npm run build`；Rust 修改至少运行 rustfmt、Cargo check 和 Clippy。认证、指纹、跳板、传输覆盖与取消、路径或终端协议变更还应运行 `./tests/run.ps1`。

完整检查入口为 `npm run check`，环境准备与可选 Python 参数见 README。不要将未运行的检查写成通过，也不要用本机测试结果代替真实服务器或其他操作系统的验收。

## 许可

提交贡献前，请确认你有权提供相关代码、文档和素材。向本项目提交的贡献按项目的 [MIT License](LICENSE) 提供；引入第三方内容时须保留其适用许可和版权声明。
