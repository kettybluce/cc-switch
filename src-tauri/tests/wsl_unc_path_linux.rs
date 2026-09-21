//! Linux-cloud integration for WSL UNC / stand-in HOME path normalization.
//!
//! Live Windows layout (never written by these tests):
//! ```text
//! \\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude
//! \\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.codex
//! \\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi\agent
//! ```
//!
//! SCHEMA 18 only. No C: `pi-wsl-sessions` mirror. No Portable / MSI.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use cc_switch_lib::{
    get_claude_settings_path, get_codex_auth_path, get_codex_config_path, update_settings,
    AppSettings, AppType, Provider, ProviderService, SCHEMA_VERSION,
};

#[path = "support.rs"]
mod support;
use support::{create_test_state, ensure_test_home, reset_test_fs, test_mutex};

fn posix_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

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

fn assert_schema18_no_pi_proxy_row(state: &cc_switch_lib::AppState) {
    assert_eq!(SCHEMA_VERSION, 18, "fixtures must stay on SCHEMA 18");
    assert!(
        !state
            .db
            .has_proxy_config_row("pi")
            .expect("pi proxy_config row"),
        "SCHEMA 18 CHECK must not gain a proxy_config row for pi"
    );
}

fn ends_with_tfdx(path: &Path, suffix: &str) -> bool {
    posix_slash(path).ends_with(suffix)
}

fn pi_openai_provider() -> Provider {
    let mut provider = Provider::with_id(
        "standin-edge".to_string(),
        "Stand-in Edge".to_string(),
        json!({
            "name": "Stand-in Edge",
            "baseUrl": "https://edge.example/v1",
            "apiKey": "edge-key",
            "api": "openai-completions",
            "models": [{ "id": "edge-model" }]
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
}

fn write_minimal_claude_codex_dirs(user_home: &Path) -> (PathBuf, PathBuf) {
    let claude_dir = user_home.join(".claude");
    let codex_dir = user_home.join(".codex");
    fs::create_dir_all(&claude_dir).expect("create Claude dir");
    fs::create_dir_all(&codex_dir).expect("create Codex dir");
    (claude_dir, codex_dir)
}

/// Trailing slashes on the Ubuntu-22.04 / tfdx8045 stand-in still land Claude,
/// Codex, and Pi (`~/.pi/agent`) under the same isolated HOME tree.
#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations"
)]
async fn linux_standin_trailing_slash_overrides_use_tfdx8045_agent_and_homes() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    let (claude_dir, codex_dir) = write_minimal_claude_codex_dirs(&user_home);
    let pi_agent = user_home.join(".pi").join("agent");
    fs::create_dir_all(&pi_agent).expect("create Pi agent");

    update_settings(AppSettings {
        claude_config_dir: Some(format!("{}/", posix_slash(&claude_dir))),
        codex_config_dir: Some(format!("{}/", posix_slash(&codex_dir))),
        pi_config_dir: Some(format!("{}/", posix_slash(&pi_agent))),
        ..AppSettings::default()
    })
    .expect("apply trailing-slash stand-in overrides");

    let claude_settings = get_claude_settings_path();
    let codex_config = get_codex_config_path();
    let codex_auth = get_codex_auth_path();
    assert_linux_standin_path(&claude_settings);
    assert_linux_standin_path(&codex_config);
    assert_linux_standin_path(&codex_auth);
    assert!(
        ends_with_tfdx(&claude_settings, "tfdx8045/.claude/settings.json"),
        "Claude home must be the stand-in user: {}",
        claude_settings.display()
    );
    assert!(
        ends_with_tfdx(&codex_config, "tfdx8045/.codex/config.toml"),
        "Codex home must be the stand-in user: {}",
        codex_config.display()
    );

    let state = create_test_state().expect("create test state");
    assert_schema18_no_pi_proxy_row(&state);
    ProviderService::add(&state, AppType::Pi, pi_openai_provider(), true)
        .expect("create Pi provider under trailing-slash agent dir");

    let models_path = pi_agent.join("models.json");
    assert_linux_standin_path(&models_path);
    assert!(
        ends_with_tfdx(&models_path, "tfdx8045/.pi/agent/models.json"),
        "Pi must write ~/.pi/agent/models.json: {}",
        models_path.display()
    );
    assert!(
        !user_home.join(".pi").join("models.json").exists(),
        "must not create the Pi-root models.json"
    );
    let live: Value = serde_json::from_str(&fs::read_to_string(&models_path).expect("read models"))
        .expect("json");
    assert_eq!(
        live["providers"]["standin-edge"]["baseUrl"],
        json!("https://edge.example/v1")
    );
    assert_schema18_no_pi_proxy_row(&state);
}

/// Pointing `pi_config_dir` at `~/.pi` (not `~/.pi/agent`) still canonicalizes
/// onto the agent directory under the tfdx8045 stand-in home.
#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations"
)]
async fn linux_standin_dot_pi_override_canonicalizes_to_agent() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    let (claude_dir, codex_dir) = write_minimal_claude_codex_dirs(&user_home);
    let pi_root = user_home.join(".pi");
    fs::create_dir_all(pi_root.join("agent")).expect("create agent under .pi");

    update_settings(AppSettings {
        claude_config_dir: Some(claude_dir.to_string_lossy().to_string()),
        codex_config_dir: Some(codex_dir.to_string_lossy().to_string()),
        pi_config_dir: Some(format!("{}/", posix_slash(&pi_root))),
        ..AppSettings::default()
    })
    .expect("point Pi at .pi with trailing slash");

    let state = create_test_state().expect("create test state");
    assert_schema18_no_pi_proxy_row(&state);
    ProviderService::add(&state, AppType::Pi, pi_openai_provider(), true)
        .expect("create provider after .pi canonicalize");

    let models_path = pi_root.join("agent").join("models.json");
    assert!(
        models_path.is_file(),
        "canonicalize_dot_pi must write agent/models.json"
    );
    assert!(
        !pi_root.join("models.json").exists(),
        "must not write ~/.pi/models.json"
    );
    assert!(ends_with_tfdx(
        &models_path,
        "tfdx8045/.pi/agent/models.json"
    ));
    assert_linux_standin_path(&get_claude_settings_path());
    assert_linux_standin_path(&get_codex_config_path());
    assert_schema18_no_pi_proxy_row(&state);
}

/// POSIX Claude/Codex stand-in is not a Win32 UNC path, so Pi must not invent
/// `\\wsl.localhost\…` — it falls back to `$HOME/.pi/agent` under the isolated
/// test HOME. Explicit `pi_config_dir` is what pins the tfdx8045 tree.
#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations"
)]
async fn posix_claude_codex_standin_does_not_infer_unc_pi_home() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    let user_home = wsl_standin_user_home(test_home);
    let (claude_dir, codex_dir) = write_minimal_claude_codex_dirs(&user_home);

    update_settings(AppSettings {
        claude_config_dir: Some(claude_dir.to_string_lossy().to_string()),
        codex_config_dir: Some(codex_dir.to_string_lossy().to_string()),
        pi_config_dir: None,
        ..AppSettings::default()
    })
    .expect("Claude/Codex stand-in, default Pi");

    assert!(
        ends_with_tfdx(
            &get_claude_settings_path(),
            "tfdx8045/.claude/settings.json"
        ),
        "Claude still uses the stand-in home"
    );
    assert!(ends_with_tfdx(
        &get_codex_config_path(),
        "tfdx8045/.codex/config.toml"
    ));

    let state = create_test_state().expect("create test state");
    ProviderService::add(&state, AppType::Pi, pi_openai_provider(), true)
        .expect("create Pi under default non-UNC home");

    let default_models = test_home.join(".pi").join("agent").join("models.json");
    let standin_models = user_home.join(".pi").join("agent").join("models.json");
    assert!(
        default_models.is_file(),
        "non-UNC default must be $TEST_HOME/.pi/agent: {}",
        default_models.display()
    );
    assert!(
        !standin_models.exists(),
        "POSIX Claude/Codex dirs must not be treated as WSL UNC homes"
    );
    assert_linux_standin_path(&default_models);
    assert_schema18_no_pi_proxy_row(&state);
}

/// Non-UNC isolated HOME: Claude `~/.claude`, Codex `~/.codex`, Pi `~/.pi/agent`.
#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME / settings mutations"
)]
async fn non_unc_test_home_uses_dot_pi_agent_and_schema18() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let test_home = ensure_test_home();
    update_settings(AppSettings::default()).expect("clear overrides");

    let claude_settings = get_claude_settings_path();
    let codex_config = get_codex_config_path();
    assert!(
        posix_slash(&claude_settings).ends_with("/.claude/settings.json"),
        "non-UNC Claude must live under test HOME: {}",
        claude_settings.display()
    );
    assert!(
        posix_slash(&codex_config).ends_with("/.codex/config.toml"),
        "non-UNC Codex must live under test HOME: {}",
        codex_config.display()
    );
    assert!(!posix_slash(&claude_settings).contains("tfdx8045"));
    assert!(!posix_slash(&codex_config).contains("wsl.localhost"));

    let state = create_test_state().expect("create test state");
    assert_schema18_no_pi_proxy_row(&state);
    ProviderService::add(&state, AppType::Pi, pi_openai_provider(), true)
        .expect("create Pi under default HOME");

    let models_path = test_home.join(".pi").join("agent").join("models.json");
    assert!(
        models_path.is_file(),
        "Pi default is ~/.pi/agent, not ~/.pi: {}",
        models_path.display()
    );
    assert!(!test_home.join(".pi").join("models.json").exists());
    assert_linux_standin_path(&models_path);
    assert_schema18_no_pi_proxy_row(&state);
}
