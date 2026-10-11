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
        "doc" | "docx" => "word",
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
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("doc"))
    {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        return Ok(finish(
            legacy_doc_text(&bytes)?,
            threshold,
            chunk_size,
            false,
            None,
        ));
    }
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

/// 只提取 Word 97–2003 正文；按文档结构保留重复段落与表格单元格。
/// 不使用库的 flat text 路径，因为该路径会合并连续重复正文。
pub fn legacy_doc_text(bytes: &[u8]) -> Result<String, String> {
    let document = rwml::Document::open(bytes).map_err(|e| match e {
        rwml::Error::Encrypted { .. } => "DOC 已加密，请在 Word 中解密后另存再导入".into(),
        rwml::Error::UnsupportedVersion(_) => {
            "暂不支持 Word 6/95，请另存为 Word 97–2003 DOC 或 DOCX".into()
        }
        rwml::Error::NotOle2 | rwml::Error::Docx(_) => {
            "文件不是 Word 97–2003 二进制 DOC，请另存为 DOCX 后导入".into()
        }
        _ => format!("DOC 解析失败，文件可能已损坏：{e}"),
    })?;
    let model = document.model();
    let mut out = String::new();
    for region in model.source_regions(rwml::SourceRegionKind::Main) {
        legacy_blocks_text(model.source_region_blocks(region), &mut out);
    }
    let text = out.trim().to_string();
    if text.is_empty() {
        return Err("DOC 未提取到可评审正文；图片和嵌入对象不支持 OCR".into());
    }
    Ok(text)
}

fn legacy_blocks_text(blocks: &[rwml::Block], out: &mut String) {
    for block in blocks {
        match block {
            rwml::Block::Paragraph(paragraph) => {
                out.push_str(paragraph.text().trim_end_matches(['\r', '\n', '\u{7}']));
                out.push('\n');
            }
            rwml::Block::Table(table) => {
                for row in &table.rows {
                    let cells: Vec<String> = row
                        .cells
                        .iter()
                        .map(|cell| {
                            let mut text = String::new();
                            legacy_blocks_text(&cell.blocks, &mut text);
                            text.trim_end().replace('\n', " / ")
                        })
                        .collect();
                    out.push_str(&cells.join(" | "));
                    out.push('\n');
                }
            }
            rwml::Block::PageBreak => out.push('\n'),
            _ => {}
        }
    }
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
            Event::Text(e) if in_text => out.push_str(e.as_ref()),
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
