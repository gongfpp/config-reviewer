use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::commands::{self, AppState};
use crate::config::{self, AppConfig};
use crate::model::{
    FileResult, GatewayConfig, ReviewFileInput, ReviewProgress, ReviewRequest, RuleSet, ScanEntry,
};
use crate::{git_sync, scan, store};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleConfig {
    pub id: String,
    pub name: String,
    pub directory: String,
    pub interval_minutes: u32,
    pub enabled: bool,
    pub git_pull: bool,
    pub only_changed: bool,
    pub rule_set_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScheduleRun {
    pub task_id: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub state: String,
    pub phase: String,
    pub processed: usize,
    pub total: Option<usize>,
    pub discovered: usize,
    pub matched: usize,
    pub current: String,
    pub message: String,
    pub before_commit: String,
    pub after_commit: String,
    pub history_id: Option<String>,
    pub pass: u64,
    pub conditional: u64,
    pub block: u64,
    pub fail: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    pub config: ScheduleConfig,
    pub next_run_at: Option<i64>,
    pub last_run: Option<ScheduleRun>,
    #[serde(default)]
    pub last_fingerprint: String,
}

struct ActiveRun {
    job_id: String,
    task_id: String,
    completed: HashSet<usize>,
    parse_failures: usize,
    last_emit: Instant,
}

pub struct SchedulerState {
    data: Mutex<SchedulerData>,
}

struct SchedulerData {
    jobs: Vec<ScheduledJob>,
    active: Option<ActiveRun>,
    load_error: Option<String>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn next_run(finished: i64, minutes: u32) -> i64 {
    finished.saturating_add(i64::from(minutes) * 60_000)
}

fn db(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| format!("定时任务数据库无法打开：{e}"))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS schedules (id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL);").map_err(|e| e.to_string())?;
    Ok(conn)
}

fn load_jobs(path: &Path) -> Result<Vec<ScheduledJob>, String> {
    use rusqlite::OptionalExtension;
    let raw: Option<String> = db(path)?
        .query_row("SELECT payload FROM schedules WHERE id=1", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    raw.map(|s| {
        serde_json::from_str(&s).map_err(|e| format!("定时任务记录损坏，已停止自动运行：{e}"))
    })
    .unwrap_or(Ok(Vec::new()))
}

fn persist(path: &Path, jobs: &[ScheduledJob]) -> Result<(), String> {
    let raw = serde_json::to_string(jobs).map_err(|e| e.to_string())?;
    db(path)?.execute("INSERT INTO schedules(id,payload) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", params![raw]).map_err(|e| format!("定时任务保存失败：{e}"))?;
    Ok(())
}

impl SchedulerState {
    pub fn load(path: &Path) -> Self {
        let (mut jobs, mut load_error) = match load_jobs(path) {
            Ok(jobs) => (jobs, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let mut changed = false;
        for job in &mut jobs {
            if let Some(run) = &mut job.last_run {
                if run.state == "running" {
                    run.state = "interrupted".into();
                    run.finished_at = Some(now());
                    run.message = "上次运行在程序退出时中断；下一轮将重新检查资料。".into();
                    job.last_fingerprint.clear();
                    job.next_run_at = job
                        .config
                        .enabled
                        .then(|| next_run(now(), job.config.interval_minutes));
                    changed = true;
                }
            }
        }
        if changed {
            if let Err(error) = persist(path, &jobs) {
                load_error = Some(error);
            }
        }
        Self {
            data: Mutex::new(SchedulerData {
                jobs,
                active: None,
                load_error,
            }),
        }
    }
}

fn publish(app: &AppHandle, jobs: &[ScheduledJob]) {
    let _ = app.emit("schedule://changed", jobs);
}

fn validate(config: &mut ScheduleConfig, app_config: &AppConfig) -> Result<(), String> {
    Uuid::parse_str(&config.id).map_err(|_| "定时任务 ID 无效".to_string())?;
    config.name = config.name.trim().to_string();
    config.directory = config.directory.trim().to_string();
    if config.name.is_empty() || config.name.chars().count() > 80 {
        return Err("任务名称需为 1–80 个字符".into());
    }
    if !(1..=525600).contains(&config.interval_minutes) {
        return Err("执行间隔需为 1–525600 分钟".into());
    }
    if !Path::new(&config.directory).is_absolute() || !Path::new(&config.directory).is_dir() {
        return Err("请选择存在的绝对目录路径".into());
    }
    if !app_config
        .rule_sets
        .iter()
        .any(|r| r.id == config.rule_set_id)
    {
        return Err("文件匹配方案已不存在，请重新选择".into());
    }
    Ok(())
}

#[tauri::command]
pub fn list_schedules(scheduler: State<'_, SchedulerState>) -> Result<Vec<ScheduledJob>, String> {
    let data = scheduler.data.lock().map_err(|_| "定时任务状态不可用")?;
    if let Some(error) = &data.load_error {
        return Err(error.clone());
    }
    Ok(data.jobs.clone())
}

#[tauri::command]
pub fn save_schedule(
    app: AppHandle,
    state: State<'_, AppState>,
    scheduler: State<'_, SchedulerState>,
    mut config: ScheduleConfig,
) -> Result<(), String> {
    validate(&mut config, &config::load(&state.config_path))?;
    let mut data = scheduler.data.lock().map_err(|_| "定时任务状态不可用")?;
    if let Some(error) = &data.load_error {
        return Err(error.clone());
    }
    if data.active.as_ref().is_some_and(|a| a.job_id == config.id) {
        return Err("本任务正在执行，请先取消或等待完成再修改".into());
    }
    let mut jobs = data.jobs.clone();
    if let Some(job) = jobs.iter_mut().find(|j| j.config.id == config.id) {
        // 保存不会丢掉最近结果；配置变化在下一次内容签名中重新检查。
        job.next_run_at = config
            .enabled
            .then(|| next_run(now(), config.interval_minutes));
        job.config = config;
    } else {
        jobs.push(ScheduledJob {
            next_run_at: config
                .enabled
                .then(|| next_run(now(), config.interval_minutes)),
            config,
            last_run: None,
            last_fingerprint: String::new(),
        });
    }
    persist(&state.db_path, &jobs)?;
    data.jobs = jobs;
    publish(&app, &data.jobs);
    Ok(())
}

#[tauri::command]
pub fn delete_schedule(
    app: AppHandle,
    state: State<'_, AppState>,
    scheduler: State<'_, SchedulerState>,
    id: String,
) -> Result<(), String> {
    let mut data = scheduler.data.lock().map_err(|_| "定时任务状态不可用")?;
    if let Some(error) = &data.load_error {
        return Err(error.clone());
    }
    if data.active.as_ref().is_some_and(|a| a.job_id == id) {
        return Err("请先取消正在执行的任务".into());
    }
    let jobs: Vec<_> = data
        .jobs
        .iter()
        .filter(|j| j.config.id != id)
        .cloned()
        .collect();
    persist(&state.db_path, &jobs)?;
    data.jobs = jobs;
    publish(&app, &data.jobs);
    Ok(())
}

#[tauri::command]
pub fn run_schedule_now(app: AppHandle, id: String) -> Result<(), String> {
    launch(&app, &id)
}

#[tauri::command]
pub fn cancel_schedule(
    state: State<'_, AppState>,
    scheduler: State<'_, SchedulerState>,
    id: String,
) -> Result<(), String> {
    let data = scheduler.data.lock().map_err(|_| "定时任务状态不可用")?;
    if let Some(active) = data.active.as_ref().filter(|a| a.job_id == id) {
        if let Some(flag) = state
            .cancels
            .lock()
            .map_err(|_| "任务状态不可用")?
            .get(&active.task_id)
        {
            flag.store(true, Ordering::Relaxed);
        }
    }
    Ok(())
}

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            let scheduler = app.state::<SchedulerState>();
            let due = {
                let Ok(data) = scheduler.data.lock() else {
                    continue;
                };
                if data.active.is_some() || data.load_error.is_some() {
                    continue;
                }
                data.jobs
                    .iter()
                    .filter(|j| j.config.enabled && j.next_run_at.is_some_and(|t| t <= now()))
                    .min_by_key(|j| j.next_run_at)
                    .map(|j| j.config.id.clone())
            };
            if let Some(id) = due {
                let _ = launch(&app, &id);
            }
        }
    });
}

/// Let cancellation reach Git's process-tree cleanup before ending the runtime.
pub fn request_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.shutting_down.swap(true, Ordering::Relaxed) {
        return;
    }
    if let Ok(flags) = state.cancels.lock() {
        for flag in flags.values() {
            flag.store(true, Ordering::Relaxed);
        }
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let empty = app
                .state::<AppState>()
                .cancels
                .lock()
                .map(|m| m.is_empty())
                .unwrap_or(true);
            let git_running = app
                .state::<SchedulerState>()
                .data
                .lock()
                .map(|d| {
                    d.active.as_ref().is_some_and(|a| {
                        d.jobs.iter().any(|j| {
                            j.config.id == a.job_id
                                && j.last_run.as_ref().is_some_and(|r| r.phase == "git")
                        })
                    })
                })
                .unwrap_or(false);
            if empty || (Instant::now() >= deadline && !git_running) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        app.exit(0);
    });
}

fn launch(app: &AppHandle, id: &str) -> Result<(), String> {
    let scheduler = app.state::<SchedulerState>();
    let state = app.state::<AppState>();
    let mut data = scheduler.data.lock().map_err(|_| "定时任务状态不可用")?;
    if let Some(error) = &data.load_error {
        return Err(error.clone());
    }
    if data.active.is_some() {
        return Err("定时任务正在执行，请等待完成".into());
    }
    let index = data
        .jobs
        .iter()
        .position(|j| j.config.id == id)
        .ok_or("定时任务不存在")?;
    let job = data.jobs[index].clone();
    let task_id = Uuid::new_v4().to_string();
    let cancel = commands::reserve_task(&state, &task_id)?;
    let run = ScheduleRun {
        task_id: task_id.clone(),
        started_at: now(),
        state: "running".into(),
        phase: if job.config.git_pull {
            "git"
        } else {
            "discover"
        }
        .into(),
        message: "准备开始".into(),
        ..Default::default()
    };
    let mut jobs = data.jobs.clone();
    jobs[index].last_run = Some(run);
    if let Err(error) = persist(&state.db_path, &jobs) {
        state.cancels.lock().unwrap().remove(&task_id);
        return Err(error);
    }
    data.jobs = jobs;
    data.active = Some(ActiveRun {
        job_id: id.into(),
        task_id: task_id.clone(),
        completed: HashSet::new(),
        parse_failures: 0,
        last_emit: Instant::now(),
    });
    publish(app, &data.jobs);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = std::panic::AssertUnwindSafe(execute(&app, &job, &task_id, &cancel))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err("定时任务线程异常，下一轮将重试。".into()));
        finish(&app, &job.config.id, &task_id, &cancel, result);
    });
    Ok(())
}

fn update(app: &AppHandle, task_id: &str, force: bool, f: impl FnOnce(&mut ScheduleRun)) {
    let scheduler = app.state::<SchedulerState>();
    let Ok(mut data) = scheduler.data.lock() else {
        return;
    };
    let Some(active) = data.active.as_mut().filter(|a| a.task_id == task_id) else {
        return;
    };
    let emit = force || active.last_emit.elapsed() >= Duration::from_millis(100);
    if emit {
        active.last_emit = Instant::now();
    }
    let job_id = active.job_id.clone();
    if let Some(run) = data
        .jobs
        .iter_mut()
        .find(|j| j.config.id == job_id)
        .and_then(|j| j.last_run.as_mut())
    {
        f(run);
    }
    if emit {
        publish(app, &data.jobs);
    }
}

pub fn observe_review(app: &AppHandle, progress: &ReviewProgress) {
    let Some(scheduler) = app.try_state::<SchedulerState>() else {
        return;
    };
    let Ok(mut data) = scheduler.data.lock() else {
        return;
    };
    let Some(active) = data
        .active
        .as_mut()
        .filter(|a| a.task_id == progress.task_id)
    else {
        return;
    };
    if progress.result.is_some() {
        active.completed.insert(progress.index);
    }
    let processed = active.completed.len() + active.parse_failures;
    let total = progress.total + active.parse_failures;
    let emit =
        progress.result.is_some() || active.last_emit.elapsed() >= Duration::from_millis(150);
    if emit {
        active.last_emit = Instant::now();
    }
    let job_id = active.job_id.clone();
    if let Some(run) = data
        .jobs
        .iter_mut()
        .find(|j| j.config.id == job_id)
        .and_then(|j| j.last_run.as_mut())
    {
        run.phase = "review".into();
        run.processed = processed;
        run.total = Some(total);
        run.current = progress.file.clone();
    }
    if emit {
        publish(app, &data.jobs);
    }
}

struct Completion {
    state: &'static str,
    message: String,
    fingerprint: Option<String>,
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("任务已取消".into())
    } else {
        Ok(())
    }
}

fn stable_path(path: &str) -> &str {
    if path.len() > 37
        && path.as_bytes().get(36) == Some(&b'-')
        && path.is_char_boundary(36)
        && Uuid::parse_str(&path[..36]).is_ok()
    {
        &path[37..]
    } else {
        path
    }
}

fn fingerprint(
    config: &AppConfig,
    entries: &[ScanEntry],
    cancel: &AtomicBool,
) -> Result<String, String> {
    let mut digest = Sha256::new();
    let mut relevant = config.clone();
    relevant.history_retention_days = 0;
    relevant.concurrency = 1;
    relevant.granularity = "per_file".into();
    digest.update(serde_json::to_vec(&relevant).map_err(|e| e.to_string())?);
    let mut records = Vec::with_capacity(entries.len());
    for entry in entries {
        check_cancel(cancel)?;
        let mut record = Sha256::new();
        record.update(stable_path(&entry.path).as_bytes());
        record.update([0]);
        record.update(entry.system.as_deref().unwrap_or("").as_bytes());
        record.update([0]);
        record.update(entry.template_id.as_deref().unwrap_or("").as_bytes());
        record.update([0]);
        let mut file = std::fs::File::open(&entry.abs_path)
            .map_err(|e| format!("读取 {} 失败：{e}", entry.name))?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            record.update(&buffer[..n]);
        }
        records.push(record.finalize());
    }
    // 同名 ZIP 的稳定显示名可能相同；按完整内容摘要排序消除遍历顺序影响。
    records.sort();
    for record in records {
        digest.update(record);
    }
    Ok(format!("{:x}", digest.finalize()))
}

async fn execute(
    app: &AppHandle,
    job: &ScheduledJob,
    task_id: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<Completion, String> {
    let state = app.state::<AppState>();
    let cfg = config::load(&state.config_path);
    let mut rule_set: RuleSet = cfg
        .rule_sets
        .iter()
        .find(|r| r.id == job.config.rule_set_id)
        .cloned()
        .ok_or("匹配方案已删除，请修改定时任务")?;
    cfg.normalize_rule_set(&mut rule_set)?;
    if job.config.git_pull {
        let path = PathBuf::from(&job.config.directory);
        let flag = cancel.clone();
        let git =
            tauri::async_runtime::spawn_blocking(move || git_sync::pull_repository(&path, &flag))
                .await
                .map_err(|e| format!("Git 更新线程异常：{e}"))??;
        update(app, task_id, true, |run| {
            run.before_commit = git.before_commit;
            run.after_commit = git.after_commit;
            run.message = format!(
                "{}：{}",
                if git.updated {
                    "Git 已更新"
                } else {
                    "Git 已是最新"
                },
                git.repo_root.display()
            );
        });
    }
    check_cancel(cancel)?;
    let workspace = state
        .workspace_root
        .with_file_name("scheduled-workspace")
        .join(task_id);
    std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
    let cleanup = WorkspaceCleanup(workspace.clone());
    let opts = scan::ScanOptions {
        recursive: true,
        matched_only: true,
        threshold: cfg.chunk_threshold.max(1000),
        chunk_size: cfg.chunk_size.max(500),
        max_file_bytes: cfg.max_file_mb.saturating_mul(1024 * 1024),
    };
    let (dir, rules, work, app2, task, flag) = (
        PathBuf::from(&job.config.directory),
        rule_set.clone(),
        workspace.clone(),
        app.clone(),
        task_id.to_string(),
        cancel.clone(),
    );
    let mut entries = tauri::async_runtime::spawn_blocking(move || {
        let mut progress = scan::initial_progress(task.clone());
        scan::scan_directory_with_progress(&dir, &rules, &work, &opts, &mut progress, &mut |p| {
            check_cancel(&flag)?;
            update(&app2, &task, p.phase == "done", |run| {
                run.phase = p.phase.clone();
                run.current = p.current.clone();
                run.discovered = p.discovered;
                run.matched = p.matched;
                if p.phase == "extract" {
                    run.processed = p.extract_processed;
                    run.total = p.extract_total;
                } else {
                    run.processed = p.processed;
                    run.total = p.total;
                }
                run.message.clear();
            });
            Ok(())
        })
    })
    .await
    .map_err(|e| format!("扫描线程异常：{e}"))??;
    check_cancel(cancel)?;
    if entries.is_empty() {
        return Ok(Completion {
            state: "no_matches",
            message: "扫描完成，没有命中评审模块的文件。".into(),
            fingerprint: None,
        });
    }
    // 将实际评审输入冻结到本轮工作区，Git 下一轮更新不会影响这一轮。
    update(app, task_id, true, |run| {
        run.phase = "snapshot".into();
        run.processed = 0;
        run.total = Some(entries.len());
    });
    let (app2, task, work, flag) = (
        app.clone(),
        task_id.to_string(),
        workspace.clone(),
        cancel.clone(),
    );
    entries = tauri::async_runtime::spawn_blocking(move || {
        let total = entries.len();
        for (index, entry) in entries.iter_mut().enumerate() {
            check_cancel(&flag)?;
            if entry.parse_error.is_none() {
                let destination = work.join(format!("input-{index}.{}", entry.ext));
                std::fs::copy(&entry.abs_path, &destination)
                    .map_err(|e| format!("保存输入快照失败 {}：{e}", entry.name))?;
                entry.abs_path = destination.to_string_lossy().into();
            }
            update(&app2, &task, index + 1 == total, |run| {
                run.processed = index + 1;
                run.current = entry.name.clone();
            });
        }
        Ok::<_, String>(entries)
    })
    .await
    .map_err(|e| e.to_string())??;
    let mut failures = Vec::new();
    let mut files = Vec::new();
    for entry in &entries {
        if let Some(error) = &entry.parse_error {
            failures.push(FileResult {
                file: entry.path.clone(),
                system: entry.system.clone(),
                template_id: entry.template_id.clone().unwrap_or_default(),
                verdict: "失败".into(),
                error: Some(error.clone()),
                ..Default::default()
            });
        } else {
            files.push(ReviewFileInput {
                path: entry.path.clone(),
                abs_path: entry.abs_path.clone(),
                name: entry.name.clone(),
                kind: entry.kind.clone(),
                system: entry.system.clone(),
                template_id: entry.template_id.clone().unwrap_or_default(),
                template_content: String::new(),
                output_format: "standard".into(),
            });
        }
    }
    let signature = if failures.is_empty() {
        update(app, task_id, true, |run| {
            run.phase = "compare".into();
            run.total = None;
            run.current.clear();
        });
        let (config, input, flag) = (cfg.clone(), entries, cancel.clone());
        Some(
            tauri::async_runtime::spawn_blocking(move || fingerprint(&config, &input, &flag))
                .await
                .map_err(|e| e.to_string())??,
        )
    } else {
        None
    };
    if job.config.only_changed
        && signature
            .as_ref()
            .is_some_and(|s| s == &job.last_fingerprint)
    {
        return Ok(Completion {
            state: "unchanged",
            message: "资料和评审配置均未变化，已跳过重复模型评审。".into(),
            fingerprint: None,
        });
    }
    let mut request = ReviewRequest {
        task_id: task_id.into(),
        workspace: workspace.to_string_lossy().into(),
        files,
        cfg: GatewayConfig {
            session_id: String::new(),
            api_key: String::new(),
            base_url: cfg.base_url.clone(),
            model: cfg.model.clone(),
            allow_http: cfg.allow_http,
            timeout_secs: cfg.timeout_secs,
            api_key_header: cfg.api_key_header.clone(),
            proxy_url: cfg.proxy_url.clone(),
            stream_enabled: cfg.stream_enabled,
        },
        concurrency: cfg.concurrency,
        chunk_threshold: cfg.chunk_threshold,
        chunk_size: cfg.chunk_size,
        granularity: "per_file".into(),
        output_format: cfg.output_format.clone(),
        retry_count: cfg.retry_count,
    };
    if !request.files.is_empty() {
        cfg.prepare_review(&mut request)?;
    }
    {
        let scheduler = app.state::<SchedulerState>();
        let mut data = scheduler.data.lock().unwrap();
        if let Some(active) = &mut data.active {
            active.parse_failures = failures.len();
        }
    }
    update(app, task_id, true, |run| {
        run.phase = "review".into();
        run.processed = failures.len();
        run.total = Some(request.files.len() + failures.len());
        run.current.clear();
    });
    let review = if request.files.is_empty() {
        commands::ReviewSummary {
            task_id: task_id.into(),
            results: Vec::new(),
            pass: 0,
            conditional: 0,
            block: 0,
            fail: 0,
            cancelled: false,
        }
    } else {
        commands::run_review_inner(app, &state, &request, task_id, cancel).await?
    };
    failures.extend(review.results);
    let task = history_summary(task_id, &job.config.directory, &failures);
    let (db, task2, rules, templates) = (
        state.db_path.clone(),
        task.clone(),
        rule_set,
        cfg.templates.clone(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        store::save_task(&db, &task2, &failures, &rules, &templates)
    })
    .await
    .map_err(|e| e.to_string())??;
    update(app, task_id, true, |run| {
        run.history_id = Some(task_id.into());
        run.pass = task.pass;
        run.conditional = task.conditional;
        run.block = task.block;
        run.fail = task.fail;
    });
    drop(cleanup);
    if review.cancelled || cancel.load(Ordering::Relaxed) {
        return Err("任务已取消；已完成结果保存在评审历史。".into());
    }
    Ok(Completion {
        state: if task.fail > 0 { "failed" } else { "succeeded" },
        message: format!(
            "评审完成：{} 个文件，{} 通过，{} 有条件通过，{} 阻断，{} 执行失败。",
            task.total_files, task.pass, task.conditional, task.block, task.fail
        ),
        fingerprint: (task.fail == 0).then_some(signature).flatten(),
    })
}

struct WorkspaceCleanup(PathBuf);
impl Drop for WorkspaceCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn finish(
    app: &AppHandle,
    id: &str,
    task_id: &str,
    cancel: &AtomicBool,
    outcome: Result<Completion, String>,
) {
    let scheduler = app.state::<SchedulerState>();
    let state = app.state::<AppState>();
    let mut data = scheduler.data.lock().unwrap();
    if let Some(job) = data.jobs.iter_mut().find(|j| j.config.id == id) {
        complete_job(job, now(), cancel.load(Ordering::Relaxed), outcome);
    }
    data.active = None;
    if let Err(error) = persist(&state.db_path, &data.jobs) {
        data.load_error = Some(error);
    }
    state.cancels.lock().unwrap().remove(task_id);
    publish(app, &data.jobs);
}

fn complete_job(
    job: &mut ScheduledJob,
    finished: i64,
    cancelled: bool,
    outcome: Result<Completion, String>,
) {
    if let Some(run) = &mut job.last_run {
        run.finished_at = Some(finished);
        run.current.clear();
        match outcome {
            Ok(done) => {
                run.state = done.state.into();
                run.message = done.message;
                if let Some(fingerprint) = done.fingerprint {
                    job.last_fingerprint = fingerprint;
                }
            }
            Err(error) => {
                run.state = if cancelled { "cancelled" } else { "failed" }.into();
                run.message = error;
            }
        }
        if run.state == "failed" || run.state == "cancelled" {
            job.last_fingerprint.clear();
        }
    }
    job.next_run_at = job
        .config
        .enabled
        .then(|| next_run(finished, job.config.interval_minutes));
}

fn history_summary(task_id: &str, directory: &str, results: &[FileResult]) -> store::TaskSummary {
    let (pass, conditional, block, fail) = commands::tally(results);
    let mut systems: Vec<_> = results
        .iter()
        .filter_map(|r| r.system.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    systems.sort();
    store::TaskSummary {
        id: task_id.into(),
        created_at: now(),
        source_type: "scheduled".into(),
        source_ref: directory.into(),
        systems,
        total_files: results.len() as u64,
        pass,
        conditional,
        block,
        fail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> ScheduledJob {
        ScheduledJob {
            config: ScheduleConfig {
                id: Uuid::new_v4().to_string(),
                name: "每日评审".into(),
                directory: "/tmp".into(),
                interval_minutes: 10,
                enabled: true,
                git_pull: false,
                only_changed: true,
                rule_set_id: "default".into(),
            },
            next_run_at: Some(100),
            last_run: Some(ScheduleRun {
                task_id: "task".into(),
                state: "running".into(),
                ..Default::default()
            }),
            last_fingerprint: "previous-success".into(),
        }
    }

    #[test]
    fn success_unchanged_failure_cancel_and_retry_preserve_state_contract() {
        let mut task = job();
        complete_job(
            &mut task,
            1000,
            false,
            Ok(Completion {
                state: "succeeded",
                message: "done".into(),
                fingerprint: Some("new-success".into()),
            }),
        );
        assert_eq!(task.last_fingerprint, "new-success");
        assert_eq!(task.next_run_at, Some(601000));
        complete_job(
            &mut task,
            2000,
            false,
            Ok(Completion {
                state: "unchanged",
                message: "skip".into(),
                fingerprint: None,
            }),
        );
        assert_eq!(task.last_fingerprint, "new-success");
        assert_eq!(task.last_run.as_ref().unwrap().state, "unchanged");
        complete_job(
            &mut task,
            3000,
            false,
            Ok(Completion {
                state: "failed",
                message: "invalid format".into(),
                fingerprint: None,
            }),
        );
        assert!(
            task.last_fingerprint.is_empty(),
            "failed runs must be retried with identical inputs"
        );
        task.last_fingerprint = "success".into();
        complete_job(&mut task, 4000, true, Err("cancel".into()));
        assert!(task.last_fingerprint.is_empty());
        assert_eq!(task.last_run.as_ref().unwrap().state, "cancelled");
        assert_eq!(task.next_run_at, Some(604000));
        task.config.enabled = false;
        complete_job(&mut task, 5000, false, Err("network".into()));
        assert_eq!(task.next_run_at, None);
    }

    #[test]
    fn restart_marks_running_interrupted_and_does_not_backfill_missed_intervals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reviewer.db");
        let task = job();
        persist(&path, &[task]).unwrap();
        let before = now();
        let loaded = SchedulerState::load(&path);
        let data = loaded.data.lock().unwrap();
        assert!(data.load_error.is_none());
        assert!(data.active.is_none());
        let restored = &data.jobs[0];
        assert_eq!(restored.last_run.as_ref().unwrap().state, "interrupted");
        assert!(restored.last_fingerprint.is_empty());
        assert!(restored.next_run_at.unwrap() >= before + 600000);
        assert_eq!(
            load_jobs(&path).unwrap()[0]
                .last_run
                .as_ref()
                .unwrap()
                .state,
            "interrupted"
        );
    }

    #[test]
    fn corrupt_schedule_database_stops_automatic_runs_without_overwriting_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reviewer.db");
        db(&path)
            .unwrap()
            .execute("INSERT INTO schedules VALUES(1,'broken')", [])
            .unwrap();
        let loaded = SchedulerState::load(&path);
        assert!(loaded.data.lock().unwrap().load_error.is_some());
        let raw: String = db(&path)
            .unwrap()
            .query_row("SELECT payload FROM schedules", [], |r| r.get(0))
            .unwrap();
        assert_eq!(raw, "broken");
    }

    fn entry(path: String, abs: &Path) -> ScanEntry {
        serde_json::from_value(serde_json::json!({ "path": path, "abs_path": abs.to_string_lossy(), "name": "a.txt", "ext": "txt", "kind": "text", "size": 3, "matched": true, "system": "deployment", "template_id": "template", "text_len": 3, "chunks": 1, "parse_status": "ok", "preview": "", "parse_error": null })).unwrap()
    }

    #[test]
    fn fingerprint_ignores_random_zip_namespace_order_and_unrelated_settings() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "aaa").unwrap();
        std::fs::write(&b, "bbb").unwrap();
        let first = vec![
            entry(format!("{}-same/a.txt", Uuid::new_v4()), &a),
            entry(format!("{}-same/a.txt", Uuid::new_v4()), &b),
        ];
        let second = vec![
            entry(format!("{}-same/a.txt", Uuid::new_v4()), &b),
            entry(format!("{}-same/a.txt", Uuid::new_v4()), &a),
        ];
        let mut cfg = AppConfig::default();
        let flag = AtomicBool::new(false);
        let signature = fingerprint(&cfg, &first, &flag).unwrap();
        cfg.history_retention_days = 14;
        cfg.concurrency = 8;
        cfg.granularity = "single".into();
        assert_eq!(signature, fingerprint(&cfg, &second, &flag).unwrap());
        cfg.model = "another-model".into();
        assert_ne!(signature, fingerprint(&cfg, &second, &flag).unwrap());
        cfg.model.clear();
        std::fs::write(&a, "changed").unwrap();
        assert_ne!(signature, fingerprint(&cfg, &second, &flag).unwrap());
        flag.store(true, Ordering::Relaxed);
        assert!(fingerprint(&cfg, &second, &flag).is_err());
    }

    #[test]
    fn scheduled_history_keeps_parse_and_invalid_verdict_failures() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("reviewer.db");
        let results = vec![
            FileResult {
                file: "broken.xlsx".into(),
                verdict: "失败".into(),
                error: Some("无法解析".into()),
                ..Default::default()
            },
            FileResult {
                file: "bad.txt".into(),
                verdict: "invalid".into(),
                raw: "invalid response".into(),
                ..Default::default()
            },
        ];
        let summary = history_summary("scheduled-test", "/documents", &results);
        assert_eq!((summary.total_files, summary.pass, summary.fail), (2, 0, 2));
        let cfg = AppConfig::default();
        store::save_task(&db, &summary, &results, &cfg.rule_sets[0], &cfg.templates).unwrap();
        let restored = store::get_task(&db, "scheduled-test").unwrap().unwrap();
        assert_eq!(restored.summary.source_type, "scheduled");
        assert_eq!(restored.summary.fail, 2);
        assert_eq!(restored.files.len(), 2);
        assert_eq!(restored.files[1].raw, "invalid response");
    }

    #[test]
    fn workspace_is_cleaned_when_a_run_returns_early() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("run");
        std::fs::create_dir(&work).unwrap();
        std::fs::write(work.join("input.txt"), "private").unwrap();
        {
            let _cleanup = WorkspaceCleanup(work.clone());
        }
        assert!(!work.exists());
    }
}
