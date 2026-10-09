use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::{RuleSet, SystemType, Template};
use crate::rules;

/// 应用配置（不含 API Key；Key 由系统凭据库单独保存）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub base_url: String,
    pub model: String,
    pub allow_http: bool,
    pub timeout_secs: u64,
    #[serde(default)]
    pub api_key_header: String,
    #[serde(default)]
    pub proxy_url: String,
    #[serde(default = "default_true")]
    pub stream_enabled: bool,

    pub concurrency: usize,
    pub max_file_mb: u64,
    pub max_files: usize,
    pub chunk_threshold: usize,
    pub chunk_size: usize,
    #[serde(default = "default_retry")]
    pub retry_count: usize,
    #[serde(default = "default_granularity")]
    pub granularity: String,
    #[serde(default = "default_output_format")]
    pub output_format: String,
    #[serde(default)]
    pub history_retention_days: u32,
    pub systems: Vec<SystemType>,
    pub templates: Vec<Template>,
    pub rule_sets: Vec<RuleSet>,
}

fn default_true() -> bool {
    true
}

fn default_retry() -> usize {
    1
}
fn default_granularity() -> String {
    "per_file".to_string()
}
fn default_output_format() -> String {
    "standard".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            model: String::new(),
            allow_http: true,
            timeout_secs: 180,
            api_key_header: String::new(),
            proxy_url: String::new(),
            stream_enabled: true,
            concurrency: 3,
            max_file_mb: 1228,
            max_files: 800,
            chunk_threshold: 12000,
            chunk_size: 8000,
            retry_count: 1,
            granularity: "per_file".into(),
            output_format: "standard".into(),
            history_retention_days: 0,
            systems: rules::default_systems(),
            templates: rules::default_templates(),
            rule_sets: rules::default_rule_sets(),
        }
    }
}

impl AppConfig {
    /// 补齐内置系统、模板与规则集：持久化的同 id 项覆盖内置项，新增项自动合并。
    pub fn merged(mut self) -> Self {
        self.systems = merge_systems(rules::default_systems(), self.systems);
        self.templates = merge_templates(rules::default_templates(), self.templates);
        self.rule_sets = merge_rule_sets(rules::default_rule_sets(), self.rule_sets);
        self
    }
}

fn merge_systems(defaults: Vec<SystemType>, stored: Vec<SystemType>) -> Vec<SystemType> {
    let mut result = defaults;
    for saved in stored {
        if let Some(index) = result.iter().position(|s| s.id == saved.id) {
            result[index] = saved;
        } else {
            result.push(saved);
        }
    }
    result
}

fn merge_templates(defaults: Vec<Template>, stored: Vec<Template>) -> Vec<Template> {
    let mut result = Vec::new();
    for mut def in defaults {
        if let Some(saved) = stored.iter().find(|t| t.id == def.id) {
            def.overridden = saved.content != def.content;
            def.name = saved.name.clone();
            def.content = saved.content.clone();
        }
        result.push(def);
    }
    for saved in stored {
        if !result.iter().any(|t| t.id == saved.id) {
            result.push(saved);
        }
    }
    result
}

fn merge_rule_sets(defaults: Vec<RuleSet>, stored: Vec<RuleSet>) -> Vec<RuleSet> {
    if stored.is_empty() {
        return defaults;
    }
    let mut result = stored;
    for def in defaults {
        if !result.iter().any(|r| r.id == def.id) {
            result.push(def);
        }
    }
    result
}

pub fn load(path: &Path) -> AppConfig {
    for candidate in [path.to_path_buf(), path.with_extension("json.bak")] {
        if let Ok(raw) = std::fs::read_to_string(candidate) {
            if let Ok(config) = serde_json::from_str::<AppConfig>(&raw) {
                return config.merged();
            }
        }
    }
    AppConfig::default()
}

pub fn save(path: &Path, config: &AppConfig) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
        file.write_all(&raw)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        if let Ok(previous) = std::fs::read(path) {
            if serde_json::from_slice::<AppConfig>(&previous).is_ok() {
                std::fs::write(path.with_extension("json.bak"), previous)
                    .map_err(|e| e.to_string())?;
            }
        }
        std::fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

/// 隔离桌面 QA 数据，仅 debug 构建接受，发布版没有此入口。
#[cfg(debug_assertions)]
pub fn qa_data_dir() -> Option<std::path::PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|s| s == "--qa-data-dir")?;
    let path = std::path::PathBuf::from(args.get(i + 1)?);
    if path.is_absolute() && path.starts_with(std::env::temp_dir()) {
        Some(path)
    } else {
        None
    }
}
