use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::config::{self, AppConfig};
use crate::export::{self, ExportRow};
use crate::gateway;
use crate::model::{
    FileResult, GatewayConfig, ReviewFileInput, ReviewProgress, ReviewRequest, RuleSet, ScanEntry,
    ScanResult,
};
use crate::parse;
use crate::results;
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
    let workspace = state.workspace_root.join(&task_id);
    std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
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
    let workspace = state.workspace_root.join(&task_id);
    std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
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
    let workspace = state.workspace_root.join(&task_id);
    std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
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
    if !request.workspace.trim().is_empty() {
        let _ = std::fs::remove_dir_all(&request.workspace);
    }
    outcome
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

    let mut all: Vec<FileResult> = Vec::new();
    let total = request.files.len();
    let mut base_index = 0usize;
    let mut cancelled = false;

    for batch in request.files.chunks(concurrency) {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let task_id = task_id.to_string();
        let cancel_flag = cancel.clone();
        let futures = batch.iter().enumerate().map(|(offset, file)| {
            let app = app.clone();
            let cfg = cfg.clone();
            let task_id = task_id.clone();
            let cancel_flag = cancel_flag.clone();
            let index = base_index + offset;
            async move {
                review_one(&app, &cfg, file, threshold, chunk_size, &task_id, index, total, cancel_flag).await
            }
        });
        let results = join_all(futures).await;
        all.extend(results);
        base_index += batch.len();
    }

    if cancel.load(Ordering::Relaxed) {
        cancelled = true;
    }
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

#[allow(clippy::too_many_arguments)]
async fn review_one(
    app: &AppHandle,
    cfg: &GatewayConfig,
    file: &ReviewFileInput,
    threshold: usize,
    chunk_size: usize,
    task_id: &str,
    index: usize,
    total: usize,
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

    let instruction = results::build_instruction(&file.template_content, &file.name, None);
    let raw = if parsed.chunks.len() <= 1 {
        let content = format!("{instruction}\n\n待评审内容：\n{}", parsed.text);
        stream_into(app, cfg, task_id, index, total, &file.name, "analyze", &content, cancel.clone()).await
    } else {
        let mut chunk_outputs: Vec<String> = Vec::new();
        for (i, chunk) in parsed.chunks.iter().enumerate() {
            let instr = results::build_instruction(&file.template_content, &file.name, Some((i + 1, parsed.chunks.len())));
            let content = format!("{instr}\n\n第 {} 块内容：\n{chunk}", i + 1);
            match stream_into(app, cfg, task_id, index, total, &file.name, "analyze", &content, cancel.clone()).await {
                Ok(text) => chunk_outputs.push(format!("【第 {} 块】\n{}", i + 1, text)),
                Err(err) => chunk_outputs.push(format!("【第 {} 块】分析失败：{err}", i + 1)),
            }
            if cancel.load(Ordering::Relaxed) {
                break;
            }
        }
        let merge = format!(
            "以下是对同一文件分块评审得到的结论。请合并为一份完整、无重复的评审结论，仍以“通过/有条件通过/阻断”开头，并保留【依据】【风险】【整改项】与引用。\n\n{}",
            chunk_outputs.join("\n\n---\n\n")
        );
        stream_into(app, cfg, task_id, index, total, &file.name, "merge", &merge, cancel.clone()).await
    };

    let duration_ms = start.elapsed().as_millis() as u64;
    let mut result = match raw {
        Ok(text) => results::parse_result(
            &file.path,
            file.system.clone(),
            &file.template_id,
            parsed.chunks.len(),
            duration_ms,
            &text,
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

#[tauri::command]
pub fn list_history(state: State<'_, AppState>) -> Result<Vec<TaskSummary>, String> {
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
