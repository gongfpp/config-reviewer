use crate::model::{Rule, RuleSet, SystemType, Template};

/// 内置系统类型。用户可在此基础上新增。
pub fn default_systems() -> Vec<SystemType> {
    vec![
        SystemType {
            id: "apollo".into(),
            name: "Apollo".into(),
            color: "#3a5a78".into(),
            default_template_id: "tpl-apollo".into(),
        },
        SystemType {
            id: "xxl".into(),
            name: "XXL-JOB".into(),
            color: "#4f7a52".into(),
            default_template_id: "tpl-xxl".into(),
        },
        SystemType {
            id: "nebula".into(),
            name: "星云".into(),
            color: "#6a5570".into(),
            default_template_id: "tpl-nebula".into(),
        },
        SystemType {
            id: "other".into(),
            name: "其他".into(),
            color: "#a89f8c".into(),
            default_template_id: "tpl-generic".into(),
        },
    ]
}

/// 内置评审模板；沿用配置评审口径。
pub fn default_templates() -> Vec<Template> {
    vec![
        Template {
            id: "tpl-xxl".into(),
            name: "XXL-JOB 配置评审".into(),
            content: "评审 XXL-JOB 配置。Cron 采用 6 个必填字段（秒、分、时、日、月、星期）及可选年份，必须逐位解释频率，不能按五位 Unix Cron 解读；给出修正表达式时再次逐位核对。单机串行不等于跨节点互斥，分片广播需业务实现分片与幂等。核对执行器、JobHandler、调度表达式、运行参数、路由与阻塞策略、超时、失败重试、幂等、告警、启停时间、容量和回滚方案；资料未展示的项目标为待确认，不得自行假定。".into(),
            input_instructions: String::new(),
            output_instructions: String::new(),
            output_format: "standard".into(),
            builtin: true,
            overridden: false,
        },
        Template {
            id: "tpl-nebula".into(),
            name: "星云配置评审".into(),
            content: "评审星云配置。核对环境与命名空间、配置对象、依赖关系、参数和值域、版本与生效范围、超时重试、容量、灰度步骤、监控告警和回滚方案；资料未展示的项目标为待确认，不得自行假定。".into(),
            input_instructions: String::new(),
            output_instructions: String::new(),
            output_format: "standard".into(),
            builtin: true,
            overridden: false,
        },
        Template {
            id: "tpl-apollo".into(),
            name: "Apollo 配置评审".into(),
            content: "评审 Apollo 配置。核对应用标识、环境、集群、Namespace、配置项和值类型、发布范围、敏感信息、客户端兼容性、发布顺序、灰度验证、监控告警和回滚版本；资料未展示的项目标为待确认，不得自行假定。".into(),
            input_instructions: String::new(),
            output_instructions: String::new(),
            output_format: "standard".into(),
            builtin: true,
            overridden: false,
        },
        Template {
            id: "tpl-generic".into(),
            name: "通用文档评审".into(),
            content: "评审这份文档或配置。核对完整性、一致性、风险点、依赖关系、超时与重试、容量、监控告警和回滚方案；资料未展示的项目标为待确认，不得自行假定。".into(),
            input_instructions: String::new(),
            output_instructions: String::new(),
            output_format: "standard".into(),
            builtin: true,
            overridden: false,
        },
    ]
}

/// 预置规则集（可增删改）。具体业务清单仍待业务方补充。
pub fn default_rule_sets() -> Vec<RuleSet> {
    vec![RuleSet {
        id: "rs-default".into(),
        name: "默认混合包规则".into(),
        builtin: true,
        rules: vec![
            rule("r-apollo", "*apollo*.json", "apollo", "tpl-apollo"),
            rule("r-xxl-json", "*xxl-job*.json", "xxl", "tpl-xxl"),
            rule("r-xxl-xlsx", "job-config-*.xlsx", "xxl", "tpl-xxl"),
            rule("r-xxl-docx", "*操作手册*.{doc,docx}", "xxl", "tpl-xxl"),
            rule("r-nebula", "星云/**", "nebula", "tpl-nebula"),
        ],
    }]
}

fn rule(id: &str, pattern: &str, system: &str, template_id: &str) -> Rule {
    Rule {
        id: id.into(),
        pattern: pattern.into(),
        match_type: "glob".into(),
        system: system.into(),
        template_id: template_id.into(),
        enabled: true,
    }
}

/// 匹配单个 glob 模式；同时尝试完整相对路径与文件名。
pub fn pattern_matches(pattern: &str, rel_path: &str, name: &str) -> bool {
    let normalized = rel_path.replace('\\', "/");
    let pattern = pattern.replace('\\', "/");
    for target in [normalized.as_str(), name] {
        if let Ok(glob) = globset::GlobBuilder::new(&pattern)
            .case_insensitive(true)
            .literal_separator(false)
            .backslash_escape(false)
            .build()
        {
            if glob.compile_matcher().is_match(target) {
                return true;
            }
        }
    }
    false
}

/// 按规则集解析文件归属；返回 (rule_id, system, template_id)。
pub fn resolve(rule_set: &RuleSet, rel_path: &str, name: &str) -> Option<(String, String, String)> {
    for rule in &rule_set.rules {
        if !rule.enabled {
            continue;
        }
        let pattern = rule.pattern.trim();
        let matched = match rule.match_type.as_str() {
            "filename" => name.to_lowercase() == pattern.to_lowercase(),
            "keyword" => {
                !pattern.is_empty() && name.to_lowercase().contains(&pattern.to_lowercase())
            }
            "extension" => std::path::Path::new(name)
                .extension()
                .and_then(|s| s.to_str())
                .map(|ext| ext.eq_ignore_ascii_case(pattern.trim_start_matches('.')))
                .unwrap_or(false),
            "glob" => pattern_matches(pattern, rel_path, name),
            _ => false,
        };
        if matched {
            return Some((
                rule.id.clone(),
                rule.system.clone(),
                rule.template_id.clone(),
            ));
        }
    }
    None
}

/// 配置错误应在保存或扫描前反馈，不能静默变成“零命中”。
pub fn validate(rule_set: &RuleSet) -> Result<(), String> {
    for (index, rule) in rule_set.rules.iter().enumerate().filter(|(_, r)| r.enabled) {
        let p = rule.pattern.trim();
        let invalid = match rule.match_type.as_str() {
            "glob" => globset::GlobBuilder::new(&p.replace('\\', "/"))
                .backslash_escape(false)
                .build()
                .is_err(),
            "filename" | "keyword" => p.contains('/') || p.contains('\\'),
            "extension" => {
                p.trim_start_matches('.').is_empty()
                    || p.trim_start_matches('.')
                        .chars()
                        .any(|c| !c.is_ascii_alphanumeric())
            }
            _ => true,
        };
        if p.is_empty() || invalid {
            return Err(format!(
                "规则集「{}」第 {} 条规则的匹配条件无效",
                rule_set.name,
                index + 1
            ));
        }
    }
    Ok(())
}
