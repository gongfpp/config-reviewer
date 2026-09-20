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
    pub concurrency: usize,
    pub max_file_mb: u64,
    pub max_files: usize,
    pub chunk_threshold: usize,
    pub chunk_size: usize,
    pub history_retention_days: u32,
    pub systems: Vec<SystemType>,
    pub templates: Vec<Template>,
    pub rule_sets: Vec<RuleSet>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            model: String::new(),
            allow_http: true,
            timeout_secs: 180,
            concurrency: 3,
            max_file_mb: 1228,
            max_files: 800,
            chunk_threshold: 12000,
            chunk_size: 8000,
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
        if !result.iter().any(|s| s.id == saved.id) {
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
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<AppConfig>(&raw)
            .map(AppConfig::merged)
            .unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

pub fn save(path: &Path, config: &AppConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())
}
