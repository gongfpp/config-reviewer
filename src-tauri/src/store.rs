use std::path::Path;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::model::{FileResult, RuleSet, Template};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    pub created_at: i64,
    pub source_type: String,
    pub source_ref: String,
    pub systems: Vec<String>,
    pub total_files: u64,
    pub pass: u64,
    pub conditional: u64,
    pub block: u64,
    pub fail: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDetail {
    pub summary: TaskSummary,
    pub files: Vec<FileResult>,
    pub rule_set: Option<RuleSet>,
    pub templates: Vec<Template>,
}

fn open(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL,
            source_type TEXT NOT NULL,
            source_ref TEXT NOT NULL,
            systems TEXT NOT NULL,
            stats TEXT NOT NULL,
            rule_set_json TEXT NOT NULL,
            templates_json TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS task_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id TEXT NOT NULL,
            path TEXT NOT NULL,
            name TEXT NOT NULL,
            system TEXT,
            template_id TEXT NOT NULL,
            verdict TEXT NOT NULL,
            result_json TEXT NOT NULL,
            duration_ms INTEGER NOT NULL,
            created_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_task_files_task ON task_files(task_id);",
    )
    .map_err(|e| e.to_string())?;
    Ok(conn)
}

/// 保存成功任务（仅成功评审入库）。
pub fn save_task(
    db_path: &Path,
    summary: &TaskSummary,
    results: &[FileResult],
    rule_set: &RuleSet,
    templates: &[Template],
) -> Result<(), String> {
    let mut conn = open(db_path)?;
    let conn = conn.transaction().map_err(|e| e.to_string())?;
    // 同一任务重复保存（如失败项重试）时先清理旧的明细，保证幂等。
    conn.execute(
        "DELETE FROM task_files WHERE task_id = ?1",
        params![summary.id],
    )
    .map_err(|e| e.to_string())?;
    let systems = serde_json::to_string(&summary.systems).map_err(|e| e.to_string())?;
    let stats = serde_json::to_string(&(
        summary.total_files,
        summary.pass,
        summary.conditional,
        summary.block,
        summary.fail,
    ))
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO tasks (id, created_at, source_type, source_ref, systems, stats, rule_set_json, templates_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            summary.id,
            summary.created_at,
            summary.source_type,
            summary.source_ref,
            systems,
            stats,
            serde_json::to_string(rule_set).map_err(|e| e.to_string())?,
            serde_json::to_string(templates).map_err(|e| e.to_string())?,
        ],
    )
    .map_err(|e| e.to_string())?;
    for r in results {
        let name = Path::new(&r.file)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        conn.execute(
            "INSERT INTO task_files (task_id, path, name, system, template_id, verdict, result_json, duration_ms, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                summary.id,
                r.file,
                name,
                r.system,
                r.template_id,
                r.verdict,
                serde_json::to_string(r).map_err(|e| e.to_string())?,
                r.duration_ms as i64,
                summary.created_at,
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    conn.commit().map_err(|e| e.to_string())
}

pub fn list_tasks(db_path: &Path) -> Result<Vec<TaskSummary>, String> {
    let conn = open(db_path)?;
    let mut stmt = conn
        .prepare("SELECT id, created_at, source_type, source_ref, systems, stats FROM tasks ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let systems: String = row.get(4)?;
            let stats: String = row.get(5)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                systems,
                stats,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        let (id, created_at, source_type, source_ref, systems, stats) =
            row.map_err(|e| e.to_string())?;
        let stats: (u64, u64, u64, u64, u64) =
            serde_json::from_str(&stats).unwrap_or((0, 0, 0, 0, 0));
        out.push(TaskSummary {
            id,
            created_at,
            source_type,
            source_ref,
            systems: serde_json::from_str(&systems).unwrap_or_default(),
            total_files: stats.0,
            pass: stats.1,
            conditional: stats.2,
            block: stats.3,
            fail: stats.4,
        });
    }
    Ok(out)
}

pub fn get_task(db_path: &Path, id: &str) -> Result<Option<TaskDetail>, String> {
    let conn = open(db_path)?;
    let summary = {
        let mut stmt = conn
            .prepare("SELECT id, created_at, source_type, source_ref, systems, stats, rule_set_json, templates_json FROM tasks WHERE id = ?1")
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query(params![id]).map_err(|e| e.to_string())?;
        match rows.next().map_err(|e| e.to_string())? {
            None => return Ok(None),
            Some(row) => {
                let systems: String = row.get(4).map_err(|e| e.to_string())?;
                let stats: String = row.get(5).map_err(|e| e.to_string())?;
                let rule_json: String = row.get(6).map_err(|e| e.to_string())?;
                let tpl_json: String = row.get(7).map_err(|e| e.to_string())?;
                let stats: (u64, u64, u64, u64, u64) =
                    serde_json::from_str(&stats).unwrap_or((0, 0, 0, 0, 0));
                (
                    TaskSummary {
                        id: row.get(0).map_err(|e| e.to_string())?,
                        created_at: row.get(1).map_err(|e| e.to_string())?,
                        source_type: row.get(2).map_err(|e| e.to_string())?,
                        source_ref: row.get(3).map_err(|e| e.to_string())?,
                        systems: serde_json::from_str(&systems).unwrap_or_default(),
                        total_files: stats.0,
                        pass: stats.1,
                        conditional: stats.2,
                        block: stats.3,
                        fail: stats.4,
                    },
                    serde_json::from_str::<RuleSet>(&rule_json).ok(),
                    serde_json::from_str::<Vec<Template>>(&tpl_json).unwrap_or_default(),
                )
            }
        }
    };
    let (summary, rule_set, templates) = summary;
    let mut stmt = conn
        .prepare("SELECT result_json FROM task_files WHERE task_id = ?1 ORDER BY id ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![id], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    for row in rows {
        let json = row.map_err(|e| e.to_string())?;
        files.push(
            serde_json::from_str::<FileResult>(&json).map_err(|e| format!("历史结果损坏：{e}"))?,
        );
    }
    Ok(Some(TaskDetail {
        summary,
        files,
        rule_set,
        templates,
    }))
}

/// 按保留天数清理历史（`retention_days` <= 0 时不清理）。
pub fn prune(db_path: &Path, retention_days: u32) -> Result<(), String> {
    if retention_days == 0 {
        return Ok(());
    }
    let cutoff = chrono::Utc::now().timestamp_millis() - (retention_days as i64) * 86_400_000;
    let conn = open(db_path)?;
    conn.execute(
        "DELETE FROM task_files WHERE task_id IN (SELECT id FROM tasks WHERE created_at < ?1)",
        params![cutoff],
    )
    .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM tasks WHERE created_at < ?1", params![cutoff])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn delete_task(db_path: &Path, id: &str) -> Result<(), String> {
    let mut conn = open(db_path)?;
    let conn = conn.transaction().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM task_files WHERE task_id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    conn.commit().map_err(|e| e.to_string())
}
