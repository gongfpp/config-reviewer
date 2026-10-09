# 兼容性检查与验证记录（2026-10-09）

目标：Windows 10 x64、完全断外网、HTTP + API Key 的内网文本模型网关。代码版本 0.1.2。

## 已完成整改

- 内置 WebView2 完整离线安装器；Windows MSVC CRT 静态链接；交付 ZIP 带说明、预检脚本、SHA256。
- 修复 SSE 中文拆字、普通 JSON 回退、错误事件/截断误判、HTTP 开关无效和取消等待。默认直连内网，增加自定义 Key 请求头、代理和非流式模式；连接测试覆盖实际响应模式；禁止重定向。
- 长文本/单键 JSON 分块按字符上限切分。分块失败不再交给模型合并为成功；合并评审包含各文件模板，任一文件解析失败即失败。
- 修复标准【依据】【风险】【整改项】（含同一行正文）解析与必填校验，以及 JSON 结果校验；历史保存/重试事务化，历史坏行显式报错；已修改系统名称可持久化；配置原子替换并保留有效备份；目录读取失败和上传文件失效不再静默遗漏。
- 文件规则按 Windows 习惯忽略大小写，兼容反斜杠路径；Windows UI 样例使用大写 `.JSON` 验证识别。
- Word 使用 XML 解析，避免元数据混入和实体二次解码；Excel 保留真实起始行；增加 UTF-16 BOM；同名 ZIP 隔离，限制路径、重复项与解压大小。
- 运行中拒绝新评审和新扫描，防止正在评审的临时文件被下一任务清理；前端进度按 task_id 过滤，运行页直接提供取消按钮，汇总按分析轮次隔离。
- JSON/Markdown/HTML 改为与 Excel 一样的原生保存对话框和后端文件写入。
- 升级 source-map-js 修复 npm audit 报告的高危依赖；npm audit 0 项漏洞。

## 已有证据

- 依赖：npm audit 0 项；cargo audit 漏洞条目 0 项。另有 proc-macro-error/unic 系列维护告警、Linux GTK 的 glib 0.18.5 unsound 告警，以及 yoke-derive 0.8.3 已撤回告警。锁定版本仍可构建；维护/平台告警与当前 Windows 运行漏洞分开记录，后续跟随 Tauri 上游处理。原始报告保存在本机 artifacts/local-validation/cargo-audit.json。
- macOS：34 项 Rust 回归全部通过；前端类型检查/构建通过；Clippy 全 target、warnings-as-errors 通过；macOS debug 应用打包成功。
- 实际桌面 UI（隔离数据目录与模拟网关）：设置和凭据保存、流式连通性、中文目录扫描、JSON/DOCX/UTF-16 文本三系统识别、3 文件评审、3 份完整依据/风险/整改/引用、本机历史读取均通过。
- 导出原生 Save 对话框可打开；macOS 后台 GUI 控制无法完成保存按钮操作，因此此处不宣称四格式 GUI 保存已验证。Rust 集成测试完成真实 Excel 文件写入并重新解析。
- 私有仓库 Actions 37913445367 在任何步骤运行前被 GitHub 拒绝，注释为付款失败或 spending limit；不是代码测试失败。按用户要求转用 public 构建仓库，明文源码仍不公开，以一次性 AES-GCM 加密包进入 runner。
- Windows 最终构建与验收：[public Actions 37928119990](https://github.com/gongfpp/config-reviewer-builds/actions/runs/37928119990) 全部通过。构建输入为 `39cf6fcf1f8033e6762c3c501a35128883b668eb`，版本 0.1.2。
- Windows Server 2022 x64 runner：34 项回归通过（含 Windows 凭据管理器）；主程序 PE 依赖检查没有动态 VCRUNTIME/MSVCP；NSIS 静默安装成功。
- 真实已安装 EXE / WebView2：配置与 Key 保存、流式连通性、中文目录和大写 `.JSON` 识别、预览、评审详情、运行中第二任务/扫描拒绝、四格式实际文件写入、重启后历史与 Key 保留、GUI 取消后重试均通过。仅保存对话框的路径选择被测试替代，其余 IPC/文件写入/模型请求/数据库/凭据均由真实应用执行。
- CI 原始 ZIP：`artifacts/windows-0.1.2/ConfigReviewer-0.1.2-windows-x64-offline.zip`，221398762 bytes（约 211 MiB），ZIP CRC 和全部内嵌 SHA256 通过。ZIP SHA256：`bf38caf4a64a876a93c885dd6b9274a00eae29fbbba317fd841504c2c5477e8e`。内含安装 EXE、说明、预检脚本、build-info.json、SHA256SUMS.txt；安装器 Authenticode 表为空，尚未代码签名。
- Windows UI 原始证据：`artifacts/windows-ui-final/result.json`、`results.png`；最终 CI 日志：`artifacts/windows-final-ci.log`。
- 私有源码通过一次性 AES-GCM 包构建，public 仓库不保存明文源码。交付后清理临时加密输入和对应 Secret。

## 当前兼容范围与待验收

详情见 intranet-deployment.md。标准 OpenAI `/chat/completions` + Bearer 或自定义单一 Key 请求头可接；非标准协议、SSO/NTLM、网页登录鉴权不能直接接。HTTP 不要求证书；HTTPS 要系统信任根。

只支持 Windows x64 安装包；32 位和 Windows ARM 未构建。Windows Server CI 结果不能替代真实内网 Windows 10 验收；本轮安装器检查使用 runner 已有的 WebView2，缺少 WebView2 的 Win10 断网安装场景与原生保存对话框的人工选择仍待实机验证；尚无具体 Win10 build、单位 AppLocker/WDAC/签名要求、真实模型名/上下文/限流、业务规则清单和真实发布包。

Excel `.xlsx` 有样例，`.xls/.xlsm/.xlsb` 是解析库支持范围，尚未用业务样例验证；不执行宏或公式。PDF、旧 `.doc`、图片/OCR、密码 ZIP、RAR/7z 不支持。分块是字符而非 token，容量限制不代表大文件性能已经验收；大量分块结论的最终合并仍可能需要按真实模型上下文调整。

ZIP 解压内容为便于失败重试保留到下次扫描或下次启动清理；应用崩溃后下一次启动清理，不承诺所有任务结束即删除。历史与报告有原文引用，需要按源资料管理。

最终交付 ZIP 在 CI 原始安装器之外补入本报告和 Windows UI 证据，安装 EXE 字节完全一致。最终大小 221445215 bytes，SHA256 `764710cc24a0ee668a4b4e8c4f5b53be4bd6c28e35736a3d83642ef0338f2d7e`；再次通过 CRC 和全部内部 SHA256。校验文件为 artifacts/windows-0.1.2/ZIP-SHA256.txt。
