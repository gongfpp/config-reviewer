use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::gateway;
use crate::model::{
    FileResult, GatewayConfig, ReviewFileInput, ReviewProgress, ReviewRequest, RuleSet, ScanResult,
    SystemType, Template,
};
use crate::parse;
use crate::results;
use crate::rules;
use crate::scan::{self, ScanOptions};
use crate::store::{self, TaskSummary};

pub struct AppState {
    pub db_path: PathBuf,
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    pub systems: Vec<SystemType>,
    pub templates: Vec<Template>,
    pub rule_sets: Vec<RuleSet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub task_id: String,
    pub results: Vec<FileResult>,
    pub pass: u64,
    pub conditional: u64,
    pub block: u64,
    pub fail: u64,
}

#[tauri::command]
pub fn get_defaults() -> Defaults {
    Defaults {
        systems: rules::default_systems(),
        templates: rules::default_templates(),
        rule_sets: rules::default_rule_sets(),
    }
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
    let task_id = Uuid::new_v4().to_string();
    let workspace = state.workspace_root.join(&task_id);
    std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
    let opts = ScanOptions {
        recursive,
        matched_only,
        threshold,
        chunk_size,
    };
    let entries = scan::scan_directory(PathBuf::from(&path).as_path(), &rule_set, &workspace, &opts)?;
    Ok(build_result("dir", &path, task_id, workspace, entries, rule_set))
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
    let task_id = Uuid::new_v4().to_string();
    let workspace = state.workspace_root.join(&task_id);
    std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
    let opts = ScanOptions {
        recursive: true,
        matched_only,
        threshold,
        chunk_size,
    };
    let entries = scan::scan_uploads(&paths, &rule_set, &workspace, &opts)?;
    let source_ref = if paths.len() == 1 {
        PathBuf::from(&paths[0])
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        format!("{} 个文件", paths.len())
    };
    Ok(build_result("upload", &source_ref, task_id, workspace, entries, rule_set))
}

fn build_result(
    source_type: &str,
    source_ref: &str,
    task_id: String,
    workspace: PathBuf,
    entries: Vec<crate::model::ScanEntry>,
    rule_set: RuleSet,
) -> ScanResult {
    let total_files = entries.len() as u64;
    let matched_files = entries.iter().filter(|e| e.matched).count() as u64;
    let total_bytes = entries.iter().map(|e| e.size).sum();
    ScanResult {
        task_id,
        source_type: source_type.into(),
        source_ref: source_ref.into(),
        total_files,
        matched_files,
        total_bytes,
        entries,
        workspace: workspace.to_string_lossy().to_string(),
        rule_set,
        templates: rules::default_templates(),
        systems: rules::default_systems(),
    }
}

#[tauri::command]
pub async fn test_connection(cfg: GatewayConfig) -> Result<String, String> {
    gateway::test_connection(&cfg).await
}

#[tauri::command]
pub async fn run_review(
    app: AppHandle,
    _state: State<'_, AppState>,
    request: ReviewRequest,
) -> Result<ReviewSummary, String> {
    let task_id = Uuid::new_v4().to_string();
    let concurrency = request.concurrency.clamp(1, 8);
    let threshold = request.chunk_threshold.max(1000);
    let chunk_size = request.chunk_size.max(500);
    let cfg = Arc::new(request.cfg.clone());

    let mut all: Vec<FileResult> = Vec::new();
    let total = request.files.len();
    let mut base_index = 0usize;

    for batch in request.files.chunks(concurrency) {
        let task_id = task_id.clone();
        let futures = batch.iter().enumerate().map(|(offset, file)| {
            let app = app.clone();
            let cfg = cfg.clone();
            let task_id = task_id.clone();
            let index = base_index + offset;
            async move {
                review_one(&app, &cfg, file, threshold, chunk_size, &task_id, index, total).await
            }
        });
        let results = join_all(futures).await;
        all.extend(results);
        base_index += batch.len();
    }

    let (pass, conditional, block, fail) = tally(&all);
    let summary = ReviewSummary {
        task_id: task_id.clone(),
        results: all,
        pass,
        conditional,
        block,
        fail,
    };
    Ok(summary)
}

async fn review_one(
    app: &AppHandle,
    cfg: &GatewayConfig,
    file: &ReviewFileInput,
    threshold: usize,
    chunk_size: usize,
    task_id: &str,
    index: usize,
    total: usize,
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
        stream_into(app, cfg, task_id, index, total, &file.name, "analyze", &content).await
    } else {
        let mut chunk_outputs: Vec<String> = Vec::new();
        for (i, chunk) in parsed.chunks.iter().enumerate() {
            let instr = results::build_instruction(&file.template_content, &file.name, Some((i + 1, parsed.chunks.len())));
            let content = format!("{instr}\n\n第 {} 块内容：\n{chunk}", i + 1);
            match stream_into(app, cfg, task_id, index, total, &file.name, "analyze", &content).await {
                Ok(text) => chunk_outputs.push(format!("【第 {} 块】\n{}", i + 1, text)),
                Err(err) => chunk_outputs.push(format!("【第 {} 块】分析失败：{err}", i + 1)),
            }
        }
        let merge = format!(
            "以下是对同一文件分块评审得到的结论。请合并为一份完整、无重复的评审结论，仍以“通过/有条件通过/阻断”开头，并保留【依据】【风险】【整改项】与引用。\n\n{}",
            chunk_outputs.join("\n\n---\n\n")
        );
        stream_into(app, cfg, task_id, index, total, &file.name, "merge", &merge).await
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

async fn stream_into(
    app: &AppHandle,
    cfg: &GatewayConfig,
    task_id: &str,
    index: usize,
    total: usize,
    file: &str,
    phase: &str,
    content: &str,
) -> Result<String, String> {
    let app_for_cb = app.clone();
    let task = task_id.to_string();
    let name = file.to_string();
    let phase = phase.to_string();
    gateway::chat_stream(cfg, content, move |delta| {
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

#[tauri::command]
pub fn save_task(
    state: State<'_, AppState>,
    source_type: String,
    source_ref: String,
    rule_set: RuleSet,
    templates: Vec<Template>,
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
