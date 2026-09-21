//! SCHEMA 18：`http_client` 全局代理 + `fetch_models` 生产路径矩阵。
//!
//! `--lib` 里不能 `apply_proxy`（会污染进程级 `GLOBAL_CLIENT`，Ubuntu 上曾把
//! Codex forwarder 打成 502）。本文件是独立 `--test` 二进制，专门走
//! `fetch_models` → `model_fetch_client()` → `http_client::get()`。
//!
//! 硬约束：不升 SCHEMA、不把供应商 URL 改写到 Claude listen `15721`、
//! 不写 Windows C:、不出 Portable。
#![allow(clippy::await_holding_lock)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use cc_switch_lib::{
    build_models_url_candidates, fetch_models, global_http_client, SCHEMA_VERSION,
};
use serial_test::serial;

#[path = "support.rs"]
mod support;
use support::{create_test_state, ensure_test_home, reset_test_fs, test_mutex};

const MODELS_BODY: &str = r#"{"object":"list","data":[{"id":"glm-5.1","owned_by":"test"}]}"#;

#[derive(Clone, Copy)]
enum MockMode {
    Origin,
    HttpProxy,
}

struct CapturingServer {
    url: String,
    hits: Arc<AtomicUsize>,
    captured: Arc<Mutex<Vec<String>>>,
    shutdown: mpsc::Sender<()>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for CapturingServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl CapturingServer {
    fn last_raw(&self) -> String {
        self.captured
            .lock()
            .expect("capture lock")
            .last()
            .cloned()
            .unwrap_or_default()
    }

    fn header(&self, name: &str) -> Option<String> {
        let raw = self.last_raw();
        let needle = format!("{}:", name.to_ascii_lowercase());
        raw.lines().find_map(|line| {
            let lower = line.to_ascii_lowercase();
            lower
                .starts_with(&needle)
                .then(|| line.split_once(':').map(|(_, v)| v.trim().to_string()))
                .flatten()
        })
    }

    fn request_target(&self) -> String {
        self.last_raw()
            .lines()
            .find(|line| {
                let method = line.split_whitespace().next().unwrap_or("");
                method.eq_ignore_ascii_case("GET")
            })
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("")
            .to_string()
    }
}

fn http_ok(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn spawn_capturing_server(mode: MockMode) -> CapturingServer {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock http");
    listener
        .set_nonblocking(true)
        .expect("nonblocking mock http");
    let port = listener.local_addr().expect("mock addr").port();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured.clone();
    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _ = mode;
        let ok = http_ok(MODELS_BODY);
        loop {
            if shutdown_rx.try_recv().is_ok() {
                break;
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    hits_clone.fetch_add(1, Ordering::SeqCst);
                    let mut buf = [0u8; 8192];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let mut raw = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let method = raw
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().next())
                        .unwrap_or("");
                    if method.eq_ignore_ascii_case("CONNECT") {
                        let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
                        let n2 = stream.read(&mut buf).unwrap_or(0);
                        raw.push_str(&String::from_utf8_lossy(&buf[..n2]));
                    }
                    assert!(
                        !raw.contains("15721"),
                        "captured request must not target Claude listen 15721: {raw}"
                    );
                    captured_clone.lock().expect("capture").push(raw);
                    let _ = stream.write_all(ok.as_bytes());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    CapturingServer {
        url: format!("http://127.0.0.1:{port}"),
        hits,
        captured,
        shutdown: shutdown_tx,
        handle: Some(handle),
    }
}

fn dead_proxy_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve dead proxy");
    let port = listener.local_addr().expect("dead proxy addr").port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

fn clear_proxy_env() {
    for key in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        std::env::remove_var(key);
    }
}

fn restore_direct_on_drop() -> DirectGuard {
    DirectGuard
}

struct DirectGuard;

impl Drop for DirectGuard {
    fn drop(&mut self) {
        let _ = global_http_client::apply_proxy(None);
    }
}

fn assert_not_claude_listen(value: &str) {
    assert!(
        !value.contains("15721"),
        "must never rewrite to Claude listen 15721: {value}"
    );
}

#[test]
fn schema_version_stays_18_without_pi_proxy_config_row() {
    let _guard = test_mutex().lock().expect("HOME mutex");
    reset_test_fs();
    let _home = ensure_test_home();
    let state = create_test_state().expect("ephemeral db");

    assert_eq!(SCHEMA_VERSION, 18, "this matrix must stay on SCHEMA 18");
    assert!(
        state.db.has_proxy_config_row("claude").expect("claude row"),
        "Claude still owns the shared listen row"
    );
    assert!(
        !state.db.has_proxy_config_row("pi").expect("pi row"),
        "SCHEMA 18 CHECK must not gain a proxy_config row for pi"
    );
}

#[test]
fn candidate_matrix_never_rewrites_to_claude_listen_15721() {
    let cases: &[(&str, &str, bool, Option<&str>)] = &[
        ("claude-official", "https://api.anthropic.com", false, None),
        (
            "claude-deepseek-modelsUrl",
            "https://api.deepseek.com/anthropic",
            false,
            Some("https://api.deepseek.com/models"),
        ),
        (
            "claude-full-url",
            "https://api.anthropic.com/v1/messages",
            true,
            None,
        ),
        ("codex-v1", "https://api.openai.com/v1", false, None),
        (
            "codex-full-url-responses",
            "https://api.openai.com/v1/responses",
            true,
            None,
        ),
        (
            "pi-deepseek",
            "https://api.deepseek.com/v1",
            false,
            Some("https://api.deepseek.com/models"),
        ),
        (
            "pi-ppio",
            "https://api.ppio.com/openai/v1",
            false,
            Some("https://api.ppio.com/openai/v1/models"),
        ),
        (
            "pi-novita",
            "https://api.novita.ai/openai/v1",
            false,
            Some("https://api.novita.ai/openai/v1/models"),
        ),
        (
            "pi-tencent-plan",
            "https://api.lkeap.cloud.tencent.com/plan/anthropic",
            false,
            Some("https://api.lkeap.cloud.tencent.com/plan/v3/models"),
        ),
    ];

    for (name, base, is_full, models_url) in cases {
        let got = build_models_url_candidates(base, *is_full, *models_url)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            !got.is_empty(),
            "{name} should produce at least one candidate"
        );
        for url in &got {
            assert_not_claude_listen(url);
            assert!(
                url.starts_with("http://") || url.starts_with("https://"),
                "{name} candidate must stay on the provider host: {url}"
            );
        }
    }
}

#[test]
fn validate_proxy_matrix_on_off_and_bad_url() {
    global_http_client::validate_proxy(None).expect("off / None");
    global_http_client::validate_proxy(Some("")).expect("off / empty");
    global_http_client::validate_proxy(Some("http://127.0.0.1:7890")).expect("http on");
    global_http_client::validate_proxy(Some("socks5://127.0.0.1:1080")).expect("socks5 on");
    global_http_client::validate_proxy(Some("socks5h://127.0.0.1:1080")).expect("socks5h on");
    global_http_client::validate_proxy(Some("http://user:s3cret@127.0.0.1:7890"))
        .expect("http with auth");

    let err = global_http_client::validate_proxy(Some("ftp://user:s3cret@127.0.0.1:21"))
        .expect_err("ftp is a bad proxy URL");
    assert!(
        !err.contains("s3cret"),
        "bad proxy errors must mask credentials: {err}"
    );
    assert!(global_http_client::validate_proxy(Some("invalid-scheme://127.0.0.1:1")).is_err());
    assert!(global_http_client::validate_proxy(Some("not-a-url")).is_err());

    let masked = global_http_client::mask_url("http://user:s3cret@127.0.0.1:7890");
    assert_eq!(masked, "http://127.0.0.1:7890");
    assert!(!masked.contains("s3cret"));
}

#[tokio::test]
#[serial]
async fn production_fetch_models_proxy_on_off_dead_and_working() {
    let _home_guard = test_mutex().lock().expect("HOME mutex");
    let _direct = restore_direct_on_drop();
    clear_proxy_env();

    let origin = spawn_capturing_server(MockMode::Origin);
    let proxy = spawn_capturing_server(MockMode::HttpProxy);
    let dead = dead_proxy_url();
    assert_not_claude_listen(&origin.url);
    assert_not_claude_listen(&proxy.url);
    assert_not_claude_listen(&dead);

    global_http_client::validate_proxy(Some(&dead)).expect("dead URL is still a valid proxy URL");
    global_http_client::apply_proxy(Some(&dead)).expect("apply dead proxy");
    assert!(global_http_client::is_proxy_enabled());
    assert_eq!(
        global_http_client::get_current_proxy_url().as_deref(),
        Some(dead.as_str())
    );

    let err = fetch_models(&origin.url, "prod-key", false, None, None, None, None)
        .await
        .expect_err("dead global_proxy_url must fail production fetch_models");
    assert!(
        err.contains("Request failed"),
        "production client must go through the configured proxy: {err}"
    );
    assert_eq!(origin.hits.load(Ordering::SeqCst), 0);

    global_http_client::apply_proxy(Some(&proxy.url)).expect("apply working proxy");
    let models = fetch_models(&origin.url, "prod-key", false, None, None, None, None)
        .await
        .expect("working global_proxy_url");
    assert_eq!(models[0].id, "glm-5.1");
    assert_eq!(
        origin.hits.load(Ordering::SeqCst),
        0,
        "working proxy answers itself; origin stays 0"
    );
    assert!(proxy.hits.load(Ordering::SeqCst) >= 1);
    assert_not_claude_listen(&proxy.request_target());
    assert_eq!(
        proxy.header("authorization").as_deref(),
        Some("Bearer prod-key")
    );

    global_http_client::apply_proxy(None).expect("clear proxy = direct");
    assert!(!global_http_client::is_proxy_enabled());
    assert!(global_http_client::get_current_proxy_url().is_none());
    let models = fetch_models(&origin.url, "prod-key", false, None, None, None, None)
        .await
        .expect("direct production fetch_models");
    assert_eq!(models[0].id, "glm-5.1");
    assert!(origin.hits.load(Ordering::SeqCst) >= 1);
    assert_not_claude_listen(&origin.request_target());
}

#[tokio::test]
#[serial]
async fn production_fetch_models_auth_and_url_variants_for_claude_codex_pi() {
    let _home_guard = test_mutex().lock().expect("HOME mutex");
    let _direct = restore_direct_on_drop();
    clear_proxy_env();
    global_http_client::apply_proxy(None).expect("direct");

    // Claude：anthropic-messages + isFullUrl。
    let claude = spawn_capturing_server(MockMode::Origin);
    let claude_full = format!("{}/v1/messages", claude.url);
    let models = fetch_models(
        &claude_full,
        "sk-ant-prod",
        true,
        None,
        None,
        Some("anthropic-messages"),
        None,
    )
    .await
    .expect("claude isFullUrl");
    assert_eq!(models[0].id, "glm-5.1");
    assert!(claude.request_target().contains("/v1/models"));
    assert_eq!(claude.header("x-api-key").as_deref(), Some("sk-ant-prod"));
    assert!(claude.header("authorization").is_none());
    assert_not_claude_listen(&claude.request_target());

    // Codex：openai-responses + /v1 base。
    let codex = spawn_capturing_server(MockMode::Origin);
    let models = fetch_models(
        &format!("{}/v1", codex.url),
        "sk-codex-prod",
        false,
        None,
        None,
        Some("openai-responses"),
        None,
    )
    .await
    .expect("codex v1");
    assert_eq!(models[0].id, "glm-5.1");
    assert_eq!(
        codex.header("authorization").as_deref(),
        Some("Bearer sk-codex-prod")
    );
    assert_not_claude_listen(&codex.request_target());

    // Pi 聚合商：modelsUrl 覆写 + 默认 Bearer，不改写 15721。
    let pi = spawn_capturing_server(MockMode::Origin);
    let models_url = format!("{}/models", pi.url);
    let models = fetch_models(
        &format!("{}/v1", pi.url),
        "sk-pi-agg",
        false,
        Some(models_url.as_str()),
        None,
        None,
        None,
    )
    .await
    .expect("pi modelsUrl");
    assert_eq!(models[0].id, "glm-5.1");
    assert!(
        pi.request_target().contains("/models"),
        "Pi modelsUrl must win: {}",
        pi.request_target()
    );
    assert_eq!(
        pi.header("authorization").as_deref(),
        Some("Bearer sk-pi-agg")
    );
    assert_not_claude_listen(&pi.request_target());

    // Pi header-only Token（表单 api 不当成拉取鉴权时仍可走自定义头）。
    let pi_header = spawn_capturing_server(MockMode::Origin);
    let headers = BTreeMap::from([("Authorization".to_string(), "Token literal".to_string())]);
    let models = fetch_models(
        &pi_header.url,
        "",
        false,
        None,
        None,
        Some("openai-completions"),
        Some(&headers),
    )
    .await
    .expect("pi header-only");
    assert_eq!(models[0].id, "glm-5.1");
    assert_eq!(
        pi_header.header("authorization").as_deref(),
        Some("Token literal")
    );
    assert_not_claude_listen(&pi_header.request_target());
}
