; 只定制 MUI 外观；保留 Tauri 默认的安装、升级、WebView2 和卸载逻辑。
; 此文件由 Tauri 在 MUI 页面与语言载入前包含，语言 ID 使用 NSIS 标准值。
SetFont "Microsoft YaHei UI" 10
SetFont /LANG=2052 "Microsoft YaHei UI" 10
SetFont /LANG=1033 "Segoe UI" 10

LangString TermRillWelcomeTitle 2052 "欢迎安装 TermRill"
LangString TermRillWelcomeTitle 1033 "Welcome to TermRill"
LangString TermRillWelcomeText 2052 "你的 SSH 与 SFTP 桌面工作台。$\r$\n$\r$\n在一个窗口中管理文件、传输任务和终端会话。$\r$\n$\r$\n将为当前 Windows 用户安装。若 TermRill 正在运行，请先保存工作并退出。$\r$\n$\r$\n点击「下一步」继续。"
LangString TermRillWelcomeText 1033 "Your desktop workspace for SSH and SFTP.$\r$\n$\r$\nManage files, transfers and terminal sessions in one place.$\r$\n$\r$\nTermRill will be installed for the current Windows user. Save your work and close TermRill before continuing.$\r$\n$\r$\nSelect Next to continue."

!define MUI_WELCOMEPAGE_TITLE "$(TermRillWelcomeTitle)"
!define MUI_WELCOMEPAGE_TEXT "$(TermRillWelcomeText)"
!define MUI_TEXTCOLOR "252943"
!define MUI_WELCOMEFINISHPAGE_BITMAP_STRETCH AspectFitHeight
