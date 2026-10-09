use serde::{Deserialize, Serialize};

/// 一条文件名规则：glob 模式 + 归属系统 + 评审模板。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub pattern: String,
    pub system: String,
    pub template_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// 系统类型：数据驱动、可扩展。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemType {
    pub id: String,
    pub name: String,
    pub color: String,
    pub default_template_id: String,
}

/// 评审模板。`builtin` 的内置模板可被「另存覆盖」。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub content: String,
    pub builtin: bool,
    #[serde(default)]
    pub overridden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSet {
    pub id: String,
    pub name: String,
    pub builtin: bool,
    pub rules: Vec<Rule>,
}

/// 扫描得到的单个文件条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanEntry {
    /// 相对来源的路径。
    pub path: String,
    /// 工作区中的绝对路径。
    pub abs_path: String,
    pub name: String,
    pub ext: String,
    /// json / excel / word / text / binary / other
    pub kind: String,
    pub size: u64,
    pub system: Option<String>,
    pub rule_id: Option<String>,
    pub template_id: Option<String>,
    pub matched: bool,
    #[serde(default)]
    pub selected: bool,
    pub chunks: usize,
    pub parse_status: String,
    pub parse_error: Option<String>,
    pub preview: String,
    pub text_len: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub task_id: String,
    pub source_type: String,
    pub source_ref: String,
    pub total_files: u64,
    pub matched_files: u64,
    pub total_bytes: u64,
    pub entries: Vec<ScanEntry>,
    pub workspace: String,
    pub rule_set: RuleSet,
    pub templates: Vec<Template>,
    pub systems: Vec<SystemType>,
}

/// 结构化引用：文件 + 定位 + 片段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub file: String,
    pub locator: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileResult {
    pub file: String,
    pub system: Option<String>,
    pub template_id: String,
    pub verdict: String,
    #[serde(default)]
    pub basis: Vec<String>,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub remediations: Vec<String>,
    #[serde(default)]
    pub citations: Vec<Citation>,
    pub chunks: usize,
    pub duration_ms: u64,
    pub raw: String,
    pub error: Option<String>,
}

/// 网关配置（API Key 仅内存/DPAPI，不进入历史与导出）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_true")]
    pub allow_http: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub api_key_header: String,
    #[serde(default)]
    pub proxy_url: String,
    #[serde(default = "default_true")]
    pub stream_enabled: bool,
}

fn default_timeout() -> u64 {
    180
}

/// 分析请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewRequest {
    pub task_id: String,
    #[serde(default)]
    pub workspace: String,
    pub files: Vec<ReviewFileInput>,
    pub cfg: GatewayConfig,
    #[serde(default)]
    pub concurrency: usize,
    #[serde(default = "default_threshold")]
    pub chunk_threshold: usize,
    #[serde(default = "default_chunk")]
    pub chunk_size: usize,
    /// per_file / per_file_system / single
    #[serde(default = "default_granularity")]
    pub granularity: String,
    /// standard / checklist / brief / json
    #[serde(default = "default_output_format")]
    pub output_format: String,
    /// 失败自动重试次数（不含首次）。
    #[serde(default = "default_retry")]
    pub retry_count: usize,
}

fn default_threshold() -> usize {
    12000
}
fn default_chunk() -> usize {
    8000
}
fn default_granularity() -> String {
    "per_file".to_string()
}
fn default_output_format() -> String {
    "standard".to_string()
}
fn default_retry() -> usize {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewFileInput {
    pub path: String,
    pub abs_path: String,
    pub name: String,
    pub kind: String,
    pub system: Option<String>,
    pub template_id: String,
    pub template_content: String,
}

/// 分析进度事件负载。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewProgress {
    pub task_id: String,
    pub index: usize,
    pub total: usize,
    pub file: String,
    pub phase: String,
    pub delta: Option<String>,
    pub result: Option<FileResult>,
}
