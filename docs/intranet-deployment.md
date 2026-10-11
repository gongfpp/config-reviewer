# 内网安装与兼容性说明

适用环境：Windows 10/11 **x64**，完全断外网，能够访问内网 HTTP/HTTPS 文本模型网关。

## 交付与安装

交付物为 `ConfigReviewer-0.1.3-windows-x64-offline.zip`，包含 NSIS 安装 EXE、独立主程序 `config-reviewer.exe`、此说明、环境检查脚本、源码版本记录 `build-info.json` 和 SHA256 清单。安装 EXE 内置 Microsoft WebView2 **完整离线安装器**，不是仍需联网的 bootstrapper。主程序静态链接 MSVC CRT，不要求另装 Node、Rust、Office、SQLite 或 VC++ Redistributable。

1. 在允许联网的电脑下载完整 ZIP，按内网介质管理流程导入。
2. 使用 `Get-FileHash .\ConfigReviewer_0.1.3_x64-setup.exe -Algorithm SHA256` 与 `SHA256SUMS.txt` 核对文件；实际 EXE 名以压缩包内容为准。
3. 首次使用运行 `*-setup.exe`。已装好 WebView2 的电脑也可直接运行独立 `config-reviewer.exe`；它本身不包含 WebView2 安装器。默认安装到当前用户目录；WebView2 安装需服从单位权限策略。杀毒/AppLocker/WDAC 阻止 EXE 或 WebView2 时需由管理员放行。本项目没有配置机构代码签名。
4. 启动「配置评审台」，在「模型与规则」填写网关 Base URL（通常含 `/v1`）、模型标识和 API Key，保存后测试连接。
5. 默认鉴权为 `Authorization: Bearer <Key>`。要求 `api-key` 或 `X-API-Key` 的网关可填写「API Key 请求头」。默认直连，代理留空，不继承系统外网代理；需要代理时显式填写。
6. 启用「允许内网 HTTP」。默认使用流式响应；网关拒绝流式参数时关闭。测试连接使用相同响应模式。

API Key 保存到当前 Windows 用户凭据库；配置使用临时文件原子替换，保留上一份有效配置 `config.json.bak`；主配置损坏时可回退。配置、规则、模板和历史在当前用户 `%APPDATA%\com.gongfpp.configreviewer`。更换用户/电脑需要重新配置 Key。备份时先关闭应用，保留配置及 SQLite 数据库和可能存在的 `-wal`/`-shm` 文件；配置与报告不包含 Key，但评审报告及历史可能包含原文引用，应按原资料的保密要求保管。

## 网关契约

请求为 `POST <Base URL>/chat/completions`，正文含 `model`、`messages`、`stream`。支持已经包含 `/chat/completions` 的完整地址。

非流式：读取 `choices[0].message.content`。流式：SSE `data:`，读取 `choices[0].delta.content`，结束必须收到 `[DONE]` 或 `finish_reason=stop`。中文支持跨网络包拆分；响应错误、截断或 `finish_reason=length` 不会算作评审成功。网关忽略 stream 返回普通 JSON 时自动回退。

不跟随重定向，避免 Key 和正文被转送。API Key 请求头支持单一 Key，不支持网页登录、SSO/NTLM、多字段签名鉴权或非 OpenAI 协议。HTTPS 使用系统受信任证书；自签证书需按单位流程加入受信任根，应用没有跳过证书校验开关。

## 文件兼容性与边界

- ZIP：支持普通未加密 Stored/Deflate ZIP；不支持 RAR、7z、密码 ZIP，嵌套 ZIP 不递归解压。ZIP 中文文件名要求标准 UTF-8 或 ZIP Unicode 扩展；GBK 旧 ZIP 若名称乱码，应先由单位工具解压再扫描目录。同名包隔离，路径越界、大小写冲突及不兼容 Windows 的路径拒绝导入。
- JSON：合法 JSON 格式化，解析失败保留原文作为文本评审。
- Excel：识别 `.xlsx/.xls/.xlsm/.xlsb`；本轮样例实测 `.xlsx`。按 Sheet 提取缓存单元格值，不执行公式或宏，不渲染合并单元格、图表、图片。
- Word：`.docx` 和 Word 97–2003 二进制 `.doc`，读取正文段落与表格；不读取图片/OCR、页眉页脚、批注或嵌入对象，不执行宏。加密 DOC、Word 6/95、使用 DOC 后缀的 RTF/HTML 需先另存为 DOCX。原有规则不会自动修改；需要时添加 `doc` 后缀规则或文件名关键字规则。
- 文本：UTF-8（含 BOM）、GB18030、带 BOM 的 UTF-16 LE/BE。无 BOM UTF-16 不自动猜测。
- PDF、图片、JAR/class 等二进制不解析。规则忽略大小写，接受 `/` 和 `\` 路径分隔符；规则未命中文件默认跳过，预置命名规则需结合实际发布包调整。
- 默认 800 文件、单文件 1228 MB；这是可配置的输入限制，不是大文件性能保证。ZIP 单包解压总量限单文件上限的 4 倍且不超过 2 GB；大文件仍需先以真实样例确认内存和耗时。
- 分块大小按字符而非 token；需结合模型上下文与输出上限设置。任何块失败会使该文件评审失败，可重试，不会合并成成功报告。

## 本机预检

Windows PowerShell 5.1 可运行 `powershell -ExecutionPolicy Bypass -File .\check-intranet.ps1`（执行策略仍须符合单位要求）。脚本仅检查系统、WebView2 和用户指定的网关，不访问公共网站。

检查网关端口：`powershell -File .\check-intranet.ps1 -BaseUrl http://gateway.intra:8080/v1`。

可选测试模型：增加 `-Model your-model -TestModel`；会安全提示输入 API Key，仅发送固定测试文本。可增加 `-ApiKeyHeader api-key`。脚本不保存 Key，也不输出完整响应。

## 验收与证据边界

必须在目标内网 Windows 10 x64 上验收：断网安装（分别覆盖无/有 WebView2）、普通用户启动、真实网关连接及中文流式评审、真实混合发布包导入、失败重试/取消、四种报告导出、重启后历史与 Key 保留。还需确认单位签名/执行策略，以及实际 Windows build、CPU 架构、模型上下文和并发限额。

本机 macOS 回归测试与 Windows CI 编译/凭据测试/安装器冒烟分别提供证据，不等同于目标电脑或真实内网网关验收。执行结果以本轮 [兼容性检查记录](compatibility-audit.md) 为准。

参考：[Tauri 离线 WebView2 安装](https://v2.tauri.app/distribute/windows-installer/#offline-installer)、[Microsoft WebView2 分发](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution)。
