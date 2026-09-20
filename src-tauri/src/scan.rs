use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::model::ScanEntry;
use crate::parse::{kind_for, parse_file};
use crate::rules::resolve;
use crate::model::RuleSet;

pub struct ScanOptions {
    pub recursive: bool,
    pub matched_only: bool,
    pub threshold: usize,
    pub chunk_size: usize,
    pub max_file_bytes: u64,
}

/// 扫描本地目录（普通文件原地读取，ZIP 解压到工作区后再扫描）。
pub fn scan_directory(dir: &Path, rule_set: &RuleSet, workspace: &Path, opts: &ScanOptions) -> Result<Vec<ScanEntry>, String> {
    if !dir.is_dir() {
        return Err(format!("目录不存在：{}", dir.display()));
    }
    let max_depth = if opts.recursive { usize::MAX } else { 1 };
    let mut entries = Vec::new();
    let mut zips: Vec<PathBuf> = Vec::new();
    for item in WalkDir::new(dir).max_depth(max_depth).into_iter().filter_map(Result::ok) {
        if !item.file_type().is_file() {
            continue;
        }
        let path = item.path();
        let rel = path.strip_prefix(dir).unwrap_or(path).to_string_lossy().replace('\\', "/");
        if kind_for(ext_of(path)) == "zip" {
            zips.push(path.to_path_buf());
            continue;
        }
        if let Some(entry) = build_entry(path, &rel, rule_set, opts) {
            entries.push(entry);
        }
    }
    for zip_path in zips {
        let stem = zip_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "pkg".into());
        let extracted = extract_zip(&zip_path, &workspace.join(format!("unzip-{stem}")))?;
        for (abs, rel) in extracted {
            if let Some(entry) = build_entry(&abs, &rel, rule_set, opts) {
                entries.push(entry);
            }
        }
    }
    Ok(entries)
}

/// 扫描上传路径：ZIP 解压到工作区，普通文件登记为条目。
pub fn scan_uploads(paths: &[String], rule_set: &RuleSet, workspace: &Path, opts: &ScanOptions) -> Result<Vec<ScanEntry>, String> {
    let mut entries = Vec::new();
    for raw in paths {
        let path = PathBuf::from(raw);
        if !path.is_file() {
            continue;
        }
        if kind_for(ext_of(&path)) == "zip" {
            let dest = workspace.join(format!(
                "unzip-{}",
                path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "pkg".into())
            ));
            let extracted = extract_zip(&path, &dest)?;
            for (abs, rel) in extracted {
                if let Some(entry) = build_entry(&abs, &rel, rule_set, opts) {
                    entries.push(entry);
                }
            }
        } else if let Some(entry) = build_entry(&path, &path.file_name().unwrap_or_default().to_string_lossy(), rule_set, opts) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// 解压 ZIP，返回 (绝对路径, 包内相对路径)。跳过目录与越界条目。
pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    let file = File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else { continue };
        if entry.is_dir() {
            continue;
        }
        let out_path = dest.join(&rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = File::create(&out_path).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        let _ = out.flush();
        result.push((out_path, rel.to_string_lossy().replace('\\', "/")));
    }
    Ok(result)
}

fn build_entry(abs: &Path, rel: &str, rule_set: &RuleSet, opts: &ScanOptions) -> Option<ScanEntry> {
    let name = abs.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
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
