use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::{RuleSet, SystemType, Template};
use crate::rules;

/// 应用配置（不含 API Key；Key 由系统凭据库单独保存）。
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub schema_version: u32,
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
            schema_version: CURRENT_SCHEMA_VERSION,
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
    /// 旧版仅迁移一次；新版配置里的删除必须保持，不再每次补回内置项。
    pub fn merged(mut self) -> Self {
        if self.schema_version >= CURRENT_SCHEMA_VERSION {
            return self;
        }
        self.systems = merge_systems(rules::default_systems(), self.systems);
        self.templates = merge_templates(rules::default_templates(), self.templates);
        self.rule_sets = merge_rule_sets(rules::default_rule_sets(), self.rule_sets);
        for template in &mut self.templates {
            template.output_format = self.output_format.clone();
        }
        // 保留规则原本实际使用的规范；有分歧时拆成可见模块，不静默换 Prompt。
        let old_rules: Vec<_> = self
            .rule_sets
            .iter()
            .flat_map(|r| &r.rules)
            .cloned()
            .collect();
        let mut mappings = std::collections::HashMap::new();
        for rule in old_rules {
            let module = self.systems.iter().find(|s| s.id == rule.system).cloned();
            if module
                .as_ref()
                .is_some_and(|s| s.default_template_id == rule.template_id)
            {
                continue;
            }
            let key = (rule.system.clone(), rule.template_id.clone());
            if mappings.contains_key(&key) {
                continue;
            }
            if let Some(template) = self.templates.iter().find(|t| t.id == rule.template_id) {
                let id = format!("module-migrated-{}", uuid::Uuid::new_v4());
                self.systems.push(SystemType {
                    id: id.clone(),
                    name: format!(
                        "{} · {}",
                        module
                            .as_ref()
                            .map(|s| s.name.as_str())
                            .unwrap_or(&rule.system),
                        template.name
                    ),
                    color: module.map(|s| s.color).unwrap_or_else(|| "#a89f8c".into()),
                    default_template_id: template.id.clone(),
                });
                mappings.insert(key, id);
            }
        }
        for rule in self.rule_sets.iter_mut().flat_map(|r| &mut r.rules) {
            if let Some(id) = mappings.get(&(rule.system.clone(), rule.template_id.clone())) {
                rule.system = id.clone();
            }
        }
        // 旧版模板管理中未绑定模块的模板也保留为模块，方便继续查看和编辑。
        let orphaned: Vec<_> = self
            .templates
            .iter()
            .filter(|t| !self.systems.iter().any(|s| s.default_template_id == t.id))
            .cloned()
            .collect();
        for template in orphaned {
            self.systems.push(SystemType {
                id: format!("module-migrated-{}", uuid::Uuid::new_v4()),
                name: template.name.clone(),
                color: "#a89f8c".into(),
                default_template_id: template.id,
            });
        }
        let mut owned = std::collections::HashSet::new();
        for module in &mut self.systems {
            if !owned.insert(module.default_template_id.clone()) {
                if let Some(mut template) = self
                    .templates
                    .iter()
                    .find(|t| t.id == module.default_template_id)
                    .cloned()
                {
                    template.id = format!("tpl-module-{}", uuid::Uuid::new_v4());
                    template.builtin = false;
                    module.default_template_id = template.id.clone();
                    self.templates.push(template);
                }
            }
        }
        let systems = self.systems.clone();
        for rule in self.rule_sets.iter_mut().flat_map(|r| &mut r.rules) {
            if let Some(module) = systems.iter().find(|s| s.id == rule.system) {
                rule.template_id = module.default_template_id.clone();
            }
        }
        self.schema_version = CURRENT_SCHEMA_VERSION;
        self
    }

    pub fn module_template(&self, module_id: &str) -> Result<&Template, String> {
        let module = self
            .systems
            .iter()
            .find(|s| s.id == module_id)
            .ok_or_else(|| format!("评审模块不存在：{module_id}，请重新选择模块"))?;
        self.templates
            .iter()
            .find(|t| t.id == module.default_template_id)
            .ok_or_else(|| format!("评审模块「{}」缺少评审规范", module.name))
    }

    pub fn normalize_rule_set(&self, rule_set: &mut RuleSet) -> Result<(), String> {
        rules::validate(rule_set)?;
        for rule in &mut rule_set.rules {
            rule.template_id = self.module_template(&rule.system)?.id.clone();
        }
        Ok(())
    }

    pub fn normalize_and_validate(&mut self) -> Result<(), String> {
        if self.schema_version > CURRENT_SCHEMA_VERSION {
            return Err("配置由更新版本创建，请更新应用后再修改".into());
        }
        let mut ids = std::collections::HashSet::new();
        for template in &self.templates {
            if template.id.trim().is_empty() || !ids.insert(&template.id) {
                return Err("评审规范 ID 为空或重复".into());
            }
            if template.name.trim().is_empty() || template.content.trim().is_empty() {
                return Err("评审规范名称和评审要求不能为空".into());
            }
            if !["standard", "checklist", "brief", "json"]
                .contains(&template.output_format.as_str())
            {
                return Err(format!("评审规范「{}」输出格式无效", template.name));
            }
            if [
                &template.content,
                &template.input_instructions,
                &template.output_instructions,
            ]
            .iter()
            .any(|s| s.len() > 200_000)
            {
                return Err(format!("评审规范「{}」内容过长", template.name));
            }
        }
        let mut module_ids = std::collections::HashSet::new();
        let mut module_templates = std::collections::HashSet::new();
        for module in &self.systems {
            if module.id.trim().is_empty()
                || module.name.trim().is_empty()
                || !module_ids.insert(&module.id)
            {
                return Err("评审模块名称或 ID 为空，或 ID 重复".into());
            }
            self.module_template(&module.id)?;
            if !module_templates.insert(&module.default_template_id) {
                return Err("每个评审模块必须拥有独立的评审规范".into());
            }
        }
        let mut rule_sets = self.rule_sets.clone();
        let mut rule_set_ids = std::collections::HashSet::new();
        for rule_set in &mut rule_sets {
            if rule_set.id.trim().is_empty() || !rule_set_ids.insert(rule_set.id.clone()) {
                return Err("文件匹配方案 ID 为空或重复".into());
            }
            let mut rule_ids = std::collections::HashSet::new();
            for rule in &rule_set.rules {
                if rule.id.trim().is_empty() || !rule_ids.insert(rule.id.clone()) {
                    return Err("文件匹配条件 ID 为空或重复".into());
                }
            }
            self.normalize_rule_set(rule_set)?;
        }
        self.rule_sets = rule_sets;
        self.schema_version = CURRENT_SCHEMA_VERSION;
        Ok(())
    }

    /// 启动前重新读取模块规范，旧清单或调用方不能携带过期/自定义模板覆盖配置。
    pub fn prepare_review(&self, request: &mut crate::model::ReviewRequest) -> Result<(), String> {
        if request.files.is_empty() {
            return Err("请先选择待评审文件".into());
        }
        for file in &mut request.files {
            let module_id = file.system.as_deref().ok_or("请为文件选择评审模块")?;
            let template = self.module_template(module_id)?;
            file.template_id = template.id.clone();
            file.template_content = crate::results::module_prompt(template);
            file.output_format = template.output_format.clone();
        }
        if request.granularity == "single" {
            let first = &request.files[0];
            if request.files.iter().any(|file| file.system != first.system) {
                return Err("合并评审仅支持同一个评审模块；不同模块请使用逐文件评审，分别执行各自的输入与输出规范".into());
            }
        }
        Ok(())
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
            def.input_instructions = saved.input_instructions.clone();
            def.output_instructions = saved.output_instructions.clone();
            def.output_format = saved.output_format.clone();
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
                let migrating = config.schema_version < CURRENT_SCHEMA_VERSION;
                let config = config.merged();
                if migrating {
                    let _ = save(path, &config);
                }
                return config;
            }
        }
    }
    AppConfig::default()
}

pub fn save(path: &Path, config: &AppConfig) -> Result<(), String> {
    use std::io::Write;
    let mut config = config.clone();
    config.normalize_and_validate()?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?;
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
