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
        max_files: 800,
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
    zip_fixture(&path, &[("a.txt", b"x"), ("b.txt", b"x")]);
    small = opts();
    small.max_files = 1;
    assert!(scan::extract_zip(&path, &temp.path().join("out"), &small).is_err());
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
    assert_eq!(r.remediations, vec!["补回滚"]);
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
fn scan_enforces_total_limit_even_when_no_rules_match() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("a.txt"), "a").unwrap();
    std::fs::write(temp.path().join("b.txt"), "b").unwrap();
    let mut options = opts();
    options.max_files = 1;
    options.matched_only = true;
    let result = scan::scan_directory(
        temp.path(),
        &rules::default_rule_sets()[0],
        &temp.path().join("work"),
        &options,
    );
    assert!(result.unwrap_err().contains("超过上限"));
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
