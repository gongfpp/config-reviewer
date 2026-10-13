//! 回归测试直接调用生产解析、网关、历史和导出模块；使用临时目录与模拟 Key。
use crate::{chunk, config, export, gateway, model, parse, results, rules, scan, store};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::Duration;
use zip::write::SimpleFileOptions;

fn cfg(url: String) -> model::GatewayConfig {
    model::GatewayConfig {
        session_id: String::new(),
        base_url: url,
        model: "mock-model".into(),
        api_key: "mock-key".into(),
        allow_http: true,
        timeout_secs: 3,
        api_key_header: String::new(),
        proxy_url: String::new(),
        stream_enabled: true,
    }
}
fn mock(
    parts: Vec<Vec<u8>>,
    content_type: &str,
    status: u16,
    delay: u64,
) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    let kind = content_type.to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
            if let Some(pos) = request.windows(4).position(|p| p == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..pos]);
                let len = headers
                    .lines()
                    .find_map(|l| {
                        l.to_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if request.len() >= pos + 4 + len {
                    break;
                }
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&request).to_string());
        std::thread::sleep(Duration::from_millis(delay));
        if write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: {kind}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").is_err(){return;}
        for part in parts {
            if write!(stream, "{:x}\r\n", part.len())
                .and_then(|_| stream.write_all(&part))
                .and_then(|_| stream.write_all(b"\r\n"))
                .and_then(|_| stream.flush())
                .is_err()
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = stream.write_all(b"0\r\n\r\n");
    });
    (format!("http://{addr}/v1"), rx)
}
fn json_reply(text: &str) -> Vec<u8> {
    serde_json::to_vec_pretty(
        &serde_json::json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}]}),
    )
    .unwrap()
}
fn sse(text: &str) -> Vec<u8> {
    format!(
        "data: {}\r\n\r\ndata: [DONE]\r\n\r\n",
        serde_json::json!({"choices":[{"delta":{"content":text}}]})
    )
    .into_bytes()
}
fn opts() -> scan::ScanOptions {
    scan::ScanOptions {
        recursive: true,
        matched_only: false,
        threshold: 12000,
        chunk_size: 8000,
        max_file_bytes: 1024 * 1024,
    }
}
fn zip_fixture(path: &std::path::Path, files: &[(&str, &[u8])]) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, body) in files {
        zip.start_file(
            *name,
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap();
}
#[tokio::test]
async fn sse_preserves_chinese_across_every_byte_boundary() {
    let bytes = sse("中文🌱");
    let (url, _) = mock(
        bytes.chunks(1).map(|s| s.to_vec()).collect(),
        "text/event-stream",
        200,
        0,
    );
    let mut deltas = String::new();
    let text = gateway::chat_stream(&cfg(url), "test", None, |s| deltas.push_str(s))
        .await
        .unwrap();
    assert_eq!(text, "中文🌱");
    assert_eq!(text, deltas);
}
#[tokio::test]
async fn pretty_json_fallback_with_wrong_content_type() {
    let (url, _) = mock(vec![json_reply("可用")], "text/event-stream", 200, 0);
    assert_eq!(
        gateway::chat_stream(&cfg(url), "test", None, |_| {})
            .await
            .unwrap(),
        "可用"
    );
}
#[tokio::test]
async fn non_streaming_and_custom_header_are_sent() {
    let (url, rx) = mock(vec![json_reply("可用")], "application/json", 200, 0);
    let mut c = cfg(url);
    c.stream_enabled = false;
    c.api_key_header = "api-key".into();
    assert_eq!(gateway::test_connection(&c).await.unwrap(), "可用");
    let request = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(request.to_lowercase().contains("api-key: mock-key"));
    assert!(!request.to_lowercase().contains("authorization:"));
    assert!(request.contains("\"stream\":false"));
    assert!(request.starts_with("POST /v1/chat/completions "));
}
#[tokio::test]
async fn bearer_and_complete_endpoint_are_sent() {
    let (url, rx) = mock(vec![json_reply("可用")], "application/json", 200, 0);
    let c = cfg(format!("{url}/chat/completions/"));
    assert_eq!(gateway::chat(&c, "test").await.unwrap(), "可用");
    let request = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(request
        .to_lowercase()
        .contains("authorization: bearer mock-key"));
    assert!(request.starts_with("POST /v1/chat/completions "));
}
#[tokio::test]
async fn cancellation_interrupts_waiting_for_headers() {
    let (url, _) = mock(vec![json_reply("可用")], "application/json", 200, 1000);
    let flag = Arc::new(AtomicBool::new(false));
    let cancel = flag.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(70)).await;
        cancel.store(true, Ordering::Relaxed);
    });
    let before = std::time::Instant::now();
    assert!(gateway::chat_stream(&cfg(url), "test", Some(flag), |_| {})
        .await
        .unwrap_err()
        .contains("取消"));
    assert!(before.elapsed() < Duration::from_millis(700));
}
#[tokio::test]
async fn cancellation_interrupts_silent_stream() {
    let (url, _) = mock(
        vec![b": heartbeat\n\n".to_vec(), sse("可用")],
        "text/event-stream",
        200,
        0,
    );
    let flag = Arc::new(AtomicBool::new(true));
    assert!(gateway::chat_stream(&cfg(url), "test", Some(flag), |_| {})
        .await
        .unwrap_err()
        .contains("取消"));
}
#[tokio::test]
async fn truncated_stream_is_failure() {
    let data = format!(
        "data: {}\n\n",
        serde_json::json!({"choices":[{"delta":{"content":"通过"}}]})
    );
    let (url, _) = mock(vec![data.into_bytes()], "text/event-stream", 200, 0);
    assert!(gateway::chat_stream(&cfg(url), "test", None, |_| {})
        .await
        .unwrap_err()
        .contains("中断"));
}
#[tokio::test]
async fn finish_reason_stop_without_done_is_accepted() {
    let data = format!(
        "data: {}\n\n",
        serde_json::json!({"choices":[{"delta":{"content":"中文"},"finish_reason":"stop"}]})
    );
    let (url, _) = mock(vec![data.into_bytes()], "text/event-stream", 200, 0);
    assert_eq!(
        gateway::chat_stream(&cfg(url), "test", None, |_| {})
            .await
            .unwrap(),
        "中文"
    );
}
#[tokio::test]
async fn malformed_stream_is_failure() {
    let (url, _) = mock(
        vec![b"data: {broken}\n\ndata: [DONE]\n\n".to_vec()],
        "text/event-stream",
        200,
        0,
    );
    assert!(gateway::chat_stream(&cfg(url), "test", None, |_| {})
        .await
        .is_err());
}
#[tokio::test]
async fn output_limit_is_not_a_success() {
    let body = serde_json::to_vec(
        &serde_json::json!({"choices":[{"message":{"content":"通过"},"finish_reason":"length"}]}),
    )
    .unwrap();
    let (url, _) = mock(vec![body], "application/json", 200, 0);
    assert!(gateway::chat(&cfg(url), "test")
        .await
        .unwrap_err()
        .contains("未完整"));
}
#[tokio::test]
async fn rate_limit_is_reported_without_key() {
    let (url, _) = mock(
        vec![b"sensitive mock-key".to_vec()],
        "application/json",
        429,
        0,
    );
    let error = gateway::chat(&cfg(url), "test").await.unwrap_err();
    assert!(error.contains("429"));
    assert!(!error.contains("mock-key"));
}
#[test]
fn http_disable_and_invalid_url_and_header_are_enforced() {
    let mut c = cfg("http://localhost/v1".into());
    c.allow_http = false;
    assert!(gateway::validate(&c).is_err());
    c.allow_http = true;
    c.base_url = "file:///tmp/local".into();
    assert!(gateway::validate(&c).is_err());
    c.base_url = "http://user:secret@localhost".into();
    assert!(gateway::validate(&c).is_err());
    c.base_url = "http://localhost/v1".into();
    c.api_key_header = "invalid\r\nheader".into();
    assert!(gateway::validate(&c).is_err());
}
#[test]
fn windows_text_encodings() {
    assert_eq!(parse::decode_text(b"\xef\xbb\xbfhello"), "hello");
    let bytes = encoding_rs::GB18030.encode("中文配置").0;
    assert_eq!(parse::decode_text(&bytes), "中文配置");
    for little in [true, false] {
        let mut bytes = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for n in "中文配置".encode_utf16() {
            bytes.extend_from_slice(&if little {
                n.to_le_bytes()
            } else {
                n.to_be_bytes()
            });
        }
        assert_eq!(parse::decode_text(&bytes), "中文配置");
    }
}
#[test]
fn docx_tables_entities_and_metadata() {
    let xml = r#"<w:document xmlns:w="x"><w:p><w:pPr><w:numPr><w:numId w:val="23"/></w:numPr></w:pPr><w:r><w:t>甲&amp;lt;&#x4e2d;</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>键</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>值</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:document>"#;
    let text = parse::docx_text(xml).unwrap();
    assert!(text.contains("甲&lt;中"));
    assert!(text.contains("键 | 值"));
    assert!(!text.contains("23"));
    assert!(parse::docx_text("<w:p><w:t>bad</w:p>").is_err());
}
#[test]
fn excel_sheet_row_location_and_cached_values() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("任务 中文.xlsx");
    let mut book = rust_xlsxwriter::Workbook::new();
    let sheet = book.add_worksheet();
    sheet.set_name("配置").unwrap();
    sheet.write_string(10, 1, "执行器").unwrap();
    sheet
        .write_formula(10, 2, rust_xlsxwriter::Formula::new("1+2").set_result("3"))
        .unwrap();
    book.add_worksheet().set_name("空表").unwrap();
    book.save(&path).unwrap();
    let parsed = parse::parse_file(&path, "excel", 12000, 8000).unwrap();
    assert!(parsed.text.contains("11 | 执行器 | 3"));
    assert!(!parsed.text.contains("空表"));
}
#[test]
fn invalid_json_is_preserved_as_text() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("apollo.json");
    std::fs::write(&path, "{配置: 中文}").unwrap();
    assert_eq!(
        parse::parse_file(&path, "json", 12000, 8000).unwrap().text,
        "{配置: 中文}"
    );
}
#[test]
fn zip_traversal_and_limits_and_duplicates_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("输入.zip");
    zip_fixture(&path, &[("../outside.txt", b"x")]);
    assert!(scan::extract_zip(&path, &temp.path().join("out"), &opts()).is_err());
    assert!(!temp.path().join("outside.txt").exists());
    zip_fixture(&path, &[("A.json", b"{}"), ("a.json", b"{}")]);
    assert!(scan::extract_zip(&path, &temp.path().join("out"), &opts()).is_err());
    zip_fixture(&path, &[("large.txt", b"12345")]);
    let mut small = opts();
    small.max_file_bytes = 4;
    assert!(scan::extract_zip(&path, &temp.path().join("out"), &small).is_err());
    zip_fixture(
        &path,
        &[
            ("a.txt", b"1234"),
            ("b.txt", b"1234"),
            ("c.txt", b"1234"),
            ("d.txt", b"1234"),
            ("e.txt", b"1234"),
        ],
    );
    assert!(scan::extract_zip(&path, &temp.path().join("out"), &small)
        .unwrap_err()
        .contains("解压总量"));
}
#[test]
fn same_named_packages_are_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for folder in ["甲", "乙"] {
        let dir = temp.path().join(folder);
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("发布.zip");
        zip_fixture(&path, &[("apollo.json", b"{}")]);
        paths.push(path.to_string_lossy().into_owned());
    }
    let entries = scan::scan_uploads(
        &paths,
        &rules::default_rule_sets()[0],
        &temp.path().join("workspace"),
        &opts(),
    )
    .unwrap();
    assert_eq!(entries.len(), 2);
    assert_ne!(entries[0].path, entries[1].path);
    assert_ne!(entries[0].abs_path, entries[1].abs_path);
}
#[test]
fn standard_sections_and_verdict_are_parsed() {
    let r=results::parse_result("apollo.json",None,"tpl",1,0,"有条件通过\n【依据】\n- 配置完整\n【风险】\n- 缺回滚\n【整改项】\n- 补回滚\n引用：$.key | value","standard");
    assert_eq!(r.verdict, "有条件通过");
    assert_eq!(r.basis, vec!["配置完整"]);
    assert_eq!(r.risks, vec!["缺回滚"]);
    assert_eq!(r.remediations, vec!["补回滚", "引用：$.key | value"]);
    assert_eq!(r.citations.len(), 1);
    assert_eq!(
        results::detect_verdict("未通过\n后续通过测试"),
        "未识别结论"
    );
}
#[test]
fn invalid_review_json_fails_closed() {
    for raw in [
        "通过",
        "{\"verdict\":\"通过\"}",
        "{\"verdict\":\"未通过\",\"basis\":[],\"risks\":[],\"remediations\":[],\"citations\":[]}",
    ] {
        assert!(results::parse_result("a", None, "t", 1, 0, raw, "json")
            .error
            .is_some());
    }
}
#[test]
fn config_migrates_and_preserves_system_edits() {
    let mut value = serde_json::to_value(config::AppConfig::default()).unwrap();
    for key in [
        "api_key_header",
        "proxy_url",
        "stream_enabled",
        "retry_count",
        "granularity",
        "output_format",
        "history_retention_days",
    ] {
        value.as_object_mut().unwrap().remove(key);
    }
    let old: config::AppConfig = serde_json::from_value(value).unwrap();
    assert!(old.stream_enabled);
    assert_eq!(old.retry_count, 1);
    let mut edited = config::AppConfig::default();
    edited.systems[0].name = "自定义 Apollo".into();
    assert_eq!(edited.merged().systems[0].name, "自定义 Apollo");
}
fn summary(id: &str) -> store::TaskSummary {
    store::TaskSummary {
        id: id.into(),
        created_at: chrono::Utc::now().timestamp_millis(),
        source_type: "dir".into(),
        source_ref: "中文目录".into(),
        systems: vec!["apollo".into()],
        total_files: 1,
        pass: 1,
        conditional: 0,
        block: 0,
        fail: 0,
    }
}
#[test]
fn history_save_retry_delete_and_rollback() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("历史.db");
    let summary = summary("task");
    let rules = rules::default_rule_sets().remove(0);
    let old = model::FileResult {
        file: "旧.json".into(),
        verdict: "通过".into(),
        ..Default::default()
    };
    store::save_task(&db, &summary, std::slice::from_ref(&old), &rules, &[]).unwrap();
    store::save_task(&db, &summary, std::slice::from_ref(&old), &rules, &[]).unwrap();
    assert_eq!(store::list_tasks(&db).unwrap().len(), 1);
    assert_eq!(
        store::get_task(&db, "task").unwrap().unwrap().files.len(),
        1
    );
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_insert BEFORE INSERT ON task_files WHEN NEW.path = 'fail' BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
    let bad = model::FileResult {
        file: "fail".into(),
        ..Default::default()
    };
    assert!(store::save_task(&db, &summary, &[bad], &rules, &[]).is_err());
    assert_eq!(
        store::get_task(&db, "task").unwrap().unwrap().files[0].file,
        "旧.json"
    );
    store::delete_task(&db, "task").unwrap();
    assert!(store::get_task(&db, "task").unwrap().is_none());
}
#[test]
fn history_retention_removes_only_old_tasks() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("history.db");
    let mut old = summary("old");
    old.created_at -= 10 * 86_400_000;
    let rs = rules::default_rule_sets().remove(0);
    for s in [old, summary("new")] {
        store::save_task(&db, &s, &[], &rs, &[]).unwrap();
    }
    store::prune(&db, 7).unwrap();
    let tasks = store::list_tasks(&db).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "new");
}
#[tokio::test]
async fn mixed_package_to_review_history_and_excel() {
    let temp = tempfile::tempdir().unwrap();
    let zip_path = temp.path().join("发布包.zip");
    zip_fixture(
        &zip_path,
        &[
            ("apollo.json", br#"{"retry":3}"#),
            ("readme.txt", b"skip"),
            ("XXL操作手册.docx", b"not a valid docx"),
        ],
    );
    let rs = rules::default_rule_sets().remove(0);
    let entries = scan::scan_uploads(
        &[zip_path.to_string_lossy().into_owned()],
        &rs,
        &temp.path().join("workspace"),
        &opts(),
    )
    .unwrap();
    let entry = entries.iter().find(|e| e.name == "apollo.json").unwrap();
    assert!(entry.selected);
    assert_eq!(entry.system.as_deref(), Some("apollo"));
    assert!(
        !entries
            .iter()
            .find(|e| e.name == "readme.txt")
            .unwrap()
            .selected
    );
    assert_eq!(
        entries
            .iter()
            .find(|e| e.kind == "word")
            .unwrap()
            .parse_status,
        "error"
    );
    let parsed = parse::parse_file(
        std::path::Path::new(&entry.abs_path),
        &entry.kind,
        12000,
        8000,
    )
    .unwrap();
    let reply =
        "通过\n【依据】\n- retry 已配置\n【风险】\n- 无\n【整改项】\n- 无\n引用：$.retry | 3";
    let (url, _) = mock(vec![sse(reply)], "text/event-stream", 200, 0);
    let raw = gateway::chat_stream(
        &cfg(url),
        &format!(
            "{}\n{}",
            results::build_instruction("核对重试", &entry.path, None, "standard"),
            parsed.text
        ),
        None,
        |_| {},
    )
    .await
    .unwrap();
    let result = results::parse_result(
        &entry.path,
        entry.system.clone(),
        "tpl-apollo",
        1,
        0,
        &raw,
        "standard",
    );
    assert!(result.error.is_none());
    assert_eq!(result.verdict, "通过");
    let db = temp.path().join("history.db");
    store::save_task(
        &db,
        &summary("mixed"),
        std::slice::from_ref(&result),
        &rs,
        &rules::default_templates(),
    )
    .unwrap();
    assert_eq!(
        store::get_task(&db, "mixed").unwrap().unwrap().files[0].raw,
        reply
    );
    let report = temp.path().join("报告.xlsx");
    let row = export::ExportRow {
        system: "Apollo".into(),
        file: entry.path.clone(),
        file_type: "JSON".into(),
        rule: "*apollo*.json".into(),
        verdict: result.verdict,
        citations: "$.retry | 3".into(),
        time: "2026-10-09".into(),
    };
    export::write_xlsx(report.to_str().unwrap(), &[row]).unwrap();
    let text = parse::parse_file(&report, "excel", 12000, 8000)
        .unwrap()
        .text;
    assert!(text.contains("Apollo"));
    assert!(text.contains("$.retry"));
    assert!(chunk::char_len(&parsed.text) > 0);
}
#[cfg(target_os = "windows")]
#[test]
fn windows_credential_store_round_trip() {
    let name = format!("qa-{}", uuid::Uuid::new_v4());
    let entry = keyring::Entry::new("config-reviewer-qa", &name).unwrap();
    entry.set_password("temporary-mock-key").unwrap();
    let result = entry.get_password();
    let cleanup = entry.delete_credential();
    assert_eq!(result.unwrap(), "temporary-mock-key");
    cleanup.unwrap();
    assert!(entry.get_password().is_err());
}

#[test]
fn config_atomic_replace_and_backup_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    let first = config::AppConfig {
        model: "old-model".into(),
        ..Default::default()
    };
    config::save(&path, &first).unwrap();
    let second = config::AppConfig {
        model: "new-model".into(),
        ..Default::default()
    };
    config::save(&path, &second).unwrap();
    assert_eq!(config::load(&path).model, "new-model");
    std::fs::write(&path, "truncated").unwrap();
    assert_eq!(config::load(&path).model, "old-model");
    assert!(!std::fs::read_dir(temp.path()).unwrap().any(|e| e
        .unwrap()
        .path()
        .extension()
        .unwrap()
        .to_string_lossy()
        .starts_with("tmp-")));
}
#[test]
fn missing_upload_is_not_silently_omitted() {
    let temp = tempfile::tempdir().unwrap();
    assert!(scan::scan_uploads(
        &[temp
            .path()
            .join("missing.json")
            .to_string_lossy()
            .into_owned()],
        &rules::default_rule_sets()[0],
        &temp.path().join("work"),
        &opts()
    )
    .is_err());
}
#[cfg(unix)]
#[test]
fn unreadable_directory_is_not_silently_omitted() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let blocked = temp.path().join("blocked");
    std::fs::create_dir(&blocked).unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o0)).unwrap();
    let result = scan::scan_directory(
        temp.path(),
        &rules::default_rule_sets()[0],
        &temp.path().join("work"),
        &opts(),
    );
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
}

#[test]
fn inline_standard_sections_and_missing_sections() {
    let good = results::parse_result(
        "a",
        None,
        "t",
        1,
        0,
        "通过\n**【依据】** 配置齐全\n【风险】无\n整改项：无",
        "standard",
    );
    assert!(good.error.is_none());
    assert_eq!(good.basis, vec!["配置齐全"]);
    assert_eq!(good.risks, vec!["无"]);
    let missing = results::parse_result("a", None, "t", 1, 0, "通过", "standard");
    assert!(missing.error.is_some());
}

#[test]
fn windows_globs_ignore_case_and_accept_backslashes() {
    assert!(rules::pattern_matches(
        "*apollo*.json",
        "configs/APOLLO-export.JSON",
        "APOLLO-export.JSON"
    ));
    assert!(rules::pattern_matches(
        "星云\\**",
        "星云/中文.CONF",
        "中文.CONF"
    ));
}

#[test]
fn rule_match_modes_priority_disabled_and_legacy_config() {
    use crate::model::{Rule, RuleSet};
    let legacy: Rule = serde_json::from_value(serde_json::json!({
        "id": "legacy", "pattern": "*xxl*", "system": "other", "template_id": "tpl-generic"
    }))
    .unwrap();
    assert_eq!(legacy.match_type, "glob");
    let mut set = RuleSet {
        id: "test".into(),
        name: "test".into(),
        builtin: false,
        rules: vec![legacy],
    };
    for (mode, pattern, positive, negative) in [
        (
            "filename",
            "XXL配置.xlsx",
            "xxl配置.XLSX",
            "old-XXL配置.xlsx",
        ),
        (
            "keyword",
            "XXL-JOB",
            "系统-xxl-job说明.docx",
            "系统说明.docx",
        ),
        ("extension", ".xlsx", "普通配置.XLSX", "配置.xlsx.txt"),
        (
            "glob",
            "*xxl*.{xlsx,docx,txt}",
            "xxl任务.txt",
            "xxl任务.pdf",
        ),
    ] {
        let rule = Rule {
            id: "new".into(),
            pattern: pattern.into(),
            match_type: mode.into(),
            system: "xxl".into(),
            template_id: "tpl-xxl".into(),
            enabled: true,
        };
        set.rules.insert(0, rule);
        rules::validate(&set).unwrap();
        assert_eq!(rules::resolve(&set, positive, positive).unwrap().0, "new");
        assert_ne!(
            rules::resolve(&set, negative, negative).map(|r| r.0),
            Some("new".into())
        );
        set.rules[0].enabled = false;
        assert_ne!(
            rules::resolve(&set, positive, positive).map(|r| r.0),
            Some("new".into())
        );
        set.rules.remove(0);
    }
    set.rules[0].pattern = "[".into();
    assert!(rules::validate(&set).is_err());
}

#[test]
fn scan_reports_real_totals_and_can_cancel_before_parsing() {
    let temp = tempfile::tempdir().unwrap();
    for name in ["apollo-a.json", "apollo-b.json", "unmatched.txt"] {
        std::fs::write(temp.path().join(name), "{\"timeout\":30}").unwrap();
    }
    let mut progress = scan::initial_progress("test".into());
    let mut events = Vec::new();
    let entries = scan::scan_directory_with_progress(
        temp.path(),
        &rules::default_rule_sets()[0],
        &temp.path().join("work"),
        &opts(),
        &mut progress,
        &mut |p| {
            events.push(p.clone());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(entries.iter().filter(|e| e.matched).count(), 2);
    assert!(events
        .iter()
        .any(|p| p.phase == "discover" && p.total.is_none()));
    let last = events.last().unwrap();
    assert_eq!(last.phase, "done");
    assert_eq!(last.total, Some(3));
    assert_eq!(last.processed, 3);
    assert_eq!(last.matched, 2);
    let result = scan::scan_directory_with_progress(
        temp.path(),
        &rules::default_rule_sets()[0],
        &temp.path().join("work"),
        &opts(),
        &mut scan::initial_progress("cancel".into()),
        &mut |p| {
            if p.phase == "parse" {
                Err("扫描已取消".into())
            } else {
                Ok(())
            }
        },
    );
    assert_eq!(result.unwrap_err(), "扫描已取消");
}

#[test]
fn unmatched_large_scan_counts_every_file_without_a_file_limit() {
    let temp = tempfile::tempdir().unwrap();
    for i in 0..1001 {
        std::fs::write(temp.path().join(format!("unmatched-{i}.txt")), "a").unwrap();
    }
    let mut options = opts();
    options.matched_only = true;
    let mut progress = scan::initial_progress("unmatched".into());
    let entries = scan::scan_directory_with_progress(
        temp.path(),
        &rules::default_rule_sets()[0],
        &temp.path().join("work"),
        &options,
        &mut progress,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert!(entries.is_empty());
    assert_eq!(progress.discovered, 1001);
    assert_eq!(progress.processed, 1001);
    assert_eq!(progress.total, Some(1001));
    assert_eq!(progress.matched, 0);
}

#[test]
fn zip_progress_and_cancel_during_extraction() {
    let temp = tempfile::tempdir().unwrap();
    let zip_path = temp.path().join("release.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("apollo.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    use std::io::Write;
    zip.write_all(b"{\"retry\":3}").unwrap();
    zip.finish().unwrap();
    let mut progress = scan::initial_progress("zip".into());
    let mut saw_extract = false;
    let result = scan::scan_uploads_with_progress(
        &[zip_path.to_string_lossy().into_owned()],
        &rules::default_rule_sets()[0],
        &temp.path().join("work"),
        &opts(),
        &mut progress,
        &mut |p| {
            if p.phase == "extract" {
                saw_extract = true;
                return Err("扫描已取消".into());
            }
            Ok(())
        },
    );
    assert!(saw_extract);
    assert_eq!(result.unwrap_err(), "扫描已取消");
    assert!(!temp.path().join("work").exists());
}

#[test]
fn legacy_doc_chinese_tables_repetition_and_chunks() {
    let bytes = include_bytes!("../tests/fixtures/legacy-word.doc");
    let text = parse::legacy_doc_text(bytes).unwrap();
    assert!(
        text.contains("执行器：订单同步；超时 30 秒；失败重试 3 次。"),
        "{text}"
    );
    assert!(text.contains("订单同步 | orderSyncJob | 3"), "{text}");
    assert!(text.contains("库存同步 | stockSyncJob | 2"), "{text}");
    assert_eq!(text.matches("重复检查").count(), 2);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("XXL操作手册.DOC");
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(parse::kind_for("DOC"), "word");
    let parsed = parse::parse_file(&path, "word", 40, 30).unwrap();
    assert_eq!(parsed.text, text);
    assert!(parsed.chunks.len() > 1);
}

#[test]
fn legacy_doc_rejects_fake_and_truncated_files() {
    assert!(parse::legacy_doc_text(b"fake doc")
        .unwrap_err()
        .contains("不是"));
    let bytes = include_bytes!("../tests/fixtures/legacy-word.doc");
    assert!(parse::legacy_doc_text(&bytes[..512])
        .unwrap_err()
        .contains("损坏"));
}

#[test]
fn legacy_doc_rejects_encrypted_and_old_versions() {
    use std::io::{Cursor, Seek, SeekFrom};
    let original = include_bytes!("../tests/fixtures/legacy-word.doc");
    for (offset, replacement, expected) in [(10, 0x0100u16, "已加密"), (2, 0x0065u16, "Word 6/95")]
    {
        let mut compound = cfb::CompoundFile::open(Cursor::new(original.to_vec())).unwrap();
        {
            let mut word = compound.open_stream("WordDocument").unwrap();
            word.seek(SeekFrom::Start(offset)).unwrap();
            word.write_all(&replacement.to_le_bytes()).unwrap();
        }
        let bytes = compound.into_inner().into_inner();
        assert!(parse::legacy_doc_text(&bytes)
            .unwrap_err()
            .contains(expected));
    }
}

#[test]
fn legacy_doc_in_zip_routes_and_parses() {
    let temp = tempfile::tempdir().unwrap();
    let zip = temp.path().join("发布包.zip");
    zip_fixture(
        &zip,
        &[(
            "XXL操作手册.DOC",
            include_bytes!("../tests/fixtures/legacy-word.doc"),
        )],
    );
    let rs = rules::default_rule_sets().remove(0);
    let entries = scan::scan_uploads(
        &[zip.to_string_lossy().into_owned()],
        &rs,
        &temp.path().join("workspace"),
        &opts(),
    )
    .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].parse_status, "ok");
    assert_eq!(entries[0].system.as_deref(), Some("xxl"));
    assert!(entries[0].preview.contains("orderSyncJob"));
}

#[test]
fn module_migration_preserves_conflicting_rules_and_shared_templates() {
    let mut config = config::AppConfig {
        schema_version: 0,
        output_format: "json".into(),
        ..Default::default()
    };
    config.systems[1].default_template_id = "tpl-apollo".into();
    config.rule_sets[0].rules[0].template_id = "tpl-generic".into();
    let migrated = config.merged();
    let rule = &migrated.rule_sets[0].rules[0];
    assert_ne!(rule.system, "apollo");
    assert!(migrated
        .systems
        .iter()
        .find(|m| m.id == rule.system)
        .unwrap()
        .name
        .contains("通用文档评审"));
    assert_eq!(
        migrated.module_template(&rule.system).unwrap().content,
        rules::default_templates()
            .into_iter()
            .find(|t| t.id == "tpl-generic")
            .unwrap()
            .content
    );
    assert_eq!(
        rule.template_id,
        migrated.module_template(&rule.system).unwrap().id
    );
    let unique: std::collections::HashSet<_> = migrated
        .systems
        .iter()
        .map(|m| &m.default_template_id)
        .collect();
    assert_eq!(unique.len(), migrated.systems.len());
    assert!(migrated.templates.iter().all(|t| t.output_format == "json"));
    let once = serde_json::to_value(&migrated).unwrap();
    assert_eq!(once, serde_json::to_value(migrated.merged()).unwrap());
}

#[test]
fn deleted_modules_and_templates_stay_deleted_after_reload() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    let mut config = config::AppConfig::default();
    config.systems.clear();
    config.templates.clear();
    config.rule_sets.clear();
    config::save(&path, &config).unwrap();
    let loaded = config::load(&path);
    assert!(loaded.systems.is_empty());
    assert!(loaded.templates.is_empty());
    assert!(loaded.rule_sets.is_empty());
}

#[test]
fn save_derives_rule_template_from_module_and_validates_references() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    let mut config = config::AppConfig::default();
    config.rule_sets[0].rules[0].template_id = "stale-template".into();
    config::save(&path, &config).unwrap();
    assert_eq!(
        config::load(&path).rule_sets[0].rules[0].template_id,
        "tpl-apollo"
    );
    config.rule_sets[0].rules[0].system = "removed-module".into();
    assert!(config::save(&path, &config)
        .unwrap_err()
        .contains("评审模块不存在"));
    config.rule_sets.clear();
    config.systems[0].name = " ".into();
    assert!(config::save(&path, &config).is_err());
    config.systems[0].name = "Apollo".into();
    config.templates[0].content.clear();
    assert!(config::save(&path, &config).is_err());
}

fn module_review_request() -> crate::model::ReviewRequest {
    serde_json::from_value(serde_json::json!({
        "task_id": "test", "cfg": {"base_url":"http://localhost", "model":"mock"},
        "files": [{"path":"apollo.txt", "abs_path":"/tmp/apollo.txt", "name":"apollo.txt", "kind":"text", "system":"apollo", "template_id":"stale", "template_content":"UNTRUSTED PROMPT"}]
    })).unwrap()
}

#[test]
fn review_uses_current_module_input_prompt_and_output_contract() {
    let mut config = config::AppConfig::default();
    let template = config
        .templates
        .iter_mut()
        .find(|t| t.id == "tpl-apollo")
        .unwrap();
    template.input_instructions = "输入为变更前后配置，请对比差异".into();
    template.output_instructions = "依据中列出所有差异项".into();
    template.content = "检查新增端点".into();
    template.output_format = "json".into();
    let mut request = module_review_request();
    config.prepare_review(&mut request).unwrap();
    let file = &request.files[0];
    assert_eq!(file.template_id, "tpl-apollo");
    assert_eq!(file.output_format, "json");
    let instruction = results::build_instruction(
        &file.template_content,
        &file.name,
        None,
        &file.output_format,
    );
    for expected in [
        "输入为变更前后配置",
        "依据中列出所有差异项",
        "检查新增端点",
        "只输出一个 JSON",
    ] {
        assert!(instruction.contains(expected), "missing {expected}");
    }
    assert!(!instruction.contains("UNTRUSTED PROMPT"));
    config
        .templates
        .iter_mut()
        .find(|t| t.id == "tpl-apollo")
        .unwrap()
        .content = "新的检查要求".into();
    config.prepare_review(&mut request).unwrap();
    assert!(request.files[0].template_content.contains("新的检查要求"));
    config.systems.retain(|m| m.id != "apollo");
    assert!(config.prepare_review(&mut request).is_err());
}

#[test]
fn combined_review_rejects_mixed_module_contracts() {
    let config = config::AppConfig::default();
    let mut request = module_review_request();
    request.granularity = "single".into();
    let mut second = request.files[0].clone();
    second.system = Some("xxl".into());
    request.files.push(second);
    assert!(config
        .prepare_review(&mut request)
        .unwrap_err()
        .contains("同一个评审模块"));
    request.files[1].system = Some("apollo".into());
    config.prepare_review(&mut request).unwrap();
}

#[test]
fn legacy_template_snapshot_deserializes_without_new_fields() {
    let template: crate::model::Template = serde_json::from_value(serde_json::json!({
        "id":"legacy", "name":"旧规范", "content":"旧 Prompt", "builtin":false
    }))
    .unwrap();
    assert_eq!(template.output_format, "standard");
    assert!(template.input_instructions.is_empty());
    assert!(template.output_instructions.is_empty());
}

#[test]
fn migration_is_saved_once_and_subsequent_edits_remain_stable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    let mut old = config::AppConfig {
        schema_version: 0,
        ..Default::default()
    };
    old.rule_sets[0].rules[0].template_id = "tpl-generic".into();
    std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    let mut migrated = config::load(&path);
    let first = serde_json::to_value(&migrated).unwrap();
    assert_eq!(first, serde_json::to_value(config::load(&path)).unwrap());
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["schema_version"], config::CURRENT_SCHEMA_VERSION);
    migrated.systems.retain(|m| m.id != "apollo");
    migrated.templates.retain(|t| t.id != "tpl-apollo");
    config::save(&path, &migrated).unwrap();
    assert!(!config::load(&path).systems.iter().any(|m| m.id == "apollo"));
}

#[tokio::test]
async fn http_errors_include_safe_provider_details_and_own_user_agent() {
    for streaming in [false, true] {
        let body = serde_json::to_vec(&serde_json::json!({"error":{"type":"MissingSessionID","message":"missing conversation header; key=mock-key"}})).unwrap();
        let (url, rx) = mock(vec![body], "application/json", 400, 0);
        let config = cfg(url);
        let error = if streaming {
            gateway::chat_stream(&config, "test", None, |_| {})
                .await
                .unwrap_err()
        } else {
            gateway::chat(&config, "test").await.unwrap_err()
        };
        assert!(error.contains("HTTP 400"));
        assert!(error.contains("MissingSessionID"));
        assert!(error.contains("missing conversation header"));
        assert!(!error.contains("mock-key"));
        let received = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(received.to_lowercase().contains(&format!(
            "user-agent: config-reviewer/{}",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(!received.to_lowercase().contains("x-opencode-session"));
    }
}

#[test]
fn citation_only_basis_is_valid_standard_review_and_keeps_business_ids() {
    let raw = "阻断\n【依据】\n- 引用：Sheet!7 | 9003 调度器缺少回滚方案\n【风险】\n9003 任务可能重复执行\n9003.5 参数缺少说明\n【整改项】\n1. 为任务 9003 补齐回滚方案\n2、 明确重试次数";
    let result = results::parse_result("任务表.xlsx", None, "tpl", 1, 0, raw, "standard");
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.verdict, "阻断");
    assert_eq!(
        result.basis,
        vec!["引用：Sheet!7 | 9003 调度器缺少回滚方案"]
    );
    assert_eq!(
        result.risks,
        vec!["9003 任务可能重复执行", "9003.5 参数缺少说明"]
    );
    assert_eq!(
        result.remediations,
        vec!["为任务 9003 补齐回滚方案", "明确重试次数"]
    );
    assert_eq!(result.citations.len(), 1);
    assert_eq!(result.citations[0].locator, "Sheet!7");
    assert_eq!(result.citations[0].snippet, "9003 调度器缺少回滚方案");
}

#[test]
fn numbered_citations_remain_available_as_basis_and_structured_citations() {
    let raw = "有条件通过\n【依据】\n1. 引用：第 3 行 | 1234 参数已配置\n【风险】\n1) 缺少确认记录\n【整改项】\n1. 补充确认记录";
    let result = results::parse_result("配置.txt", None, "tpl", 1, 0, raw, "standard");
    assert!(result.error.is_none());
    assert_eq!(result.basis, vec!["引用：第 3 行 | 1234 参数已配置"]);
    assert_eq!(result.citations.len(), 1);
    assert_eq!(result.citations[0].snippet, "1234 参数已配置");
}

#[test]
fn inline_task_prefixed_citations_extract_separately_without_swallowing_commentary() {
    let raw = "阻断\n【依据】\n- 9003：引用：XXL-JOB配置.xlsx / Sheet: 发布约定 第11行 | `必填 bizDate、batchNo、dryRun；当前接口无幂等键`；引用：XXL-JOB配置.xlsx / Sheet: 调度运行 第8行 | `9003 | CRON | 0 0 3 * * ?`；六段 Cron 逐位复核。\n【风险】\n- 9004：引用：任务基础第9行 | 报警邮件为空；需确认替代告警。\n【整改项】\n- 9003：补齐 batchNo。";
    let result = results::parse_result("配置.xlsx", None, "tpl", 1, 0, raw, "standard");
    assert!(result.error.is_none());
    assert!(result.basis[0].starts_with("9003：引用："));
    assert_eq!(result.citations.len(), 3);
    assert_eq!(
        result.citations[0].locator,
        "XXL-JOB配置.xlsx / Sheet: 发布约定 第11行"
    );
    assert_eq!(
        result.citations[0].snippet,
        "必填 bizDate、batchNo、dryRun；当前接口无幂等键"
    );
    assert_eq!(
        result.citations[1].locator,
        "XXL-JOB配置.xlsx / Sheet: 调度运行 第8行"
    );
    assert_eq!(result.citations[1].snippet, "9003 | CRON | 0 0 3 * * ?");
    assert_eq!(result.citations[2].snippet, "报警邮件为空");
}

#[test]
fn xls_inline_citations_keep_table_cells_and_skip_shorthand_without_locator() {
    let raw = "阻断\n【依据】\n- 9006 支付异常订单补偿：依据：引用：发布约定第14行 | `必填 bizDate、dryRun；未读取分片序号`；同引用：`P95 80 秒；两台实例部署相同 Handler`；引用：高级配置第11行 | `SHARDING_BROADCAST（分片广播） | 超时 180 | 失败重试次数 2`；单机串行不能防止跨机重复。\n【风险】\n- 引用：发布约定第14行 | `必填 bizDate、dryRun；未读取分片序号`。\n【整改项】\n- 增加幂等保护。";
    let result = results::parse_result("配置.xls", None, "tpl", 1, 0, raw, "standard");
    assert!(result.error.is_none());
    assert_eq!(result.citations.len(), 2);
    assert_eq!(result.citations[0].locator, "发布约定第14行");
    assert_eq!(
        result.citations[0].snippet,
        "必填 bizDate、dryRun；未读取分片序号"
    );
    assert_eq!(result.citations[1].locator, "高级配置第11行");
    assert_eq!(
        result.citations[1].snippet,
        "SHARDING_BROADCAST（分片广播） | 超时 180 | 失败重试次数 2"
    );
}

#[test]
fn chinese_quoted_citations_preserve_semicolons_and_following_implicit_sheet_locators() {
    let raw = "阻断\n【依据】\n9006：引用：Sheet: 发布约定 行14 | “必填 bizDate、dryRun；每次查询当天全部待补偿订单；未读取分片序号；外部接口无幂等键。”；Sheet: 调度运行 行11 | “0 0/15 * * * ? | paymentCompensateJob”；Sheet: 高级配置 行11 | “SHARDING_BROADCAST | SERIAL_EXECUTION | 180 | 2”。两台会全量处理。\n【风险】\n9006：引用：Sheet: 发布约定 行14 | “未读取分片序号；外部接口无幂等键”；Sheet: 高级配置 行11 | “SHARDING_BROADCAST | SERIAL_EXECUTION | 180 | 2”。\n【整改项】\n补充幂等。";
    let result = results::parse_result("配置.xlsx", None, "tpl", 1, 0, raw, "standard");
    assert!(result.error.is_none());
    assert_eq!(result.citations.len(), 4);
    assert_eq!(
        result.citations[0].snippet,
        "必填 bizDate、dryRun；每次查询当天全部待补偿订单；未读取分片序号；外部接口无幂等键。"
    );
    assert_eq!(result.citations[1].locator, "Sheet: 调度运行 行11");
    assert_eq!(
        result.citations[1].snippet,
        "0 0/15 * * * ? | paymentCompensateJob"
    );
    assert_eq!(result.citations[2].locator, "Sheet: 高级配置 行11");
    assert_eq!(
        result.citations[2].snippet,
        "SHARDING_BROADCAST | SERIAL_EXECUTION | 180 | 2"
    );
    assert!(result.basis[0].starts_with("9006：引用："));
}

#[test]
fn bang_row_locators_are_extracted_without_repeated_marker_but_narrative_is_not() {
    let raw = "阻断\n【依据】\n任务ID 9003：引用：调度运行!第8行 | “9003 | CRON | 0 0 3 * * ?”；发布约定!第11行 | “必填 bizDate、batchNo、dryRun；当前接口无幂等键”；高级配置!第8行 | “9003 | ROUND | 300 | 2”；不要把叙述当定位 | 这不是引用。\n【风险】\n重复发放。\n【整改项】\n补充参数。";
    let result = results::parse_result("配置.xls", None, "tpl", 1, 0, raw, "standard");
    assert!(result.error.is_none());
    assert_eq!(result.citations.len(), 3);
    assert_eq!(result.citations[1].locator, "发布约定!第11行");
    assert_eq!(
        result.citations[1].snippet,
        "必填 bizDate、batchNo、dryRun；当前接口无幂等键"
    );
    assert_eq!(result.citations[2].locator, "高级配置!第8行");
    assert_eq!(result.citations[2].snippet, "9003 | ROUND | 300 | 2");
}

#[test]
fn large_directory_and_multiple_uploads_have_exact_counts_without_git_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("repo");
    std::fs::create_dir_all(dir.join(".git/objects")).unwrap();
    std::fs::create_dir_all(dir.join("worktree")).unwrap();
    std::fs::write(dir.join(".git/objects/apollo-private.json"), "{}").unwrap();
    std::fs::write(dir.join("worktree/.git"), "gitdir: ../.git/worktrees/test").unwrap();
    std::fs::write(dir.join(".gitignore"), "target").unwrap();
    std::fs::write(dir.join(".gitmodules"), "test").unwrap();
    let mut paths = Vec::new();
    for i in 0..1001 {
        let path = dir.join(format!("apollo-{i:04}.json"));
        std::fs::write(&path, "{\"timeout\":30}").unwrap();
        paths.push(path.to_string_lossy().into_owned());
    }
    let mut progress = scan::initial_progress("directory".into());
    let entries = scan::scan_directory_with_progress(
        &dir,
        &rules::default_rule_sets()[0],
        &temp.path().join("work"),
        &opts(),
        &mut progress,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(entries.len(), 1003);
    assert_eq!(progress.discovered, 1003);
    assert_eq!(progress.processed, 1003);
    assert_eq!(progress.total, Some(1003));
    assert_eq!(progress.matched, 1001);
    assert!(entries
        .iter()
        .all(|entry| !entry.path.starts_with(".git/") && entry.name != ".git"));
    assert!(entries.iter().any(|entry| entry.name == ".gitignore"));
    assert!(entries.iter().any(|entry| entry.name == ".gitmodules"));
    paths.push(
        dir.join(".git/objects/apollo-private.json")
            .to_string_lossy()
            .into_owned(),
    );
    paths.push(dir.join("worktree/.git").to_string_lossy().into_owned());
    let mut uploaded = scan::initial_progress("uploads".into());
    let entries = scan::scan_uploads_with_progress(
        &paths,
        &rules::default_rule_sets()[0],
        &temp.path().join("work"),
        &opts(),
        &mut uploaded,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(entries.len(), 1001);
    assert_eq!(uploaded.discovered, 1001);
    assert_eq!(uploaded.processed, 1001);
    assert_eq!(uploaded.total, Some(1001));
    assert!(entries.iter().all(|entry| entry.parse_status == "ok"));
}

#[test]
fn large_zip_has_exact_extraction_and_parse_counts_without_git_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("large.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    zip.add_directory("configs/", SimpleFileOptions::default())
        .unwrap();
    for i in 0..1001 {
        zip.start_file(
            format!("configs/apollo-{i:04}.json"),
            SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"{\"timeout\":30}").unwrap();
    }
    for name in [
        ".git/config",
        "worktree/.git",
        "nested/.GIT/objects/apollo-secret.json",
        "windows\\.git\\config",
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(b"metadata").unwrap();
    }
    zip.finish().unwrap();
    let mut progress = scan::initial_progress("large-zip".into());
    let mut extract_events = Vec::new();
    let entries = scan::scan_uploads_with_progress(
        &[path.to_string_lossy().into_owned()],
        &rules::default_rule_sets()[0],
        &temp.path().join("work"),
        &opts(),
        &mut progress,
        &mut |p| {
            if p.phase == "extract" {
                extract_events.push(p.clone());
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(entries.len(), 1001);
    assert_eq!(progress.discovered, 1001);
    assert_eq!(progress.processed, 1001);
    assert_eq!(progress.total, Some(1001));
    assert!(entries.iter().all(|entry| entry.parse_status == "ok"));
    assert!(extract_events
        .iter()
        .any(|p| p.extract_total == Some(1001) && p.extract_processed == 0));
    assert!(extract_events
        .iter()
        .any(|p| p.extract_total == Some(1001) && p.extract_processed == 1001));
    assert!(extract_events
        .iter()
        .all(|p| p.processed == 0 && p.total.is_none()));
    assert!(extract_events
        .windows(2)
        .all(|pair| pair[0].extract_processed <= pair[1].extract_processed));
    let mut cancelled = scan::initial_progress("cancel-zip".into());
    let cancel_workspace = temp.path().join("cancel-work");
    let error = scan::scan_uploads_with_progress(
        &[path.to_string_lossy().into_owned()],
        &rules::default_rule_sets()[0],
        &cancel_workspace,
        &opts(),
        &mut cancelled,
        &mut |p| {
            if p.extract_processed == 3 {
                Err("扫描已取消".into())
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    assert_eq!(error, "扫描已取消");
    assert_eq!(cancelled.extract_processed, 3);
    assert_eq!(cancelled.extract_total, Some(1001));
    assert_eq!(
        walkdir::WalkDir::new(&cancel_workspace)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .count(),
        3
    );
}

#[test]
fn old_config_file_limit_is_ignored_and_not_saved_again() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.json");
    let mut value = serde_json::to_value(config::AppConfig::default()).unwrap();
    value["max_files"] = serde_json::json!(800);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let loaded = config::load(&path);
    config::save(&path, &loaded).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(saved.get("max_files").is_none());
    assert_eq!(saved["max_file_mb"], value["max_file_mb"]);
    value["max_files"] = serde_json::json!(0);
    assert!(serde_json::from_value::<config::AppConfig>(value).is_ok());
}
