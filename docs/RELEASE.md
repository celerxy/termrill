# 打包与安装说明

当前产品与应用标识为 TermRill / `dev.termrill.sftpclient`，版本为 `0.3.0`。最新源码尚未完成安装包验收；历史打包成功记录不代表当前包可交付。验证状态见 [验收记录](READINESS.md)。

## 构建

在 TermRill 目录运行 `npm run release:windows`。Rust 依赖已缓存时可运行 `npm run release:windows -- -Offline`。首次 release 编译拥有独立缓存，OpenSSL 和 Rust 依赖可能需要较长时间。

脚本使用 Windows PowerShell，源码保存为 UTF-8 BOM，保证 PowerShell 5.1 能正确读取中文。脚本先复用 `scripts/check-release.ps1`，检查 package.json、tauri.conf.json、Cargo.toml 三处版本一致性、格式、前端测试与构建、Rust 静态检查及本机 SSH 回归，再调用 Tauri 正式构建；任一步失败都会停止，不会使用旧安装包冒充成功。Cargo 离线变量只在脚本进程中临时设置并恢复。

输出位于 `src-tauri/target/release/bundle/nsis/`，每个本次生成的 `.exe` 同时生成 `.exe.sha256`。校验文件使用 UTF-8，无 BOM，格式为 SHA256 和安装包文件名。安装包校验示例：

```powershell
Get-FileHash -LiteralPath '安装包的完整路径.exe' -Algorithm SHA256
```

构建不会安装、启动、签名或上传应用。当前采用当前用户安装模式，不需要修改为全机安装。应用版本、标识和配置目录保持现有设置。

本机完整检查使用锁文件及离线 Cargo，需先按 README 准备 Python fixture 和依赖缓存。可先运行 `cargo fetch --locked --manifest-path src-tauri/Cargo.toml`；指定 Python 使用 `npm run release:windows -- -Python tests/.runtime-fixture-py312/Scripts/python.exe`，可与 `-Offline` 组合。`./scripts/check-release.ps1 -Package` 保留为离线打包别名，委托发行脚本，完整检查只执行一次。

## Windows 安装器外观

NSIS 沿用 Tauri 默认的安装、覆盖升级、WebView2 准备与卸载流程，仅通过 `src-tauri/installer/branding.nsh` 设置中英文欢迎文案和字体。中文使用 Microsoft YaHei UI，英文使用 Segoe UI，默认字号为 10pt；安装与卸载图标复用应用图标，且显式共用品牌页眉。进度页使用紫色进度条，详情列表沿用当前语言的界面字体；默认折叠详情，保留展开排查入口。只有进度条关闭主题绘制，不更改系统主题或按钮。安装模式仍为当前用户，语言选择保持简体中文与英文。

侧图 `sidebar.bmp` 为 328×628，页眉 `header.bmp` 为 300×114，均为 24 位 RGB BMP，按 NSIS 推荐比例提供双倍分辨率素材。图形沿用 `src/assets/termrill.svg` 的品牌标志；不分发系统字体。素材已纳入仓库，正常构建和 CI 不需要图形生成依赖。需要重新生成时，在装有 Pillow 的 Python 环境运行：

```powershell
python scripts/generate-installer-artwork.py --font "$env:WINDIR/Fonts/segoeuib.ttf"
```

已使用现有调试二进制执行 `tauri bundle --debug --bundles nsis --ci --no-sign`，生成安装包并检查实际欢迎页与维护页外观；未执行安装、覆盖升级或卸载。这是外观验证，不代表当前源码已完成发行构建与安装验收，也不会替换已发布版本。

## macOS 免费签名

`src-tauri/tauri.macos.conf.json` 使用 Tauri 官方支持的 `signingIdentity: "-"`。macOS 本机与 Actions 构建会在生成 DMG 前对应用进行 ad-hoc 签名，不需要 Apple 开发者账户、证书或 Secrets，也不改变应用标识、网络权限或系统安全设置。

Actions 在收集产物前执行 `scripts/verify-macos-signature.sh`：对构建目录及只读挂载 DMG 中的 `.app` 分别运行严格签名校验，确认 ad-hoc 类型、应用标识、非空 UUID 和目标架构，并比较主程序完整 SHA256。未签名、签名失效、缺失或错误 UUID、镜像内容不一致都将使任务失败，不上传安装包。校验结束卸载镜像，只删除脚本创建的空目录。

ad-hoc 签名不建立 Apple 认证的开发者身份，也不等于公证；首次打开仍可能需要系统的来源确认，更新后的本地网络权限识别也不保证稳定。它针对当前完全未签名的 Mac 测试包作改进，不能据此声称所有版本 macOS 的连接问题都已修复。正式分发的签名要求见 [Tauri macOS 签名说明](https://v2.tauri.app/distribute/sign/macos/)。本机 Windows 只能验证脚本语法和工作流结构；签名工具、DMG 内容及真实连接需由后续 Mac 构建和实机验证。

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

`.github/workflows/installers.yml` 提供手动工作流 **Test installers**，不随每次推送自动打包。默认只上传 Artifacts；勾选 `publish_release` 后，全部平台通过才创建对应新版本的预发布 Release。提交并推送工作流到默认分支后：

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

该流程使用 GitHub 自动提供的工作流权限，不需要手动 PAT、SSH 私钥、服务器密码或签名 Secrets。Windows 测试包未使用开发者签名；macOS 源码配置使用免费的 ad-hoc 签名，未使用 Developer ID 签名或公证。系统可能提示来源未验证，正式分发仍需签名与各平台实机验收，不关闭系统安全设置来掩盖这些限制。普通使用者无需安装 Node.js、Rust 或 Python，运行环境和系统库要求由相应安装包及系统决定。

2026-10-24，[托管构建 `40043928487`](https://github.com/celerxy/termrill/actions/runs/40043928487) 已生成并上传基于 `478894f` 的 Windows x64 测试包，可在运行页面的 Artifacts 下载。该测试包不包含 EPUB 阅读器；Windows 安装、卸载和升级仍需实机验收。macOS Intel 依赖下载超时，macOS ARM 的收集入口测试失败，Linux 的 Clippy 检查失败，因此整次运行显示失败。

收集脚本通过真实路径比较兼容 macOS 路径别名；可变声明只用于 Windows 分支。锁文件下载地址统一为 npm 官方源，版本与完整性校验值保持不变；安装包工作流对 npm 下载设置有界重试与超时，不忽略失败。这些修复已通过本机检查和 [四平台托管构建](https://github.com/celerxy/termrill/actions/runs/40048404019)，对应源码已发布 `v0.1.0` 预发布；安装包产物与 GitHub Release 分别管理，构建通过不代表四平台安装验收完成。

## 新版本发布

先同步 package.json、package-lock.json、Cargo.toml、Cargo.lock 与 Tauri 的版本，并为该版本新增 `docs/releases/v版本.md`。推送源码后手动运行 Test installers 并勾选 `publish_release`。发行任务只接受当前提交的四平台完整产物，验证 ZIP 摘要与五个安装包的校验文件，再上传十个附件到草稿；逐项核对上传摘要后才公开预发布。同版本已公开内容不允许覆盖，重试只复用同一提交和说明的草稿附件。
