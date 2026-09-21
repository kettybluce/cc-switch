//! Linux-cloud stand-in for WSL Pi / Claude / Codex proxy projection.
//!
//! Live Windows layout (never written by these tests):
//! ```text
//! \\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude
//! \\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.codex
//! \\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi\agent
//! ```
//!
//! Cloud stand-in is a POSIX tree under the isolated test HOME
//! (`CC_SWITCH_TEST_HOME`). No C: writes, no `pi-wsl-sessions` mirror,
//! SCHEMA 18 only (no `proxy_config` row for `pi`). Happy-path Claude/Codex
//! roundtrips live on main (#51); this module also fail-closes missing,
//! malformed, and foreign-local-proxy Live files.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use cc_switch_lib::{
    get_claude_settings_path, get_codex_auth_path, get_codex_config_path, read_json_file,
    update_settings, AppSettings, AppType, Provider, ProviderService, SCHEMA_VERSION,
};

#[path = "support.rs"]
mod support;
use support::{create_test_state, ensure_test_home, reset_test_fs, test_mutex};

const PI_MODELS_FIXTURE: &str =
    include_str!("../src/pi_config/fixtures/models_with_unknown_fields.json");

const PI_SETTINGS_FIXTURE: &str =
    r#"{"defaultProvider":"anthropic","defaultModel":"claude-sonnet-4","theme":"keep-me"}"#;

const PI_AUTH_FIXTURE: &str = r#"{"anthropic":{"type":"oauth"}}"#;

/// A local proxy that is not the CC Switch listen under test.
const FOREIGN_LOCAL_PROXY: &str = "http://127.0.0.1:9999";
const FOREIGN_LOCAL_PROXY_CODEX: &str = "http://127.0.0.1:9999/v1";

fn wsl_standin_user_home(test_home: &Path) -> PathBuf {
    test_home
        .join("profiles")
        .join("wsl.localhost")
        .join("Ubuntu-22.04")
        .join("home")
        .join("tfdx8045")
}

fn assert_linux_standin_path(path: &Path) {
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

fn apply_wsl_standin_overrides(user_home: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let claude_dir = user_home.join(".claude");
    let codex_dir = user_home.join(".codex");
    let pi_agent = user_home.join(".pi").join("agent");
    fs::create_dir_all(&claude_dir).expect("create stand-in Claude dir");
    fs::create_dir_all(&codex_dir).expect("create stand-in Codex dir");
    fs::create_dir_all(&pi_agent).expect("create stand-in Pi agent dir");

    update_settings(AppSettings {
        claude_config_dir: Some(claude_dir.to_string_lossy().to_string()),
        codex_config_dir: Some(codex_dir.to_string_lossy().to_string()),
        pi_config_dir: Some(pi_agent.to_string_lossy().to_string()),
        ..AppSettings::default()
    })
    .expect("point Claude/Codex/Pi at the Linux WSL stand-in");

    (claude_dir, codex_dir, pi_agent)
}

fn write_pi_live_fixtures(pi_agent: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let models_path = pi_agent.join("models.json");
    let settings_path = pi_agent.join("settings.json");
    let auth_path = pi_agent.parent().expect("Pi root").join("auth.json");
    fs::write(&models_path, PI_MODELS_FIXTURE).expect("write stand-in models.json");
    fs::write(&settings_path, PI_SETTINGS_FIXTURE).expect("write stand-in Pi settings.json");
    fs::write(&auth_path, PI_AUTH_FIXTURE).expect("write stand-in Pi auth.json");
    (models_path, settings_path, auth_path)
}

fn claude_provider() -> Provider {
    claude_provider_named(
        "standin-claude",
        "Stand-in Claude",
        "https://api.anthropic.example",
        "claude-live-token",
    )
}

fn claude_provider_named(id: &str, name: &str, url: &str, token: &str) -> Provider {
    let mut provider = Provider::with_id(
        id.to_string(),
        name.to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": token,
                "ANTHROPIC_BASE_URL": url,
                "CC_SWITCH_KEEP": "roundtrip"
            },
            "permissions": { "allow": ["Read"] },
            "customTopLevel": "keep-me"
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
}

fn codex_provider() -> Provider {
    codex_provider_named(
        "standin-codex",
        "Stand-in",
        "https://api.openai.example/v1",
        "codex-live-key",
    )
}

fn codex_provider_named(id: &str, toml_name: &str, url: &str, key: &str) -> Provider {
    let table = id.replace('-', "_");
    let config = format!(
        r#"model_provider = "{table}"
model = "gpt-5"

[model_providers.{table}]
name = "{toml_name}"
base_url = "{url}"
wire_api = "responses"

[projects."/tmp/cc-switch-keep"]
trust_level = "trusted"
"#
    );
    let mut provider = Provider::with_id(
        id.to_string(),
        format!("Stand-in Codex {toml_name}"),
        json!({
            "auth": {"OPENAI_API_KEY": key, "keepMe": "roundtrip"},
            "config": config
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
}

fn seed_codex_auth_with_unknown_fields(auth_path: &Path, key: &str) {
    fs::write(
        auth_path,
        format!(
            r#"{{"OPENAI_API_KEY":"{key}","keepMe":"roundtrip","tokens":{{"access_token":"leave-me"}}}}"#
        ),
    )
    .expect("seed Codex auth.json extra fields");
}

async fn use_ephemeral_shared_listen(state: &cc_switch_lib::AppState) {
    let mut proxy_config = state.db.get_proxy_config().await.expect("get proxy config");
    proxy_config.listen_port = 0;
    proxy_config.listen_address = "0.0.0.0".to_string();
    state
        .db
        .update_proxy_config(proxy_config)
        .await
        .expect("use ephemeral 0.0.0.0 listen (rewritten to 127.0.0.1 for clients)");
}

fn assert_schema18_pi_has_no_proxy_config_row(state: &cc_switch_lib::AppState) {
    assert_eq!(
        SCHEMA_VERSION, 18,
        "Linux stand-in fixtures must stay on SCHEMA 18"
    );
    assert!(
        state
            .db
            .has_proxy_config_row("claude")
            .expect("claude proxy_config row"),
        "Claude still owns the shared listen row"
    );
    assert!(
        !state
            .db
            .has_proxy_config_row("pi")
            .expect("pi proxy_config row"),
        "SCHEMA 18 CHECK must not gain a proxy_config row for pi"
    );
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("read json")).expect("parse json")
}

fn assert_projected_pi_models(models_path: &Path, origin: &str) {
    let written = read_json(models_path);
    assert_eq!(written["providers"]["anthropic"]["baseUrl"], json!(origin));
    assert_eq!(
        written["providers"]["openai"]["baseUrl"],
        json!(format!("{origin}/v1"))
    );
    assert_eq!(written["providers"]["gemini"]["baseUrl"], json!(origin));
    assert_eq!(
        written["providers"]["anthropic"]["apiKey"],
        json!("PROXY_MANAGED")
    );
    assert_eq!(
        written["providers"]["anthropic"]["headers"]["x-cc-switch-app"],
        json!("pi")
    );
    assert_eq!(
        written["providers"]["anthropic"]["headers"]["X-Custom"],
        json!("hdr")
    );
    assert_eq!(written["customTopLevel"], json!("keep-me"));
    assert!(written.get("defaultProvider").is_none());
    assert!(written.get("defaultModel").is_none());
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_projects_pi_models_json_under_takeover() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    let (_claude_dir, _codex_dir, pi_agent) = apply_wsl_standin_overrides(&user_home);
    let (models_path, settings_path, auth_path) = write_pi_live_fixtures(&pi_agent);

    assert_linux_standin_path(&models_path);
    assert!(
        models_path.ends_with(Path::new("tfdx8045/.pi/agent/models.json"))
            || models_path
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("tfdx8045/.pi/agent/models.json"),
        "takeover must project ~/.pi/agent/models.json: {}",
        models_path.display()
    );

    let settings_before = fs::read_to_string(&settings_path).expect("Pi settings before");
    let auth_before = fs::read_to_string(&auth_path).expect("Pi auth before");

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    assert_schema18_pi_has_no_proxy_config_row(&state);

    state
        .proxy_service
        .set_takeover_for_app("pi", true)
        .await
        .expect("enable Pi takeover");

    assert!(
        state.db.is_pi_takeover_enabled().expect("pi flag"),
        "Pi takeover must persist in settings.proxy_takeover_pi"
    );
    assert_eq!(
        state
            .db
            .get_setting("proxy_takeover_pi")
            .expect("read setting")
            .as_deref(),
        Some("true")
    );
    assert_schema18_pi_has_no_proxy_config_row(&state);

    let status = state
        .proxy_service
        .get_status()
        .await
        .expect("proxy status");
    let origin = format!("http://127.0.0.1:{}", status.port);
    assert_ne!(status.port, 0, "ephemeral listen must bind a real port");
    assert_projected_pi_models(&models_path, &origin);
    assert_eq!(
        fs::read_to_string(&settings_path).expect("Pi settings after"),
        settings_before,
        "Pi settings.json defaults must stay untouched"
    );
    assert_eq!(
        fs::read_to_string(&auth_path).expect("Pi auth after"),
        auth_before,
        "Pi auth.json must stay untouched"
    );

    state
        .proxy_service
        .set_takeover_for_app("pi", false)
        .await
        .expect("disable Pi takeover");

    let restored = read_json(&models_path);
    assert_eq!(
        restored["providers"]["anthropic"]["baseUrl"],
        json!("https://api.anthropic.com")
    );
    assert_eq!(
        restored["providers"]["openai"]["baseUrl"],
        json!("https://api.openai.com/v1")
    );
    assert_ne!(
        restored["providers"]["anthropic"]["apiKey"],
        json!("PROXY_MANAGED")
    );
    assert!(restored["providers"]["anthropic"]
        .get("headers")
        .and_then(|headers| headers.get("x-cc-switch-app"))
        .is_none());
    assert_eq!(
        fs::read_to_string(&settings_path).expect("Pi settings final"),
        settings_before
    );
    assert_eq!(
        fs::read_to_string(&auth_path).expect("Pi auth final"),
        auth_before
    );
    assert!(!state.db.is_pi_takeover_enabled().expect("pi flag after"));
    assert!(
        state
            .db
            .get_live_backup("pi")
            .await
            .expect("read Pi backup")
            .is_none(),
        "disable must drop the Pi live backup"
    );
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_projects_claude_and_codex_proxy_settings() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    apply_wsl_standin_overrides(&user_home);

    let claude_settings = get_claude_settings_path();
    let codex_config = get_codex_config_path();
    let codex_auth = get_codex_auth_path();
    assert_linux_standin_path(&claude_settings);
    assert_linux_standin_path(&codex_config);
    assert_linux_standin_path(&codex_auth);
    assert!(
        claude_settings
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("tfdx8045/.claude/settings.json"),
        "Claude live must be the stand-in WSL home: {}",
        claude_settings.display()
    );
    assert!(
        codex_config
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("tfdx8045/.codex/config.toml"),
        "Codex live must be the stand-in WSL home: {}",
        codex_config.display()
    );

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;

    state
        .db
        .save_provider(AppType::Claude.as_str(), &claude_provider())
        .expect("save Claude provider");
    state
        .db
        .save_provider(AppType::Codex.as_str(), &codex_provider())
        .expect("save Codex provider");
    ProviderService::switch(&state, AppType::Claude, "standin-claude")
        .expect("write Claude live settings");
    ProviderService::switch(&state, AppType::Codex, "standin-codex")
        .expect("write Codex live config");

    let claude_before: Value = read_json_file(&claude_settings).expect("Claude live before");
    assert_eq!(
        claude_before["env"]["ANTHROPIC_BASE_URL"],
        json!("https://api.anthropic.example")
    );
    let codex_config_before = fs::read_to_string(&codex_config).expect("Codex config before");
    assert!(codex_config_before.contains("https://api.openai.example/v1"));

    state
        .proxy_service
        .set_takeover_for_app("claude", true)
        .await
        .expect("enable Claude takeover");
    state
        .proxy_service
        .set_takeover_for_app("codex", true)
        .await
        .expect("enable Codex takeover");

    let status = state
        .proxy_service
        .get_status()
        .await
        .expect("proxy status");
    let origin = format!("http://127.0.0.1:{}", status.port);
    let codex_origin = format!("{origin}/v1");

    let claude_live: Value = read_json_file(&claude_settings).expect("Claude live after");
    assert_eq!(claude_live["env"]["ANTHROPIC_BASE_URL"], json!(origin));
    assert_eq!(
        claude_live["env"]["ANTHROPIC_AUTH_TOKEN"],
        json!("PROXY_MANAGED")
    );
    assert!(
        !claude_live["env"]["ANTHROPIC_BASE_URL"]
            .as_str()
            .unwrap_or_default()
            .contains("0.0.0.0"),
        "0.0.0.0 listen must be rewritten to 127.0.0.1 for Claude clients"
    );

    let codex_live = fs::read_to_string(&codex_config).expect("Codex config after");
    assert!(
        codex_live.contains(&codex_origin),
        "Codex config.toml must project onto the shared listen /v1: {codex_live}"
    );
    assert!(
        codex_live.contains("PROXY_MANAGED"),
        "Codex takeover should inject the proxy placeholder: {codex_live}"
    );
    assert!(
        !codex_live.contains("https://api.openai.example/v1"),
        "Codex live must not keep the upstream endpoint during takeover"
    );
    // Third-party Codex takeover projects config.toml (placeholder + local
    // /v1). auth.json may be absent after a preservation-off switch and is
    // not the settings projection target.
    if let Ok(auth_after) = read_json_file::<Value>(&codex_auth) {
        let auth_url = auth_after
            .get("OPENAI_BASE_URL")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !auth_url.contains("127.0.0.1"),
            "Codex auth.json must not receive the proxy origin: {auth_after}"
        );
    }

    let (claude_enabled, _) = state.db.get_proxy_flags_sync("claude");
    let (codex_enabled, _) = state.db.get_proxy_flags_sync("codex");
    assert!(claude_enabled && codex_enabled);
    assert_schema18_pi_has_no_proxy_config_row(&state);

    state
        .proxy_service
        .set_takeover_for_app("claude", false)
        .await
        .expect("disable Claude takeover");
    state
        .proxy_service
        .set_takeover_for_app("codex", false)
        .await
        .expect("disable Codex takeover");

    let claude_restored: Value = read_json_file(&claude_settings).expect("Claude restored");
    assert_eq!(
        claude_restored["env"]["ANTHROPIC_BASE_URL"],
        json!("https://api.anthropic.example")
    );
    assert_eq!(
        claude_restored["env"]["ANTHROPIC_AUTH_TOKEN"],
        json!("claude-live-token")
    );
    let codex_restored = fs::read_to_string(&codex_config).expect("Codex restored");
    assert!(
        codex_restored.contains("https://api.openai.example/v1"),
        "disable must restore the Codex upstream endpoint"
    );
    assert!(
        !codex_restored.contains("PROXY_MANAGED"),
        "restored Codex config must not keep PROXY_MANAGED"
    );
    let (claude_enabled_after, _) = state.db.get_proxy_flags_sync("claude");
    let (codex_enabled_after, _) = state.db.get_proxy_flags_sync("codex");
    assert!(!claude_enabled_after && !codex_enabled_after);
    assert!(state
        .db
        .get_live_backup("claude")
        .await
        .expect("claude backup")
        .is_none());
    assert!(state
        .db
        .get_live_backup("codex")
        .await
        .expect("codex backup")
        .is_none());
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_restore_disable_cleans_pi_claude_codex_projection() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    let (_claude_dir, _codex_dir, pi_agent) = apply_wsl_standin_overrides(&user_home);
    let (models_path, settings_path, auth_path) = write_pi_live_fixtures(&pi_agent);
    let settings_before = fs::read_to_string(&settings_path).expect("Pi settings before");
    let auth_before = fs::read_to_string(&auth_path).expect("Pi auth before");

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    state
        .db
        .save_provider(AppType::Claude.as_str(), &claude_provider())
        .expect("save Claude provider");
    state
        .db
        .save_provider(AppType::Codex.as_str(), &codex_provider())
        .expect("save Codex provider");
    ProviderService::switch(&state, AppType::Claude, "standin-claude").expect("write Claude live");
    ProviderService::switch(&state, AppType::Codex, "standin-codex").expect("write Codex live");

    state
        .proxy_service
        .set_takeover_for_app("claude", true)
        .await
        .expect("enable Claude takeover");
    state
        .proxy_service
        .set_takeover_for_app("codex", true)
        .await
        .expect("enable Codex takeover");
    state
        .proxy_service
        .set_takeover_for_app("pi", true)
        .await
        .expect("enable Pi takeover onto the shared Claude listen");

    let status = state
        .proxy_service
        .get_status()
        .await
        .expect("proxy status");
    let origin = format!("http://127.0.0.1:{}", status.port);
    assert_projected_pi_models(&models_path, &origin);
    let claude_live: Value = read_json_file(&get_claude_settings_path()).expect("Claude projected");
    assert_eq!(claude_live["env"]["ANTHROPIC_BASE_URL"], json!(origin));
    let codex_live = fs::read_to_string(get_codex_config_path()).expect("Codex projected");
    assert!(codex_live.contains(&format!("{origin}/v1")));
    assert_eq!(
        fs::read_to_string(&settings_path).expect("Pi settings during takeover"),
        settings_before
    );
    assert_eq!(
        fs::read_to_string(&auth_path).expect("Pi auth during takeover"),
        auth_before
    );
    assert_schema18_pi_has_no_proxy_config_row(&state);
    assert!(state.db.is_pi_takeover_enabled().expect("pi flag"));

    state
        .proxy_service
        .set_takeover_for_app("pi", false)
        .await
        .expect("disable Pi");
    state
        .proxy_service
        .set_takeover_for_app("codex", false)
        .await
        .expect("disable Codex");
    state
        .proxy_service
        .set_takeover_for_app("claude", false)
        .await
        .expect("disable Claude");

    let restored_models = read_json(&models_path);
    assert_eq!(
        restored_models["providers"]["anthropic"]["baseUrl"],
        json!("https://api.anthropic.com")
    );
    assert_eq!(
        restored_models["providers"]["anthropic"]["apiKey"],
        json!("sk-ant-live")
    );
    let claude_restored: Value =
        read_json_file(&get_claude_settings_path()).expect("Claude restored");
    assert_eq!(
        claude_restored["env"]["ANTHROPIC_BASE_URL"],
        json!("https://api.anthropic.example")
    );
    let codex_restored = fs::read_to_string(get_codex_config_path()).expect("Codex restored");
    assert!(codex_restored.contains("https://api.openai.example/v1"));
    assert!(!codex_restored.contains("PROXY_MANAGED"));
    assert_eq!(
        fs::read_to_string(&settings_path).expect("Pi settings after restore"),
        settings_before
    );
    assert_eq!(
        fs::read_to_string(&auth_path).expect("Pi auth after restore"),
        auth_before
    );

    for app in ["claude", "codex", "pi"] {
        assert!(
            state
                .db
                .get_live_backup(app)
                .await
                .unwrap_or_else(|_| panic!("read {app} backup"))
                .is_none(),
            "{app} backup must be deleted on disable"
        );
        assert!(
            !state
                .db
                .is_app_takeover_enabled(app)
                .await
                .unwrap_or_else(|_| panic!("read {app} takeover flag")),
            "{app} takeover flag must be cleared"
        );
    }
    assert_schema18_pi_has_no_proxy_config_row(&state);
    assert!(
        !state
            .proxy_service
            .get_status()
            .await
            .expect("final proxy status")
            .running
    );
}

fn assert_isolated_test_home(test_home: &Path, live_path: &Path) {
    let marked = std::env::var_os("CC_SWITCH_TEST_HOME").expect("CC_SWITCH_TEST_HOME must be set");
    assert_eq!(
        Path::new(&marked),
        test_home,
        "tests must use the isolated CC_SWITCH_TEST_HOME"
    );
    assert!(
        live_path.starts_with(test_home),
        "live path must stay under isolated test HOME: {} vs {}",
        live_path.display(),
        test_home.display()
    );
    assert_linux_standin_path(live_path);
}

fn seed_current_claude(state: &cc_switch_lib::AppState) {
    state
        .db
        .save_provider(AppType::Claude.as_str(), &claude_provider())
        .expect("save Claude provider");
    state
        .db
        .set_current_provider(AppType::Claude.as_str(), "standin-claude")
        .expect("set Claude current");
}

fn seed_current_codex(state: &cc_switch_lib::AppState) {
    state
        .db
        .save_provider(AppType::Codex.as_str(), &codex_provider())
        .expect("save Codex provider");
    state
        .db
        .set_current_provider(AppType::Codex.as_str(), "standin-codex")
        .expect("set Codex current");
}

fn write_claude_live_settings(settings_path: &Path, base_url: &str, token: &str) {
    let body = json!({
        "env": {
            "ANTHROPIC_AUTH_TOKEN": token,
            "ANTHROPIC_BASE_URL": base_url
        },
        "keepUnknown": "keep-me"
    });
    fs::write(
        settings_path,
        serde_json::to_string_pretty(&body).expect("serialize Claude live"),
    )
    .expect("write Claude settings.json");
}

fn write_codex_live_config(config_path: &Path, base_url: &str, token: &str) {
    fs::write(
        config_path,
        format!(
            r#"model_provider = "standin"
model = "gpt-5"
keep_unknown = true

[model_providers.standin]
name = "Stand-in"
base_url = "{base_url}"
wire_api = "responses"
experimental_bearer_token = "{token}"
"#
        ),
    )
    .expect("write Codex config.toml");
}

async fn assert_enable_fails_closed(
    state: &cc_switch_lib::AppState,
    app: &str,
    live_path: &Path,
    before: Option<&str>,
    err_needles: &[&str],
) {
    let err = state
        .proxy_service
        .set_takeover_for_app(app, true)
        .await
        .expect_err("enable must fail closed");
    assert!(
        err_needles.iter().any(|needle| err.contains(needle)),
        "unexpected fail-closed error for {app}: {err}"
    );
    match before {
        None => assert!(
            !live_path.exists(),
            "{app} live must stay missing after fail-closed enable: {}",
            live_path.display()
        ),
        Some(expected) => {
            assert_eq!(
                fs::read_to_string(live_path).unwrap_or_default(),
                expected,
                "{app} live must be left unchanged after fail-closed enable"
            );
        }
    }
    assert!(
        state
            .db
            .get_live_backup(app)
            .await
            .expect("read backup")
            .is_none(),
        "{app} must not persist a live backup after fail-closed enable"
    );
    assert!(
        !state
            .db
            .is_app_takeover_enabled(app)
            .await
            .expect("read takeover flag"),
        "{app} takeover flag must stay off"
    );
    let _ = state.proxy_service.stop().await;
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_claude_takeover_fails_closed_when_settings_json_missing() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    apply_wsl_standin_overrides(&user_home);

    let claude_settings = get_claude_settings_path();
    assert_isolated_test_home(test_home, &claude_settings);
    assert!(
        !claude_settings.exists(),
        "fixture must start without settings.json"
    );

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    seed_current_claude(&state);
    assert_schema18_pi_has_no_proxy_config_row(&state);

    assert_enable_fails_closed(
        &state,
        "claude",
        &claude_settings,
        None,
        &["不存在", "missing"],
    )
    .await;
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_codex_takeover_fails_closed_when_config_toml_missing() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    apply_wsl_standin_overrides(&user_home);

    let codex_config = get_codex_config_path();
    assert_isolated_test_home(test_home, &codex_config);
    assert!(
        !codex_config.exists(),
        "fixture must start without config.toml"
    );

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    seed_current_codex(&state);
    assert_schema18_pi_has_no_proxy_config_row(&state);

    assert_enable_fails_closed(&state, "codex", &codex_config, None, &["不存在", "missing"]).await;
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_claude_takeover_fails_closed_on_malformed_settings_json() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    apply_wsl_standin_overrides(&user_home);

    let claude_settings = get_claude_settings_path();
    assert_isolated_test_home(test_home, &claude_settings);
    let malformed = "{not-json";
    fs::write(&claude_settings, malformed).expect("write malformed Claude JSON");

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    seed_current_claude(&state);

    assert_enable_fails_closed(
        &state,
        "claude",
        &claude_settings,
        Some(malformed),
        &["JSON", "json", "解析", "格式"],
    )
    .await;
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_codex_takeover_fails_closed_on_malformed_config_toml() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    apply_wsl_standin_overrides(&user_home);

    let codex_config = get_codex_config_path();
    assert_isolated_test_home(test_home, &codex_config);
    let malformed = "[[[not toml";
    fs::write(&codex_config, malformed).expect("write malformed Codex TOML");

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    seed_current_codex(&state);

    assert_enable_fails_closed(
        &state,
        "codex",
        &codex_config,
        Some(malformed),
        &["TOML", "toml", "解析", "格式"],
    )
    .await;
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_claude_takeover_fails_closed_when_proxy_already_pointing_elsewhere() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    apply_wsl_standin_overrides(&user_home);

    let claude_settings = get_claude_settings_path();
    let codex_config = get_codex_config_path();
    assert_isolated_test_home(test_home, &claude_settings);
    write_claude_live_settings(&claude_settings, FOREIGN_LOCAL_PROXY, "PROXY_MANAGED");
    write_codex_live_config(
        &codex_config,
        "https://api.openai.example/v1",
        "codex-live-key",
    );
    let claude_before = fs::read_to_string(&claude_settings).expect("Claude before");
    let codex_before = fs::read_to_string(&codex_config).expect("Codex before");

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    seed_current_claude(&state);
    seed_current_codex(&state);

    assert_enable_fails_closed(
        &state,
        "claude",
        &claude_settings,
        Some(&claude_before),
        &["其他本地代理", "another local proxy"],
    )
    .await;
    assert_eq!(
        fs::read_to_string(&codex_config).expect("Codex after refused Claude enable"),
        codex_before,
        "refusing Claude takeover must not touch Codex config.toml"
    );
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_codex_takeover_fails_closed_when_proxy_already_pointing_elsewhere() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    apply_wsl_standin_overrides(&user_home);

    let claude_settings = get_claude_settings_path();
    let codex_config = get_codex_config_path();
    assert_isolated_test_home(test_home, &codex_config);
    write_claude_live_settings(
        &claude_settings,
        "https://api.anthropic.example",
        "claude-live-token",
    );
    write_codex_live_config(&codex_config, FOREIGN_LOCAL_PROXY_CODEX, "PROXY_MANAGED");
    let claude_before = fs::read_to_string(&claude_settings).expect("Claude before");
    let codex_before = fs::read_to_string(&codex_config).expect("Codex before");

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    seed_current_claude(&state);
    seed_current_codex(&state);

    assert_enable_fails_closed(
        &state,
        "codex",
        &codex_config,
        Some(&codex_before),
        &["其他本地代理", "another local proxy"],
    )
    .await;
    assert_eq!(
        fs::read_to_string(&claude_settings).expect("Claude after refused Codex enable"),
        claude_before,
        "refusing Codex takeover must not touch Claude settings.json"
    );
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

fn pi_openai_provider(id: &str, name: &str, model: &str, url: &str, key: &str) -> Provider {
    let mut provider = Provider::with_id(
        id.to_string(),
        name.to_string(),
        json!({
            "name": name,
            "baseUrl": url,
            "apiKey": key,
            "api": "openai-completions",
            "models": [{ "id": model }]
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
}

fn setup_linux_standin_pi_home() -> (PathBuf, PathBuf, PathBuf) {
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    let (_claude_dir, _codex_dir, pi_agent) = apply_wsl_standin_overrides(&user_home);
    let models_path = pi_agent.join("models.json");
    let settings_path = pi_agent.join("settings.json");
    let auth_path = pi_agent.parent().expect("Pi root").join("auth.json");
    fs::write(&models_path, r#"{"providers":{}}"#).expect("seed empty models.json");
    fs::write(
        &settings_path,
        r#"{"theme":"keep-me","sessionDir":"/tmp/pi-sessions"}"#,
    )
    .expect("seed Pi settings.json");
    fs::write(&auth_path, PI_AUTH_FIXTURE).expect("seed Pi auth.json");
    assert_linux_standin_path(&models_path);
    assert!(
        models_path
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("tfdx8045/.pi/agent/models.json"),
        "CRUD must write the WSL stand-in agent models.json: {}",
        models_path.display()
    );
    (models_path, settings_path, auth_path)
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across Pi CRUD"
)]
async fn linux_standin_create_openai_completions_pins_developer_role_false() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let (models_path, settings_path, auth_path) = setup_linux_standin_pi_home();
    let settings_before = fs::read_to_string(&settings_path).expect("settings before");
    let auth_before = fs::read_to_string(&auth_path).expect("auth before");

    let state = create_test_state().expect("create test state");
    assert_schema18_pi_has_no_proxy_config_row(&state);

    ProviderService::add(
        &state,
        AppType::Pi,
        pi_openai_provider(
            "standin-yum",
            "Yum",
            "glm-5.1",
            "https://api.llm.prd.yumc.local/v1",
            "yum-key",
        ),
        true,
    )
    .expect("create live openai-completions provider");

    let live = read_json(&models_path);
    assert_eq!(
        live["providers"]["standin-yum"]["api"],
        json!("openai-completions")
    );
    assert_eq!(
        live["providers"]["standin-yum"]["baseUrl"],
        json!("https://api.llm.prd.yumc.local/v1")
    );
    assert_eq!(live["providers"]["standin-yum"]["apiKey"], json!("yum-key"));
    assert_eq!(
        live["providers"]["standin-yum"]["compat"]["supportsDeveloperRole"],
        json!(false)
    );
    assert_eq!(
        live["providers"]["standin-yum"]["models"][0]["compat"]["supportsDeveloperRole"],
        json!(false)
    );
    assert_eq!(
        fs::read_to_string(&settings_path).expect("settings after create"),
        settings_before,
        "create must not rewrite Pi settings.json defaults"
    );
    assert_eq!(
        fs::read_to_string(&auth_path).expect("auth after create"),
        auth_before,
        "create must not touch auth.json"
    );
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across Pi CRUD"
)]
async fn linux_standin_update_writes_url_and_key_to_models_json() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let (models_path, _settings_path, auth_path) = setup_linux_standin_pi_home();
    let auth_before = fs::read_to_string(&auth_path).expect("auth before");

    let state = create_test_state().expect("create test state");
    ProviderService::add(
        &state,
        AppType::Pi,
        pi_openai_provider(
            "standin-yum",
            "Yum",
            "glm-5.1",
            "https://api.llm.prd.yumc.local/v1",
            "yum-key",
        ),
        true,
    )
    .expect("create live provider");

    let mut updated = pi_openai_provider(
        "standin-yum",
        "Yum",
        "glm-5.1",
        "https://rotated.example/v1",
        "rotated-key",
    );
    updated.settings_config["unknownField"] = json!({ "keep": true });
    ProviderService::update(&state, AppType::Pi, Some("standin-yum"), updated)
        .expect("update live url/key");

    let live = read_json(&models_path);
    assert_eq!(
        live["providers"]["standin-yum"]["baseUrl"],
        json!("https://rotated.example/v1")
    );
    assert_eq!(
        live["providers"]["standin-yum"]["apiKey"],
        json!("rotated-key")
    );
    assert_eq!(
        live["providers"]["standin-yum"]["unknownField"],
        json!({ "keep": true })
    );
    assert_eq!(
        live["providers"]["standin-yum"]["compat"]["supportsDeveloperRole"],
        json!(false)
    );
    assert!(
        !live["providers"]["standin-yum"]["baseUrl"]
            .as_str()
            .unwrap_or_default()
            .contains("15721"),
        "update without takeover must keep the real upstream"
    );
    assert_eq!(
        fs::read_to_string(&auth_path).expect("auth after update"),
        auth_before
    );
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across Pi CRUD"
)]
async fn linux_standin_delete_removes_models_json_and_invalidates_takeover_backup() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let (models_path, settings_path, auth_path) = setup_linux_standin_pi_home();
    let auth_before = fs::read_to_string(&auth_path).expect("auth before");
    let settings_before = fs::read_to_string(&settings_path).expect("settings before");

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    ProviderService::add(
        &state,
        AppType::Pi,
        pi_openai_provider(
            "keep",
            "Keep",
            "keep-model",
            "https://keep.example/v1",
            "keep-key",
        ),
        true,
    )
    .expect("add keep");
    ProviderService::add(
        &state,
        AppType::Pi,
        pi_openai_provider(
            "gone",
            "Gone",
            "gone-model",
            "https://gone.example/v1",
            "gone-key",
        ),
        true,
    )
    .expect("add gone");

    state
        .proxy_service
        .set_takeover_for_app("pi", true)
        .await
        .expect("enable Pi takeover to seed a whole-file backup");
    assert!(state
        .db
        .get_live_backup("pi")
        .await
        .expect("read seeded backup")
        .is_some());

    ProviderService::delete(&state, AppType::Pi, "gone").expect("delete live gone");

    let projected = read_json(&models_path);
    assert!(
        projected["providers"].get("gone").is_none(),
        "delete must drop the node from live models.json"
    );
    assert!(projected["providers"].get("keep").is_some());

    let backup = state
        .db
        .get_live_backup("pi")
        .await
        .expect("read refreshed backup")
        .expect("valid backup is refreshed, not left stale");
    let parsed: Value =
        serde_json::from_str(&backup.original_config).expect("parse refreshed backup");
    assert!(
        parsed["providers"].get("gone").is_none(),
        "takeover backup must drop the deleted node so disable cannot restore it"
    );
    assert!(parsed["providers"].get("keep").is_some());

    state
        .proxy_service
        .set_takeover_for_app("pi", false)
        .await
        .expect("disable Pi takeover");

    let restored = read_json(&models_path);
    assert!(
        restored["providers"].get("gone").is_none(),
        "disable must not resurrect the deleted provider"
    );
    assert_eq!(
        restored["providers"]["keep"]["baseUrl"],
        json!("https://keep.example/v1")
    );
    assert_eq!(restored["providers"]["keep"]["apiKey"], json!("keep-key"));
    assert_eq!(
        fs::read_to_string(&auth_path).expect("auth after delete"),
        auth_before
    );
    assert_eq!(
        fs::read_to_string(&settings_path).expect("settings after delete"),
        settings_before,
        "delete of a non-default provider must not rewrite settings.json"
    );
    assert!(state
        .db
        .get_live_backup("pi")
        .await
        .expect("backup after disable")
        .is_none());
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across Pi CRUD"
)]
async fn linux_standin_delete_default_reassigns_settings_default_provider() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let (models_path, settings_path, auth_path) = setup_linux_standin_pi_home();
    let auth_before = fs::read_to_string(&auth_path).expect("auth before");

    let state = create_test_state().expect("create test state");
    ProviderService::add(
        &state,
        AppType::Pi,
        pi_openai_provider(
            "keep",
            "Keep",
            "keep-model",
            "https://keep.example/v1",
            "keep-key",
        ),
        true,
    )
    .expect("add keep");
    ProviderService::add(
        &state,
        AppType::Pi,
        pi_openai_provider(
            "gone",
            "Gone",
            "gone-model",
            "https://gone.example/v1",
            "gone-key",
        ),
        true,
    )
    .expect("add gone");

    fs::write(
        &settings_path,
        r#"{"defaultProvider":"gone","defaultModel":"gone-model","theme":"keep-me","sessionDir":"/tmp/pi-sessions"}"#,
    )
    .expect("set live default to gone");

    ProviderService::delete(&state, AppType::Pi, "gone").expect("delete default provider");

    let live = read_json(&models_path);
    assert!(live["providers"].get("gone").is_none());
    assert!(live["providers"].get("keep").is_some());

    let settings = read_json(&settings_path);
    assert_eq!(settings["defaultProvider"], json!("keep"));
    assert_eq!(settings["defaultModel"], json!("keep-model"));
    assert_eq!(settings["theme"], json!("keep-me"));
    assert_eq!(settings["sessionDir"], json!("/tmp/pi-sessions"));
    assert_eq!(
        fs::read_to_string(&auth_path).expect("auth after default delete"),
        auth_before,
        "delete default must not touch auth.json"
    );
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

fn setup_linux_standin_claude_codex_home() -> (PathBuf, PathBuf, PathBuf) {
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    apply_wsl_standin_overrides(&user_home);
    let claude_settings = get_claude_settings_path();
    let codex_config = get_codex_config_path();
    let codex_auth = get_codex_auth_path();
    assert_linux_standin_path(&claude_settings);
    assert_linux_standin_path(&codex_config);
    assert_linux_standin_path(&codex_auth);
    (claude_settings, codex_config, codex_auth)
}

fn assert_claude_unknown_fields(live: &Value) {
    assert_eq!(live["customTopLevel"], json!("keep-me"));
    assert_eq!(live["permissions"]["allow"], json!(["Read"]));
    assert_eq!(live["env"]["CC_SWITCH_KEEP"], json!("roundtrip"));
}

fn assert_codex_unknown_toml(config: &str) {
    assert!(
        config.contains("[projects.\"/tmp/cc-switch-keep\"]")
            || config.contains("[projects.'/tmp/cc-switch-keep']")
            || config.contains("/tmp/cc-switch-keep"),
        "Codex extra TOML table must survive: {config}"
    );
    assert!(
        config.contains("trust_level") && config.contains("trusted"),
        "Codex extra project trust_level must survive: {config}"
    );
}

/// Claude takeover roundtrip on the Linux WSL stand-in: unknown settings fields
/// survive projection and disable restores the real token / URL (parallel to Pi
/// `models.json` unknown-field fixtures).
#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_claude_takeover_roundtrip_preserves_unknown_settings_fields() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let (claude_settings, _codex_config, _codex_auth) = setup_linux_standin_claude_codex_home();

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    state
        .db
        .save_provider(AppType::Claude.as_str(), &claude_provider())
        .expect("save Claude provider");
    ProviderService::switch(&state, AppType::Claude, "standin-claude").expect("write Claude live");

    let before: Value = read_json_file(&claude_settings).expect("Claude live before");
    assert_eq!(
        before["env"]["ANTHROPIC_BASE_URL"],
        json!("https://api.anthropic.example")
    );
    assert_eq!(
        before["env"]["ANTHROPIC_AUTH_TOKEN"],
        json!("claude-live-token")
    );
    assert_claude_unknown_fields(&before);
    assert_schema18_pi_has_no_proxy_config_row(&state);

    state
        .proxy_service
        .set_takeover_for_app("claude", true)
        .await
        .expect("enable Claude takeover");

    let status = state
        .proxy_service
        .get_status()
        .await
        .expect("proxy status");
    let origin = format!("http://127.0.0.1:{}", status.port);
    let projected: Value = read_json_file(&claude_settings).expect("Claude projected");
    assert_eq!(projected["env"]["ANTHROPIC_BASE_URL"], json!(origin));
    assert_eq!(
        projected["env"]["ANTHROPIC_AUTH_TOKEN"],
        json!("PROXY_MANAGED")
    );
    assert_claude_unknown_fields(&projected);
    assert_schema18_pi_has_no_proxy_config_row(&state);

    state
        .proxy_service
        .set_takeover_for_app("claude", false)
        .await
        .expect("disable Claude takeover");

    let restored: Value = read_json_file(&claude_settings).expect("Claude restored");
    assert_eq!(
        restored["env"]["ANTHROPIC_BASE_URL"],
        json!("https://api.anthropic.example")
    );
    assert_eq!(
        restored["env"]["ANTHROPIC_AUTH_TOKEN"],
        json!("claude-live-token")
    );
    assert_claude_unknown_fields(&restored);
    assert!(state
        .db
        .get_live_backup("claude")
        .await
        .expect("claude backup")
        .is_none());
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

/// Codex takeover roundtrip: extra TOML tables survive; auth.json extra fields
/// are not rewritten (same contract as Pi `auth.json` / `settings.json`).
#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_codex_takeover_roundtrip_preserves_toml_and_auth() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let (_claude_settings, codex_config, codex_auth) = setup_linux_standin_claude_codex_home();

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    state
        .db
        .save_provider(AppType::Codex.as_str(), &codex_provider())
        .expect("save Codex provider");
    ProviderService::switch(&state, AppType::Codex, "standin-codex").expect("write Codex live");
    seed_codex_auth_with_unknown_fields(&codex_auth, "codex-live-key");
    let auth_before = fs::read_to_string(&codex_auth).expect("Codex auth before");
    let config_before = fs::read_to_string(&codex_config).expect("Codex config before");
    assert_codex_unknown_toml(&config_before);
    assert_schema18_pi_has_no_proxy_config_row(&state);

    state
        .proxy_service
        .set_takeover_for_app("codex", true)
        .await
        .expect("enable Codex takeover");

    let status = state
        .proxy_service
        .get_status()
        .await
        .expect("proxy status");
    let origin = format!("http://127.0.0.1:{}/v1", status.port);
    let projected = fs::read_to_string(&codex_config).expect("Codex projected");
    assert!(
        projected.contains(&origin),
        "Codex config.toml must project onto the shared listen /v1: {projected}"
    );
    assert!(projected.contains("PROXY_MANAGED"));
    assert!(!projected.contains("https://api.openai.example/v1"));
    assert_codex_unknown_toml(&projected);
    assert_eq!(
        fs::read_to_string(&codex_auth).expect("Codex auth during takeover"),
        auth_before,
        "Codex takeover must not rewrite auth.json"
    );
    let auth_live: Value = read_json_file(&codex_auth).expect("parse auth during takeover");
    assert_eq!(auth_live["keepMe"], json!("roundtrip"));
    assert_eq!(auth_live["tokens"]["access_token"], json!("leave-me"));
    assert_eq!(auth_live["OPENAI_API_KEY"], json!("codex-live-key"));
    assert_schema18_pi_has_no_proxy_config_row(&state);

    state
        .proxy_service
        .set_takeover_for_app("codex", false)
        .await
        .expect("disable Codex takeover");

    let restored = fs::read_to_string(&codex_config).expect("Codex restored");
    assert!(restored.contains("https://api.openai.example/v1"));
    assert!(!restored.contains("PROXY_MANAGED"));
    assert_codex_unknown_toml(&restored);
    assert_eq!(
        fs::read_to_string(&codex_auth).expect("Codex auth after restore"),
        auth_before,
        "disable must not rewrite auth.json"
    );
    assert!(state
        .db
        .get_live_backup("codex")
        .await
        .expect("codex backup")
        .is_none());
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

/// Parallel to Pi delete-during-takeover: Claude hot-switch refreshes the live
/// backup so disable restores the new card, not the pre-takeover one.
#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_claude_switch_during_takeover_refreshes_backup() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let (claude_settings, _codex_config, _codex_auth) = setup_linux_standin_claude_codex_home();

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    let provider_a = claude_provider_named(
        "standin-claude-a",
        "Claude A",
        "https://api.a.example",
        "token-a",
    );
    let provider_b = claude_provider_named(
        "standin-claude-b",
        "Claude B",
        "https://api.b.example",
        "token-b",
    );
    state
        .db
        .save_provider(AppType::Claude.as_str(), &provider_a)
        .expect("save A");
    state
        .db
        .save_provider(AppType::Claude.as_str(), &provider_b)
        .expect("save B");
    ProviderService::switch(&state, AppType::Claude, "standin-claude-a").expect("switch A");

    state
        .proxy_service
        .set_takeover_for_app("claude", true)
        .await
        .expect("enable Claude takeover");
    ProviderService::switch(&state, AppType::Claude, "standin-claude-b")
        .expect("hot-switch B while takeover is on");

    let status = state
        .proxy_service
        .get_status()
        .await
        .expect("proxy status");
    let origin = format!("http://127.0.0.1:{}", status.port);
    let projected: Value = read_json_file(&claude_settings).expect("Claude after hot-switch");
    assert_eq!(projected["env"]["ANTHROPIC_BASE_URL"], json!(origin));
    assert_eq!(
        projected["env"]["ANTHROPIC_AUTH_TOKEN"],
        json!("PROXY_MANAGED")
    );
    assert_claude_unknown_fields(&projected);
    let backup = state
        .db
        .get_live_backup("claude")
        .await
        .expect("read refreshed backup")
        .expect("backup exists after hot-switch");
    let parsed: Value = serde_json::from_str(&backup.original_config).expect("parse Claude backup");
    assert_eq!(
        parsed["env"]["ANTHROPIC_BASE_URL"],
        json!("https://api.b.example"),
        "backup must be B so disable cannot restore A"
    );
    assert_eq!(parsed["env"]["ANTHROPIC_AUTH_TOKEN"], json!("token-b"));
    assert_eq!(parsed["customTopLevel"], json!("keep-me"));

    state
        .proxy_service
        .set_takeover_for_app("claude", false)
        .await
        .expect("disable Claude takeover");

    let restored: Value = read_json_file(&claude_settings).expect("Claude restored after switch");
    assert_eq!(
        restored["env"]["ANTHROPIC_BASE_URL"],
        json!("https://api.b.example")
    );
    assert_eq!(restored["env"]["ANTHROPIC_AUTH_TOKEN"], json!("token-b"));
    assert_ne!(
        restored["env"]["ANTHROPIC_BASE_URL"],
        json!("https://api.a.example")
    );
    assert_claude_unknown_fields(&restored);
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

/// Parallel to Pi delete-during-takeover: Codex hot-switch refreshes backup.
#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_codex_switch_during_takeover_refreshes_backup() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let (_claude_settings, codex_config, codex_auth) = setup_linux_standin_claude_codex_home();

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    let provider_a = codex_provider_named(
        "standin-codex-a",
        "CodexA",
        "https://api.a.example/v1",
        "key-a",
    );
    let provider_b = codex_provider_named(
        "standin-codex-b",
        "CodexB",
        "https://api.b.example/v1",
        "key-b",
    );
    state
        .db
        .save_provider(AppType::Codex.as_str(), &provider_a)
        .expect("save A");
    state
        .db
        .save_provider(AppType::Codex.as_str(), &provider_b)
        .expect("save B");
    ProviderService::switch(&state, AppType::Codex, "standin-codex-a").expect("switch A");
    seed_codex_auth_with_unknown_fields(&codex_auth, "key-a");
    let auth_before = fs::read_to_string(&codex_auth).expect("auth before takeover");

    state
        .proxy_service
        .set_takeover_for_app("codex", true)
        .await
        .expect("enable Codex takeover");
    ProviderService::switch(&state, AppType::Codex, "standin-codex-b")
        .expect("hot-switch B while takeover is on");

    let projected = fs::read_to_string(&codex_config).expect("Codex after hot-switch");
    assert!(
        projected.contains("PROXY_MANAGED"),
        "hot-switch must keep takeover placeholder: {projected}"
    );
    assert!(
        !projected.contains("https://api.a.example/v1"),
        "live must not keep A's upstream: {projected}"
    );
    assert_codex_unknown_toml(&projected);
    assert_eq!(
        fs::read_to_string(&codex_auth).expect("auth during hot-switch"),
        auth_before,
        "Codex hot-switch must not rewrite auth.json"
    );

    let backup = state
        .db
        .get_live_backup("codex")
        .await
        .expect("read refreshed backup")
        .expect("backup exists after hot-switch");
    let parsed: Value = serde_json::from_str(&backup.original_config).expect("parse Codex backup");
    let backup_config = parsed["config"].as_str().unwrap_or_default();
    assert!(
        backup_config.contains("https://api.b.example/v1"),
        "backup must be B so disable cannot restore A: {backup_config}"
    );
    assert!(
        !backup_config.contains("https://api.a.example/v1"),
        "backup must drop A: {backup_config}"
    );

    state
        .proxy_service
        .set_takeover_for_app("codex", false)
        .await
        .expect("disable Codex takeover");

    let restored = fs::read_to_string(&codex_config).expect("Codex restored after switch");
    assert!(restored.contains("https://api.b.example/v1"));
    assert!(!restored.contains("https://api.a.example/v1"));
    assert!(!restored.contains("PROXY_MANAGED"));
    assert_codex_unknown_toml(&restored);
    assert_eq!(
        fs::read_to_string(&codex_auth).expect("auth after restore"),
        auth_before
    );
    assert_schema18_pi_has_no_proxy_config_row(&state);
}

/// Disable Claude while Codex stays projected (and the reverse).
#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations across takeover awaits"
)]
async fn linux_standin_independent_disable_leaves_the_other_app_projected() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let (claude_settings, codex_config, _codex_auth) = setup_linux_standin_claude_codex_home();

    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    state
        .db
        .save_provider(AppType::Claude.as_str(), &claude_provider())
        .expect("save Claude");
    state
        .db
        .save_provider(AppType::Codex.as_str(), &codex_provider())
        .expect("save Codex");
    ProviderService::switch(&state, AppType::Claude, "standin-claude").expect("write Claude");
    ProviderService::switch(&state, AppType::Codex, "standin-codex").expect("write Codex");

    state
        .proxy_service
        .set_takeover_for_app("claude", true)
        .await
        .expect("enable Claude");
    state
        .proxy_service
        .set_takeover_for_app("codex", true)
        .await
        .expect("enable Codex");

    let status = state
        .proxy_service
        .get_status()
        .await
        .expect("proxy status");
    let origin = format!("http://127.0.0.1:{}", status.port);
    let codex_origin = format!("{origin}/v1");

    state
        .proxy_service
        .set_takeover_for_app("claude", false)
        .await
        .expect("disable Claude only");

    let claude_restored: Value =
        read_json_file(&claude_settings).expect("Claude independent restore");
    assert_eq!(
        claude_restored["env"]["ANTHROPIC_BASE_URL"],
        json!("https://api.anthropic.example")
    );
    assert_eq!(
        claude_restored["env"]["ANTHROPIC_AUTH_TOKEN"],
        json!("claude-live-token")
    );
    assert_claude_unknown_fields(&claude_restored);
    let (claude_enabled, _) = state.db.get_proxy_flags_sync("claude");
    let (codex_enabled, _) = state.db.get_proxy_flags_sync("codex");
    assert!(!claude_enabled);
    assert!(codex_enabled);
    let codex_still = fs::read_to_string(&codex_config).expect("Codex still projected");
    assert!(
        codex_still.contains(&codex_origin),
        "disabling Claude must not restore Codex: {codex_still}"
    );
    assert!(codex_still.contains("PROXY_MANAGED"));
    assert_schema18_pi_has_no_proxy_config_row(&state);

    state
        .proxy_service
        .set_takeover_for_app("codex", false)
        .await
        .expect("disable Codex only");
    let claude_still: Value = read_json_file(&claude_settings).expect("Claude stays restored");
    assert_eq!(
        claude_still["env"]["ANTHROPIC_BASE_URL"],
        json!("https://api.anthropic.example")
    );
    let codex_restored = fs::read_to_string(&codex_config).expect("Codex independent restore");
    assert!(codex_restored.contains("https://api.openai.example/v1"));
    assert!(!codex_restored.contains("PROXY_MANAGED"));
    let (codex_enabled_after, _) = state.db.get_proxy_flags_sync("codex");
    assert!(!codex_enabled_after);
    assert_schema18_pi_has_no_proxy_config_row(&state);
}
