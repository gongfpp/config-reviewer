use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::chunk;
use crate::config::{self, AppConfig};
use crate::export::{self, ExportRow};
use crate::gateway;
use crate::model::{
    FileResult, GatewayConfig, ReviewFileInput, ReviewProgress, ReviewRequest, RuleSet, ScanEntry,
    ScanResult, SystemType, Template,
};
use crate::parse;
use crate::results;
use crate::rules;
use crate::scan::{self, ScanOptions};
use crate::secrets;
use crate::store::{self, TaskSummary};

pub struct AppState {
    pub db_path: PathBuf,
    pub workspace_root: PathBuf,
    pub config_path: PathBuf,
    pub cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub task_id: String,
    pub results: Vec<FileResult>,
    pub pass: u64,
    pub conditional: u64,
    pub block: u64,
    pub fail: u64,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemFileSummary {
    pub file: String,
    pub verdict: String,
    #[serde(default)]
    pub risks: Vec<String>,
}

#[tauri::command]
pub fn load_config(state: State<'_, AppState>) -> AppConfig {
    config::load(&state.config_path)
}

#[tauri::command]
pub fn save_settings(
    state: State<'_, AppState>,
    config: AppConfig,
    api_key: Option<String>,
) -> Result<(), String> {
    config::save(&state.config_path, &config)?;
    if let Some(key) = api_key {
        if !key.trim().is_empty() {
            secrets::save_api_key(&key)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn has_api_key() -> bool {
    secrets::has_api_key()
}

#[tauri::command]
pub fn clear_api_key() -> Result<(), String> {
    secrets::save_api_key("")
}

#[tauri::command]
pub fn scan_directory(
    state: State<'_, AppState>,
    path: String,
    rule_set: RuleSet,
    recursive: bool,
    matched_only: bool,
    threshold: usize,
    chunk_size: usize,
) -> Result<ScanResult, String> {
    let cfg = config::load(&state.config_path);
    let task_id = Uuid::new_v4().to_string();
    let workspace = fresh_workspace(&state.workspace_root, &task_id)?;
    let opts = scan_opts(&cfg, recursive, matched_only, threshold, chunk_size);
    let entries = match scan::scan_directory(PathBuf::from(&path).as_path(), &rule_set, &workspace, &opts) {
        Ok(entries) => entries,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&workspace);
            return Err(e);
        }
    };
    build_result("dir", &path, task_id, workspace, entries, rule_set, &cfg)
}

#[tauri::command]
pub fn scan_uploads(
    state: State<'_, AppState>,
    paths: Vec<String>,
    rule_set: RuleSet,
    matched_only: bool,
    threshold: usize,
    chunk_size: usize,
) -> Result<ScanResult, String> {
    let cfg = config::load(&state.config_path);
    let task_id = Uuid::new_v4().to_string();
    let workspace = fresh_workspace(&state.workspace_root, &task_id)?;
    let opts = scan_opts(&cfg, true, matched_only, threshold, chunk_size);
    let entries = match scan::scan_uploads(&paths, &rule_set, &workspace, &opts) {
        Ok(entries) => entries,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&workspace);
            return Err(e);
        }
    };
    let source_ref = if paths.len() == 1 {
        PathBuf::from(&paths[0])
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        format!("{} 个文件", paths.len())
    };
    build_result("upload", &source_ref, task_id, workspace, entries, rule_set, &cfg)
}

/// 粘贴文本来源：写入工作区后作为单个文本文件评审。
#[tauri::command]
pub fn scan_text(
    state: State<'_, AppState>,
    content: String,
    system: String,
    template_id: String,
    rule_set: RuleSet,
    threshold: usize,
    chunk_size: usize,
) -> Result<ScanResult, String> {
    if content.trim().is_empty() {
        return Err("内容为空".into());
    }
    let cfg = config::load(&state.config_path);
    let task_id = Uuid::new_v4().to_string();
    let workspace = fresh_workspace(&state.workspace_root, &task_id)?;
    let name = format!("pasted-{}.txt", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
    let abs = workspace.join(&name);
    if let Err(e) = std::fs::write(&abs, content.as_bytes()) {
        let _ = std::fs::remove_dir_all(&workspace);
        return Err(e.to_string());
    }
    let parsed = match parse::parse_file(&abs, "text", threshold, chunk_size) {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&workspace);
            return Err(e);
        }
    };
    let entry = ScanEntry {
        path: name.clone(),
        abs_path: abs.to_string_lossy().to_string(),
        name: name.clone(),
        ext: "txt".into(),
        kind: "text".into(),
        size: content.len() as u64,
        system: Some(system),
        rule_id: None,
        template_id: Some(template_id),
        matched: true,
        selected: true,
        chunks: parsed.chunks.len(),
        parse_status: "ok".into(),
        parse_error: None,
        preview: parsed.preview,
        text_len: parsed.text.chars().count(),
    };
    build_result("text", &name, task_id, workspace, vec![entry], rule_set, &cfg)
}

fn scan_opts(cfg: &AppConfig, recursive: bool, matched_only: bool, threshold: usize, chunk_size: usize) -> ScanOptions {
    ScanOptions {
        recursive,
        matched_only,
        threshold,
        chunk_size,
        max_file_bytes: cfg.max_file_mb.saturating_mul(1024 * 1024),
    }
}

fn build_result(
    source_type: &str,
    source_ref: &str,
    task_id: String,
    workspace: PathBuf,
    entries: Vec<ScanEntry>,
    rule_set: RuleSet,
    cfg: &AppConfig,
) -> Result<ScanResult, String> {
    if entries.len() > cfg.max_files {
        let _ = std::fs::remove_dir_all(&workspace);
        return Err(format!("文件数 {} 超过上限 {}", entries.len(), cfg.max_files));
    }
    let total_files = entries.len() as u64;
    let matched_files = entries.iter().filter(|e| e.matched).count() as u64;
    let total_bytes = entries.iter().map(|e| e.size).sum();
    Ok(ScanResult {
        task_id,
        source_type: source_type.into(),
        source_ref: source_ref.into(),
        total_files,
        matched_files,
        total_bytes,
        entries,
        workspace: workspace.to_string_lossy().to_string(),
        rule_set,
        templates: cfg.templates.clone(),
        systems: cfg.systems.clone(),
    })
}

#[tauri::command]
pub async fn test_connection(cfg: GatewayConfig, api_key: Option<String>) -> Result<String, String> {
    let mut cfg = cfg;
    if let Some(key) = api_key {
        if !key.trim().is_empty() {
            cfg.api_key = key;
        }
    }
    if cfg.api_key.trim().is_empty() {
        cfg.api_key = secrets::load_api_key();
    }
    gateway::test_connection(&cfg).await
}

#[tauri::command]
pub fn cancel_review(state: State<'_, AppState>, task_id: String) {
    if let Some(flag) = state.cancels.lock().unwrap().get(&task_id) {
        flag.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub async fn run_review(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ReviewRequest,
) -> Result<ReviewSummary, String> {
    let task_id = if request.task_id.trim().is_empty() {
        Uuid::new_v4().to_string()
    } else {
        request.task_id.clone()
    };
    let cancel = Arc::new(AtomicBool::new(false));
    state.cancels.lock().unwrap().insert(task_id.clone(), cancel.clone());

    let outcome = run_review_inner(&app, &state, &request, &task_id, &cancel).await;

    state.cancels.lock().unwrap().remove(&task_id);
    // 工作区推迟到下一次扫描或应用退出时清理，以便失败项重试仍能读取文件。
    outcome
}

/// 新建任务工作区，并清掉上一任务的残留目录（任务结束即清理）。
fn fresh_workspace(root: &PathBuf, task_id: &str) -> Result<PathBuf, String> {
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
    let workspace = root.join(task_id);
    std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
    Ok(workspace)
}

async fn run_review_inner(
    app: &AppHandle,
    _state: &AppState,
    request: &ReviewRequest,
    task_id: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<ReviewSummary, String> {
    let mut cfg = request.cfg.clone();
    cfg.api_key = secrets::load_api_key();
    if cfg.api_key.trim().is_empty() {
        return Err("未配置 API Key，请在「模型与规则」中填写。".into());
    }
    if cfg.base_url.trim().is_empty() || cfg.model.trim().is_empty() {
        return Err("请先配置 Base URL 与模型标识。".into());
    }
    let cfg = Arc::new(cfg);
    let concurrency = request.concurrency.clamp(1, 8);
    let threshold = request.chunk_threshold.max(1000);
    let chunk_size = request.chunk_size.max(500);
    let retry_count = request.retry_count.min(5);
    let output_format = request.output_format.clone();
    // 统一分析单元信号量：文件级与块级共享同一并发上限。
    let sem = Arc::new(Semaphore::new(concurrency));

    let mut all: Vec<FileResult> = Vec::new();
    let total = request.files.len();

    if request.granularity == "single" {
        let result = review_combined(
            app, &cfg, &request.files, threshold, chunk_size, &output_format, task_id, retry_count, &sem, cancel,
        )
        .await;
        all.push(result);
    } else {
        let futures = request.files.iter().enumerate().map(|(index, file)| {
            let app = app.clone();
            let cfg = cfg.clone();
            let sem = sem.clone();
            let cancel = cancel.clone();
            let task_id = task_id.to_string();
            let output_format = output_format.clone();
            async move {
                review_one(
                    &app, &cfg, file, threshold, chunk_size, &output_format, &task_id, index, total, retry_count, &sem,
                    cancel,
                )
                .await
            }
        });
        let results = join_all(futures).await;
        all.extend(results);
    }

    let cancelled = cancel.load(Ordering::Relaxed);
    let (pass, conditional, block, fail) = tally(&all);
    Ok(ReviewSummary {
        task_id: task_id.to_string(),
        results: all,
        pass,
        conditional,
        block,
        fail,
        cancelled,
    })
}

/// 逐文件分析：解析 → 单块直评 / 多块并行后按序合并。
#[allow(clippy::too_many_arguments)]
async fn review_one(
    app: &AppHandle,
    cfg: &Arc<GatewayConfig>,
    file: &ReviewFileInput,
    threshold: usize,
    chunk_size: usize,
    output_format: &str,
    task_id: &str,
    index: usize,
    total: usize,
    retry_count: usize,
    sem: &Arc<Semaphore>,
    cancel: Arc<AtomicBool>,
) -> FileResult {
    let start = Instant::now();
    emit(app, task_id, index, total, &file.name, "parse", Some("正在解析…"), None);

    let path = PathBuf::from(&file.abs_path);
    let parsed = match parse::parse_file(&path, &file.kind, threshold, chunk_size) {
        Ok(p) => p,
        Err(err) => {
            let result = FileResult {
                file: file.path.clone(),
                system: file.system.clone(),
                template_id: file.template_id.clone(),
                verdict: "失败".into(),
                error: Some(err),
                chunks: 0,
                duration_ms: start.elapsed().as_millis() as u64,
                ..Default::default()
            };
            emit(app, task_id, index, total, &file.name, "done", None, Some(result.clone()));
            return result;
        }
    };

    let raw = analyze_parsed(
        app, cfg, &file.name, &file.template_content, &parsed, output_format, task_id, index, total, retry_count, sem,
        cancel.clone(),
    )
    .await;

    let duration_ms = start.elapsed().as_millis() as u64;
    let mut result = match raw {
        Ok(text) => results::parse_result(
            &file.path,
            file.system.clone(),
            &file.template_id,
            parsed.chunks.len(),
            duration_ms,
            &text,
            output_format,
        ),
        Err(err) => FileResult {
            file: file.path.clone(),
            system: file.system.clone(),
            template_id: file.template_id.clone(),
            verdict: "失败".into(),
            error: Some(err),
            chunks: parsed.chunks.len(),
            duration_ms,
            ..Default::default()
        },
    };
    if result.error.is_some() {
        result.verdict = "失败".into();
    }
    emit(app, task_id, index, total, &file.name, "done", None, Some(result.clone()));
    result
}

/// 对已解析内容做评审：多块时各块并行、按序合并。返回模型原始输出。
#[allow(clippy::too_many_arguments)]
async fn analyze_parsed(
    app: &AppHandle,
    cfg: &Arc<GatewayConfig>,
    file_name: &str,
    template_content: &str,
    parsed: &parse::Parsed,
    output_format: &str,
    task_id: &str,
    index: usize,
    total: usize,
    retry_count: usize,
    sem: &Arc<Semaphore>,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    if parsed.chunks.len() <= 1 {
        let instruction = results::build_instruction(template_content, file_name, None, output_format);
        let content = format!("{instruction}\n\n待评审内容：\n{}", parsed.text);
        return call_with_retry(
            app, cfg, task_id, index, total, file_name, "analyze", &content, true, retry_count, sem, cancel,
        )
        .await;
    }

    let total_chunks = parsed.chunks.len();
    let mut futures = Vec::with_capacity(total_chunks);
    for (i, chunk) in parsed.chunks.iter().enumerate() {
        let instruction = results::build_instruction(template_content, file_name, Some((i + 1, total_chunks)), output_format);
        let content = format!("{instruction}\n\n第 {} 块内容：\n{chunk}", i + 1);
        let app = app.clone();
        let cfg = cfg.clone();
        let sem = sem.clone();
        let cancel = cancel.clone();
        let task_id = task_id.to_string();
        let file_name = file_name.to_string();
        futures.push(async move {
            let phase = format!("analyze {}/{}", i + 1, total_chunks);
            emit(&app, &task_id, index, total, &file_name, &phase, None, None);
            let res = call_with_retry(
                &app, &cfg, &task_id, index, total, &file_name, &phase, &content, false, retry_count, &sem, cancel,
            )
            .await;
            (i, res)
        });
    }
    let mut outputs = join_all(futures).await;
    outputs.sort_by_key(|(i, _)| *i);
    let mut chunk_outputs = Vec::with_capacity(outputs.len());
    for (i, res) in outputs {
        match res {
            Ok(text) => chunk_outputs.push(format!("【第 {} 块】\n{}", i + 1, text)),
            Err(err) => chunk_outputs.push(format!("【第 {} 块】分析失败：{err}", i + 1)),
        }
    }
    let merge = format!(
        "以下是对同一文件分块评审得到的结论。请合并为一份完整、无重复的评审结论，仍以“通过/有条件通过/阻断”开头，并保留【依据】【风险】【整改项】与引用。\n\n{}",
        chunk_outputs.join("\n\n---\n\n")
    );
    call_with_retry(app, cfg, task_id, index, total, file_name, "merge", &merge, true, retry_count, sem, cancel).await
}

/// 合并一次性：把所有文件正文合并为单一评审单元。
#[allow(clippy::too_many_arguments)]
async fn review_combined(
    app: &AppHandle,
    cfg: &Arc<GatewayConfig>,
    files: &[ReviewFileInput],
    threshold: usize,
    chunk_size: usize,
    output_format: &str,
    task_id: &str,
    retry_count: usize,
    sem: &Arc<Semaphore>,
    cancel: &Arc<AtomicBool>,
) -> FileResult {
    let start = Instant::now();
    let total = files.len().max(1);
    let mut combined = String::new();
    let mut parsed_any = 0usize;
    for (i, file) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        emit(app, task_id, i, total, &file.name, "parse", Some("正在解析…"), None);
        match parse::parse_file(&PathBuf::from(&file.abs_path), &file.kind, threshold, chunk_size) {
            Ok(p) => {
                parsed_any += 1;
                combined.push_str(&format!("\n\n===== 文件：{} =====\n{}", file.path, p.text));
            }
            Err(err) => {
                combined.push_str(&format!("\n\n===== 文件：{}（解析失败：{}）=====", file.path, err));
            }
        }
    }
    let file_name = format!("全部文件合并评审（{} 个）", files.len());
    let duration_ms = start.elapsed().as_millis() as u64;
    if parsed_any == 0 {
        let result = FileResult {
            file: file_name.clone(),
            verdict: "失败".into(),
            error: Some("没有可解析的文件".into()),
            duration_ms,
            ..Default::default()
        };
        emit(app, task_id, 0, 1, &result.file, "done", None, Some(result.clone()));
        return result;
    }
    let template_content = files.first().map(|f| f.template_content.as_str()).unwrap_or("");
    let parsed = parse::Parsed {
        text: combined.clone(),
        chunks: chunk::chunk_text(&combined, threshold, chunk_size),
        preview: combined.chars().take(800).collect(),
    };
    let raw = analyze_parsed(
        app, cfg, &file_name, template_content, &parsed, output_format, task_id, 0, 1, retry_count, sem, cancel.clone(),
    )
    .await;
    let duration_ms = start.elapsed().as_millis() as u64;
    let mut result = match raw {
        Ok(text) => results::parse_result(&file_name, None, "tpl-generic", parsed.chunks.len(), duration_ms, &text, output_format),
        Err(err) => FileResult {
            file: file_name.clone(),
            verdict: "失败".into(),
            error: Some(err),
            chunks: parsed.chunks.len(),
            duration_ms,
            ..Default::default()
        },
    };
    if result.error.is_some() {
        result.verdict = "失败".into();
    }
    emit(app, task_id, 0, 1, &result.file, "done", None, Some(result.clone()));
    result
}

/// 单次模型调用（带并发信号量）；失败按 `retry_count` 自动重试。
#[allow(clippy::too_many_arguments)]
async fn call_with_retry(
    app: &AppHandle,
    cfg: &Arc<GatewayConfig>,
    task_id: &str,
    index: usize,
    total: usize,
    file: &str,
    phase: &str,
    content: &str,
    stream: bool,
    retry_count: usize,
    sem: &Arc<Semaphore>,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let attempts = retry_count + 1;
    let mut last_err = "模型调用失败。".to_string();
    for attempt in 0..attempts {
        if cancel.load(Ordering::Relaxed) {
            return Err("任务已取消".into());
        }
        if attempt > 0 {
            emit(app, task_id, index, total, file, phase, Some("\n[已重试]\n"), None);
        }
        let permit = sem
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "并发控制不可用".to_string())?;
        let result = if stream {
            stream_into(app, cfg, task_id, index, total, file, phase, content, cancel.clone()).await
        } else {
            gateway::chat(cfg, content).await
        };
        drop(permit);
        match result {
            Ok(text) if !text.trim().is_empty() => return Ok(text),
            Ok(_) => last_err = "模型返回为空，请重试。".to_string(),
            Err(err) => {
                if err.contains("取消") {
                    return Err(err);
                }
                last_err = err;
            }
        }
    }
    Err(last_err)
}

#[allow(clippy::too_many_arguments)]
async fn stream_into(
    app: &AppHandle,
    cfg: &GatewayConfig,
    task_id: &str,
    index: usize,
    total: usize,
    file: &str,
    phase: &str,
    content: &str,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let app_for_cb = app.clone();
    let task = task_id.to_string();
    let name = file.to_string();
    let phase = phase.to_string();
    gateway::chat_stream(cfg, content, Some(cancel), move |delta| {
        emit(&app_for_cb, &task, index, total, &name, &phase, Some(delta), None);
    })
    .await
}

fn emit(
    app: &AppHandle,
    task_id: &str,
    index: usize,
    total: usize,
    file: &str,
    phase: &str,
    delta: Option<&str>,
    result: Option<FileResult>,
) {
    let payload = ReviewProgress {
        task_id: task_id.to_string(),
        index,
        total,
        file: file.to_string(),
        phase: phase.to_string(),
        delta: delta.map(str::to_string),
        result,
    };
    let _ = app.emit("review://progress", payload);
}

fn tally(results: &[FileResult]) -> (u64, u64, u64, u64) {
    let (mut pass, mut conditional, mut block, mut fail) = (0, 0, 0, 0);
    for r in results {
        if r.error.is_some() {
            fail += 1;
            continue;
        }
        match r.verdict.as_str() {
            "通过" => pass += 1,
            "有条件通过" => conditional += 1,
            "阻断" => block += 1,
            _ => fail += 1,
        }
    }
    (pass, conditional, block, fail)
}

/// 按系统 LLM 汇总（不含跨系统全局汇总）。
#[tauri::command]
pub async fn summarize_system(
    cfg: GatewayConfig,
    api_key: Option<String>,
    system_name: String,
    files: Vec<SystemFileSummary>,
) -> Result<String, String> {
    let mut cfg = cfg;
    if let Some(key) = api_key {
        if !key.trim().is_empty() {
            cfg.api_key = key;
        }
    }
    if cfg.api_key.trim().is_empty() {
        cfg.api_key = secrets::load_api_key();
    }
    let mut body = format!("以下是「{system_name}」系统下各文件的评审结论：\n\n");
    for f in &files {
        body.push_str(&format!("- {}：结论 {}\n", f.file, f.verdict));
        for risk in &f.risks {
            body.push_str(&format!("  · 风险：{risk}\n"));
        }
    }
    body.push_str(
        "\n请汇总为一份系统级评审摘要：总体结论、主要风险（按优先级）、上线前必须完成的整改项。不要臆测未提供的信息。",
    );
    gateway::chat(&cfg, &body).await
}

#[tauri::command]
pub fn export_xlsx(path: String, rows: Vec<ExportRow>) -> Result<(), String> {
    export::write_xlsx(&path, &rows)
}

#[tauri::command]
pub fn save_task(
    state: State<'_, AppState>,
    source_type: String,
    source_ref: String,
    rule_set: RuleSet,
    templates: Vec<crate::model::Template>,
    summary: ReviewSummary,
) -> Result<(), String> {
    let systems: Vec<String> = summary
        .results
        .iter()
        .filter_map(|r| r.system.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let task = TaskSummary {
        id: summary.task_id.clone(),
        created_at: chrono::Utc::now().timestamp_millis(),
        source_type,
        source_ref,
        systems,
        total_files: summary.results.len() as u64,
        pass: summary.pass,
        conditional: summary.conditional,
        block: summary.block,
        fail: summary.fail,
    };
    store::save_task(&state.db_path, &task, &summary.results, &rule_set, &templates)
}

/// 内置系统、模板与规则集默认值，供「恢复内置」使用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    pub systems: Vec<SystemType>,
    pub templates: Vec<Template>,
    pub rule_sets: Vec<RuleSet>,
}

#[tauri::command]
pub fn load_defaults() -> Defaults {
    Defaults {
        systems: rules::default_systems(),
        templates: rules::default_templates(),
        rule_sets: rules::default_rule_sets(),
    }
}

#[tauri::command]
pub fn list_history(state: State<'_, AppState>) -> Result<Vec<TaskSummary>, String> {
    let cfg = config::load(&state.config_path);
    if cfg.history_retention_days > 0 {
        let _ = store::prune(&state.db_path, cfg.history_retention_days);
    }
    store::list_tasks(&state.db_path)
}

#[tauri::command]
pub fn get_history(state: State<'_, AppState>, id: String) -> Result<Option<store::TaskDetail>, String> {
    store::get_task(&state.db_path, &id)
}

#[tauri::command]
pub fn delete_history(state: State<'_, AppState>, id: String) -> Result<(), String> {
    store::delete_task(&state.db_path, &id)
}
