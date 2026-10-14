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
    mut config: AppConfig,
    api_key: Option<String>,
) -> Result<(), String> {
    config.normalize_and_validate()?;
    if let Some(key) = api_key {
        if !key.trim().is_empty() {
            secrets::save_api_key(&key)?;
        }
    }
    config::save(&state.config_path, &config)
}

#[tauri::command]
pub fn has_api_key() -> bool {
    secrets::has_api_key()
}

#[tauri::command]
pub fn clear_api_key() -> Result<(), String> {
    secrets::save_api_key("")
}

/// 原子登记扫描/评审，异步后仍保持互斥，避免清理正在使用的工作区。
fn reserve_task(state: &AppState, task_id: &str) -> Result<Arc<AtomicBool>, String> {
    let mut active = state
        .cancels
        .lock()
        .map_err(|_| "任务状态不可用".to_string())?;
    if !active.is_empty() {
        return Err("扫描或评审正在运行，请先取消或等待完成。".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    active.insert(task_id.into(), cancel.clone());
    Ok(cancel)
}

enum ScanSource {
    Directory(String),
    Uploads(Vec<String>),
    Text { content: String, system: String },
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // IPC 参数保持兼容。
pub async fn scan_directory(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    rule_set: RuleSet,
    recursive: bool,
    matched_only: bool,
    threshold: usize,
    chunk_size: usize,
    request_id: Option<String>,
) -> Result<ScanResult, String> {
    run_scan(
        app,
        state,
        ScanSource::Directory(path),
        rule_set,
        recursive,
        matched_only,
        threshold,
        chunk_size,
        request_id,
    )
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // IPC 参数保持兼容。
pub async fn scan_uploads(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    rule_set: RuleSet,
    matched_only: bool,
    threshold: usize,
    chunk_size: usize,
    request_id: Option<String>,
) -> Result<ScanResult, String> {
    run_scan(
        app,
        state,
        ScanSource::Uploads(paths),
        rule_set,
        true,
        matched_only,
        threshold,
        chunk_size,
        request_id,
    )
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // IPC 参数保持兼容。
pub async fn scan_text(
    app: AppHandle,
    state: State<'_, AppState>,
    content: String,
    system: String,
    template_id: String,
    rule_set: RuleSet,
    threshold: usize,
    chunk_size: usize,
    request_id: Option<String>,
) -> Result<ScanResult, String> {
    // 保留 IPC 参数兼容旧客户端；真实规范只由模块决定。
    let _ = template_id;
    run_scan(
        app,
        state,
        ScanSource::Text { content, system },
        rule_set,
        false,
        false,
        threshold,
        chunk_size,
        request_id,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    source: ScanSource,
    rule_set: RuleSet,
    recursive: bool,
    matched_only: bool,
    threshold: usize,
    chunk_size: usize,
    request_id: Option<String>,
) -> Result<ScanResult, String> {
    rules::validate(&rule_set)?;
    let task_id = request_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    // 请求 ID 同时用于事件关联及目录名称，只接受 UUID。
    Uuid::parse_str(&task_id).map_err(|_| "扫描请求 ID 无效".to_string())?;
    let cancel = reserve_task(&state, &task_id)?;
    let root = state.workspace_root.clone();
    let config_path = state.config_path.clone();
    let worker_id = task_id.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let mut progress = scan::initial_progress(worker_id.clone());
        let mut last_event = Instant::now();
        let mut last_phase = String::new();
        let mut observer = |p: &scan::ScanProgress| -> Result<(), String> {
            if cancel.load(Ordering::Relaxed) {
                return Err("扫描已取消".into());
            }
            if p.phase != last_phase || last_event.elapsed().as_millis() >= 75 {
                let _ = app.emit("scan://progress", p);
                last_event = Instant::now();
                last_phase = p.phase.clone();
            }
            Ok(())
        };
        observer(&progress)?;
        let cfg = config::load(&config_path);
        let mut rule_set = rule_set;
        cfg.normalize_rule_set(&mut rule_set)?;
        let opts = scan_opts(&cfg, recursive, matched_only, threshold, chunk_size);
        let workspace = fresh_workspace(&root, &worker_id)?;
        let result = (|| {
            let (source_type, source_ref, entries) = match source {
                ScanSource::Directory(path) => {
                    let entries = scan::scan_directory_with_progress(
                        PathBuf::from(&path).as_path(),
                        &rule_set,
                        &workspace,
                        &opts,
                        &mut progress,
                        &mut observer,
                    )?;
                    ("dir", path, entries)
                }
                ScanSource::Uploads(paths) => {
                    let entries = scan::scan_uploads_with_progress(
                        &paths,
                        &rule_set,
                        &workspace,
                        &opts,
                        &mut progress,
                        &mut observer,
                    )?;
                    let source_ref = if paths.len() == 1 {
                        PathBuf::from(&paths[0])
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string()
                    } else {
                        format!("{} 个文件", paths.len())
                    };
                    ("upload", source_ref, entries)
                }
                ScanSource::Text { content, system } => {
                    let template_id = cfg.module_template(&system)?.id.clone();
                    if content.trim().is_empty() {
                        return Err("内容为空".into());
                    }
                    if content.len() as u64 > opts.max_file_bytes {
                        return Err("粘贴内容超过单文件大小上限".into());
                    }
                    let name = format!("pasted-{}.txt", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
                    let abs = workspace.join(&name);
                    std::fs::write(&abs, content.as_bytes()).map_err(|e| e.to_string())?;
                    progress.phase = "parse".into();
                    progress.discovered = 1;
                    progress.total = Some(1);
                    progress.current = name.clone();
                    observer(&progress)?;
                    let parsed = parse::parse_file(&abs, "text", opts.threshold, opts.chunk_size)?;
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
                    progress.processed = 1;
                    progress.matched = 1;
                    progress.phase = "done".into();
                    observer(&progress)?;
                    ("text", name, vec![entry])
                }
            };
            build_result(
                source_type,
                &source_ref,
                worker_id,
                workspace.clone(),
                entries,
                rule_set,
                &cfg,
            )
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&workspace);
        }
        result
    })
    .await
    .map_err(|e| format!("扫描工作线程异常：{e}"));
    state
        .cancels
        .lock()
        .map_err(|_| "任务状态不可用".to_string())?
        .remove(&task_id);
    outcome?
}

#[tauri::command]
pub fn cancel_scan(state: State<'_, AppState>, task_id: String) {
    if let Some(flag) = state.cancels.lock().unwrap().get(&task_id) {
        flag.store(true, Ordering::Relaxed);
    }
}

fn scan_opts(
    cfg: &AppConfig,
    recursive: bool,
    matched_only: bool,
    threshold: usize,
    chunk_size: usize,
) -> ScanOptions {
    ScanOptions {
        recursive,
        matched_only,
        threshold: threshold.max(1000),
        chunk_size: chunk_size.max(500),
        max_file_bytes: cfg.max_file_mb.saturating_mul(1024 * 1024),
        max_files: cfg.max_files,
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
        return Err(format!(
            "文件数 {} 超过上限 {}",
            entries.len(),
            cfg.max_files
        ));
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
pub async fn test_connection(
    cfg: GatewayConfig,
    api_key: Option<String>,
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
    mut request: ReviewRequest,
) -> Result<ReviewSummary, String> {
    let task_id = if request.task_id.trim().is_empty() {
        Uuid::new_v4().to_string()
    } else {
        request.task_id.clone()
    };
    let cancel = reserve_task(&state, &task_id)?;

    let outcome = match config::load(&state.config_path).prepare_review(&mut request) {
        Ok(()) => run_review_inner(&app, &state, &request, &task_id, &cancel).await,
        Err(error) => Err(error),
    };

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
    cfg.session_id = task_id.to_string();
    cfg.api_key = secrets::load_api_key();
    if cfg.api_key.trim().is_empty() {
        return Err("未配置 API Key，请在「设置」中填写。".into());
    }
    if cfg.base_url.trim().is_empty() || cfg.model.trim().is_empty() {
        return Err("请先配置 Base URL 与模型标识。".into());
    }
    let cfg = Arc::new(cfg);
    let concurrency = request.concurrency.clamp(1, 8);
    let threshold = request.chunk_threshold.max(1000);
    let chunk_size = request.chunk_size.max(500);
    let retry_count = request.retry_count.min(5);
    let output_format = request
        .files
        .first()
        .map(|f| f.output_format.clone())
        .unwrap_or_else(|| "standard".into());
    // 统一分析单元信号量：文件级与块级共享同一并发上限。
    let sem = Arc::new(Semaphore::new(concurrency));

    let mut all: Vec<FileResult> = Vec::new();
    let total = request.files.len();

    if request.granularity == "single" {
        let result = review_combined(
            app,
            &cfg,
            &request.files,
            threshold,
            chunk_size,
            &output_format,
            task_id,
            retry_count,
            &sem,
            cancel,
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
            let output_format = file.output_format.clone();
            async move {
                review_one(
                    &app,
                    &cfg,
                    file,
                    threshold,
                    chunk_size,
                    &output_format,
                    &task_id,
                    index,
                    total,
                    retry_count,
                    &sem,
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

async fn parse_in_background(
    path: PathBuf,
    kind: String,
    threshold: usize,
    chunk_size: usize,
) -> Result<parse::Parsed, String> {
    tauri::async_runtime::spawn_blocking(move || {
        parse::parse_file(&path, &kind, threshold, chunk_size)
    })
    .await
    .map_err(|e| format!("解析工作线程异常：{e}"))?
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
    emit(
        app,
        task_id,
        index,
        total,
        &file.name,
        "parse",
        Some("正在解析…"),
        None,
    );

    let path = PathBuf::from(&file.abs_path);
    // 解析也占用并发额度，避免数百个大文档同时进入阻塞线程池。
    let permit = sem.acquire().await.expect("review semaphore stays open");
    let parsed_result = if cancel.load(Ordering::Relaxed) {
        Err("已取消".into())
    } else {
        parse_in_background(path, file.kind.clone(), threshold, chunk_size).await
    };
    drop(permit);
    let parsed = match parsed_result {
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
            emit(
                app,
                task_id,
                index,
                total,
                &file.name,
                "done",
                None,
                Some(result.clone()),
            );
            return result;
        }
    };

    let raw = analyze_parsed(
        app,
        cfg,
        &file.name,
        &file.template_content,
        &parsed,
        output_format,
        task_id,
        index,
        total,
        retry_count,
        sem,
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
    emit(
        app,
        task_id,
        index,
        total,
        &file.name,
        "done",
        None,
        Some(result.clone()),
    );
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
        let instruction =
            results::build_instruction(template_content, file_name, None, output_format);
        let content = format!("{instruction}\n\n待评审内容：\n{}", parsed.text);
        return call_with_retry(
            app,
            cfg,
            task_id,
            index,
            total,
            file_name,
            "analyze",
            &content,
            true,
            retry_count,
            sem,
            cancel,
        )
        .await;
    }

    let total_chunks = parsed.chunks.len();
    let mut futures = Vec::with_capacity(total_chunks);
    for (i, chunk) in parsed.chunks.iter().enumerate() {
        let instruction = results::build_instruction(
            template_content,
            file_name,
            Some((i + 1, total_chunks)),
            output_format,
        );
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
                &app,
                &cfg,
                &task_id,
                index,
                total,
                &file_name,
                &phase,
                &content,
                false,
                retry_count,
                &sem,
                cancel,
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
            Err(err) => return Err(format!("第 {} 块分析失败，文件评审未完成：{err}", i + 1)),
        }
    }
    let merge = format!(
        "{}\n以下是对同一文件分块评审得到的结论。请合并为一份完整、无重复的评审结论，保留引用。\n\n{}",
        results::build_instruction(template_content, file_name, None, output_format),
        chunk_outputs.join("\n\n---\n\n")
    );
    call_with_retry(
        app,
        cfg,
        task_id,
        index,
        total,
        file_name,
        "merge",
        &merge,
        true,
        retry_count,
        sem,
        cancel,
    )
    .await
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
        emit(
            app,
            task_id,
            i,
            total,
            &file.name,
            "parse",
            Some("正在解析…"),
            None,
        );
        match parse_in_background(
            PathBuf::from(&file.abs_path),
            file.kind.clone(),
            threshold,
            chunk_size,
        )
        .await
        {
            Ok(p) => {
                parsed_any += 1;
                combined.push_str(&format!(
                    "\n\n===== 文件：{} =====\n评审要求：{}\n{}",
                    file.path, file.template_content, p.text
                ));
            }
            Err(err) => {
                let result = FileResult {
                    file: format!("全部文件合并评审（{} 个）", files.len()),
                    verdict: "失败".into(),
                    error: Some(format!(
                        "文件 {} 解析失败，合并评审未完成：{err}",
                        file.path
                    )),
                    duration_ms: start.elapsed().as_millis() as u64,
                    ..Default::default()
                };
                emit(
                    app,
                    task_id,
                    0,
                    1,
                    &result.file,
                    "done",
                    None,
                    Some(result.clone()),
                );
                return result;
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
        emit(
            app,
            task_id,
            0,
            1,
            &result.file,
            "done",
            None,
            Some(result.clone()),
        );
        return result;
    }
    let template_content = files
        .first()
        .map(|f| f.template_content.as_str())
        .unwrap_or("");
    let parsed = parse::Parsed {
        text: combined.clone(),
        chunks: chunk::chunk_text(&combined, threshold, chunk_size),
        preview: combined.chars().take(800).collect(),
    };
    let raw = analyze_parsed(
        app,
        cfg,
        &file_name,
        template_content,
        &parsed,
        output_format,
        task_id,
        0,
        1,
        retry_count,
        sem,
        cancel.clone(),
    )
    .await;
    let duration_ms = start.elapsed().as_millis() as u64;
    let mut result = match raw {
        Ok(text) => results::parse_result(
            &file_name,
            files.first().and_then(|f| f.system.clone()),
            files.first().map(|f| f.template_id.as_str()).unwrap_or(""),
            parsed.chunks.len(),
            duration_ms,
            &text,
            output_format,
        ),
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
    emit(
        app,
        task_id,
        0,
        1,
        &result.file,
        "done",
        None,
        Some(result.clone()),
    );
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
        let permit = gateway::cancellable(sem.clone().acquire_owned(), Some(&cancel))
            .await?
            .map_err(|_| "并发控制不可用".to_string())?;
        let waiting = if attempt == 0 {
            "\n正在等待模型响应…\n".to_string()
        } else {
            format!("\n第 {attempt}/{retry_count} 次重试，正在等待模型响应…\n")
        };
        emit(
            app,
            task_id,
            index,
            total,
            file,
            "waiting",
            Some(&waiting),
            None,
        );
        let result = if stream {
            stream_into(
                app,
                cfg,
                task_id,
                index,
                total,
                file,
                phase,
                content,
                cancel.clone(),
            )
            .await
        } else {
            gateway::cancellable(gateway::chat(cfg, content), Some(&cancel)).await?
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
        emit(
            &app_for_cb,
            &task,
            index,
            total,
            &name,
            &phase,
            Some(delta),
            None,
        );
    })
    .await
}

#[allow(clippy::too_many_arguments)]
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
pub async fn export_xlsx(path: String, rows: Vec<ExportRow>) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || export::write_xlsx(&path, &rows))
        .await
        .map_err(|e| format!("导出工作线程异常：{e}"))?
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
    store::save_task(
        &state.db_path,
        &task,
        &summary.results,
        &rule_set,
        &templates,
    )
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
pub fn get_history(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<store::TaskDetail>, String> {
    store::get_task(&state.db_path, &id)
}

#[tauri::command]
pub fn delete_history(state: State<'_, AppState>, id: String) -> Result<(), String> {
    store::delete_task(&state.db_path, &id)
}

#[tauri::command]
pub async fn export_text(path: String, content: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::write(path, content.as_bytes()).map_err(|e| format!("导出失败：{e}"))
    })
    .await
    .map_err(|e| format!("导出工作线程异常：{e}"))?
}

#[cfg(test)]
mod task_tests {
    use super::*;

    #[test]
    fn scan_and_review_share_atomic_reservation_and_release() {
        let state = AppState {
            db_path: PathBuf::new(),
            workspace_root: PathBuf::new(),
            config_path: PathBuf::new(),
            cancels: Mutex::new(HashMap::new()),
        };
        let flag = reserve_task(&state, "scan").unwrap();
        assert!(reserve_task(&state, "review").is_err());
        assert!(reserve_task(&state, "another-scan").is_err());
        state
            .cancels
            .lock()
            .unwrap()
            .get("scan")
            .unwrap()
            .store(true, Ordering::Relaxed);
        assert!(flag.load(Ordering::Relaxed));
        state.cancels.lock().unwrap().remove("scan");
        assert!(reserve_task(&state, "next").is_ok());
    }
}
