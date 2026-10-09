# 配置评审台

内网 Windows 桌面端的**配置 / 文档评审工具**。上传混合发布包（ZIP）或扫描本地目录，自动识别
Apollo / XXL-JOB / 星云 等来源，调用内网文本 LLM 网关做上线前评审，按系统分组给出结论并导出。

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

Windows 离线安装包在 public 构建仓库 [`config-reviewer-builds`](https://github.com/gongfpp/config-reviewer-builds) 由 GitHub Actions 构建，内置完整 WebView2 离线安装器，并静态链接 MSVC CRT。交付物是 `ConfigReviewer-0.1.2-windows-x64-offline.zip`，含安装 EXE、安装说明、环境检查脚本及 SHA256 清单。运行端无需 Node、Rust、Office 或外网连接。私有源码仅以一次性 AES-GCM 加密包进入 runner，不公开明文。

源码提交并通过本地检查后，可运行 `python3 scripts/dispatch-public-build.py`（需要已登录的 `gh` 与 Python `cryptography`）派发 public Windows 构建。脚本拒绝脏工作区和重叠构建，密钥仅通过 stdin 写入仓库 Secret；交付完成后删除临时输入和 Secret。Windows UI 验收依赖由 `scripts/package.json` 单独固定，不进入运行端。

安装、网关配置与文件支持范围见 [`docs/intranet-deployment.md`](docs/intranet-deployment.md)；本轮验证与待验收边界见 [`docs/compatibility-audit.md`](docs/compatibility-audit.md)。

## 功能状态

- [x] 扫描来源：ZIP / 多文件上传、本地目录递归扫描、粘贴文本
- [x] 通用解析：JSON、Excel（全 Sheet）、Word（段落 + 表格）、文本；UTF-8 / GB18030 回退
- [x] 规则路由：glob → 归属系统 + 评审模板，预置可改、可增删、可持久化
- [x] 大文件分块（JSON 顶层键优先）与 LLM 合并为文件结论
- [x] 统一分析单元并发：文件级与块级共享信号量，同文件分块并行、按序合并
- [x] 内网 OpenAI 兼容网关（流式 + 一次性），并发可配、可取消、失败自动重试（默认 1 次）
- [x] 分析粒度（逐文件 / 逐文件 + 按系统汇总 / 合并一次性）与输出格式（标准 / 检查表 / 简洁摘要 / JSON）
- [x] 发送前「确认数据去向」；失败项可手动重试
- [x] 按系统分组结果、结构化引用、每系统 LLM 汇总
- [x] 导出 Excel（`系统|文件路径|文件类型|命中规则|结论|引用|时间`）/ JSON / Markdown / HTML
- [x] 本机 SQLite 历史（任务 + 文件级结果）；搜索、导出、目录来源重评、保留期清理
- [x] 规则集 / 系统类型 / 模板的增删改，内置模板可恢复
- [x] 配置持久化、API Key 通过系统凭据库加密（Windows 凭据管理器 / macOS Keychain）
- [x] Windows 安装包 CI

## 尚未确定的事实输入

- 预置文件名规则清单（需业务样例）
- 内网网关契约细节（模型名、上下文上限、限流/错误码）

