use rust_xlsxwriter::{Format, Workbook};
use serde::{Deserialize, Serialize};

/// 导出一行：系统 | 文件路径 | 文件类型 | 命中规则 | 结论 | 引用 | 时间
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRow {
    pub system: String,
    pub file: String,
    pub file_type: String,
    pub rule: String,
    pub verdict: String,
    pub citations: String,
    pub time: String,
}

const HEADERS: [&str; 7] = ["系统", "文件路径", "文件类型", "命中规则", "结论", "引用", "时间"];

pub fn write_xlsx(path: &str, rows: &[ExportRow]) -> Result<(), String> {
    let mut workbook = Workbook::new();
    let header_format = Format::new().set_bold();
    let sheet = workbook.add_worksheet();
    sheet.set_name("评审结果").map_err(|e| e.to_string())?;

    for (col, header) in HEADERS.iter().enumerate() {
        sheet
            .write_string_with_format(0, col as u16, *header, &header_format)
            .map_err(|e| e.to_string())?;
    }
    for (i, row) in rows.iter().enumerate() {
        let r = (i + 1) as u32;
        for (col, value) in [
            &row.system,
            &row.file,
            &row.file_type,
            &row.rule,
            &row.verdict,
            &row.citations,
            &row.time,
        ]
        .iter()
        .enumerate()
        {
            sheet
                .write_string(r, col as u16, value.as_str())
                .map_err(|e| e.to_string())?;
        }
    }
    sheet.set_column_width(0, 10).map_err(|e| e.to_string())?;
    sheet.set_column_width(1, 46).map_err(|e| e.to_string())?;
    sheet.set_column_width(2, 10).map_err(|e| e.to_string())?;
    sheet.set_column_width(3, 22).map_err(|e| e.to_string())?;
    sheet.set_column_width(4, 12).map_err(|e| e.to_string())?;
    sheet.set_column_width(5, 48).map_err(|e| e.to_string())?;
    sheet.set_column_width(6, 18).map_err(|e| e.to_string())?;

    workbook.save(path).map_err(|e| e.to_string())
}
