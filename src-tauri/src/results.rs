use crate::model::{Citation, FileResult};

/// 依据模板与文件正文构造用户指令。
pub fn build_instruction(template_content: &str, file_name: &str, chunk_hint: Option<(usize, usize)>) -> String {
    let mut instruction = String::new();
    instruction.push_str("你正在执行配置/文档评审。标准结论必须以“通过”“有条件通过”或“阻断”之一开头，并分别给出【依据】【风险】【整改项】。\n");
    instruction.push_str("每条依据与风险尽量给出原文引用，格式：引用：定位 | 原文片段；无法引用时写“待确认”。\n");
    instruction.push_str("只输出评审结论本体，不要复述本说明。\n\n");
    instruction.push_str("评审要求：\n");
    instruction.push_str(template_content.trim());
    instruction.push_str("\n\n待评审文件：");
    instruction.push_str(file_name);
    if let Some((index, total)) = chunk_hint {
        instruction.push_str(&format!("（第 {index}/{total} 块，仅评估本块内容，不要臆测其他块）"));
    }
    instruction
}

/// 从模型输出解析结论、依据、风险、整改项与引用。
pub fn parse_result(file: &str, system: Option<String>, template_id: &str, chunks: usize, duration_ms: u64, raw: &str) -> FileResult {
    let verdict = detect_verdict(raw);
    let (basis, risks, remediations) = split_sections(raw);
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
        error: None,
    }
}

pub fn detect_verdict(raw: &str) -> String {
    let head: String = raw.chars().take(200).collect();
    let lowered = head.replace(' ', "");
    if lowered.contains("阻断") {
        "阻断".into()
    } else if lowered.contains("有条件通过") {
        "有条件通过".into()
    } else if lowered.contains("通过") {
        "通过".into()
    } else {
        "未识别结论".into()
    }
}

fn split_sections(raw: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    enum Section {
        None,
        Basis,
        Risk,
        Fix,
    }
    let mut section = Section::None;
    let (mut basis, mut risks, mut fixes) = (Vec::new(), Vec::new(), Vec::new());
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let header = trimmed
            .trim_start_matches(['#', '*', '-', ' '])
            .trim_end_matches(['：', ':'])
            .trim();
        if matches!(header, "依据" | "标准依据") {
            section = Section::Basis;
            continue;
        }
        if matches!(header, "风险" | "主要风险") {
            section = Section::Risk;
            continue;
        }
        if matches!(header, "整改项" | "整改建议" | "建议") {
            section = Section::Fix;
            continue;
        }
        if trimmed.starts_with("引用") {
            continue;
        }
        let item = trimmed
            .trim_start_matches(['-', '*', '•', ' '])
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '、')
            .trim();
        if item.is_empty() {
            continue;
        }
        match section {
            Section::Basis => basis.push(item.to_string()),
            Section::Risk => risks.push(item.to_string()),
            Section::Fix => fixes.push(item.to_string()),
            Section::None => {}
        }
    }
    (basis, risks, fixes)
}

/// 解析「引用：定位 | 片段」形式的结构化引用。
fn extract_citations(file: &str, raw: &str) -> Vec<Citation> {
    let mut citations = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim().trim_start_matches(['>', '-', '*', ' ']);
        let Some(rest) = trimmed.strip_prefix("引用") else { continue };
        let rest = rest.trim_start_matches(['：', ':']).trim();
        if rest.is_empty() {
            continue;
        }
        let (locator, snippet) = match rest.split_once('|') {
            Some((l, s)) => (l.trim().to_string(), s.trim().to_string()),
            None => (rest.to_string(), String::new()),
        };
        let snippet: String = snippet.chars().take(120).collect();
        citations.push(Citation {
            file: file.to_string(),
            locator,
            snippet,
        });
    }
    citations
}
