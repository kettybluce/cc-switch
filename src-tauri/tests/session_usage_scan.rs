//! Isolated-HOME session usage scan for Claude / Codex / Pi JSONL.
//!
//! Live Windows layout (never written by these tests):
//! ```text
//! \\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude\projects\...\*.jsonl
//! \\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.codex\sessions\YYYY\MM\DD\*.jsonl
//! \\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi\agent\sessions\<project>\*.jsonl
//! ```
//!
//! Cloud stand-in is a POSIX tree under the isolated test HOME. No C: writes,
//! no `pi-wsl-sessions` mirror, SCHEMA 18 only (no `proxy_config` row for `pi`).
//! Session JSONL has tokens/cost but no latency/TTFT; stored `latency_ms=0`
//! (NOT NULL) and `first_token_ms=NULL` must not be invented from timestamps.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use cc_switch_lib::{
    get_data_source_breakdown, session_sync_mutex, sync_all_unlocked, Database, LogFilters,
    RequestLogDetail, SessionSyncResult, SCHEMA_VERSION,
};

#[path = "support.rs"]
mod support;
use support::{create_test_state, ensure_test_home, reset_test_fs, test_mutex};

const CLAUDE_MODEL: &str = "fixture-claude-scan-model";
const CODEX_MODEL: &str = "fixture-codex-scan-model";
const PI_MODEL: &str = "fixture-pi-scan-model";
const CODEX_THREAD: &str = "00000000-0000-4000-8000-00000000c0de";
const SCAN_TS: &str = "2026-09-01T12:00:00Z";

/// Claude main-session assistant A.
const CLAUDE_A: (u32, u32, u32, u32) = (11, 22, 33, 44);
/// Claude main-session assistant B.
const CLAUDE_B: (u32, u32, u32, u32) = (55, 66, 77, 88);
/// Claude sub-agent assistant (deeper than the project JSONL).
const CLAUDE_SUB: (u32, u32, u32, u32) = (1, 2, 3, 4);
/// Codex cumulative snapshot (input / cached_input / output).
const CODEX_TOTAL: (u64, u64, u64) = (90, 12, 34);
/// Pi assistant (input / output / cacheRead / cacheWrite).
const PI_USAGE: (u32, u32, u32, u32) = (13, 17, 19, 23);

struct ScanDirs {
    home: PathBuf,
    claude_projects: PathBuf,
    codex_day: PathBuf,
    codex_archived: PathBuf,
    pi_sessions: PathBuf,
}

fn assert_isolated_home_path(path: &Path) {
    let text = path.to_string_lossy();
    assert!(
        !text.to_ascii_lowercase().contains("pi-wsl-sessions"),
        "must not use a C: session mirror: {text}"
    );
    #[cfg(unix)]
    {
        assert!(
            !text.contains("C:") && !text.to_ascii_lowercase().contains("c:\\"),
            "Linux stand-in must not touch Windows C:: {text}"
        );
    }
}

fn assert_schema18_no_pi_proxy_row(db: &Database) {
    assert_eq!(
        SCHEMA_VERSION, 18,
        "session scan fixtures must stay on SCHEMA 18"
    );
    assert!(
        db.has_proxy_config_row("claude")
            .expect("claude proxy_config row"),
        "Claude still owns the shared listen row"
    );
    assert!(
        !db.has_proxy_config_row("pi").expect("pi proxy_config row"),
        "SCHEMA 18 CHECK must not gain a proxy_config row for pi"
    );
}

fn scan_dirs(home: &Path) -> ScanDirs {
    ScanDirs {
        home: home.to_path_buf(),
        claude_projects: home.join(".claude").join("projects"),
        codex_day: home
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("09")
            .join("01"),
        codex_archived: home.join(".codex").join("archived_sessions"),
        pi_sessions: home.join(".pi").join("agent").join("sessions"),
    }
}

fn create_empty_session_dirs(dirs: &ScanDirs) {
    fs::create_dir_all(dirs.claude_projects.join("empty-project")).expect("empty Claude project");
    fs::create_dir_all(&dirs.codex_day).expect("empty Codex day dir");
    fs::create_dir_all(&dirs.codex_archived).expect("empty Codex archive");
    fs::create_dir_all(dirs.pi_sessions.join("empty-project")).expect("empty Pi project");
    for path in [
        &dirs.home,
        &dirs.claude_projects,
        &dirs.codex_day,
        &dirs.codex_archived,
        &dirs.pi_sessions,
    ] {
        assert_isolated_home_path(path);
    }
}

fn write_jsonl(path: &Path, lines: &[String]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create jsonl parent");
    }
    assert_isolated_home_path(path);
    let mut body = lines.join("\n");
    body.push('\n');
    fs::write(path, body).expect("write jsonl");
}

fn claude_assistant_line(msg_id: &str, tokens: (u32, u32, u32, u32), session_id: &str) -> String {
    let (input, output, cache_read, cache_create) = tokens;
    json!({
        "type": "assistant",
        "message": {
            "id": msg_id,
            "model": CLAUDE_MODEL,
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "cache_read_input_tokens": cache_read,
                "cache_creation_input_tokens": cache_create
            },
            "stop_reason": "end_turn"
        },
        "timestamp": SCAN_TS,
        "sessionId": session_id
    })
    .to_string()
}

fn codex_session_meta() -> String {
    json!({
        "timestamp": SCAN_TS,
        "type": "session_meta",
        "payload": {
            "id": CODEX_THREAD,
            "source": "cli"
        }
    })
    .to_string()
}

fn codex_turn_context() -> String {
    json!({
        "timestamp": SCAN_TS,
        "type": "turn_context",
        "payload": { "model": CODEX_MODEL }
    })
    .to_string()
}

fn codex_token_count() -> String {
    let (input, cached, output) = CODEX_TOTAL;
    json!({
        "timestamp": SCAN_TS,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": {
                    "input_tokens": input,
                    "cached_input_tokens": cached,
                    "output_tokens": output,
                    "reasoning_output_tokens": 0,
                    "total_tokens": input + output
                }
            }
        }
    })
    .to_string()
}

fn pi_session_header() -> String {
    json!({
        "type": "session",
        "version": 3,
        "id": "scan-session-a",
        "timestamp": SCAN_TS,
        "cwd": "/work/scan"
    })
    .to_string()
}

fn pi_assistant_line() -> String {
    let (input, output, cache_read, cache_write) = PI_USAGE;
    json!({
        "type": "message",
        "id": "scan-assistant",
        "parentId": null,
        "timestamp": SCAN_TS,
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "ok"}],
            "provider": "fixture-pi",
            "model": PI_MODEL,
            "responseId": "scan-response",
            "timestamp": 1_756_728_000_000i64,
            "usage": {
                "input": input,
                "output": output,
                "cacheRead": cache_read,
                "cacheWrite": cache_write,
                "totalTokens": input + output + cache_read + cache_write,
                "cost": {
                    "input": 0,
                    "output": 0,
                    "cacheRead": 0,
                    "cacheWrite": 0,
                    "total": 0
                }
            },
            "stopReason": "stop"
        }
    })
    .to_string()
}

fn corrupt_lines() -> Vec<String> {
    vec![
        "{this is not json".to_string(),
        String::new(),
        "   ".to_string(),
        "null".to_string(),
        "{\"type\":\"truncated\"".to_string(),
        "[]".to_string(),
        "{\"type\":\"user\",\"message\":\"ignore me\"}".to_string(),
    ]
}

fn write_valid_claude_codex_pi_jsonl(dirs: &ScanDirs, with_corrupt: bool) {
    let mut claude_main = vec![
        json!({"type":"user","message":{"role":"user","content":"hi"}}).to_string(),
        claude_assistant_line("msg_scan_a", CLAUDE_A, "scan-claude-session"),
        claude_assistant_line("msg_scan_b", CLAUDE_B, "scan-claude-session"),
        claude_assistant_line("msg_zero", (0, 0, 0, 0), "scan-claude-session"),
    ];
    let mut claude_sub = vec![claude_assistant_line(
        "msg_scan_sub",
        CLAUDE_SUB,
        "scan-claude-session",
    )];
    let mut codex = vec![
        codex_session_meta(),
        codex_turn_context(),
        json!({"timestamp": SCAN_TS, "type": "event_msg", "payload": {"type": "agent_message"}})
            .to_string(),
        codex_token_count(),
    ];
    let mut pi = vec![
        pi_session_header(),
        json!({"type":"message","id":"user","parentId":null,"timestamp":SCAN_TS,"message":{"role":"user","content":"q"}}).to_string(),
        pi_assistant_line(),
    ];
    if with_corrupt {
        let junk = corrupt_lines();
        claude_main.splice(1..1, junk.clone());
        claude_sub.extend(junk.clone());
        codex.splice(2..2, junk.clone());
        pi.splice(1..1, junk);
    }

    write_jsonl(
        &dirs.claude_projects.join("scan-project").join("main.jsonl"),
        &claude_main,
    );
    write_jsonl(
        &dirs
            .claude_projects
            .join("scan-project")
            .join("scan-claude-session")
            .join("subagents")
            .join("agent-scan.jsonl"),
        &claude_sub,
    );
    write_jsonl(
        &dirs
            .codex_day
            .join(format!("rollout-2026-09-01T12-00-00-{CODEX_THREAD}.jsonl")),
        &codex,
    );
    write_jsonl(
        &dirs
            .pi_sessions
            .join("scan-project")
            .join("scan-session.jsonl"),
        &pi,
    );

    fs::create_dir_all(dirs.claude_projects.join("empty-project")).expect("empty Claude project");
    fs::create_dir_all(
        dirs.claude_projects
            .join("scan-project")
            .join("scan-claude-session")
            .join("subagents")
            .join("workflows"),
    )
    .expect("empty workflow dir");
    fs::create_dir_all(&dirs.codex_archived).expect("empty Codex archive");
    fs::create_dir_all(dirs.pi_sessions.join("empty-project")).expect("empty Pi project");
}

fn add_tokens(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    (a.0 + b.0, a.1 + b.1, a.2 + b.2, a.3 + b.3)
}

fn expected_claude_tokens() -> (u32, u32, u32, u32) {
    add_tokens(add_tokens(CLAUDE_A, CLAUDE_B), CLAUDE_SUB)
}

async fn sync_home(db: &Database) -> SessionSyncResult {
    let _sync = session_sync_mutex().lock().await;
    sync_all_unlocked(db)
}

fn assert_unknown_session_ttft(log: &RequestLogDetail) {
    assert_eq!(
        log.latency_ms, 0,
        "SCHEMA 18 session JSONL has no latency; stored 0 (UI shows —) for {}",
        log.request_id
    );
    assert_eq!(
        log.first_token_ms, None,
        "session JSONL has no TTFT; first_token_ms must stay NULL for {}",
        log.request_id
    );
    let source = log
        .data_source
        .as_deref()
        .expect("session import writes data_source");
    assert!(
        source == "session_log" || source.ends_with("_session"),
        "session data_source for dash TTFT, got {source}"
    );
}

fn all_logs(db: &Database) -> Vec<RequestLogDetail> {
    db.get_request_logs(&LogFilters::default(), 0, 50)
        .expect("list request logs")
        .data
}

fn logs_for_source<'a>(logs: &'a [RequestLogDetail], source: &str) -> Vec<&'a RequestLogDetail> {
    logs.iter()
        .filter(|log| log.data_source.as_deref() == Some(source))
        .collect()
}

fn assert_token_row(log: &RequestLogDetail, tokens: (u32, u32, u32, u32)) {
    let (input, output, cache_read, cache_create) = tokens;
    assert_eq!(log.input_tokens, input, "input {}", log.request_id);
    assert_eq!(log.output_tokens, output, "output {}", log.request_id);
    assert_eq!(
        log.cache_read_tokens, cache_read,
        "cache_read {}",
        log.request_id
    );
    assert_eq!(
        log.cache_creation_tokens, cache_create,
        "cache_create {}",
        log.request_id
    );
}

fn assert_quiet_sync(result: &SessionSyncResult) {
    assert_eq!(result.imported, 0, "no session rows: {result:?}");
    assert_eq!(result.files_scanned, 0, "no jsonl files: {result:?}");
    assert!(
        result.errors.is_empty(),
        "empty/missing dirs must not error: {:?}",
        result.errors
    );
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across session scans"
)]
async fn missing_home_session_dirs_are_quiet_and_keep_schema_18() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let home = ensure_test_home();
    reset_test_fs();
    assert_isolated_home_path(home);

    let state = create_test_state().expect("create test state");
    assert_schema18_no_pi_proxy_row(&state.db);

    let result = sync_home(&state.db).await;
    assert_quiet_sync(&result);
    assert!(all_logs(&state.db).is_empty());
    assert!(get_data_source_breakdown(&state.db)
        .expect("data sources")
        .is_empty());
    assert_schema18_no_pi_proxy_row(&state.db);
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across session scans"
)]
async fn empty_home_session_dirs_import_nothing_and_keep_schema_18() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let home = ensure_test_home();
    reset_test_fs();
    let dirs = scan_dirs(home);
    create_empty_session_dirs(&dirs);

    let state = create_test_state().expect("create test state");
    assert_schema18_no_pi_proxy_row(&state.db);

    let result = sync_home(&state.db).await;
    assert_quiet_sync(&result);
    let summary = state
        .db
        .get_usage_summary(None, None, None, None, None)
        .expect("empty usage summary");
    assert_eq!(summary.total_requests, 0);
    assert_eq!(summary.total_input_tokens, 0);
    assert_eq!(summary.total_output_tokens, 0);
    assert_eq!(summary.real_total_tokens, 0);
    assert!(all_logs(&state.db).is_empty());
    assert_schema18_no_pi_proxy_row(&state.db);
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across session scans"
)]
async fn jsonl_fixtures_aggregate_claude_codex_pi_and_store_unknown_ttft() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let home = ensure_test_home();
    reset_test_fs();
    let dirs = scan_dirs(home);
    write_valid_claude_codex_pi_jsonl(&dirs, false);

    let state = create_test_state().expect("create test state");
    assert_schema18_no_pi_proxy_row(&state.db);

    let result = sync_home(&state.db).await;
    assert!(
        result.errors.is_empty(),
        "valid fixtures must not error: {:?}",
        result.errors
    );
    assert_eq!(result.imported, 5, "3 Claude + 1 Codex + 1 Pi: {result:?}");
    assert_eq!(
        result.files_scanned, 4,
        "Claude main + Claude sub-agent + Codex + Pi: {result:?}"
    );

    let logs = all_logs(&state.db);
    assert_eq!(logs.len(), 5);
    for log in &logs {
        assert_unknown_session_ttft(log);
        assert_eq!(log.status_code, 200);
    }

    let claude = logs_for_source(&logs, "session_log");
    assert_eq!(claude.len(), 3);
    assert!(claude.iter().all(|log| log.app_type == "claude"));
    assert!(claude.iter().all(|log| log.model == CLAUDE_MODEL));
    assert!(claude
        .iter()
        .all(|log| log.provider_name.as_deref() == Some("Claude (Session)")));
    let mut claude_ids: Vec<_> = claude.iter().map(|log| log.request_id.as_str()).collect();
    claude_ids.sort_unstable();
    assert_eq!(
        claude_ids,
        [
            "session:msg_scan_a",
            "session:msg_scan_b",
            "session:msg_scan_sub"
        ]
    );
    let claude_by_id = |id: &str| -> &RequestLogDetail {
        claude
            .iter()
            .copied()
            .find(|log| log.request_id == id)
            .expect("claude row")
    };
    assert_token_row(claude_by_id("session:msg_scan_a"), CLAUDE_A);
    assert_token_row(claude_by_id("session:msg_scan_b"), CLAUDE_B);
    assert_token_row(claude_by_id("session:msg_scan_sub"), CLAUDE_SUB);
    assert!(
        logs.iter().all(|log| !log.request_id.contains("msg_zero")),
        "all-zero Claude assistant rows must be skipped"
    );

    let codex = logs_for_source(&logs, "codex_session");
    assert_eq!(codex.len(), 1);
    assert_eq!(codex[0].app_type, "codex");
    assert_eq!(codex[0].model, CODEX_MODEL);
    assert_eq!(codex[0].provider_name.as_deref(), Some("Codex (Session)"));
    assert_eq!(codex[0].input_tokens, CODEX_TOTAL.0 as u32);
    assert_eq!(codex[0].cache_read_tokens, CODEX_TOTAL.1 as u32);
    assert_eq!(codex[0].output_tokens, CODEX_TOTAL.2 as u32);
    assert_eq!(codex[0].cache_creation_tokens, 0);

    let pi = logs_for_source(&logs, "pi_session");
    assert_eq!(pi.len(), 1);
    assert_eq!(pi[0].app_type, "pi");
    assert_eq!(pi[0].model, PI_MODEL);
    // Pi JSONL `provider` is preserved; `_pi_session` / "Pi (Session)" is only
    // the fallback when that field is missing.
    assert_eq!(pi[0].provider_id, "fixture-pi");
    assert_eq!(pi[0].provider_name.as_deref(), Some("fixture-pi"));
    assert_token_row(pi[0], PI_USAGE);

    let claude_tokens = expected_claude_tokens();
    let claude_summary = state
        .db
        .get_usage_summary(None, None, Some("claude"), None, None)
        .expect("claude summary");
    assert_eq!(claude_summary.total_requests, 3);
    assert_eq!(
        claude_summary.total_input_tokens,
        u64::from(claude_tokens.0)
    );
    assert_eq!(
        claude_summary.total_output_tokens,
        u64::from(claude_tokens.1)
    );
    assert_eq!(
        claude_summary.total_cache_read_tokens,
        u64::from(claude_tokens.2)
    );
    assert_eq!(
        claude_summary.total_cache_creation_tokens,
        u64::from(claude_tokens.3)
    );

    let codex_summary = state
        .db
        .get_usage_summary(None, None, Some("codex"), None, None)
        .expect("codex summary");
    assert_eq!(codex_summary.total_requests, 1);
    // Codex is cache-inclusive + legacy semantics: dashboard input is fresh.
    assert_eq!(
        codex_summary.total_input_tokens,
        CODEX_TOTAL.0 - CODEX_TOTAL.1
    );
    assert_eq!(codex_summary.total_output_tokens, CODEX_TOTAL.2);
    assert_eq!(codex_summary.total_cache_read_tokens, CODEX_TOTAL.1);

    let pi_summary = state
        .db
        .get_usage_summary(None, None, Some("pi"), None, None)
        .expect("pi summary");
    assert_eq!(pi_summary.total_requests, 1);
    assert_eq!(pi_summary.total_input_tokens, u64::from(PI_USAGE.0));
    assert_eq!(pi_summary.total_output_tokens, u64::from(PI_USAGE.1));
    assert_eq!(pi_summary.total_cache_read_tokens, u64::from(PI_USAGE.2));
    assert_eq!(
        pi_summary.total_cache_creation_tokens,
        u64::from(PI_USAGE.3)
    );

    let all_summary = state
        .db
        .get_usage_summary(None, None, None, None, None)
        .expect("all-app summary");
    assert_eq!(all_summary.total_requests, 5);
    assert_eq!(
        all_summary.total_output_tokens,
        claude_summary.total_output_tokens
            + codex_summary.total_output_tokens
            + pi_summary.total_output_tokens
    );
    assert_eq!(
        all_summary.total_input_tokens,
        claude_summary.total_input_tokens
            + codex_summary.total_input_tokens
            + pi_summary.total_input_tokens
    );
    assert_eq!(
        all_summary.real_total_tokens,
        all_summary.total_input_tokens
            + all_summary.total_output_tokens
            + all_summary.total_cache_creation_tokens
            + all_summary.total_cache_read_tokens
    );

    let by_app = state
        .db
        .get_usage_summary_by_app(None, None, None, None)
        .expect("summary by app");
    let apps: Vec<_> = by_app.iter().map(|row| row.app_type.as_str()).collect();
    assert!(apps.contains(&"claude"), "apps={apps:?}");
    assert!(apps.contains(&"codex"), "apps={apps:?}");
    assert!(apps.contains(&"pi"), "apps={apps:?}");

    let sources = get_data_source_breakdown(&state.db).expect("data sources");
    let source_names: Vec<_> = sources.iter().map(|row| row.data_source.as_str()).collect();
    assert!(source_names.contains(&"session_log"), "{source_names:?}");
    assert!(source_names.contains(&"codex_session"), "{source_names:?}");
    assert!(source_names.contains(&"pi_session"), "{source_names:?}");
    for row in &sources {
        match row.data_source.as_str() {
            "session_log" => assert_eq!(row.request_count, 3),
            "codex_session" | "pi_session" => assert_eq!(row.request_count, 1),
            other => panic!("unexpected data_source {other}"),
        }
    }

    let second = sync_home(&state.db).await;
    assert_eq!(second.imported, 0, "unchanged JSONL must not reimport");
    assert!(
        second.errors.is_empty(),
        "idempotent errors: {:?}",
        second.errors
    );
    assert_eq!(all_logs(&state.db).len(), 5);
    assert_schema18_no_pi_proxy_row(&state.db);
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across session scans"
)]
async fn corrupt_jsonl_lines_are_skipped_without_dropping_valid_rows() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let home = ensure_test_home();
    reset_test_fs();
    let dirs = scan_dirs(home);
    write_valid_claude_codex_pi_jsonl(&dirs, true);

    let state = create_test_state().expect("create test state");
    let result = sync_home(&state.db).await;
    assert!(
        result.errors.is_empty(),
        "corrupt lines must be skipped, not fail the scan: {:?}",
        result.errors
    );
    assert_eq!(result.imported, 5, "valid rows still import: {result:?}");

    let logs = all_logs(&state.db);
    assert_eq!(logs.len(), 5);
    for log in &logs {
        assert_unknown_session_ttft(log);
    }
    assert_eq!(logs_for_source(&logs, "session_log").len(), 3);
    assert_eq!(logs_for_source(&logs, "codex_session").len(), 1);
    assert_eq!(logs_for_source(&logs, "pi_session").len(), 1);

    let summary = state
        .db
        .get_usage_summary(None, None, None, None, None)
        .expect("summary after corrupt mix");
    assert_eq!(summary.total_requests, 5);
    assert_schema18_no_pi_proxy_row(&state.db);
}
