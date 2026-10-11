# 内网源码修改与直接运行

目标是在 Windows 10/11 x64 内网电脑上用 LLM 修改源码，然后直接编译运行 EXE。此流程不生成 NSIS/MSI。没有开发环境的电脑请使用[完整离线开发包](offline-development-kit.md)：包内提供工具链和 WebView2 离线安装程序。

## 交付内容与环境条件

源码包包含完整 React/TypeScript、Rust、图标、配置、锁文件、59 项回归测试和文档；不含个人 API Key、评审历史、GitHub 凭据或 Git 历史。

离线依赖包解压到源码根目录：提供 Windows npm 缓存及 Cargo vendored sources，并补充 `.cargo/config.toml` 的 source replacement。它解决第三方依赖下载，不是 Windows 开发工具链安装器。

旧版源码+依赖包需要电脑已有以下环境；新版完整开发包已携带这些工具，由 `1-install-environment.cmd` 离线安装或启用：

- Node.js 22 LTS 与 npm（验证环境 Node 22.23.3）；
- Rust MSVC 工具链（验证环境 Rust 1.99.0 / x86_64-pc-windows-msvc）；
- Visual Studio 2022 / Build Tools 的 C++ x64 工具与 Windows SDK；
- WebView2 Runtime（运行应用所需，原离线安装 EXE 内已有运行库安装器）。

如果这些工具没有安装，内网 LLM 本身不能替代它们。需在允许联网的准备机上准备对应官方离线开发环境，再导入内网。Mac 的 node_modules 和 Rust target 目录不能拿来替代 Windows 工具链。

## 修改后构建与运行

完整离线源码包已经合并源码与依赖，直接解压即可。若使用轻量源码包，则再把离线依赖包解压到同一根目录。用 PowerShell 进入根目录：

```powershell
# 开发用 EXE，较快；运行现有回归测试，然后启动应用
powershell -File .\scripts\build-and-run.ps1 -Test -Run

# 优化后的 EXE，构建较慢；仍不生成安装包
powershell -File .\scripts\build-and-run.ps1 -Release -Run
```

脚本默认断网构建。没有离线依赖且允许使用内网 npm/Cargo 镜像时，可按单位的镜像配置运行 `-Online` 模式。这个参数允许依赖解析联网，不改变应用的 LLM 网关配置。

直接运行输出：`src-tauri\target\x86_64-pc-windows-msvc\debug\config-reviewer.exe`（或 `release`）。EXE 嵌入前端资源，启动时不需要 npm/Vite 服务。复制新 EXE 到自己的运行目录即可测试；保留已有应用标识以继续使用原配置和历史。

只想实时开发前端时，在已有 node_modules 和工具链的条件下可用 `npm run tauri dev`；它会启动本机 Vite 服务，不是单独的可分发 EXE。

## 给内网 LLM 的修改指南

先读 README、docs/requirements.md、docs/compatibility-audit.md，再按需要定位：

| 修改目标 | 主要入口 |
| --- | --- |
| 界面、交互、样式 | src/App.tsx、src/App.css |
| 前后端类型与调用 | src/types.ts、src/api.ts、src-tauri/src/model.rs、commands.rs |
| 文件扫描与解析 | scan.rs、parse.rs、chunk.rs |
| 网关、鉴权、流式与取消 | gateway.rs |
| 规则、模板、配置 | rules.rs、config.rs |
| 结果识别、历史、导出 | results.rs、store.rs、export.rs |
| 凭据 | secrets.rs |
| 回归测试 | src-tauri/src/tests.rs、chunk.rs 中的测试 |

Rust 文件位于 src-tauri/src。修改前后保持 TypeScript 与 Rust 字段一致，保留旧配置 serde 默认值，避免破坏内网已有数据。

不把 API Key 写入源码、配置、日志或报告；只使用模拟 Key 和样例数据做测试。保留发送前数据去向确认、HTTPS 证书校验、ZIP 路径/容量保护，以及任务互斥和按 task_id 过滤进度。

新增第三方依赖时同步更新锁文件，并在准备机重做离线依赖包。仅改现有源码、Prompt、规则或样式一般不需要新的依赖。

## 业务配置无需编译

「模型与运行」「评审模块」里的网关地址、模型、Key 请求头、并发、分块参数、文件匹配条件和评审规范 都可直接保存。先用内网业务样例调整这些配置，只有软件行为不满足需求时才修改代码。

## 证据与边界

应用原有 Windows 构建及关键用户路径已经验证。本交付额外验证：[Windows 离线源码验证 37935184347](https://github.com/gongfpp/config-reviewer-builds/actions/runs/37935184347) 全部通过；验证源码 cde8c5fbcd55db51dbd55afcf7cc5729c90257a6。

在新位置删除 node_modules，并使用空 CARGO_HOME、vendored crates、npm 离线缓存和 CARGO_NET_OFFLINE=true 后：34 项 Rust 回归通过，`build-and-run.ps1 -Test` 成功输出 standalone debug EXE；直接启动该 EXE，在真实 WebView2 中完成配置/凭据、扫描、评审、四格式导出、重启、任务互斥和取消重试检查。没有生成或安装 NSIS/MSI。保存对话框的路径选择由测试替代，其余后端操作真实执行。

精确依赖版本与验证结果记录在 SOURCE-INFO.json / DEPENDENCY-INFO.json；UI 测试复用了旧报告标签“installed EXE”，本流程实际启动的是刚编译的独立 EXE。仍需在目标 Win10 的真实网关与业务样例上确认效果。
