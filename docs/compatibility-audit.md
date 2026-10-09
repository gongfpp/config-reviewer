# 兼容性检查与验证记录（2026-10-09）

目标：Windows 10 x64、完全断外网、HTTP + API Key 的内网文本模型网关。代码版本 0.1.2。

## 已完成整改

- 内置 WebView2 完整离线安装器；Windows MSVC CRT 静态链接；交付 ZIP 带说明、预检脚本、SHA256。
- 修复 SSE 中文拆字、普通 JSON 回退、错误事件/截断误判、HTTP 开关无效和取消等待。默认直连内网，增加自定义 Key 请求头、代理和非流式模式；连接测试覆盖实际响应模式；禁止重定向。
- 长文本/单键 JSON 分块按字符上限切分。分块失败不再交给模型合并为成功；合并评审包含各文件模板，任一文件解析失败即失败。
- 修复标准【依据】【风险】【整改项】解析和 JSON 结果校验；历史保存/重试事务化，历史坏行显式报错；已修改系统名称可持久化；配置原子替换并保留有效备份；目录读取失败和上传文件失效不再静默遗漏。
- Word 使用 XML 解析，避免元数据混入和实体二次解码；Excel 保留真实起始行；增加 UTF-16 BOM；同名 ZIP 隔离，限制路径、重复项与解压大小。
- JSON/Markdown/HTML 改为与 Excel 一样的原生保存对话框和后端文件写入。
- 升级 source-map-js 修复 npm audit 报告的高危依赖；npm audit 0 项漏洞。

## 已有证据

- macOS：32 项 Rust 回归全部通过；前端类型检查/构建通过；Clippy 全 target、warnings-as-errors 通过；macOS debug 应用打包成功。
- 实际桌面 UI（隔离数据目录与模拟网关）：设置和凭据保存、流式连通性、中文目录扫描、JSON/DOCX/UTF-16 文本三系统识别、3 文件评审、3 份完整依据/风险/整改/引用、本机历史读取均通过。
- 导出原生 Save 对话框可打开；macOS 后台 GUI 控制无法完成保存按钮操作，因此此处不宣称四格式 GUI 保存已验证。Rust 集成测试完成真实 Excel 文件写入并重新解析。
- 私有仓库 Actions 37913445367 在任何步骤运行前被 GitHub 拒绝，注释为付款失败或 spending limit；不是代码测试失败。按用户要求转用 public 构建仓库，明文源码仍不公开，以一次性 AES-GCM 加密包进入 runner。
- Windows 构建、PE DLL 依赖、凭据、安装器、已安装 EXE 的 WebView2 UI 检查结果：正在执行，完成后更新此处。

## 当前兼容范围与待验收

详情见 intranet-deployment.md。标准 OpenAI `/chat/completions` + Bearer 或自定义单一 Key 请求头可接；非标准协议、SSO/NTLM、网页登录鉴权不能直接接。HTTP 不要求证书；HTTPS 要系统信任根。

只支持 Windows x64 安装包；32 位和 Windows ARM 未构建。Windows Server CI 结果不能替代真实内网 Windows 10 验收；尚无具体 Win10 build、单位 AppLocker/WDAC/签名要求、真实模型名/上下文/限流、业务规则清单和真实发布包。

Excel `.xlsx` 有样例，`.xls/.xlsm/.xlsb` 是解析库支持范围，尚未用业务样例验证；不执行宏或公式。PDF、旧 `.doc`、图片/OCR、密码 ZIP、RAR/7z 不支持。分块是字符而非 token，容量限制不代表大文件性能已经验收；大量分块结论的最终合并仍可能需要按真实模型上下文调整。

ZIP 解压内容为便于失败重试保留到下次扫描或下次启动清理；应用崩溃后下一次启动清理，不承诺所有任务结束即删除。历史与报告有原文引用，需要按源资料管理。
