/// 优先保留行边界；超长单行按 Unicode 字符硬切，拼接后与原文一致。
pub fn chunk_text(text: &str, threshold: usize, chunk_size: usize) -> Vec<String> {
    if char_len(text) <= threshold {
        return vec![text.to_string()];
    }
    let limit = chunk_size.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for line in text.split_inclusive('\n') {
        let len = char_len(line);
        if count > 0 && count + len > limit {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
        for ch in line.chars() {
            if count == limit {
                chunks.push(std::mem::take(&mut current));
                count = 0;
            }
            current.push(ch);
            count += 1;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(text.to_string());
    }
    chunks
}

/// JSON 按顶层键成组；单键超过大小上限时按字符拆成文本片段。
pub fn chunk_json(value: &serde_json::Value, threshold: usize, chunk_size: usize) -> Vec<String> {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_default();
    if char_len(&pretty) <= threshold {
        return vec![pretty];
    }
    let limit = chunk_size.max(1);
    let Some(obj) = value.as_object() else {
        return chunk_text(&pretty, 0, limit);
    };
    let mut chunks = Vec::new();
    let mut current = serde_json::Map::new();
    for (key, val) in obj {
        let mut candidate = current.clone();
        candidate.insert(key.clone(), val.clone());
        let encoded = serde_json::to_string_pretty(&candidate).unwrap_or_default();
        if char_len(&encoded) > limit && !current.is_empty() {
            chunks.push(serde_json::to_string_pretty(&current).unwrap_or_default());
            current.clear();
        }
        current.insert(key.clone(), val.clone());
        let encoded = serde_json::to_string_pretty(&current).unwrap_or_default();
        if char_len(&encoded) > limit {
            chunks.extend(chunk_text(&encoded, 0, limit));
            current.clear();
        }
    }
    if !current.is_empty() {
        chunks.push(serde_json::to_string_pretty(&current).unwrap_or_default());
    }
    if chunks.is_empty() {
        return chunk_text(&pretty, 0, limit);
    }
    chunks
}
pub fn char_len(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_unicode_line_preserves_content_and_limit() {
        let text = "中文🌱".repeat(10000);
        let chunks = chunk_text(&text, 12000, 8000);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|s| char_len(s) <= 8000));
        assert_eq!(chunks.concat(), text);
    }
    #[test]
    fn mixed_line_endings_are_preserved() {
        let text = "一\r\n二\n三四五六";
        let chunks = chunk_text(text, 2, 3);
        assert_eq!(chunks.concat(), text);
        assert!(chunks.iter().all(|s| char_len(s) <= 3));
    }
    #[test]
    fn zero_chunk_size_terminates() {
        assert_eq!(chunk_text("中文", 0, 0), vec!["中", "文"]);
    }
    #[test]
    fn large_json_value_is_bounded() {
        let value = serde_json::json!({"config":"字".repeat(30000)});
        let chunks = chunk_json(&value, 12000, 8000);
        assert!(chunks.iter().all(|s| char_len(s) <= 8000));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&chunks.concat()).unwrap(),
            value
        );
    }
    #[test]
    fn grouped_json_chunks_remain_valid() {
        let value = serde_json::json!({"a":1,"b":2,"c":3});
        let chunks = chunk_json(&value, 0, 15);
        assert_eq!(chunks.len(), 3);
        for c in chunks {
            assert!(serde_json::from_str::<serde_json::Value>(&c).is_ok());
        }
    }
}
