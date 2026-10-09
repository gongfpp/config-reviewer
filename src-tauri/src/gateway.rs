use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::model::GatewayConfig;
use futures_util::StreamExt;
use reqwest::header::{HeaderName, HeaderValue};
use serde_json::{json, Value};

const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

fn endpoint(base_url: &str) -> String {
    let clean = base_url.trim().trim_end_matches('/');
    if clean.ends_with("/chat/completions") {
        clean.to_string()
    } else {
        format!("{clean}/chat/completions")
    }
}

fn build_client(cfg: &GatewayConfig) -> Result<reqwest::Client, String> {
    // 内网默认直连，避免继承电脑上的外网代理。
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(cfg.timeout_secs.max(1)));
    if !cfg.proxy_url.trim().is_empty() {
        builder = builder.proxy(
            reqwest::Proxy::all(cfg.proxy_url.trim()).map_err(|_| "代理地址无效".to_string())?,
        );
    }
    builder
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))
}

pub fn validate(cfg: &GatewayConfig) -> Result<(), String> {
    let url = reqwest::Url::parse(cfg.base_url.trim())
        .map_err(|_| "Base URL 无效，请填写 http:// 或 https:// 地址".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("Base URL 仅支持 HTTP/HTTPS".into());
    }
    if url.scheme() == "http" && !cfg.allow_http {
        return Err("HTTP 已禁用，请启用「允许内网 HTTP」或使用 HTTPS".into());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Base URL 不应包含账号、密码、查询参数或锚点".into());
    }
    if cfg.model.trim().is_empty() {
        return Err("请先配置模型标识。".into());
    }
    if cfg.api_key.trim().is_empty() {
        return Err("请先配置 API Key。".into());
    }
    if !cfg.api_key_header.trim().is_empty() {
        HeaderName::from_bytes(cfg.api_key_header.trim().as_bytes())
            .map_err(|_| "API Key 请求头名称无效".to_string())?;
    }
    HeaderValue::from_str(cfg.api_key.trim()).map_err(|_| "API Key 含无效字符".to_string())?;
    Ok(())
}

fn request(
    cfg: &GatewayConfig,
    content: &str,
    stream: bool,
) -> Result<reqwest::RequestBuilder, String> {
    validate(cfg)?;
    let mut req = build_client(cfg)?.post(endpoint(&cfg.base_url));
    if cfg.api_key_header.trim().is_empty() {
        req = req.bearer_auth(cfg.api_key.trim());
    } else {
        req = req.header(cfg.api_key_header.trim(), cfg.api_key.trim());
    }
    Ok(req.json(&json!({ "model": cfg.model.trim(), "messages": [{ "role": "user", "content": content }], "stream": stream })))
}

/// 等待网络或信号量时也响应取消，不依赖下一个网络包到达。
pub async fn cancellable<T>(
    future: impl Future<Output = T>,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<T, String> {
    tokio::pin!(future);
    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err("任务已取消".into());
        }
        tokio::select! {
            value = &mut future => return Ok(value),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
}

pub async fn chat(cfg: &GatewayConfig, content: &str) -> Result<String, String> {
    let resp = request(cfg, content, false)?
        .send()
        .await
        .map_err(|e| format!("模型请求失败：{e}"))?;
    check_status(&resp)?;
    let bytes = read_body(resp, None).await?;
    let value = serde_json::from_slice(&bytes).map_err(|e| format!("模型响应解析失败：{e}"))?;
    extract_content(&value)
}

fn check_status(resp: &reqwest::Response) -> Result<(), String> {
    if !resp.status().is_success() {
        return Err(format!("模型服务返回 HTTP {}", resp.status()));
    }
    Ok(())
}

async fn read_body(
    resp: reqwest::Response,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<Vec<u8>, String> {
    let mut stream = resp.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = cancellable(stream.next(), cancel).await? {
        let bytes = chunk.map_err(|e| format!("读取模型响应失败：{e}"))?;
        if body.len() + bytes.len() > MAX_RESPONSE_BYTES {
            return Err("模型响应超过 32 MB 上限".into());
        }
        body.extend_from_slice(&bytes);
    }
    Ok(body)
}

#[derive(Default)]
struct SseDecoder {
    pending: Vec<u8>,
    data: Vec<String>,
    saw_event: bool,
    done: bool,
    finished: bool,
}
impl SseDecoder {
    fn feed(&mut self, bytes: &[u8], eof: bool) -> Result<Vec<String>, String> {
        self.pending.extend_from_slice(bytes);
        let mut deltas = Vec::new();
        while let Some(pos) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=pos).collect();
            self.line(&line[..line.len() - 1], &mut deltas)?;
        }
        if eof {
            let remaining = std::mem::take(&mut self.pending);
            if !remaining.is_empty() {
                self.line(&remaining, &mut deltas)?;
            }
            self.event(&mut deltas)?;
        }
        Ok(deltas)
    }
    fn line(&mut self, bytes: &[u8], out: &mut Vec<String>) -> Result<(), String> {
        let line = std::str::from_utf8(bytes)
            .map_err(|_| "模型流包含无效 UTF-8".to_string())?
            .trim_end_matches('\r');
        if line.is_empty() {
            return self.event(out);
        }
        if let Some(data) = line.strip_prefix("data:") {
            self.saw_event = true;
            self.data
                .push(data.strip_prefix(' ').unwrap_or(data).to_string());
        }
        Ok(())
    }
    fn event(&mut self, out: &mut Vec<String>) -> Result<(), String> {
        if self.data.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.data).join("\n");
        if data.trim() == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        let value: Value =
            serde_json::from_str(&data).map_err(|e| format!("模型流事件解析失败：{e}"))?;
        if value.get("error").is_some() {
            return Err("模型流返回错误事件，请检查网关日志".into());
        }
        if let Some(choice) = value.get("choices").and_then(|v| v.get(0)) {
            if choice.get("finish_reason").is_some_and(|v| !v.is_null()) {
                let reason = choice["finish_reason"].as_str().unwrap_or("");
                if reason != "stop" {
                    return Err(format!(
                        "模型输出未完整结束（finish_reason={reason}），请调整分块或网关输出上限"
                    ));
                }
                self.finished = true;
            }
            if let Some(delta) = choice
                .get("delta")
                .and_then(|d| d.get("content"))
                .and_then(Value::as_str)
            {
                if !delta.is_empty() {
                    out.push(delta.to_string());
                }
            }
        }
        Ok(())
    }
}

pub async fn chat_stream<F>(
    cfg: &GatewayConfig,
    content: &str,
    cancel: Option<Arc<AtomicBool>>,
    mut on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) + Send,
{
    let resp = cancellable(
        request(cfg, content, cfg.stream_enabled)?
            .header("Accept", "text/event-stream, application/json")
            .send(),
        cancel.as_ref(),
    )
    .await?
    .map_err(|e| format!("模型请求失败：{e}"))?;
    check_status(&resp)?;
    let is_json = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.contains("application/json"));
    if is_json || !cfg.stream_enabled {
        let bytes = read_body(resp, cancel.as_ref()).await?;
        let value = serde_json::from_slice(&bytes).map_err(|e| format!("模型响应解析失败：{e}"))?;
        let text = extract_content(&value)?;
        on_delta(&text);
        return Ok(text);
    }
    let mut stream = resp.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut raw = Vec::new();
    let mut text = String::new();
    let mut received = 0;
    while let Some(chunk) = cancellable(stream.next(), cancel.as_ref()).await? {
        let bytes = chunk.map_err(|e| format!("读取模型流失败：{e}"))?;
        received += bytes.len();
        if received > MAX_RESPONSE_BYTES {
            return Err("模型响应超过 32 MB 上限".into());
        }
        raw.extend_from_slice(&bytes);
        // 部分网关没有正确的 Content-Type；保留完整 JSON，不让逐行 SSE 解析丢掉正文。
        if raw
            .iter()
            .find(|b| !b.is_ascii_whitespace())
            .is_some_and(|b| *b == b'{')
        {
            continue;
        }
        for delta in decoder.feed(&bytes, false)? {
            text.push_str(&delta);
            on_delta(&delta);
        }
        if decoder.done {
            break;
        }
    }
    if raw
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|b| *b == b'{')
    {
        let value = serde_json::from_slice(&raw).map_err(|e| format!("模型响应解析失败：{e}"))?;
        let text = extract_content(&value)?;
        on_delta(&text);
        return Ok(text);
    }
    for delta in decoder.feed(&[], true)? {
        text.push_str(&delta);
        on_delta(&delta);
    }
    if !decoder.done && !decoder.finished {
        return Err("模型流提前中断，未收到完成标记，请重试".into());
    }
    if text.trim().is_empty() {
        return Err("模型返回为空，请重试。".into());
    }
    Ok(text)
}

pub async fn test_connection(cfg: &GatewayConfig) -> Result<String, String> {
    let reply = chat_stream(cfg, "回复两个字：可用", None, |_| {}).await?;
    Ok(reply.trim().chars().take(40).collect())
}

fn extract_content(value: &Value) -> Result<String, String> {
    if value.get("error").is_some() {
        return Err("模型返回错误，请检查网关日志".into());
    }
    if let Some(reason) = value["choices"][0]["finish_reason"].as_str() {
        if reason != "stop" {
            return Err(format!("模型输出未完整结束（finish_reason={reason}）"));
        }
    }
    let text = value["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return Err("模型返回为空或缺少内容。".into());
    }
    Ok(text.to_string())
}
