use serde_json::Value;

use crate::model::{Citation, FileResult};

/// 模块中输入、评审和输出约定共同形成规范；输出格式由同一模块提供。
pub fn module_prompt(template: &crate::model::Template) -> String {
    format!("输入说明：\n{}\n\n评审要求：\n{}\n\n输出补充要求：\n{}\n输出补充要求必须遵循所选输出格式的结构，不能删除必需字段或结论。",
        if template.input_instructions.trim().is_empty() { "读取所附文件正文；缺失信息标为待确认，不得臆测。" } else { template.input_instructions.trim() },
        template.content.trim(),
        if template.output_instructions.trim().is_empty() { "按所选输出格式输出结论、依据、风险和整改建议。" } else { template.output_instructions.trim() })
}

/// 按输出格式给出结论组织方式的说明。
fn format_spec(output_format: &str) -> &'static str {
    match output_format {
        "checklist" => {
            "请以检查表形式输出：第一条先给出结论（通过/有条件通过/阻断），随后用 Markdown 表格逐项列出「检查项 | 结论 | 依据」，最后给出【风险】【整改项】。\n"
        }
        "brief" => {
            "请用简洁摘要输出：第一条给出结论（通过/有条件通过/阻断），随后不超过 5 条要点，覆盖关键依据、风险与整改项。\n"
        }
        "json" => {
            "请只输出一个 JSON 对象，不要包裹 Markdown 代码块、不要输出多余文字。字段结构：{\"verdict\":\"通过|有条件通过|阻断\",\"basis\":[\"\"],\"risks\":[\"\"],\"remediations\":[\"\"],\"citations\":[{\"locator\":\"\",\"snippet\":\"\"}]}。\n"
        }
        _ => {
            "标准结论必须以“通过”“有条件通过”或“阻断”之一开头。整份文件只给一个总评结论和一组【依据】【风险】【整改项】，不得按任务重复这些标题。存在多个任务或配置对象时，在各条依据、风险和整改项中注明对应任务 ID 或对象标识。\n"
        }
    }
}

/// 依据模板、输出格式与文件正文构造用户指令。
pub fn build_instruction(
    template_content: &str,
    file_name: &str,
    chunk_hint: Option<(usize, usize)>,
    output_format: &str,
) -> String {
    let mut instruction = String::new();
    instruction.push_str("你正在执行配置/文档评审。\n");
    instruction.push_str(format_spec(output_format));
    instruction.push_str(
        "每条依据与风险尽量给出原文引用，格式：引用：定位 | 原文片段；无法引用时写“待确认”。\n",
    );
    instruction.push_str("整改建议必须保持资料已声明的业务目标。不同 Handler 或处理器的能力不得互相推定，资料未证明的能力先标为待确认。存在多项独立风险时，每个备选方案都应说明尚未解决的风险，不得把局部修复描述为消除全部风险。\n");
    instruction.push_str("只输出评审结论本体，不要复述本说明。\n\n");
    instruction.push_str("评审要求：\n");
    instruction.push_str(template_content.trim());
    instruction.push_str("\n\n待评审文件：");
    instruction.push_str(file_name);
    if let Some((index, total)) = chunk_hint {
        instruction.push_str(&format!(
            "（第 {index}/{total} 块，仅评估本块内容，不要臆测其他块）"
        ));
    }
    if output_format == "json" {
        instruction.push_str("\n注意：本块也请严格输出上述 JSON 结构，不要加入解释文字。");
    }
    instruction
}

/// 从模型输出解析结论、依据、风险、整改项与引用。
pub fn parse_result(
    file: &str,
    system: Option<String>,
    template_id: &str,
    chunks: usize,
    duration_ms: u64,
    raw: &str,
    output_format: &str,
) -> FileResult {
    if output_format == "json" {
        if let Some(parsed) = parse_json_result(file, raw) {
            return FileResult {
                file: file.to_string(),
                system,
                template_id: template_id.to_string(),
                chunks,
                duration_ms,
                raw: raw.to_string(),
                ..parsed
            };
        }
    }
    let verdict = detect_verdict(raw);
    let (basis, risks, remediations) = split_sections(raw);
    let error = if output_format == "json" {
        Some("模型未返回合法的评审 JSON".to_string())
    } else if verdict == "未识别结论" {
        Some("模型输出缺少有效结论，请重试".to_string())
    } else if output_format == "standard"
        && (basis.is_empty() || risks.is_empty() || remediations.is_empty())
    {
        Some("标准结论缺少依据、风险或整改项，请重试".to_string())
    } else {
        None
    };
    let citations = extract_citations(file, raw);
    FileResult {
        file: file.to_string(),
        system,
        template_id: template_id.to_string(),
        verdict,
        basis,
        risks,
        remediations,
        citations,
        chunks,
        duration_ms,
        raw: raw.to_string(),
        error,
    }
}

fn json_object(raw: &str) -> Option<Value> {
    let trimmed = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    serde_json::from_str::<Value>(trimmed).ok().or_else(|| {
        let start = trimmed.find('{')?;
        let end = trimmed.rfind('}')?;
        if end <= start {
            return None;
        }
        serde_json::from_str::<Value>(&trimmed[start..=end]).ok()
    })
}

fn string_list(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_json_result(file: &str, raw: &str) -> Option<FileResult> {
    let value = json_object(raw)?;
    if !value.is_object() {
        return None;
    }
    let verdict = value
        .get("verdict")
        .and_then(Value::as_str)
        .map(normalize_verdict)
        .unwrap_or_else(|| "未识别结论".into());
    if verdict == "未识别结论" || verdict == "失败" {
        return None;
    }
    for key in ["basis", "risks", "remediations", "citations"] {
        if !value.get(key).is_some_and(Value::is_array) {
            return None;
        }
    }
    let citations = value
        .get("citations")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|c| {
                    let locator = c
                        .get("locator")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let snippet = c
                        .get("snippet")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .chars()
                        .take(120)
                        .collect::<String>();
                    if locator.is_empty() && snippet.is_empty() {
                        None
                    } else {
                        Some(Citation {
                            file: file.to_string(),
                            locator,
                            snippet,
                        })
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Some(FileResult {
        verdict,
        basis: string_list(&value, "basis"),
        risks: string_list(&value, "risks"),
        remediations: string_list(&value, "remediations"),
        citations,
        ..Default::default()
    })
}

fn normalize_verdict(text: &str) -> String {
    match text.trim() {
        "通过" | "有条件通过" | "阻断" | "失败" => text.trim().into(),
        _ => "未识别结论".into(),
    }
}

pub fn detect_verdict(raw: &str) -> String {
    let first = raw
        .lines()
        .find(|s| !s.trim().is_empty())
        .unwrap_or("")
        .trim()
        .trim_start_matches(['#', '*', '-', ' ', '【']);
    let first = first
        .strip_prefix("结论")
        .unwrap_or(first)
        .trim_start_matches(['：', ':', ' ', '】']);
    for verdict in ["有条件通过", "阻断", "通过"] {
        if first.starts_with(verdict) {
            return verdict.into();
        }
    }
    "未识别结论".into()
}

fn split_sections(raw: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    #[derive(Clone, Copy)]
    enum Section {
        None,
        Basis,
        Risk,
        Fix,
    }
    let mut section = Section::None;
    let (mut basis, mut risks, mut fixes) = (Vec::new(), Vec::new(), Vec::new());
    for line in raw.lines() {
        let normalized = line.trim().trim_start_matches(['#', '*', '-', ' ']);
        let mut item = normalized;
        for (name, next) in [
            ("依据", Section::Basis),
            ("标准依据", Section::Basis),
            ("风险", Section::Risk),
            ("主要风险", Section::Risk),
            ("整改项", Section::Fix),
            ("整改建议", Section::Fix),
            ("建议", Section::Fix),
        ] {
            let bracket = format!("【{name}】");
            let rest = normalized
                .strip_prefix(&bracket)
                .or_else(|| normalized.strip_prefix(&format!("{name}：")))
                .or_else(|| normalized.strip_prefix(&format!("{name}:")));
            if let Some(rest) = rest {
                section = next;
                item = rest.trim_start_matches(['*', ' ', '：', ':']);
                break;
            }
            if normalized.trim_end_matches(['*', ' ', '：', ':']) == name {
                section = next;
                item = "";
                break;
            }
        }
        let item = strip_numbered_list_prefix(item.trim().trim_start_matches(['-', '*', '•', ' ']));
        if item.is_empty() {
            continue;
        }
        match section {
            Section::Basis => basis.push(item.into()),
            Section::Risk => risks.push(item.into()),
            Section::Fix => fixes.push(item.into()),
            Section::None => {}
        }
    }
    (basis, risks, fixes)
}

/// 只移除数字 + 列表标点 + 空白；裸数字开头的任务 ID 和参数值必须保留。
fn strip_numbered_list_prefix(text: &str) -> &str {
    let digit_end = text.bytes().take_while(u8::is_ascii_digit).count();
    if digit_end == 0 {
        return text.trim();
    }
    let suffix = &text[digit_end..];
    for marker in [".", ")", "、"] {
        if let Some(rest) = suffix.strip_prefix(marker) {
            if rest.starts_with(char::is_whitespace) {
                return rest.trim();
            }
        }
    }
    text.trim()
}

/// 按引号外分隔符切开同行引用，表格内容和中文引号内的分号保持完整。
fn citation_segments(line: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut quote_end = None;
    for (index, ch) in line.char_indices() {
        if let Some(end) = quote_end {
            if ch == end {
                quote_end = None;
            }
            continue;
        }
        quote_end = match ch {
            '`' => Some('`'),
            '“' => Some('”'),
            '「' => Some('」'),
            '『' => Some('』'),
            _ => None,
        };
        if quote_end.is_some() {
            continue;
        }
        if matches!(ch, '；' | ';') {
            segments.push(&line[start..index]);
            start = index + ch.len_utf8();
        } else if (line[index..].starts_with("引用：") || line[index..].starts_with("引用:"))
            && index > start
            && line[start..index].contains('|')
        {
            segments.push(&line[start..index]);
            start = index;
        }
    }
    segments.push(&line[start..]);
    segments
}

fn explicit_sheet_locator(locator: &str) -> bool {
    locator.starts_with("Sheet:")
        || locator.starts_with("Sheet：")
        || locator.contains(" / Sheet:")
        || (locator.contains('!') && locator.contains('第') && locator.ends_with('行'))
}

/// 支持短前缀及同行省略“引用”但有明确工作表定位的引用；正文保持原样。
fn extract_citations(file: &str, raw: &str) -> Vec<Citation> {
    let mut citations = Vec::new();
    for line in raw.lines() {
        for segment in citation_segments(line) {
            let segment = segment.trim();
            let explicit = segment
                .split_once("引用：")
                .or_else(|| segment.split_once("引用:"));
            let rest = explicit.map(|(_, rest)| rest).unwrap_or(segment);
            let Some((locator, quoted)) = rest.split_once('|') else {
                continue;
            };
            let locator = locator.trim();
            if locator.is_empty()
                || locator.chars().count() > 160
                || (explicit.is_none() && !explicit_sheet_locator(locator))
            {
                continue;
            }
            let quoted = quoted.trim();
            let quoted_pair = [('`', '`'), ('“', '”'), ('「', '」'), ('『', '』')]
                .into_iter()
                .find(|(open, _)| quoted.starts_with(*open));
            let snippet = if let Some((open, close)) = quoted_pair {
                let body = &quoted[open.len_utf8()..];
                body.split_once(close).map(|(body, _)| body).unwrap_or(body)
            } else {
                quoted.split(['；', ';', '。']).next().unwrap_or("")
            }
            .trim();
            if snippet.is_empty() {
                continue;
            }
            let snippet: String = snippet.chars().take(120).collect();
            if citations
                .iter()
                .any(|c: &Citation| c.locator == locator && c.snippet == snippet)
            {
                continue;
            }
            citations.push(Citation {
                file: file.to_string(),
                locator: locator.to_string(),
                snippet,
            });
        }
    }
    citations
}
