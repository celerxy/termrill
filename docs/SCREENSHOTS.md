# 产品展示图

README 中的四张展示图来自当前 React 产品组件的浏览器渲染。外层标题、窗口装饰和页脚由截图入口添加；产品组件及其布局不为截图修改。展示图用于介绍界面，不是已安装桌面包或真实服务器兼容性的验收证据。

## 数据与隐私边界

- 截图入口为 `tests/showcase-ui.html`，生产构建不导入该入口或数据。
- 使用 Tauri 官方 `mockIPC`、事件 mock 和窗口 mock；所有后端调用由明确列出的虚构响应处理，未实现的命令直接报错，不回退到真实 IPC。
- 连接使用 `example.com` 的示例子域名；本地路径为 `C:\Demo\releases`，远程路径为 `/srv/demo/releases`，均为虚构目录。
- 终端输出来自固定文本，不执行远程命令；任务进度为暂停/完成示例，不展示虚构传输速率。
- 不读取真实连接配置、系统凭据库、SSH Agent、私人文件、主机指纹或性能日志。截图前后不需要连接任何真实服务器。
- 更新展示图时逐张检查图片里的地址、用户名、目录、文件名、文档内容和终端输出。不能用含个人信息的运行截图替换这些图片。

## 本机复现

先按 [README](../README.md) 准备 Node.js 依赖，然后在仓库根目录运行：

```powershell
npx vite --config tests/showcase-ui.vite.ts
```

该独立入口仅监听 `127.0.0.1:15422`，不修改产品开发端口。浏览器打开 `http://127.0.0.1:15422/tests/showcase-ui.html?scene=workspace`。首次模块编译可能需要等待，页面显示顶部展示标题和工作台后再操作。

| 图片                         | scene 参数   | 截图操作                                                              |
| ---------------------------- | ------------ | --------------------------------------------------------------------- |
| `screenshots/workspace.jpg`  | `workspace`  | 点击“连接”，在演示表单中点击“保存并连接”；展示双栏文件与暂停/完成队列 |
| `screenshots/terminal.jpg`   | `terminal`   | 连接演示会话，点击“SSH 终端”，新增第二个终端，切换分屏并展开终端      |
| `screenshots/preview.jpg`    | `preview`    | 连接演示会话，打开 `release-notes.md`，切换到 Markdown 预览           |
| `screenshots/connection.jpg` | `connection` | 点击侧栏“编辑 Staging · SSH 跳板”，展示 Agent 与目标/跳板的独立配置   |

四张图片使用相同的浏览器视口、缩放和外层展示框。保存浏览器原始截图为 JPEG，避免重绘产品文字或添加未实现的功能；重新生成后检查 README 图片链接和图片清晰度。
