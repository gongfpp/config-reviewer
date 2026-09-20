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
npm run build          # 仅前端构建（tsc + vite）
cd src-tauri && cargo check
```

## 当前进度（首版里程碑）

- [x] 项目脚手架与 Private Banking 主题
- [x] Rust：扫描 / ZIP 解压 / 解析（JSON、Excel、Word、文本，UTF-8/GB18030）/ 规则路由 / 分块
- [x] Rust：OpenAI 兼容网关（流式 + 一次性）与结论/引用解析
- [x] Rust：SQLite 历史（任务 + 文件级结果）
- [x] 前端：四步向导、按系统分组结果、历史、模板、模型设置
- [ ] 模板/规则集持久化（当前为会话内内存）
- [ ] Windows DPAPI 加密 API Key（当前为本地存储）
- [ ] 导出 Excel / HTML、按系统 LLM 汇总、任务取消

## 尚未确定的事实输入

- 预置文件名规则清单（需业务样例）
- 内网网关契约细节（模型名、上下文上限、限流/错误码）
