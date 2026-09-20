/// 按行边界切块，保留键路径/Sheet 等定位信息由调用方补充。
pub fn chunk_text(text: &str, threshold: usize, chunk_size: usize) -> Vec<String> {
    if char_len(text) <= threshold {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for line in text.lines() {
        let line_len = char_len(line) + 1;
        if current_len > 0 && current_len + line_len > chunk_size {
            chunks.push(std::mem::take(&mut current));
            current_len = 0;
        }
        current.push_str(line);
        current.push('\n');
        current_len += line_len;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(text.to_string());
    }
    chunks
}

/// JSON 顶层键优先切块；单个键仍超限时退回按字符硬切。
pub fn chunk_json(value: &serde_json::Value, threshold: usize, chunk_size: usize) -> Vec<String> {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_default();
    if char_len(&pretty) <= threshold {
        return vec![pretty];
    }
    if let Some(obj) = value.as_object() {
        let mut chunks = Vec::new();
        let mut current = String::from("{\n");
        let mut current_len = 2usize;
        for (key, val) in obj {
            let entry = format!(
                "  {}: {},\n",
                serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()),
                serde_json::to_string_pretty(val).unwrap_or_default()
            );
            let entry_len = char_len(&entry);
            if current_len > 2 && current_len + entry_len > chunk_size {
                chunks.push(format!("{}}}", current.trim_end_matches(",\n").to_string()));
                current = String::from("{\n");
                current_len = 2;
            }
            current.push_str(&entry);
            current_len += entry_len;
        }
        if current_len > 2 {
            chunks.push(format!("{}}}", current.trim_end_matches(",\n").to_string()));
        }
        // 单键即超限的极端情况，退回字符切分
        if chunks.iter().any(|c| char_len(c) > chunk_size * 2) {
            return chunk_text(&pretty, threshold, chunk_size);
        }
        if !chunks.is_empty() {
            return chunks;
        }
    }
    chunk_text(&pretty, threshold, chunk_size)
}

pub fn char_len(text: &str) -> usize {
    text.chars().count()
}
