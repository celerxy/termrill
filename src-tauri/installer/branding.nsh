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
!define MUI_INSTFILESPAGE_COLORS "6755D8 FFFFFF"
!define MUI_INSTFILESPAGE_PROGRESSBAR "smooth colored"
!define MUI_WELCOMEFINISHPAGE_BITMAP_STRETCH AspectFitHeight

; 默认折叠逐文件日志，保留“显示详情”入口用于失败排查，不隐藏错误或删除日志能力。
ShowInstDetails hide
ShowUninstDetails hide

; Tauri 在进度页已创建后执行这两个钩子。仅关闭进度条控件的主题绘制，
; 保留其真实进度值、系统按钮与安装/卸载行为；所有临时寄存器必须恢复。
!macro TermRillStyleProgress
  Push $1
  Push $2
  ; 使用 MUI 在页面显示回调中记录的句柄，不能用 FindWindow 猜测当前内页。
  StrCpy $1 $mui.InstFilesPage.ProgressBar
  ; System 插件把空字符串当作空指针；使用空格明确禁用此控件的主题，避免恢复系统绿色。
  System::Call 'uxtheme::SetWindowTheme(p r1, w " ", w " ")'
  SendMessage $1 ${PBM_SETBARCOLOR} 0 0x00D85567
  SendMessage $1 ${PBM_SETBKCOLOR} 0 0x00F6F0EE
  ; 默认详情列表使用等宽字体；沿用当前语言的界面字体以统一中文排版。
  StrCpy $1 $mui.InstFilesPage.Log
  SendMessage $HWNDPARENT ${WM_GETFONT} 0 0 $2
  SendMessage $1 ${WM_SETFONT} $2 1
  Pop $2
  Pop $1
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro TermRillStyleProgress
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro TermRillStyleProgress
!macroend
