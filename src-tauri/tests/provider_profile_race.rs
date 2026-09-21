//! SCHEMA 18 并发压测：profile / provider 切换竞态 + overlapping CRUD / 接管。
//!
//! 全部走 `CC_SWITCH_TEST_HOME` 夹具（support.rs），不写宿主 `C:`，不升 schema。
//! 断言：current / live / 备份 / 接管标志在并发结束后一致，且 `proxy_config`
//! 永不出现 `pi` 行。

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use serde_json::{json, Value};

use cc_switch_lib::{
    get_claude_settings_path, get_codex_auth_path, get_codex_config_path, read_json_file, AppType,
    ProfilePayload, ProfileScope, ProfileService, Provider, ProviderService, SCHEMA_VERSION,
};

#[path = "support.rs"]
mod support;
use support::{create_test_state, ensure_test_home, reset_test_fs, test_mutex};

const SWITCH_ITERS: usize = 24;
const CRUD_ITERS: usize = 16;
const TAKEOVER_ITERS: usize = 8;
const APPLY_ITERS: usize = 12;

fn assert_schema18_pi_has_no_proxy_config_row(state: &cc_switch_lib::AppState) {
    assert_eq!(SCHEMA_VERSION, 18, "fixtures must stay on SCHEMA 18");
    assert!(
        !state
            .db
            .has_proxy_config_row("pi")
            .expect("pi proxy_config row"),
        "SCHEMA 18 CHECK must not gain a proxy_config row for pi"
    );
}

async fn use_ephemeral_shared_listen(state: &cc_switch_lib::AppState) {
    let mut proxy_config = state.db.get_proxy_config().await.expect("get proxy config");
    proxy_config.listen_port = 0;
    proxy_config.listen_address = "0.0.0.0".to_string();
    state
        .db
        .update_proxy_config(proxy_config)
        .await
        .expect("use ephemeral 0.0.0.0 listen");
}

fn claude_provider(id: &str, url: &str, token: &str) -> Provider {
    let mut provider = Provider::with_id(
        id.to_string(),
        id.to_uppercase(),
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": token,
                "ANTHROPIC_BASE_URL": url
            }
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
}

fn codex_provider(id: &str, url: &str, key: &str) -> Provider {
    let table = id.replace('-', "_");
    let config = format!(
        r#"model_provider = "{table}"
model = "gpt-5"

[model_providers.{table}]
name = "{id}"
base_url = "{url}"
wire_api = "responses"
"#
    );
    let mut provider = Provider::with_id(
        id.to_string(),
        format!("Codex {id}"),
        json!({
            "auth": {"OPENAI_API_KEY": key},
            "config": config
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
}

fn pi_openai_provider(id: &str, url: &str, key: &str) -> Provider {
    let mut provider = Provider::with_id(
        id.to_string(),
        id.to_uppercase(),
        json!({
            "name": id,
            "baseUrl": url,
            "apiKey": key,
            "api": "openai-completions",
            "models": [{ "id": "glm-5.1" }]
        }),
        None,
    );
    provider.category = Some("custom".to_string());
    provider
}

fn seed_pi_live_home() {
    let home = ensure_test_home();
    let agent = home.join(".pi").join("agent");
    fs::create_dir_all(&agent).expect("create Pi agent dir");
    fs::write(agent.join("models.json"), r#"{"providers":{}}"#).expect("seed models.json");
    fs::write(
        agent.join("settings.json"),
        r#"{"theme":"keep-me","sessionDir":"/tmp/pi-sessions"}"#,
    )
    .expect("seed Pi settings.json");
    fs::write(
        home.join(".pi").join("auth.json"),
        r#"{"anthropic":{"type":"oauth"}}"#,
    )
    .expect("seed Pi auth.json");
}

fn live_points_at_local_proxy(live: &Value) -> bool {
    let url = live
        .get("env")
        .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let token = live
        .get("env")
        .and_then(|env| env.get("ANTHROPIC_AUTH_TOKEN"))
        .and_then(Value::as_str)
        .or_else(|| {
            live.get("env")
                .and_then(|env| env.get("ANTHROPIC_API_KEY"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default();
    url.contains("127.0.0.1") || url.contains("localhost") || token == "PROXY_MANAGED"
}

fn read_claude_live() -> Value {
    let path = get_claude_settings_path();
    if !path.exists() {
        return json!({});
    }
    read_json_file(&path).unwrap_or_else(|_| json!({}))
}

fn claude_live_url(live: &Value) -> Option<&str> {
    live.get("env")
        .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
        .and_then(Value::as_str)
}

fn provider_url(provider: &Provider) -> Option<&str> {
    provider
        .settings_config
        .get("env")
        .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
        .and_then(Value::as_str)
}

fn assert_exactly_one_current(state: &cc_switch_lib::AppState, app: AppType) {
    let current = state
        .db
        .get_current_provider(app.as_str())
        .expect("read current")
        .expect("must have a current provider");
    let providers = state
        .db
        .get_all_providers(app.as_str())
        .expect("list providers");
    assert!(
        providers.contains_key(&current),
        "current provider {current} must still exist"
    );
}

async fn assert_claude_takeover_consistent(state: &cc_switch_lib::AppState) {
    assert_schema18_pi_has_no_proxy_config_row(state);
    assert_exactly_one_current(state, AppType::Claude);

    let current_id = state
        .db
        .get_current_provider(AppType::Claude.as_str())
        .expect("current")
        .expect("current id");
    let providers = state
        .db
        .get_all_providers(AppType::Claude.as_str())
        .expect("providers");
    let current = providers.get(&current_id).expect("current row");
    let takeover = state
        .db
        .is_app_takeover_enabled("claude")
        .await
        .expect("takeover flag");
    let backup = state.db.get_live_backup("claude").await.expect("backup");
    let live = read_claude_live();

    if takeover {
        assert!(
            backup.is_some(),
            "enabled takeover must keep a restore backup"
        );
        assert!(
            live_points_at_local_proxy(&live),
            "enabled takeover must project a local proxy live, got {live}"
        );
    } else {
        assert!(
            backup.is_none(),
            "disabled takeover must drop the restore backup"
        );
        assert_eq!(
            claude_live_url(&live),
            provider_url(current),
            "live URL must match the current provider when takeover is off"
        );
        assert!(
            !live_points_at_local_proxy(&live),
            "disabled takeover must not leave a proxy placeholder in live"
        );
    }
}

async fn assert_profile_matches_current_claude(state: &cc_switch_lib::AppState) {
    let current_provider = state
        .db
        .get_current_provider(AppType::Claude.as_str())
        .expect("current provider");
    let current_profile = state
        .db
        .get_current_profile_id(ProfileScope::Claude.as_str())
        .expect("current profile");
    let Some(profile_id) = current_profile else {
        return;
    };
    let profile = state
        .db
        .get_profile(&profile_id)
        .expect("load profile")
        .expect("profile exists");
    let payload: ProfilePayload =
        serde_json::from_str(&profile.payload).expect("parse profile payload");
    assert_eq!(
        payload.providers.claude.as_deref(),
        current_provider.as_deref(),
        "current profile snapshot must match the live current provider"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME across concurrent switch/takeover awaits"
)]
async fn concurrent_claude_switches_keep_current_and_live_aligned() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();
    let state = create_test_state().expect("create test state");
    assert_schema18_pi_has_no_proxy_config_row(&state);

    ProviderService::add(
        &state,
        AppType::Claude,
        claude_provider("alpha", "https://alpha.example", "token-alpha"),
        true,
    )
    .expect("add alpha");
    ProviderService::add(
        &state,
        AppType::Claude,
        claude_provider("beta", "https://beta.example", "token-beta"),
        true,
    )
    .expect("add beta");
    ProviderService::switch(&state, AppType::Claude, "alpha").expect("seed live");

    let barrier = Arc::new(Barrier::new(6));
    let ok = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for i in 0..6 {
        let state = state.clone();
        let barrier = Arc::clone(&barrier);
        let ok = Arc::clone(&ok);
        let target = if i % 2 == 0 { "alpha" } else { "beta" };
        handles.push(thread::spawn(move || {
            barrier.wait();
            for _ in 0..SWITCH_ITERS {
                if ProviderService::switch(&state, AppType::Claude, target).is_ok() {
                    ok.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    for handle in handles {
        handle.join().expect("switch thread");
    }
    assert!(
        ok.load(Ordering::Relaxed) > 0,
        "at least one switch must succeed"
    );
    assert_claude_takeover_consistent(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME across concurrent profile applies"
)]
async fn concurrent_profile_applies_keep_current_profile_aligned_with_provider() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();
    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;

    ProviderService::add(
        &state,
        AppType::Claude,
        claude_provider("alpha", "https://alpha.example", "token-alpha"),
        true,
    )
    .expect("add alpha");
    ProviderService::add(
        &state,
        AppType::Claude,
        claude_provider("beta", "https://beta.example", "token-beta"),
        true,
    )
    .expect("add beta");
    ProviderService::switch(&state, AppType::Claude, "alpha").expect("current alpha");
    let profile_alpha = ProfileService::create(&state, "alpha-project", ProfileScope::Claude)
        .expect("profile alpha");
    ProviderService::switch(&state, AppType::Claude, "beta").expect("current beta");
    let profile_beta =
        ProfileService::create(&state, "beta-project", ProfileScope::Claude).expect("profile beta");

    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for i in 0..4 {
        let state = state.clone();
        let barrier = Arc::clone(&barrier);
        let profile_id = if i % 2 == 0 {
            profile_alpha.id.clone()
        } else {
            profile_beta.id.clone()
        };
        handles.push(thread::spawn(move || {
            barrier.wait();
            for _ in 0..APPLY_ITERS {
                let _ = ProfileService::apply(&state, &profile_id, ProfileScope::Claude);
            }
        }));
    }
    for handle in handles {
        handle.join().expect("apply thread");
    }

    assert_claude_takeover_consistent(&state).await;
    assert_profile_matches_current_claude(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME across overlapping CRUD and takeover"
)]
async fn overlapping_provider_crud_and_takeover_enable_disable_stay_consistent() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();
    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;
    assert_schema18_pi_has_no_proxy_config_row(&state);

    ProviderService::add(
        &state,
        AppType::Claude,
        claude_provider("alpha", "https://alpha.example", "token-alpha"),
        true,
    )
    .expect("add alpha");
    ProviderService::add(
        &state,
        AppType::Claude,
        claude_provider("beta", "https://beta.example", "token-beta"),
        true,
    )
    .expect("add beta");
    ProviderService::add(
        &state,
        AppType::Claude,
        claude_provider("spare", "https://spare.example", "token-spare"),
        true,
    )
    .expect("add spare");
    ProviderService::switch(&state, AppType::Claude, "alpha").expect("current alpha");

    let barrier = Arc::new(Barrier::new(7));

    let switch_state = state.clone();
    let switch_barrier = Arc::clone(&barrier);
    let switch_thread = thread::spawn(move || {
        switch_barrier.wait();
        for i in 0..SWITCH_ITERS {
            let id = if i % 2 == 0 { "alpha" } else { "beta" };
            let _ = ProviderService::switch(&switch_state, AppType::Claude, id);
        }
    });

    let save_state = state.clone();
    let save_barrier = Arc::clone(&barrier);
    let save_thread = thread::spawn(move || {
        save_barrier.wait();
        for i in 0..CRUD_ITERS {
            let mut provider = claude_provider(
                "alpha",
                "https://alpha.example",
                &format!("token-alpha-{i}"),
            );
            provider.name = "ALPHA".to_string();
            let _ = ProviderService::update(&save_state, AppType::Claude, None, provider);

            let mut beta =
                claude_provider("beta", "https://beta.example", &format!("token-beta-{i}"));
            beta.name = "BETA".to_string();
            let _ = ProviderService::update(&save_state, AppType::Claude, None, beta);
        }
    });

    let delete_state = state.clone();
    let delete_barrier = Arc::clone(&barrier);
    let delete_thread = thread::spawn(move || {
        delete_barrier.wait();
        for i in 0..CRUD_ITERS {
            let id = format!("ephemeral-{i}");
            let _ = ProviderService::add(
                &delete_state,
                AppType::Claude,
                claude_provider(&id, "https://ephemeral.example", "token-ephemeral"),
                true,
            );
            let _ = ProviderService::delete(&delete_state, AppType::Claude, &id);
            let _ = ProviderService::delete(&delete_state, AppType::Claude, "spare");
        }
    });

    let profile_state = state.clone();
    let profile_barrier = Arc::clone(&barrier);
    let profile_alpha =
        ProfileService::create(&state, "race-alpha", ProfileScope::Claude).expect("profile");
    ProviderService::switch(&state, AppType::Claude, "beta").expect("snap beta");
    let profile_beta =
        ProfileService::create(&state, "race-beta", ProfileScope::Claude).expect("profile beta");
    let apply_thread = thread::spawn(move || {
        profile_barrier.wait();
        for i in 0..APPLY_ITERS {
            let id = if i % 2 == 0 {
                &profile_alpha.id
            } else {
                &profile_beta.id
            };
            let _ = ProfileService::apply(&profile_state, id, ProfileScope::Claude);
        }
    });

    let takeover_state = state.clone();
    let takeover_barrier = Arc::clone(&barrier);
    let takeover_task = tokio::spawn(async move {
        takeover_barrier.wait();
        for i in 0..TAKEOVER_ITERS {
            let enabled = i % 2 == 0;
            let _ = takeover_state
                .proxy_service
                .set_takeover_for_app("claude", enabled)
                .await;
        }
        let _ = takeover_state
            .proxy_service
            .set_takeover_for_app("claude", false)
            .await;
    });

    let cross_state = state.clone();
    let cross_barrier = Arc::clone(&barrier);
    let cross_thread = thread::spawn(move || {
        let _ = ProviderService::add(
            &cross_state,
            AppType::Codex,
            codex_provider("cx-a", "https://codex-a.example/v1", "codex-a"),
            true,
        );
        let _ = ProviderService::add(
            &cross_state,
            AppType::Codex,
            codex_provider("cx-b", "https://codex-b.example/v1", "codex-b"),
            true,
        );
        cross_barrier.wait();
        for i in 0..SWITCH_ITERS {
            let id = if i % 2 == 0 { "cx-a" } else { "cx-b" };
            let _ = ProviderService::switch(&cross_state, AppType::Codex, id);
        }
    });

    let pi_state = state.clone();
    let pi_barrier = Arc::clone(&barrier);
    seed_pi_live_home();
    let pi_thread = thread::spawn(move || {
        pi_barrier.wait();
        for i in 0..CRUD_ITERS {
            let id = format!("pi-ephemeral-{i}");
            let _ = ProviderService::add(
                &pi_state,
                AppType::Pi,
                pi_openai_provider(&id, "https://pi.example/v1", "pi-key"),
                true,
            );
            let mut updated = pi_openai_provider(&id, "https://pi-updated.example/v1", "pi-key-2");
            updated.name = format!("PI-{i}");
            let _ = ProviderService::update(&pi_state, AppType::Pi, None, updated);
            let _ = ProviderService::delete(&pi_state, AppType::Pi, &id);
        }
    });

    switch_thread.join().expect("switch thread");
    save_thread.join().expect("save thread");
    delete_thread.join().expect("delete thread");
    apply_thread.join().expect("apply thread");
    cross_thread.join().expect("codex switch thread");
    pi_thread.join().expect("pi crud thread");
    takeover_task.await.expect("takeover task");

    assert_claude_takeover_consistent(&state).await;
    assert_schema18_pi_has_no_proxy_config_row(&state);
    assert!(
        !get_claude_settings_path()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("c:\\"),
        "must not write a Windows C: path"
    );
    let _ = get_codex_config_path();
    let _ = get_codex_auth_path();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::await_holding_lock,
    reason = "serialize global test HOME across profile apply vs takeover"
)]
async fn profile_apply_overlapping_takeover_does_not_split_current_and_live() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let _home = ensure_test_home();
    let state = create_test_state().expect("create test state");
    use_ephemeral_shared_listen(&state).await;

    ProviderService::add(
        &state,
        AppType::Claude,
        claude_provider("alpha", "https://alpha.example", "token-alpha"),
        true,
    )
    .expect("add alpha");
    ProviderService::add(
        &state,
        AppType::Claude,
        claude_provider("beta", "https://beta.example", "token-beta"),
        true,
    )
    .expect("add beta");
    ProviderService::switch(&state, AppType::Claude, "alpha").expect("current alpha");
    let profile_alpha =
        ProfileService::create(&state, "alpha-project", ProfileScope::Claude).expect("profile");
    ProviderService::switch(&state, AppType::Claude, "beta").expect("current beta");
    let profile_beta =
        ProfileService::create(&state, "beta-project", ProfileScope::Claude).expect("profile");

    state
        .proxy_service
        .set_takeover_for_app("claude", true)
        .await
        .expect("enable takeover");

    let barrier = Arc::new(Barrier::new(3));
    let apply_state = state.clone();
    let apply_barrier = Arc::clone(&barrier);
    let alpha_id = profile_alpha.id.clone();
    let beta_id = profile_beta.id.clone();
    let apply_thread = thread::spawn(move || {
        apply_barrier.wait();
        for i in 0..APPLY_ITERS {
            let id = if i % 2 == 0 { &alpha_id } else { &beta_id };
            let _ = ProfileService::apply(&apply_state, id, ProfileScope::Claude);
        }
    });

    let switch_state = state.clone();
    let switch_barrier = Arc::clone(&barrier);
    let switch_thread = thread::spawn(move || {
        switch_barrier.wait();
        for i in 0..SWITCH_ITERS {
            let id = if i % 2 == 0 { "alpha" } else { "beta" };
            let _ = ProviderService::switch(&switch_state, AppType::Claude, id);
        }
    });

    let takeover_state = state.clone();
    let takeover_barrier = Arc::clone(&barrier);
    let takeover_task = tokio::spawn(async move {
        takeover_barrier.wait();
        for enabled in [true, false, true, false, true, false] {
            let _ = takeover_state
                .proxy_service
                .set_takeover_for_app("claude", enabled)
                .await;
        }
        let _ = takeover_state
            .proxy_service
            .set_takeover_for_app("claude", false)
            .await;
    });

    apply_thread.join().expect("apply thread");
    switch_thread.join().expect("switch thread");
    takeover_task.await.expect("takeover task");

    assert_claude_takeover_consistent(&state).await;
}

#[test]
fn schema18_constant_is_locked() {
    assert_eq!(SCHEMA_VERSION, 18);
    let _home: &Path = ensure_test_home();
    assert!(
        std::env::var("CC_SWITCH_TEST_HOME").is_ok(),
        "race tests must isolate via CC_SWITCH_TEST_HOME"
    );
}
