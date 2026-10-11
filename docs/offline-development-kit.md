# 完整 Windows 离线开发包

此包用于 Windows 10 x64 内网电脑没有 Node、Rust、C++ 编译工具和 WebView2 的情况。源码和第三方开发工具一起交付，在完全没有外网的环境中安装工具、编辑源码、运行测试、编译并直接启动 EXE。

本次目标按用户确认的 Windows 10 22H2 x64 准备；具体目标机版本暂未获取。安装、修改源码、测试和编译使用 Windows 自带的 64 位 PowerShell 5.1，不要求额外安装 Python、Git、PowerShell 7 或 Rustup。

## 首次使用

1. 将完整 ZIP 复制进内网，校验旁边的 `FULL-KIT-SHA256.txt`，完整解压到短路径，例如 `C:\ReviewerKit`。不要直接在 ZIP 内运行；开发时需要解压后的完整目录。
2. 双击 `1-install-environment.cmd`，接受 Windows 的管理员提升。脚本先校验工具文件，安装微软离线签名证书、缺少的 .NET / VC++ 运行库、WebView2，最后从本地布局安装 C++ Build Tools 和 Windows 10 SDK。没有下载命令，VS 使用 `--noWeb`。若返回 3010，先重启，再执行本步骤。
3. 双击 `2-build-test-run.cmd`，离线安装项目依赖、检查 TypeScript、执行 Rust 回归测试、编译并打开应用。不生成应用安装包。
4. 双击 `3-edit-source.cmd` 打开包内 VS Code；也可以让内网 LLM 修改源码。修改后先关闭正在运行的应用，再重新执行 `2-build-test-run.cmd`，避免 Windows 锁住旧 EXE。仅使用已缓存依赖的改动无需外网。
5. `4-run-prebuilt.cmd` 可运行包内现成的独立 EXE（环境安装后可用）；它不会自动包含后来修改的源码。修改后的应用位于 `src-tauri\target\x86_64-pc-windows-msvc\debug\config-reviewer.exe`。

使用命令行时，打开普通 64 位 Windows PowerShell：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-and-run.ps1 -Test -Run
# 优化构建，耗时更长，但同样无需应用安装包
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-and-run.ps1 -Release -Run
# 在当前 PowerShell 临时启用包内工具，便于手动 cargo / npm 命令
. .\scripts\enter-dev-environment.ps1
```

## 包内内容

完整 React / TypeScript / Rust 源码、锁文件、图标、测试和修改指南；Windows npm 缓存和 Cargo vendor；Node 22.23.3 与 npm；Rust 1.99.0 MSVC（cargo、rustc、标准库、rustfmt、clippy，独立工具，无需 rustup）；Visual Studio 2022 C++ Build Tools 的完整离线布局和 Windows 10 SDK 10.0.19041.0；微软签名证书；WebView2 x64 完整离线安装程序；.NET Framework 4.8 离线安装程序；VC++ x64 运行库；VS Code 便携编辑器；独立应用 EXE；安装/构建入口、逐文件 SHA256 和验证证据。

Node、Rust 不改全局 PATH，不安装到用户 `.cargo`，每次构建在进程内启用包内版本。C++ 工具和系统运行时需要管理员安装；企业策略禁止安装、执行或导入微软证书时需要管理员按企业流程放行。安装日志在 `.offline\install-logs`，微软详细安装日志在 `%TEMP%\dd_*.log`。

请保留整个交付目录，包括 `tools\vs-layout`；Visual Studio 的维修和组件修改需要原布局。编辑器禁用自动更新和遥测；未带 Rust / React 扩展，基础源码编辑和构建不依赖扩展。新的 npm / Cargo 包或改动锁文件到未缓存版本，需要在外网补充依赖包，内网 LLM 无法凭空提供这些二进制依赖。

## Windows 10 条件与验收边界

交付目标是标准 Windows 10 22H2 x64。脚本的最低版本预检为 1909（18363），这一预检不等于对所有旧版本的验收承诺。此包不是适用于任意旧版、32 位、ARM、S 模式或任意企业裁剪系统的通用 Windows 修复盘。不包含 Windows 系统、系统补丁、许可证、内网 LLM 或驱动。建议 16 GB 内存、SSD、至少 30 GB 空闲（含离线布局、已安装编译器和编译中间文件）。首次安装检查编译器所在盘至少 15 GB 空闲。

运行应用不需要外网；AI 审查需要内网 LLM 的 HTTP OpenAI 兼容接口和 API Key。应用默认内网直连，按原部署说明配置地址、模型和鉴权请求头。

当前微软支持政策随 Windows 生命周期变化，Windows 10 的工具运行可行性与微软支持状态应区分；不要将 Server 2022 的验证称作实际 Win10 现场验收。工具官方来源和布局验证记录在 `tools\TOOLS-INFO.json`，完整安装/编译/源码修改/应用测试结果随包交付。测试不代表已访问真实内网网关或已验证企业策略；这些仍需目标机器验收。

离线安装依据：[微软 VS 离线布局与 noWeb](https://learn.microsoft.com/en-us/visualstudio/install/create-an-offline-installation-of-visual-studio?view=vs-2022)、[离线签名证书](https://learn.microsoft.com/en-us/visualstudio/install/install-certificates-for-visual-studio-offline)、[Rust 独立工具包](https://forge.rust-lang.org/infra/other-installation-methods)、[WebView2 离线部署](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution)。
