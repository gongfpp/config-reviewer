# 配置评审台

内网 Windows 桌面端的**配置 / 文档评审工具**。上传混合发布包（ZIP）或扫描本地目录，自动识别
Apollo / XXL-JOB / 星云 等来源，调用内网文本 LLM 网关做上线前评审，按模块分组给出结论并导出。

需求与技术决策见 [`docs/requirements.md`](docs/requirements.md)；界面设计稿见
[`prototypes/reviewer-desktop/index.html`](prototypes/reviewer-desktop/index.html)。

## 技术栈

- 桌面壳：Tauri v2（Rust）
- 前端：React + TypeScript + Vite
- 后端能力：目录/ZIP 扫描、Excel/Word/JSON 解析、文件名规则路由、分块、OpenAI 兼容流式网关、SQLite 历史

## 开发

```bash
npm install
npm run tauri dev      # 桌面应用
npm run typecheck      # 前端类型检查
npm run build          # 前端构建（tsc + vite）
npm run check:rust     # Rust 检查
cargo test --locked --manifest-path src-tauri/Cargo.toml # 回归测试
```

Windows 离线安装包在 public 构建仓库 [`config-reviewer-builds`](https://github.com/gongfpp/config-reviewer-builds) 由 GitHub Actions 构建，内置完整 WebView2 离线安装器，并静态链接 MSVC CRT。交付物是 `ConfigReviewer-0.1.4-windows-x64-offline.zip`，含安装 EXE、独立主程序 EXE、安装说明、环境检查脚本及 SHA256 清单。运行端无需 Node、Rust、Office 或外网连接。私有源码仅以一次性 AES-GCM 加密包进入 runner，不公开明文。

源码提交并通过本地检查后，可运行 `python3 scripts/dispatch-public-build.py`（需要已登录的 `gh` 与 Python `cryptography`）派发 public Windows 构建。脚本拒绝脏工作区和重叠构建，密钥仅通过 stdin 写入仓库 Secret；交付完成后删除临时输入和 Secret。Windows UI 验收依赖由 `scripts/package.json` 单独固定，不进入运行端。

安装、网关配置与文件支持范围见 [`docs/intranet-deployment.md`](docs/intranet-deployment.md)；本轮验证与待验收边界见 [`docs/compatibility-audit.md`](docs/compatibility-audit.md)。

## 功能状态

- [x] 扫描来源：ZIP / 多文件上传、本地目录递归扫描、粘贴文本
- [x] 通用解析：JSON、Excel（全 Sheet）、Word（段落 + 表格）、文本；UTF-8 / GB18030 回退
- [x] 后台扫描与解析：发现 / 解压 / 解析进度、实际文件计数、取消；大清单分页显示
- [x] 规则路由：文件名 / 文件名关键字 / 后缀 / glob → 评审模块 → 模块评审规范，支持显式新增、启停、排序和持久化
- [x] 大文件分块（JSON 顶层键优先）与 LLM 合并为文件结论
- [x] 统一分析单元并发：文件级与块级共享信号量，同文件分块并行、按序合并
- [x] 内网 OpenAI 兼容网关（流式 + 一次性），并发可配、可取消、失败自动重试（默认 1 次）
- [x] 分析粒度（逐文件 / 逐文件 + 按模块汇总 / 同模块合并评审）与输出格式（标准 / 检查表 / 简洁摘要 / JSON）
- [x] 发送前「确认数据去向」；失败项可手动重试
- [x] 按模块分组结果、结构化引用、每模块 LLM 汇总
- [x] 导出 Excel（`评审模块|文件路径|文件类型|命中规则|结论|引用|时间`）/ JSON / Markdown / HTML
- [x] 本机 SQLite 历史（任务 + 文件级结果）；搜索、导出、目录来源重评、保留期清理
- [x] 评审模块集中管理：名称、颜色、文件匹配、输入说明、评审要求、输出格式与补充要求；内置规范可恢复
- [x] 配置持久化、API Key 通过系统凭据库加密（Windows 凭据管理器 / macOS Keychain）
- [x] Windows 安装包 CI

## 尚未确定的事实输入

- 预置文件名规则清单（需业务样例）
- 内网网关契约细节（模型名、上下文上限、限流/错误码）


## 内网二次修改

源码修改后可直接构建和运行 EXE，无需重新生成安装包。入口为 `scripts/build-and-run.ps1 -Test -Run`，源码目录、离线依赖与工具链条件见 [内网开发说明](docs/onsite-development.md)。规则、Prompt 和模型配置可直接在界面保存，无需编译。

目标 Windows 10 电脑没有开发环境和外网时，使用[完整离线开发包](docs/offline-development-kit.md)。它同时提供源码、npm/Cargo 依赖、Node、Rust、C++ Build Tools/Windows 10 SDK、WebView2/.NET/VC++ 离线安装程序和便携编辑器。入口依次为 `1-install-environment.cmd`、`2-build-test-run.cmd`、`3-edit-source.cmd`；包内工具在构建进程内启用。


## 配置评审模块与解析链路

进入「评审模块」，选择现有模块或点击「新建模块」。在同一页配置名称、文件匹配条件及评审规范，点击「保存模块配置」。每个模块拥有独立规范；规则只决定归属模块，扫描页不再单独选择模板，运行页也不再覆盖模块输出格式。

例如，在 Apollo 模块中添加“文件名包含：阿波罗”，即可让 `阿波罗.txt`、`阿波罗1.doc`、`阿波罗.docx` 使用同一规范。条件支持完整文件名、关键字、后缀及高级 glob；不区分大小写，关键字不搜索正文。多条件命中时采用第一条启用条件，可展开“匹配优先级”调整。PDF 可以命名匹配，但目前不能解析。

模块规范由输入说明、评审要求、输出格式及输出补充要求组成。程序解析文件正文、拼接 Prompt，并按模块输出格式解析模型结果。格式支持标准结论、检查表、简洁摘要和 JSON；补充要求遵循所选格式结构。多个模块逐文件分别执行各自规范；同模块多文件可选择合并评审，不同模块禁止合并。

升级自动迁移旧配置：同一系统曾选择不同模板的规则拆成独立模块，共享模板复制为各模块独立规范，未绑定模板保留为无文件匹配条件的模块。旧版多个规则集作为“文件匹配方案”保留；规范跨方案共用。迁移保留旧配置备份，新版删除模块后重启不会自动恢复。修改模块会使当前扫描清单失效，需重新扫描；历史结果保留，目录重评采用当前模块配置。

扫描阶段只在本机读取内容：Excel 通过 Rust `calamine` 提取各 Sheet 的单元格及行号，Word 通过 ZIP/XML 读取 DOCX、通过内置 Rust 解析器读取 Word 97–2003 二进制 DOC 正文和表格，文本按 UTF-8 / GB18030 等编码读取。点击开始评审并确认数据去向后，才将文本和模板要求发送给配置的模型；大文件分块评审，再合并结论。DOC 不需要安装 Office，也不执行宏；加密文档、Word 6/95、伪装成 DOC 的 RTF/HTML、PDF/图片 OCR 不在当前解析范围。已有自定义规则不会自动改写，可添加 `doc` 后缀规则或使用文件名关键字规则。

扫描使用独立后台工作线程。发现文件与 ZIP 解压时显示动态进度及已发现数量，解析时显示已处理 / 总数、命中数和百分比，切换页面仍保留进度与取消入口。取消在文件边界及 ZIP 读取分段间生效；正在执行的单个 Office 文档解析需返回后才停止。文件数上限按发现的全部候选文件（含 ZIP 内文件）执行，不受「仅保留命中」影响。模型等待、连接测试、模块汇总与导出显示运行反馈，不估算虚假的模型完成百分比。
