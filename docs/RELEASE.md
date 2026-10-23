# 打包与安装说明

当前产品与应用标识为 TermRill / `dev.termrill.sftpclient`，版本为 `0.1.0`。最新源码尚未完成安装包验收；历史打包成功记录不代表当前包可交付。验证状态见 [验收记录](READINESS.md)。

## 构建

在 TermRill 目录运行 `npm run release:windows`。Rust 依赖已缓存时可运行 `npm run release:windows -- -Offline`。首次 release 编译拥有独立缓存，OpenSSL 和 Rust 依赖可能需要较长时间。

脚本使用 Windows PowerShell，源码保存为 UTF-8 BOM，保证 PowerShell 5.1 能正确读取中文。脚本先复用 `scripts/check-release.ps1`，检查 package.json、tauri.conf.json、Cargo.toml 三处版本一致性、格式、前端测试与构建、Rust 静态检查及本机 SSH 回归，再调用 Tauri 正式构建；任一步失败都会停止，不会使用旧安装包冒充成功。Cargo 离线变量只在脚本进程中临时设置并恢复。

输出位于 `src-tauri/target/release/bundle/nsis/`，每个本次生成的 `.exe` 同时生成 `.exe.sha256`。校验文件使用 UTF-8，无 BOM，格式为 SHA256 和安装包文件名。安装包校验示例：

```powershell
Get-FileHash -LiteralPath '安装包的完整路径.exe' -Algorithm SHA256
```

构建不会安装、启动、签名或上传应用。当前采用当前用户安装模式，不需要修改为全机安装。应用版本、标识和配置目录保持现有设置。

本机完整检查使用锁文件及离线 Cargo，需先按 README 准备 Python fixture 和依赖缓存。可先运行 `cargo fetch --locked --manifest-path src-tauri/Cargo.toml`；指定 Python 使用 `npm run release:windows -- -Python tests/.runtime-fixture-py312/Scripts/python.exe`，可与 `-Offline` 组合。`./scripts/check-release.ps1 -Package` 保留为离线打包别名，委托发行脚本，完整检查只执行一次。

## 安装与升级验收

以下是发布前人工检查清单，不代表已经通过。使用独立 Windows 测试账户及临时 SSH 服务，避免修改日常服务器文件。

- 首次安装并启动，检查 WebView2 准备流程、快捷方式和卸载入口。
- 建立首个已验收的 TermRill 包后，以相同应用标识测试后续版本覆盖升级，检查配置、可信主机、显示偏好和未完成任务保留；当前不提供旧品牌跨标识迁移。
- 首次连接仍要求确认主机指纹；保存密码需要主动选择系统凭据库。
- 使用临时目录上传/下载；中断后重连并续传，核对文件内容和同名保护。
- 退出运行中的任务后再次启动，检查失败任务可恢复、取消任务不恢复。
- 卸载后检查个人配置的保留行为；删除配置必须由用户明确决定。

## 能力边界

Windows release 构建成功不代表安装、升级和真实网络中断场景已经验证。macOS/Linux 需要各自在对应系统上编译、打包和检查凭据服务；Windows 构建结果不能替代这些检查。

## GitHub Actions 多平台测试包

`.github/workflows/installers.yml` 提供手动工作流 **Test installers**，不随每次推送自动打包，也不创建 GitHub Release。提交并推送工作流到默认分支后：

1. 在仓库 **Settings → Actions → General** 确认允许运行 GitHub Actions 及配置中使用的官方 Actions。
2. 打开 **Actions → Test installers → Run workflow**，选择要构建的分支。
3. 先运行可复用的 Windows 完整检查，失败时所有打包任务停止；通过后四个平台任务各自执行版本、格式、前端测试、Rust check 与全目标 Clippy，再构建前端和安装包。
4. 在成功任务的 **Artifacts** 下载 `TermRill-平台-提交SHA`。每个平台独立上传，其他平台失败不会取消已在运行的任务，但整次工作流仍显示失败，不能据此宣称所有平台通过。

| 测试包平台          | GitHub runner  | 包格式              | 安装方式                                                |
| ------------------- | -------------- | ------------------- | ------------------------------------------------------- |
| Windows x64         | windows-2022   | NSIS `.exe`         | 解压 Artifact 后双击安装；通常无需管理员权限            |
| macOS Apple Silicon | macos-15       | `.dmg`              | 打开镜像后将应用拖入 Applications                       |
| macOS Intel         | macos-15-intel | `.dmg`              | 选择 Intel 包，打开镜像后将应用拖入 Applications        |
| Linux x64           | ubuntu-22.04   | `.deb`、`.AppImage` | deb 使用发行版包管理器安装；AppImage 赋予执行权限后运行 |

Artifacts 外层 ZIP 先解压再安装。每个包附带 `.sha256`：Windows 可使用 `Get-FileHash`，macOS 使用 `shasum -a 256`，Linux 使用 `sha256sum` 对照内容。下载保留 14 天，过期后需要重新构建。Linux 包基于 Ubuntu 22.04 构建，其他发行版、FUSE 和系统库组合仍需实际验证。

所有打包任务显式传入各平台 bundles，因此不改变本地 `tauri.conf.json` 的 Windows NSIS 默认配置。缓存按平台、架构、工具链和 Cargo 依赖隔离；构建前只清理已验证处于工作区 target 下的 bundle 目录，收集阶段要求每种声明格式都有产物且输出目录未存在，避免上传旧版本或漏格式。安装包仅上传明确的安装格式与校验文件，不上传源码、测试数据或应用配置。

该流程使用 GitHub 自动提供的工作流权限，不需要手动 PAT、SSH 私钥、服务器密码或签名 Secrets。测试包未使用 Windows 开发者签名、Apple Developer 签名或公证，系统可能拦截或提示来源未验证；正式分发需要后续接入签名与各平台实机验收，不关闭系统安全设置来掩盖这些限制。普通使用者无需安装 Node.js、Rust 或 Python，运行环境和系统库要求由相应安装包及系统决定。

当前只完成工作流配置与本地可执行检查，尚未在 GitHub 托管 runner 生成并安装验证四个平台产物，不将配置完成视为跨平台兼容性验收。
