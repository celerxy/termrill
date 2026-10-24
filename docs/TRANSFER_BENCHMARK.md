# 下载性能对照

此工具只连接本机随机端口的 SSH/SFTP fixture，使用同一份随机文件，对比生产 TermRill 下载流程与系统 OpenSSH SFTP。它不连接私人服务器、不修改代理/TUN、路由、系统 SSH 配置或应用配置。

## 准备与执行

需要 Rust 依赖缓存、系统 OpenSSH `sftp`、Python 3.11 或更新版本，以及按 README 准备的 fixture 环境。脚本没有安装依赖或从网络下载工具的行为。默认编译 release 测试二进制；首次构建可能较慢，编译时间不计入样本。仅为本次编译设置 `CARGO_INCREMENTAL=0`，避免大型测试二进制产生多 GiB 增量缓存，不修改全局环境或编译检查规则。

Windows PowerShell：

```powershell
npm run benchmark:transfer -- -Python tests/.runtime-fixture-py312/Scripts/python.exe
```

只验证工具能否完成下载和校验，可使用较小的 debug 样本；debug 数据不能作为正式版速度依据：

```powershell
npm run benchmark:transfer -- -Python tests/.runtime-fixture-py312/Scripts/python.exe -BuildProfile debug -SizeMiB 8 -Rounds 2
```

Python 直接入口（跨平台源码入口，仍需各系统实测）：

```sh
python scripts/benchmark-transfer.py --build-profile release --size-mib 32 --rounds 3 --concurrency 1 3
```

`--size-mib` 为 1～256 MiB，`--rounds` 为 1～5 轮，`--concurrency` 为 TermRill 的 1～4 档设置。OpenSSH 固定单连接、32 KiB 请求、最多 64 个请求，不能把两边不同配置解释为算法本身的净加速。每轮交替先运行的客户端；两边都使用冷连接，不保留上轮连接池。系统及源文件缓存不清空。

`--read-delay-ms`（PowerShell `-ReadDelayMs`）为 fixture 每次读取设置 0～100 ms 延迟，不是链路 RTT，也不模拟真实丢包、带宽或 TUN 协议开销。`--scenario loopback|direct|tun`（`-Scenario`）仅为人工环境标签；所有报告仍标记 `scope=loopback-only`、`scenarioVerified=false`。即使标签为 `tun`，本机流量也可能绕过代理，不能据此比较美国线路。

## 校验与报告

每轮独立计时，成功退出后检查目标长度并流式计算完整 SHA256；原始文件、两客户端每次下载摘要必须相同。失败、超时、缺失产物或摘要不符即停止，不生成部分成功报告。

报告默认保存在 `tests/.runtime-benchmark-results/<随机标识>.json`，仅含客户端、构建模式、样本、字节、摘要、耗时、MiB/s 与分组中位数，不包含账号、主机地址、私钥路径、文件名或正文。Python 参数 `--output` 可选择仓库内尚不存在的报告文件；绝不覆盖已有报告。不要把大文件、原始日志、临时密钥或运行目录提交到公开仓库。

总耗时从子进程启动到退出，包含客户端启动、认证、下载和各自收尾，不包含编译、生成源文件或最终 SHA256。TermRill 额外记录建连和传输阶段及确认字节回调的最大间隔；阶段时间包含在总耗时内，不应重复相加。确认字节间隔不等于网络停收时间。两客户端的持久化、内部摘要和进程启动开销可能不同。

进程退出后关闭 fixture，清理受路径保护的临时目录和测试密钥。失败仅把本机进程日志保留在 ignored 结果目录，不复制密钥和正文。工具不自动调整读取窗口、并发偏好、限速或恢复策略。

生成源文件前会检查至少三份样本大小加 32 MiB 的空闲空间；这个预估不包括首次 Rust 编译所需空间。完整 SSH 回归若需要使用其他测试磁盘，可运行 `./tests/run.ps1 -Python <Python路径> -RuntimeRoot <已有临时目录>`。它只创建并清理该目录下本次随机命名的子目录，默认位置仍在仓库 tests 下；失败日志保存失败也不能阻止清理本次数据。

## 真实线路测量边界

真实服务器与直连/TUN 对照尚未接入自动入口。需另行明确目标、只读文件、身份校验方式及访问授权后实现；不能把本工具临时公钥机制用于信任外部服务器，也不能把回环数据当作真实线路测量或跑满带宽的证据。
