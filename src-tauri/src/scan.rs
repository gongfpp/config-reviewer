use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::model::RuleSet;
use crate::model::ScanEntry;
use crate::parse::{kind_for, parse_file};
use crate::rules::resolve;

pub struct ScanOptions {
    pub recursive: bool,
    pub matched_only: bool,
    pub threshold: usize,
    pub chunk_size: usize,
    pub max_file_bytes: u64,
}

/// 发现阶段尚不知道总数，解析阶段按实际文件数报告进度。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScanProgress {
    pub task_id: String,
    pub phase: String,
    pub discovered: usize,
    pub processed: usize,
    pub total: Option<usize>,
    /// 当前 ZIP 解压的已完成文件数及预检后已知总数，不含目录和 .git 元数据。
    pub extract_processed: usize,
    pub extract_total: Option<usize>,
    pub matched: usize,
    pub current: String,
}

pub type Observer<'a> = dyn FnMut(&ScanProgress) -> Result<(), String> + 'a;

pub fn initial_progress(task_id: String) -> ScanProgress {
    ScanProgress {
        task_id,
        phase: "discover".into(),
        discovered: 0,
        processed: 0,
        total: None,
        extract_processed: 0,
        extract_total: None,
        matched: 0,
        current: String::new(),
    }
}

#[cfg(test)]
pub fn scan_directory(
    dir: &Path,
    rules: &RuleSet,
    workspace: &Path,
    opts: &ScanOptions,
) -> Result<Vec<ScanEntry>, String> {
    scan_directory_with_progress(
        dir,
        rules,
        workspace,
        opts,
        &mut initial_progress(String::new()),
        &mut |_| Ok(()),
    )
}

pub fn scan_directory_with_progress(
    dir: &Path,
    rule_set: &RuleSet,
    workspace: &Path,
    opts: &ScanOptions,
    progress: &mut ScanProgress,
    observer: &mut Observer<'_>,
) -> Result<Vec<ScanEntry>, String> {
    if !dir.is_dir() {
        return Err(format!("目录不存在：{}", dir.display()));
    }
    let max_depth = if opts.recursive { usize::MAX } else { 1 };
    let mut candidates = Vec::new();
    // 工作区可能位于用户选择的目录内，不能扫描自己的解压输出。
    let workspace_root = workspace.parent().unwrap_or(workspace);
    for item in WalkDir::new(dir)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(|e| {
            !is_git_metadata(e.path())
                && e.path() != workspace
                && (workspace_root == dir || e.path() != workspace_root)
        })
    {
        observer(progress)?;
        let item = item.map_err(|e| format!("目录读取失败，扫描未完成：{e}"))?;
        if !item.file_type().is_file() {
            continue;
        }
        let path = item.path();
        let rel = path
            .strip_prefix(dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        discover(
            path,
            &rel,
            workspace,
            opts,
            &mut candidates,
            progress,
            observer,
        )?;
    }
    parse_candidates(candidates, rule_set, opts, progress, observer)
}

#[cfg(test)]
pub fn scan_uploads(
    paths: &[String],
    rules: &RuleSet,
    workspace: &Path,
    opts: &ScanOptions,
) -> Result<Vec<ScanEntry>, String> {
    scan_uploads_with_progress(
        paths,
        rules,
        workspace,
        opts,
        &mut initial_progress(String::new()),
        &mut |_| Ok(()),
    )
}

pub fn scan_uploads_with_progress(
    paths: &[String],
    rule_set: &RuleSet,
    workspace: &Path,
    opts: &ScanOptions,
    progress: &mut ScanProgress,
    observer: &mut Observer<'_>,
) -> Result<Vec<ScanEntry>, String> {
    let mut candidates = Vec::new();
    for (index, raw) in paths.iter().enumerate() {
        observer(progress)?;
        let path = PathBuf::from(raw);
        if is_git_metadata(&path) {
            continue;
        }
        if !path.is_file() {
            return Err(format!("文件不存在或不可读取：{}", path.display()));
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let start = candidates.len();
        discover(
            &path,
            &name,
            workspace,
            opts,
            &mut candidates,
            progress,
            observer,
        )?;
        if kind_for(ext_of(&path)) != "zip"
            && candidates.len() > start
            && candidates[..start]
                .iter()
                .any(|c: &Candidate| c.display == name)
        {
            candidates[start].display = format!("{}/{name}", index + 1);
        }
    }
    parse_candidates(candidates, rule_set, opts, progress, observer)
}

struct Candidate {
    abs: PathBuf,
    rel: String,
    display: String,
}

fn discover(
    path: &Path,
    rel: &str,
    workspace: &Path,
    opts: &ScanOptions,
    candidates: &mut Vec<Candidate>,
    progress: &mut ScanProgress,
    observer: &mut Observer<'_>,
) -> Result<(), String> {
    progress.phase = "discover".into();
    progress.extract_processed = 0;
    progress.extract_total = None;
    progress.current = rel.into();
    observer(progress)?;
    if kind_for(ext_of(path)) == "zip" {
        let namespace = format!(
            "{}-{}",
            uuid::Uuid::new_v4(),
            path.file_stem().unwrap_or_default().to_string_lossy()
        );
        progress.phase = "extract".into();
        let base_count = candidates.len();
        let extracted = extract_zip_with_progress(
            path,
            &workspace.join(&namespace),
            opts,
            &mut |name, count, total| {
                progress.current = format!("{rel} / {name}");
                progress.discovered = base_count + count;
                progress.extract_processed = count;
                progress.extract_total = total;
                observer(progress)
            },
        )?;
        for (abs, rel) in extracted {
            candidates.push(Candidate {
                display: format!("{namespace}/{rel}"),
                abs,
                rel,
            });
        }
    } else {
        candidates.push(Candidate {
            abs: path.to_path_buf(),
            rel: rel.into(),
            display: rel.into(),
        });
    }
    progress.discovered = candidates.len();
    observer(progress)
}

fn parse_candidates(
    candidates: Vec<Candidate>,
    rule_set: &RuleSet,
    opts: &ScanOptions,
    progress: &mut ScanProgress,
    observer: &mut Observer<'_>,
) -> Result<Vec<ScanEntry>, String> {
    progress.phase = "parse".into();
    progress.total = Some(candidates.len());
    let mut entries = Vec::new();
    observer(progress)?;
    for candidate in candidates {
        progress.current = candidate.display.clone();
        observer(progress)?;
        if let Some(mut entry) = build_entry(&candidate.abs, &candidate.rel, rule_set, opts) {
            entry.path = candidate.display;
            if entry.matched {
                progress.matched += 1;
            }
            entries.push(entry);
        }
        progress.processed += 1;
        observer(progress)?;
    }
    progress.phase = "done".into();
    progress.current.clear();
    observer(progress)?;
    Ok(entries)
}

/// 解压 ZIP，返回 (绝对路径, 包内相对路径)。越界、重复或超限时拒绝整个包。
#[cfg(test)]
pub fn extract_zip(
    zip_path: &Path,
    dest: &Path,
    opts: &ScanOptions,
) -> Result<Vec<(PathBuf, String)>, String> {
    extract_zip_with_progress(zip_path, dest, opts, &mut |_, _, _| Ok(()))
}

type ExtractionObserver<'a> = dyn FnMut(&str, usize, Option<usize>) -> Result<(), String> + 'a;

fn extract_zip_with_progress(
    zip_path: &Path,
    dest: &Path,
    opts: &ScanOptions,
    observer: &mut ExtractionObserver<'_>,
) -> Result<Vec<(PathBuf, String)>, String> {
    let file = File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    let mut total_bytes = 0u64;
    let mut names = std::collections::HashSet::new();
    let total_limit = opts
        .max_file_bytes
        .saturating_mul(4)
        .min(2 * 1024 * 1024 * 1024);
    let mut entries = Vec::new();
    // 只读取 ZIP 目录元数据即可获取精确文件总数；同时在写入前验证路径和大小。
    for i in 0..archive.len() {
        observer("", 0, None)?;
        let entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let rel = entry
            .enclosed_name()
            .ok_or_else(|| "ZIP 包含越界路径，已拒绝解压".to_string())?;
        let name = rel.to_string_lossy().replace('\\', "/");
        if name.contains(':')
            || name
                .split('/')
                .any(|s| s == ".." || s.ends_with('.') || s.ends_with(' '))
        {
            return Err("ZIP 含不兼容 Windows 的文件路径".into());
        }
        if entry.is_dir() || is_git_metadata(Path::new(&name)) {
            continue;
        }
        if !names.insert(name.to_lowercase()) {
            return Err("ZIP 含重复或仅大小写不同的文件路径".into());
        }
        if entry.size() > opts.max_file_bytes {
            return Err("ZIP 内文件超过单文件大小上限".into());
        }
        total_bytes = total_bytes
            .checked_add(entry.size())
            .ok_or_else(|| "ZIP 解压大小溢出".to_string())?;
        if total_bytes > total_limit {
            return Err("ZIP 解压总量超过上限（单文件上限的 4 倍，最高 2 GB）".into());
        }
        entries.push((i, rel, name));
    }
    let total_files = entries.len();
    observer("", 0, Some(total_files))?;
    let mut written_total = 0u64;
    for (index, rel, name) in entries {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        observer(&name, result.len(), Some(total_files))?;
        let out_path = dest.join(&rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = File::create(&out_path).map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 64 * 1024];
        let mut written_file = 0u64;
        loop {
            observer(&name, result.len(), Some(total_files))?;
            let read = entry.read(&mut buffer).map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            written_file += read as u64;
            written_total += read as u64;
            if written_file > opts.max_file_bytes {
                return Err("ZIP 内文件超过单文件大小上限".into());
            }
            if written_total > total_limit {
                return Err("ZIP 解压总量超过上限（单文件上限的 4 倍，最高 2 GB）".into());
            }
            out.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        }
        out.flush().map_err(|e| e.to_string())?;
        result.push((out_path, rel.to_string_lossy().replace('\\', "/")));
        observer(&name, result.len(), Some(total_files))?;
    }
    Ok(result)
}

/// 包括 Git worktree 的 .git 文件；.gitignore、.gitmodules 等普通资料保留。
fn is_git_metadata(path: &Path) -> bool {
    path.components().any(|part| {
        part.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(".git")
    })
}

fn build_entry(abs: &Path, rel: &str, rule_set: &RuleSet, opts: &ScanOptions) -> Option<ScanEntry> {
    let name = abs
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = ext_of(abs).to_string();
    let kind = kind_for(&ext).to_string();
    let size = std::fs::metadata(abs).map(|m| m.len()).unwrap_or(0);
    let resolved = resolve(rule_set, rel, &name);
    let matched = resolved.is_some();

    if opts.matched_only && !matched {
        return None;
    }

    let (rule_id, system, template_id) = match resolved {
        Some((r, s, t)) => (Some(r), Some(s), Some(t)),
        None => (None, None, None),
    };

    let mut parse_status = "skipped".to_string();
    let mut parse_error = None;
    let mut preview = String::new();
    let mut text_len = 0usize;
    let mut chunks = 0usize;

    if matched && size > opts.max_file_bytes {
        parse_status = "error".into();
        parse_error = Some(format!(
            "超过单文件上限（{:.0} MB）",
            opts.max_file_bytes as f64 / 1024.0 / 1024.0
        ));
    } else if matched {
        match parse_file(abs, &kind, opts.threshold, opts.chunk_size) {
            Ok(parsed) => {
                parse_status = "ok".into();
                text_len = parsed.text.chars().count();
                chunks = parsed.chunks.len();
                preview = parsed.preview;
            }
            Err(err) => {
                parse_status = "error".into();
                parse_error = Some(err);
                chunks = 1;
            }
        }
    }

    Some(ScanEntry {
        path: rel.to_string(),
        abs_path: abs.to_string_lossy().to_string(),
        name,
        ext,
        kind,
        size,
        system,
        rule_id,
        template_id,
        matched,
        selected: matched && parse_status == "ok",
        chunks,
        parse_status,
        parse_error,
        preview,
        text_len,
    })
}

pub fn ext_of(path: &Path) -> &str {
    path.extension().and_then(|s| s.to_str()).unwrap_or("")
}
