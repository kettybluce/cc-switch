//! Linux-cloud stand-in for WSL Pi / Claude / Codex proxy projection.
//!
//! Live Windows layout (never written by these tests):
//! ```text
//! \\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude
//! \\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.codex
//! \\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi\agent
//! ```
//!
//! Cloud stand-in is a POSIX tree under the isolated test HOME. No C: writes,
//! no `pi-wsl-sessions` mirror, SCHEMA 18 only (no `proxy_config` row for `pi`).

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
    let mut provider = Provider::with_id(
        "standin-claude".to_string(),
        "Stand-in Claude".to_string(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "claude-live-token",
                "ANTHROPIC_BASE_URL": "https://api.anthropic.example"
            }
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
}

fn codex_provider() -> Provider {
    let mut provider = Provider::with_id(
        "standin-codex".to_string(),
        "Stand-in Codex".to_string(),
        json!({
            "auth": {"OPENAI_API_KEY": "codex-live-key"},
            "config": r#"model_provider = "standin"
model = "gpt-5"

[model_providers.standin]
name = "Stand-in"
base_url = "https://api.openai.example/v1"
wire_api = "responses"
"#
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
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
    let codex_auth_before: Value = read_json_file(&codex_auth).expect("Codex auth before");
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
    let codex_auth_after: Value = read_json_file(&codex_auth).expect("Codex auth after");
    assert_eq!(
        codex_auth_after, codex_auth_before,
        "Codex auth.json is not the projection target for custom-key takeover"
    );

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
