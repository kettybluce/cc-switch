//! SCHEMA 18 smoke tests for OpenCode / Hermes / OpenClaw additive surfaces.
//!
//! These apps write live files in additive mode. They have **no**
//! `proxy_config` row (CHECK only allows claude/codex/gemini/grokbuild) and
//! **no** Pi-style takeover. Tests use isolated HOME + config-dir overrides,
//! fixture stubs, and `ProviderService` CRUD — never a real network.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use cc_switch_lib::{
    get_hermes_live_provider_ids, get_openclaw_live_provider, get_openclaw_live_provider_ids,
    get_opencode_live_provider_ids, hermes_config, openclaw_config, opencode_config,
    scan_openclaw_config_health, update_settings, AppSettings, AppType, Provider, ProviderService,
    SCHEMA_VERSION,
};

#[path = "support.rs"]
mod support;
use support::{create_test_state, ensure_test_home, reset_test_fs, test_mutex};

const OPENCODE_FIXTURE: &str = include_str!("fixtures/additive/opencode_with_unknown_fields.json");
const OPENCLAW_FIXTURE: &str = include_str!("fixtures/additive/openclaw_with_unknown_fields.json");
const HERMES_FIXTURE: &str = include_str!("fixtures/additive/hermes_with_unknown_fields.yaml");

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

fn additive_standin_root(test_home: &Path) -> PathBuf {
    test_home.join("profiles").join("additive")
}

fn apply_additive_overrides(test_home: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let root = additive_standin_root(test_home);
    let opencode_dir = root.join("opencode");
    let openclaw_dir = root.join("openclaw");
    let hermes_dir = root.join("hermes");
    fs::create_dir_all(&opencode_dir).expect("create OpenCode stand-in");
    fs::create_dir_all(&openclaw_dir).expect("create OpenClaw stand-in");
    fs::create_dir_all(&hermes_dir).expect("create Hermes stand-in");

    update_settings(AppSettings {
        opencode_config_dir: Some(opencode_dir.to_string_lossy().to_string()),
        openclaw_config_dir: Some(openclaw_dir.to_string_lossy().to_string()),
        hermes_config_dir: Some(hermes_dir.to_string_lossy().to_string()),
        ..AppSettings::default()
    })
    .expect("point OpenCode/OpenClaw/Hermes at isolated stand-in dirs");

    (opencode_dir, openclaw_dir, hermes_dir)
}

fn seed_live_fixtures(opencode_dir: &Path, openclaw_dir: &Path, hermes_dir: &Path) {
    fs::write(opencode_dir.join("opencode.json"), OPENCODE_FIXTURE).expect("seed opencode.json");
    fs::write(openclaw_dir.join("openclaw.json"), OPENCLAW_FIXTURE).expect("seed openclaw.json");
    fs::write(hermes_dir.join("config.yaml"), HERMES_FIXTURE).expect("seed hermes config.yaml");
}

fn setup_additive_home() -> (PathBuf, PathBuf, PathBuf) {
    reset_test_fs();
    let test_home = ensure_test_home();
    let dirs = apply_additive_overrides(test_home);
    seed_live_fixtures(&dirs.0, &dirs.1, &dirs.2);
    assert_linux_standin_path(&dirs.0.join("opencode.json"));
    assert_linux_standin_path(&dirs.1.join("openclaw.json"));
    assert_linux_standin_path(&dirs.2.join("config.yaml"));
    dirs
}

fn assert_schema18_additive_have_no_proxy_config_row(state: &cc_switch_lib::AppState) {
    assert_eq!(
        SCHEMA_VERSION, 18,
        "additive provider fixtures must stay on SCHEMA 18"
    );
    assert!(
        state
            .db
            .has_proxy_config_row("claude")
            .expect("claude proxy_config row"),
        "Claude still owns a SCHEMA 18 listen row"
    );
    for app in ["opencode", "openclaw", "hermes", "pi"] {
        assert!(
            !state
                .db
                .has_proxy_config_row(app)
                .expect("proxy_config lookup"),
            "SCHEMA 18 CHECK must not gain a proxy_config row for {app}"
        );
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("read json")).expect("parse json")
}

fn opencode_provider(id: &str, url: &str, key: &str) -> Provider {
    let mut provider = Provider::with_id(
        id.to_string(),
        format!("OpenCode {id}"),
        json!({
            "npm": "@ai-sdk/openai-compatible",
            "name": format!("OpenCode {id}"),
            "options": {
                "baseURL": url,
                "apiKey": key,
                "setCacheKey": true
            },
            "models": {
                "gpt-4o": { "name": "GPT-4o" }
            },
            "futureVendorFlag": true
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
}

fn openclaw_provider(id: &str, url: &str, key: &str) -> Provider {
    let mut provider = Provider::with_id(
        id.to_string(),
        format!("OpenClaw {id}"),
        json!({
            "baseUrl": url,
            "apiKey": key,
            "api": "openai-completions",
            "models": [{ "id": "model-a", "name": "Model A", "futureModelFlag": true }],
            "futureVendorFlag": true
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
}

fn hermes_provider(id: &str, url: &str, key: &str) -> Provider {
    let mut provider = Provider::with_id(
        id.to_string(),
        format!("Hermes {id}"),
        json!({
            "name": id,
            "base_url": url,
            "api_key": key,
            "api_mode": "chat_completions",
            "models": [{ "id": "gpt-4o", "context_length": 128000 }],
            "foo_bar": "keep-me-around"
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
}

#[test]
fn schema18_additive_apps_have_no_proxy_config_and_no_takeover() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let _dirs = setup_additive_home();
    let state = create_test_state().expect("create test state");
    assert_schema18_additive_have_no_proxy_config_row(&state);

    for app in [AppType::OpenCode, AppType::OpenClaw, AppType::Hermes] {
        assert!(app.is_additive_mode());
        assert!(!app.supports_local_proxy());
        assert!(!app.supports_proxy_takeover());
        assert_eq!(
            ProviderService::current(&state, app.clone()).expect("additive current"),
            "",
            "{:?} has no current-provider concept",
            app
        );
    }
}

#[test]
fn opencode_crud_projects_live_json_and_preserves_unknown_fields() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let (opencode_dir, _, _) = setup_additive_home();
    let live_path = opencode_dir.join("opencode.json");
    let theme_before = read_json(&live_path)["theme"].clone();

    let state = create_test_state().expect("create test state");
    assert_schema18_additive_have_no_proxy_config_row(&state);

    ProviderService::add(
        &state,
        AppType::OpenCode,
        opencode_provider(
            "standin-oc",
            "https://api.standin.example/v1",
            "standin-oc-key",
        ),
        true,
    )
    .expect("create live OpenCode provider");

    let live = read_json(&live_path);
    assert_eq!(live["theme"], theme_before, "CRUD must not rewrite theme");
    assert_eq!(live["customTopLevel"], json!("keep-me"));
    assert_eq!(live["model"], json!("keep-model"));
    assert_eq!(
        live["provider"]["fixture-oc"]["futureVendorFlag"],
        json!(true),
        "seeded unknown vendor field must survive a sibling create"
    );
    assert_eq!(
        live["provider"]["standin-oc"]["options"]["baseURL"],
        json!("https://api.standin.example/v1")
    );
    assert_eq!(
        live["provider"]["standin-oc"]["options"]["apiKey"],
        json!("standin-oc-key")
    );
    assert_eq!(
        live["provider"]["standin-oc"]["futureVendorFlag"],
        json!(true),
        "typed OpenCode projection must keep unknown top-level fields"
    );
    assert!(
        !live["provider"]["standin-oc"]["options"]["baseURL"]
            .as_str()
            .unwrap_or_default()
            .contains("15721"),
        "OpenCode must not rewrite to Claude listen"
    );

    let mut updated =
        opencode_provider("standin-oc", "https://rotated.example/v1", "rotated-oc-key");
    updated.settings_config["futureVendorFlag"] = json!("kept-after-update");
    ProviderService::update(&state, AppType::OpenCode, Some("standin-oc"), updated)
        .expect("update live OpenCode url/key");

    let live = read_json(&live_path);
    assert_eq!(
        live["provider"]["standin-oc"]["options"]["baseURL"],
        json!("https://rotated.example/v1")
    );
    assert_eq!(
        live["provider"]["standin-oc"]["options"]["apiKey"],
        json!("rotated-oc-key")
    );
    assert_eq!(
        live["provider"]["standin-oc"]["futureVendorFlag"],
        json!("kept-after-update")
    );
    assert_eq!(live["theme"], theme_before);

    let ids = get_opencode_live_provider_ids().expect("command: live ids");
    assert!(ids.contains(&"standin-oc".to_string()));
    assert!(ids.contains(&"fixture-oc".to_string()));

    ProviderService::delete(&state, AppType::OpenCode, "standin-oc").expect("delete live");
    let live = read_json(&live_path);
    assert!(live["provider"].get("standin-oc").is_none());
    assert!(live["provider"].get("fixture-oc").is_some());
    assert_eq!(live["theme"], theme_before);
    assert_schema18_additive_have_no_proxy_config_row(&state);
}

#[test]
fn openclaw_crud_projects_live_json_and_preserves_unknown_fields() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let (_, openclaw_dir, _) = setup_additive_home();
    let live_path = openclaw_dir.join("openclaw.json");
    let seeded = read_json(&live_path);
    let top_before = seeded["customTopLevel"].clone();

    let state = create_test_state().expect("create test state");
    assert_schema18_additive_have_no_proxy_config_row(&state);

    ProviderService::add(
        &state,
        AppType::OpenClaw,
        openclaw_provider(
            "standin-claw",
            "https://api.standin.example/v1",
            "standin-claw-key",
        ),
        true,
    )
    .expect("create live OpenClaw provider");

    let live = openclaw_config::read_openclaw_config().expect("read JSON5 after create");
    assert_eq!(live["customTopLevel"], top_before);
    assert_eq!(
        live["models"]["providers"]["fixture-claw"]["futureVendorFlag"],
        json!(true)
    );
    assert_eq!(
        live["models"]["providers"]["standin-claw"]["baseUrl"],
        json!("https://api.standin.example/v1")
    );
    assert_eq!(
        live["models"]["providers"]["standin-claw"]["apiKey"],
        json!("standin-claw-key")
    );
    assert_eq!(
        live["models"]["providers"]["standin-claw"]["futureVendorFlag"],
        json!(true)
    );
    assert_eq!(
        live["models"]["providers"]["standin-claw"]["models"][0]["futureModelFlag"],
        json!(true)
    );

    let updated = openclaw_provider(
        "standin-claw",
        "https://rotated.example/v1",
        "rotated-claw-key",
    );
    ProviderService::update(&state, AppType::OpenClaw, Some("standin-claw"), updated)
        .expect("update live OpenClaw");

    let live = openclaw_config::read_openclaw_config().expect("read JSON5 after update");
    assert_eq!(
        live["models"]["providers"]["standin-claw"]["baseUrl"],
        json!("https://rotated.example/v1")
    );
    assert_eq!(
        live["models"]["providers"]["standin-claw"]["apiKey"],
        json!("rotated-claw-key")
    );

    let ids = get_openclaw_live_provider_ids().expect("command: live ids");
    assert!(ids.contains(&"standin-claw".to_string()));
    let fragment = get_openclaw_live_provider("standin-claw".to_string())
        .expect("command: live fragment")
        .expect("standin-claw exists");
    assert_eq!(fragment["baseUrl"], json!("https://rotated.example/v1"));

    ProviderService::remove_from_live_config(&state, AppType::OpenClaw, "standin-claw")
        .expect("remove from live without deleting DB row");
    let live = openclaw_config::read_openclaw_config().expect("read JSON5 after remove");
    assert!(live["models"]["providers"].get("standin-claw").is_none());
    assert!(
        state
            .db
            .get_provider_by_id("standin-claw", AppType::OpenClaw.as_str())
            .expect("query db")
            .is_some(),
        "remove_from_live_config must keep the database row"
    );

    ProviderService::delete(&state, AppType::OpenClaw, "standin-claw").expect("delete db row");
    assert!(state
        .db
        .get_provider_by_id("standin-claw", AppType::OpenClaw.as_str())
        .expect("query after delete")
        .is_none());
    assert_eq!(
        openclaw_config::read_openclaw_config().expect("read JSON5 after delete")["customTopLevel"],
        top_before
    );
    assert_schema18_additive_have_no_proxy_config_row(&state);
}

#[test]
fn hermes_crud_projects_live_yaml_and_preserves_unknown_fields() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let (_, _, hermes_dir) = setup_additive_home();
    let live_path = hermes_dir.join("config.yaml");
    let yaml_before = fs::read_to_string(&live_path).expect("read hermes yaml");
    assert!(yaml_before.contains("customTopLevel: keep-me"));
    assert!(yaml_before.contains("foo_bar: keep-me-around"));

    let state = create_test_state().expect("create test state");
    assert_schema18_additive_have_no_proxy_config_row(&state);

    ProviderService::add(
        &state,
        AppType::Hermes,
        hermes_provider(
            "standin-hermes",
            "https://api.standin.example/v1",
            "standin-hermes-key",
        ),
        true,
    )
    .expect("create live Hermes provider");

    let yaml = fs::read_to_string(&live_path).expect("read after add");
    assert!(
        yaml.contains("customTopLevel: keep-me"),
        "top-level unknown field must survive CRUD:\n{yaml}"
    );
    assert!(
        yaml.contains("future_model_flag: keep-me"),
        "model-section unknown field must survive CRUD:\n{yaml}"
    );
    assert!(yaml.contains("standin-hermes"));
    assert!(yaml.contains("https://api.standin.example/v1"));
    assert!(yaml.contains("standin-hermes-key"));
    assert!(
        yaml.contains("foo_bar: keep-me-around"),
        "Hermes must write unknown provider fields:\n{yaml}"
    );
    assert!(
        yaml.contains("fixture-hermes"),
        "sibling fixture provider must remain"
    );

    let updated = hermes_provider(
        "standin-hermes",
        "https://rotated.example/v1",
        "rotated-hermes-key",
    );
    ProviderService::update(&state, AppType::Hermes, Some("standin-hermes"), updated)
        .expect("update live Hermes");

    let yaml = fs::read_to_string(&live_path).expect("read after update");
    assert!(yaml.contains("https://rotated.example/v1"));
    assert!(yaml.contains("rotated-hermes-key"));
    assert!(
        !yaml.contains("15721"),
        "Hermes must not rewrite to Claude listen:\n{yaml}"
    );
    assert!(yaml.contains("foo_bar: keep-me-around"));

    let ids = get_hermes_live_provider_ids().expect("command: live ids");
    assert!(ids.contains(&"standin-hermes".to_string()));
    assert!(ids.contains(&"fixture-hermes".to_string()));

    let live_map = hermes_config::get_providers().expect("get_providers");
    let fixture = live_map
        .get("fixture-hermes")
        .expect("fixture still listed");
    assert_eq!(
        fixture.get("rate_limit_delay").and_then(Value::as_f64),
        Some(0.5)
    );
    assert_eq!(
        fixture.get("key_env").and_then(Value::as_str),
        Some("FIXTURE_KEY")
    );

    ProviderService::delete(&state, AppType::Hermes, "standin-hermes").expect("delete live");
    let yaml = fs::read_to_string(&live_path).expect("read after delete");
    assert!(!yaml.contains("standin-hermes-key"));
    assert!(yaml.contains("fixture-hermes"));
    assert!(yaml.contains("customTopLevel: keep-me"));
    assert_schema18_additive_have_no_proxy_config_row(&state);
}

#[test]
fn import_from_live_fixtures_marks_providers_live_managed() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let _dirs = setup_additive_home();
    let state = create_test_state().expect("create test state");

    let oc = ProviderService::import_from_live(&state, AppType::OpenCode)
        .expect("import OpenCode fixture");
    let claw = ProviderService::import_from_live(&state, AppType::OpenClaw)
        .expect("import OpenClaw fixture");
    let hermes =
        ProviderService::import_from_live(&state, AppType::Hermes).expect("import Hermes fixture");
    assert_eq!(oc, 1, "fixture-oc should import once");
    assert_eq!(claw, 1, "fixture-claw should import once");
    assert_eq!(hermes, 1, "fixture-hermes should import once");

    for (app, id) in [
        (AppType::OpenCode, "fixture-oc"),
        (AppType::OpenClaw, "fixture-claw"),
        (AppType::Hermes, "fixture-hermes"),
    ] {
        let saved = state
            .db
            .get_provider_by_id(id, app.as_str())
            .expect("query imported")
            .expect("imported row");
        assert_eq!(
            saved
                .meta
                .as_ref()
                .and_then(|meta| meta.live_config_managed),
            Some(true),
            "{id} imported from live must be live-managed"
        );
    }

    let oc_saved = state
        .db
        .get_provider_by_id("fixture-oc", AppType::OpenCode.as_str())
        .expect("query oc")
        .expect("oc row");
    assert_eq!(
        oc_saved.settings_config["futureVendorFlag"],
        json!(true),
        "OpenCode import must keep unknown vendor fields"
    );

    let claw_saved = state
        .db
        .get_provider_by_id("fixture-claw", AppType::OpenClaw.as_str())
        .expect("query claw")
        .expect("claw row");
    assert_eq!(claw_saved.settings_config["futureVendorFlag"], json!(true));

    assert_schema18_additive_have_no_proxy_config_row(&state);
}

#[test]
fn openclaw_health_scan_reads_fixture_without_network() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let _dirs = setup_additive_home();
    let warnings = scan_openclaw_config_health().expect("scan live fixture");
    let codes: Vec<String> = warnings.into_iter().map(|w| w.code).collect();
    assert!(
        codes.contains(&"invalid_tools_profile".to_string()),
        "fixture tools.profile=default must surface invalid_tools_profile, got {codes:?}"
    );
}

#[test]
fn live_config_modules_roundtrip_fixture_providers_offline() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    let _dirs = setup_additive_home();

    let oc = opencode_config::get_providers().expect("read OpenCode fixture");
    assert_eq!(
        oc["fixture-oc"]["options"]["baseURL"],
        json!("https://api.fixture.example/v1")
    );
    assert_eq!(oc["fixture-oc"]["futureVendorFlag"], json!(true));

    let claw = openclaw_config::get_providers().expect("read OpenClaw fixture");
    assert_eq!(claw["fixture-claw"]["api"], json!("openai-completions"));

    let hermes = hermes_config::get_providers().expect("read Hermes fixture");
    assert!(hermes.contains_key("fixture-hermes"));

    opencode_config::set_provider(
        "raw-oc",
        json!({
            "npm": "@ai-sdk/anthropic",
            "options": { "baseURL": "https://raw.example/v1" }
        }),
    )
    .expect("raw OpenCode set_provider");
    openclaw_config::set_provider(
        "raw-claw",
        json!({
            "baseUrl": "https://raw.example/v1",
            "api": "openai-completions",
            "models": [{ "id": "raw" }]
        }),
    )
    .expect("raw OpenClaw set_provider");
    hermes_config::set_provider(
        "raw-hermes",
        json!({
            "base_url": "https://raw.example/v1",
            "api_mode": "chat_completions",
            "models": [{ "id": "raw" }]
        }),
    )
    .expect("raw Hermes set_provider");

    assert!(get_opencode_live_provider_ids()
        .expect("ids")
        .contains(&"raw-oc".to_string()));
    assert!(get_openclaw_live_provider_ids()
        .expect("ids")
        .contains(&"raw-claw".to_string()));
    assert!(get_hermes_live_provider_ids()
        .expect("ids")
        .contains(&"raw-hermes".to_string()));
}
