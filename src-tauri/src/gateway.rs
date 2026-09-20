use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::model::GatewayConfig;

fn endpoint(base_url: &str) -> String {
    let clean = base_url.trim().trim_end_matches('/');
    if clean.ends_with("/chat/completions") {
        clean.to_string()
    } else {
        format!("{clean}/chat/completions")
    }
}

fn build_client(cfg: &GatewayConfig) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(cfg.timeout_secs.max(1)))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))
}

fn validate(cfg: &GatewayConfig) -> Result<(), String> {
    if cfg.base_url.trim().is_empty() {
        return Err("请先配置 Base URL。".into());
    }
    if cfg.model.trim().is_empty() {
        return Err("请先配置模型标识。".into());
    }
    if cfg.api_key.trim().is_empty() {
        return Err("请先配置 API Key。".into());
    }
    Ok(())
}

/// 一次性（非流式）请求，返回正文。
pub async fn chat(cfg: &GatewayConfig, user_content: &str) -> Result<String, String> {
    validate(cfg)?;
    let client = build_client(cfg)?;
    let body = json!({
        "model": cfg.model.trim(),
        "messages": [{ "role": "user", "content": user_content }],
        "stream": false
    });
    let resp = client
        .post(endpoint(&cfg.base_url))
        .bearer_auth(cfg.api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("模型请求失败：{e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("模型服务返回 HTTP {status}"));
    }
    let value: Value = resp.json().await.map_err(|e| format!("模型响应解析失败：{e}"))?;
    extract_content(&value)
}

/// 流式请求；每个正文增量回调一次，返回拼接后的完整正文。
pub async fn chat_stream<F>(cfg: &GatewayConfig, user_content: &str, mut on_delta: F) -> Result<String, String>
where
    F: FnMut(&str) + Send,
{
    validate(cfg)?;
    let client = build_client(cfg)?;
    let body = json!({
        "model": cfg.model.trim(),
        "messages": [{ "role": "user", "content": user_content }],
        "stream": true
    });
    let resp = client
        .post(endpoint(&cfg.base_url))
        .bearer_auth(cfg.api_key.trim())
        .header("Accept", "text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("模型请求失败：{e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("模型服务返回 HTTP {status}"));
    }

    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();
    let mut content = String::new();
    let mut saw_event = false;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("读取模型流失败：{e}"))?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer.drain(..=pos);
            let Some(payload) = line.strip_prefix("data:") else { continue };
            let payload = payload.trim();
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            saw_event = true;
            let Ok(parsed) = serde_json::from_str::<Value>(payload) else { continue };
            if let Some(delta) = parsed
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("delta"))
                .and_then(|d| d.get("content"))
                .and_then(Value::as_str)
            {
                if !delta.is_empty() {
                    content.push_str(delta);
                    on_delta(delta);
                }
            }
        }
    }

    if !saw_event {
        // 网关忽略 stream 直接返回一次性 JSON
        let value: Value = serde_json::from_str(buffer.trim()).map_err(|e| format!("模型响应解析失败：{e}"))?;
        return extract_content(&value);
    }
    if content.trim().is_empty() {
        return Err("模型返回为空，请重试。".into());
    }
    Ok(content)
}

/// 最小连通性测试。
pub async fn test_connection(cfg: &GatewayConfig) -> Result<String, String> {
    let reply = chat(cfg, "回复两个字：可用").await?;
    Ok(reply.trim().chars().take(40).collect())
}

fn extract_content(value: &Value) -> Result<String, String> {
    let content = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if content.is_empty() {
        return Err("模型返回为空或缺少内容。".into());
    }
    Ok(content)
}
