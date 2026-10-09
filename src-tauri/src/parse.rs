use std::fs::File;
use std::io::Read;
use std::path::Path;

use calamine::{open_workbook_auto, Data, Reader};

use crate::chunk::{chunk_json, chunk_text};

pub struct Parsed {
    pub text: String,
    pub chunks: Vec<String>,
    pub preview: String,
}

/// 依据扩展名判断文件类型。
pub fn kind_for(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "json" => "json",
        "xlsx" | "xls" | "xlsm" | "xlsb" => "excel",
        "docx" => "word",
        "txt" | "md" | "log" | "conf" | "properties" | "yml" | "yaml" | "xml" | "ini" | "csv" => {
            "text"
        }
        "zip" => "zip",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "pdf" | "jar" | "class" => "binary",
        _ => "other",
    }
}

/// UTF-8 优先，GB18030 回退。
pub fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    for (bom, encoding) in [
        (&[0xFF, 0xFE][..], encoding_rs::UTF_16LE),
        (&[0xFE, 0xFF][..], encoding_rs::UTF_16BE),
    ] {
        if bytes.starts_with(bom) {
            return encoding.decode(&bytes[2..]).0.into_owned();
        }
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let (decoded, _, _) = encoding_rs::GB18030.decode(bytes);
            decoded.into_owned()
        }
    }
}

/// 解析文件为可评审文本，并按配置切块。
pub fn parse_file(
    path: &Path,
    kind: &str,
    threshold: usize,
    chunk_size: usize,
) -> Result<Parsed, String> {
    match kind {
        "json" => parse_json(path, threshold, chunk_size),
        "excel" => parse_excel(path, threshold, chunk_size),
        "word" => parse_word(path, threshold, chunk_size),
        "text" => {
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            Ok(finish(
                decode_text(&bytes),
                threshold,
                chunk_size,
                false,
                None,
            ))
        }
        _ => Err("跳过：暂不支持解析该类型".into()),
    }
}

fn parse_json(path: &Path, threshold: usize, chunk_size: usize) -> Result<Parsed, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let raw = decode_text(&bytes);
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(value) => {
            let chunks = chunk_json(&value, threshold, chunk_size);
            let text = serde_json::to_string_pretty(&value).unwrap_or(raw);
            Ok(finish(text, threshold, chunk_size, false, Some(chunks)))
        }
        // 解析失败降级为纯文本，不中断
        Err(_) => Ok(finish(raw, threshold, chunk_size, false, None)),
    }
}

fn parse_excel(path: &Path, threshold: usize, chunk_size: usize) -> Result<Parsed, String> {
    let mut workbook = open_workbook_auto(path).map_err(|e| e.to_string())?;
    let names = workbook.sheet_names().to_owned();
    let mut out = String::new();
    for name in names {
        let range = match workbook.worksheet_range(&name) {
            Ok(range) => range,
            Err(e) => return Err(format!("Sheet「{name}」读取失败：{e}")),
        };
        if range.is_empty() {
            continue;
        }
        out.push_str(&format!("## Sheet: {name}\n"));
        let first_row = range.start().map(|(row, _)| row).unwrap_or(0) as usize;
        for (row_idx, row) in range.rows().enumerate() {
            let cells: Vec<String> = row.iter().map(cell_to_string).collect();
            if cells.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            out.push_str(&format!(
                "{} | {}\n",
                first_row + row_idx + 1,
                cells.join(" | ")
            ));
        }
        out.push('\n');
    }
    Ok(finish(out, threshold, chunk_size, false, None))
}

fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => format!("{f}"),
        Data::Int(i) => format!("{i}"),
        Data::Bool(b) => format!("{b}"),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::DateTime(dt) => format!("{dt:?}"),
        Data::Error(e) => format!("{e:?}"),
    }
}

fn parse_word(path: &Path, threshold: usize, chunk_size: usize) -> Result<Parsed, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|e| e.to_string())?
        .read_to_string(&mut xml)
        .map_err(|e| e.to_string())?;
    Ok(finish(docx_text(&xml)?, threshold, chunk_size, false, None))
}

/// 从 WordprocessingML 中抽取文本：段落换行 + 去标签 + 实体解码。
pub fn docx_text(xml: &str) -> Result<String, String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader
            .read_event()
            .map_err(|e| format!("Word XML 解析失败：{e}"))?
        {
            Event::Start(e) if e.local_name().as_ref() == "t" => in_text = true,
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "p" => out.push('\n'),
                "tc" => {
                    while out.ends_with('\n') {
                        out.pop();
                    }
                    out.push_str(" | ");
                }
                "tr" => {
                    if out.ends_with(" | ") {
                        out.truncate(out.len() - 3);
                    }
                    out.push('\n');
                }
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "tab" => out.push('\t'),
                "br" | "cr" => out.push('\n'),
                _ => {}
            },
            Event::Text(e) if in_text => out.push_str(&e.as_ref()),
            Event::GeneralRef(e) if in_text => {
                let name = e.as_ref();
                let encoded = format!("&{name};");
                out.push_str(&quick_xml::escape::unescape(&encoded).map_err(|e| e.to_string())?);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out.trim().to_string())
}

fn finish(
    text: String,
    threshold: usize,
    chunk_size: usize,
    _json: bool,
    chunks: Option<Vec<String>>,
) -> Parsed {
    let chunks = chunks.unwrap_or_else(|| chunk_text(&text, threshold, chunk_size));
    let preview: String = text.chars().take(800).collect();
    Parsed {
        text,
        chunks,
        preview,
    }
}
