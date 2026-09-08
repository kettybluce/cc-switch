//! Project Pi `models.json` through the CC Switch local proxy.
//!
//! Pi follows the same product path as Claude/Codex: **takeover + local
//! proxy**. The database remains the source of truth for upstream `baseUrl`s.
//! When Pi takeover is on (and the listener is running), this module rewrites
//! only the live file Pi reads, and only for providers CC Switch already
//! manages. Disable takeover or stop the proxy to restore upstream URLs.
//!
//! This is the takeover implementation for an additive app — not a
//! whole-file live-config backup, and not an automatic rewrite whenever the
//! proxy happens to be running (the previous Option B behaviour).

use crate::database::Database;
use crate::error::AppError;
use crate::pi_runtime::rewrite::{
    config_uses_proxy_base_url, is_pi_proxy_base_url, live_provider_node,
    restore_provider_base_urls, strip_proxy_base_urls,
};
use crate::pi_runtime::{self, PiRuntimeKind, PiRuntimeTarget};
use crate::provider::Provider;
use crate::store::AppState;
use serde_json::Value;

const PI_APP: &str = "pi";

#[cfg(test)]
use std::sync::Mutex;

#[cfg(test)]
static TEST_PROJECTION_ENABLED: Mutex<Option<bool>> = Mutex::new(None);

/// Whether Pi should rewrite live `models.json` onto the local proxy.
///
/// Requires both:
/// - Pi takeover enabled (`proxy_config.enabled` for `"pi"`), same control
///   surface as Claude/Codex
/// - `flags.wsl_proxy` not opted out (advanced; defaults on)
pub fn proxy_projection_enabled(db: &Database) -> bool {
    #[cfg(test)]
    {
        if let Some(forced) = *TEST_PROJECTION_ENABLED
            .lock()
            .expect("lock Pi projection test override")
        {
            return forced;
        }
    }
    wsl_proxy_allowed() && pi_takeover_enabled(db)
}

/// Advanced opt-out stored in Pi runtime flags. Defaults on.
pub fn wsl_proxy_allowed() -> bool {
    crate::pi_runtime::settings().flags.wsl_proxy
}

fn pi_takeover_enabled(db: &Database) -> bool {
    futures::executor::block_on(db.get_proxy_config_for_app(PI_APP))
        .map(|config| config.enabled)
        .unwrap_or(false)
}

/// Restore the previous runtime's live `models.json` when the user switches
/// location, so a Windows install is not left pointing at a WSL gateway
/// (and vice versa).
pub fn runtime_switch_requires_restore(
    previous: &PiRuntimeTarget,
    next_kind: PiRuntimeKind,
    next_distro: Option<&str>,
) -> bool {
    match previous {
        PiRuntimeTarget::Local => next_kind != PiRuntimeKind::Local,
        PiRuntimeTarget::Wsl { distro, .. } => {
            next_kind != PiRuntimeKind::Wsl || next_distro != Some(distro.as_str())
        }
    }
}

/// Whether the active runtime's live `models.json` currently has projected
/// `/pi/<id>` baseUrls. Used by the runtime panel so "projected" matches
/// the file Pi actually reads.
pub fn live_models_are_projected() -> bool {
    crate::pi_config::read_pi_native_providers()
        .ok()
        .is_some_and(|native| native.values().any(config_uses_proxy_base_url))
}

/// Origin (`http://host:port`) Pi should use to reach the local proxy.
///
/// On the local runtime this is loopback. Inside WSL it is whichever address
/// actually answered `/health` (mirrored loopback, NAT gateway, or resolver).
pub fn resolve_origin(
    target: &PiRuntimeTarget,
    local_proxy_port: u16,
) -> Result<Option<String>, AppError> {
    if local_proxy_port == 0 {
        return Ok(None);
    }
    match target {
        PiRuntimeTarget::Local => Ok(Some(format!("http://127.0.0.1:{local_proxy_port}"))),
        PiRuntimeTarget::Wsl { distro, .. } => {
            let health = crate::pi_runtime::proxy::resolve_gateway(distro, local_proxy_port)
                .map_err(|error| AppError::Message(error.to_string()))?;
            if health.reachable {
                if let Some(endpoint) = health.endpoint.filter(|url| !url.is_empty()) {
                    return Ok(Some(endpoint.trim_end_matches('/').to_string()));
                }
            }
            // Do not write 127.0.0.1 into WSL models.json: under NAT that
            // address is the distribution itself, so Pi would miss the proxy.
            // Test Proxy reports the miss; the live file stays on upstream.
            log::warn!(
                "[PiProxy] no reachable WSL route to port {local_proxy_port} from '{distro}'; leaving upstream URLs in place"
            );
            Ok(None)
        }
    }
}

/// Origin to write into live `models.json` right now, or `None` to restore
/// real upstream URLs. Gated on Pi takeover (plus the wsl_proxy opt-out).
pub async fn live_origin(state: &AppState) -> Result<Option<String>, AppError> {
    if !proxy_projection_enabled(state.db.as_ref()) {
        return Ok(None);
    }
    let port = state
        .proxy_service
        .get_status()
        .await
        .map(|status| status.port)
        .unwrap_or_default();
    let target = pi_runtime::target();
    resolve_origin(&target, port)
}

/// Project managed providers through a running proxy **without** checking
/// the takeover flag. Used while enabling takeover (the flag is not
/// committed yet) and when re-projecting after a listener restart.
pub fn project_through_running_proxy(
    db: &Database,
    local_proxy_port: u16,
) -> Result<Option<String>, AppError> {
    if !wsl_proxy_allowed() {
        restore_after_proxy_stop(db);
        return Ok(None);
    }
    let target = pi_runtime::target();
    let origin = resolve_origin(&target, local_proxy_port)?;
    sync_live_providers(db, origin.as_deref())?;
    Ok(origin)
}

/// True when live `models.json` already points at `/pi/…` on this origin.
pub fn live_models_match_origin_port(local_proxy_port: u16) -> bool {
    if local_proxy_port == 0 {
        return false;
    }
    let target = pi_runtime::target();
    let Ok(Some(origin)) = resolve_origin(&target, local_proxy_port) else {
        return false;
    };
    let origin = origin.trim_end_matches('/');
    crate::pi_config::read_pi_native_providers()
        .ok()
        .is_some_and(|native| {
            native.values().any(|live| {
                live.get("baseUrl")
                    .and_then(Value::as_str)
                    .is_some_and(|url| is_pi_proxy_base_url(url) && url.starts_with(origin))
            })
        })
}

/// Rewrite (or restore) every managed provider currently present in
/// `models.json`.
pub fn sync_live_providers(db: &Database, origin: Option<&str>) -> Result<usize, AppError> {
    let native = crate::pi_config::read_pi_native_providers()?;
    let saved = db.get_all_providers(PI_APP)?;
    let mut changed = 0usize;

    for (id, live) in native {
        let Some(provider) = saved.get(&id) else {
            continue;
        };
        if !rewrite_one(&id, &provider.settings_config, Some(&live), origin)? {
            continue;
        }
        changed += 1;
    }
    if changed > 0 {
        log::info!(
            "[PiProxy] {} Pi provider baseUrl(s) {}",
            changed,
            if origin.is_some() {
                "projected through the local proxy"
            } else {
                "restored to upstream URLs"
            }
        );
    }
    Ok(changed)
}

/// Project after a proxy start only when Pi takeover is already enabled.
/// Failures are logged, not fatal: Claude/Codex takeover must not be blocked.
pub fn sync_after_proxy_start(db: &Database, local_proxy_port: u16) {
    if !proxy_projection_enabled(db) {
        restore_after_proxy_stop(db);
        return;
    }
    if let Err(error) = project_through_running_proxy(db, local_proxy_port) {
        log::warn!("[PiProxy] failed to project Pi models.json after proxy start: {error}");
    }
}

/// Restore real upstream URLs when the local proxy stops so Pi can still
/// reach providers directly.
pub fn restore_after_proxy_stop(db: &Database) {
    if let Err(error) = sync_live_providers(db, None) {
        log::warn!("[PiProxy] failed to restore Pi models.json after proxy stop: {error}");
    }
}

/// Rewrite live `models.json` to match the current takeover + proxy state.
///
/// This never starts the proxy — enabling Pi takeover is what turns the
/// listener on, same as Claude/Codex.
pub async fn apply_for_state(state: &AppState) -> Result<Option<String>, AppError> {
    let origin = live_origin(state).await?;
    sync_live_providers(state.db.as_ref(), origin.as_deref())?;
    Ok(origin)
}

/// Config to write into `models.json` for one provider.
pub fn projected_live_config(
    state: &AppState,
    provider: &Provider,
    current_live: Option<&Value>,
) -> Result<Value, AppError> {
    let origin = futures::executor::block_on(live_origin(state))?;
    Ok(live_provider_node(
        &provider.settings_config,
        current_live,
        &provider.id,
        origin.as_deref(),
    ))
}

/// Drop a projected proxy URL from a native node before it is copied into
/// the database. The stored card must keep the real upstream.
pub fn sanitize_native_for_db(native: &mut Value, stored: Option<&Value>) {
    if let Some(stored) = stored {
        restore_provider_base_urls(native, stored);
        return;
    }
    strip_proxy_base_urls(native);
}

fn rewrite_one(
    provider_id: &str,
    stored: &Value,
    live: Option<&Value>,
    origin: Option<&str>,
) -> Result<bool, AppError> {
    let desired = live_provider_node(stored, live, provider_id, origin);
    match live {
        Some(current) if current == &desired => Ok(false),
        Some(_) => {
            crate::pi_config::replace_pi_provider_if_present(provider_id, &desired)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::pi_config::test_support::TestAgentDir;
    use crate::pi_runtime::rewrite::is_pi_proxy_base_url;
    use crate::provider::{Provider, ProviderMeta};
    use serde_json::json;
    use serial_test::serial;
    use std::sync::Arc;

    fn state() -> AppState {
        AppState::new(Arc::new(
            Database::memory().expect("create in-memory database"),
        ))
    }

    fn card(id: &str, api: &str, base_url: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: id.to_string(),
            settings_config: json!({
                "name": id,
                "baseUrl": base_url,
                "apiKey": "secret",
                "api": api,
                "models": [{ "id": "model-a" }]
            }),
            website_url: None,
            category: Some("custom".to_string()),
            created_at: Some(1),
            sort_index: None,
            notes: None,
            meta: Some(ProviderMeta::default()),
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    #[test]
    #[serial]
    fn projecting_rewrites_live_file_and_leaves_the_database_alone() {
        let _agent = TestAgentDir::new();
        let state = state();
        let provider = card(
            "cc-switch-test",
            "openai-completions",
            "https://api.example.com/v1",
        );
        crate::pi_config::insert_pi_provider(&provider.id, &provider.settings_config)
            .expect("insert");
        state.db.save_provider(PI_APP, &provider).expect("save");

        sync_live_providers(state.db.as_ref(), Some("http://172.30.208.1:15721")).expect("project");

        let live = crate::pi_config::read_pi_native_provider("cc-switch-test")
            .expect("read live")
            .expect("present");
        assert_eq!(
            live["baseUrl"],
            "http://172.30.208.1:15721/pi/cc-switch-test/v1"
        );
        assert!(is_pi_proxy_base_url(live["baseUrl"].as_str().unwrap()));
        assert_eq!(live["sdkOption"], Value::Null);
        assert_eq!(
            state
                .db
                .get_provider_by_id("cc-switch-test", PI_APP)
                .expect("db")
                .expect("card")
                .settings_config["baseUrl"],
            "https://api.example.com/v1"
        );

        sync_live_providers(state.db.as_ref(), None).expect("restore");
        let restored = crate::pi_config::read_pi_native_provider("cc-switch-test")
            .expect("read live")
            .expect("present");
        assert_eq!(restored["baseUrl"], "https://api.example.com/v1");
    }

    #[test]
    #[serial]
    fn projecting_preserves_unknown_native_fields() {
        let _agent = TestAgentDir::new();
        let state = state();
        let mut provider = card(
            "cc-switch-test",
            "anthropic-messages",
            "https://api.anthropic.com",
        );
        provider.settings_config["futureField"] = json!({ "keep": true });
        crate::pi_config::insert_pi_provider(&provider.id, &provider.settings_config)
            .expect("insert");
        state.db.save_provider(PI_APP, &provider).expect("save");

        // An extra native-only field that is not in the DB card.
        let mut live = provider.settings_config.clone();
        live["sdkOption"] = json!({ "timeout": 30 });
        crate::pi_config::replace_pi_provider_if_present(&provider.id, &live).expect("annotate");

        sync_live_providers(state.db.as_ref(), Some("http://127.0.0.1:15721")).expect("project");
        let projected = crate::pi_config::read_pi_native_provider("cc-switch-test")
            .expect("read")
            .expect("present");
        assert_eq!(
            projected["baseUrl"],
            "http://127.0.0.1:15721/pi/cc-switch-test"
        );
        assert_eq!(projected["sdkOption"]["timeout"], 30);
        assert_eq!(projected["futureField"]["keep"], true);
    }

    #[test]
    fn local_origin_uses_loopback() {
        let origin = resolve_origin(&PiRuntimeTarget::Local, 15721).expect("origin");
        assert_eq!(origin.as_deref(), Some("http://127.0.0.1:15721"));
        assert_eq!(
            resolve_origin(&PiRuntimeTarget::Local, 0).expect("disabled"),
            None
        );
    }

    #[test]
    fn sanitize_drops_proxy_urls_when_there_is_no_stored_card() {
        let mut native = json!({
            "baseUrl": "http://127.0.0.1:15721/pi/imported/v1",
            "api": "openai-completions"
        });
        sanitize_native_for_db(&mut native, None);
        assert!(native.get("baseUrl").is_none());
    }

    #[test]
    fn sanitize_drops_injected_model_proxy_urls_without_a_stored_card() {
        let mut native = json!({
            "api": "anthropic-messages",
            "models": [{
                "id": "gpt",
                "api": "openai-completions",
                "baseUrl": "http://127.0.0.1:15721/pi/imported/v1"
            }]
        });
        sanitize_native_for_db(&mut native, None);
        assert!(native["models"][0].get("baseUrl").is_none());
    }

    struct ProjectionOverride {
        previous: Option<bool>,
    }

    impl ProjectionOverride {
        fn set(enabled: bool) -> Self {
            let previous = TEST_PROJECTION_ENABLED
                .lock()
                .expect("lock Pi projection test override")
                .replace(enabled);
            Self { previous }
        }
    }

    impl Drop for ProjectionOverride {
        fn drop(&mut self) {
            *TEST_PROJECTION_ENABLED
                .lock()
                .expect("lock Pi projection test override") = self.previous;
        }
    }

    async fn use_ephemeral_listen_port(state: &AppState) {
        let mut config = state
            .db
            .get_proxy_config()
            .await
            .expect("get test proxy config");
        config.listen_port = 0;
        state
            .db
            .update_proxy_config(config)
            .await
            .expect("use an ephemeral listen port");
    }

    fn seed_managed_openai_provider(state: &AppState) -> Provider {
        let provider = card(
            "cc-switch-test",
            "openai-completions",
            "https://api.example.com/v1",
        );
        crate::pi_config::insert_pi_provider(&provider.id, &provider.settings_config)
            .expect("insert");
        state.db.save_provider(PI_APP, &provider).expect("save");
        provider
    }

    fn live_base_url() -> String {
        crate::pi_config::read_pi_native_provider("cc-switch-test")
            .expect("read live")
            .expect("present")["baseUrl"]
            .as_str()
            .expect("baseUrl string")
            .to_string()
    }

    #[test]
    fn projection_follows_the_local_proxy_by_default() {
        assert!(
            crate::pi_runtime::PiFeatureFlags::default().wsl_proxy,
            "absent settings must project when the local proxy is on"
        );
        let settings = serde_json::from_str::<crate::pi_runtime::PiRuntimeSettings>("{}")
            .expect("deserialize empty Pi runtime settings");
        assert!(settings.flags.wsl_proxy);
    }

    #[test]
    fn switching_runtime_restores_the_previous_models_file() {
        assert!(!runtime_switch_requires_restore(
            &PiRuntimeTarget::Local,
            PiRuntimeKind::Local,
            None
        ));
        assert!(runtime_switch_requires_restore(
            &PiRuntimeTarget::Local,
            PiRuntimeKind::Wsl,
            Some("Ubuntu")
        ));
        let ubuntu = PiRuntimeTarget::Wsl {
            distro: "Ubuntu".into(),
            home: "/home/u".into(),
            agent_dir: "/home/u/.pi/agent".into(),
        };
        assert!(runtime_switch_requires_restore(
            &ubuntu,
            PiRuntimeKind::Local,
            None
        ));
        assert!(runtime_switch_requires_restore(
            &ubuntu,
            PiRuntimeKind::Wsl,
            Some("Debian")
        ));
        assert!(!runtime_switch_requires_restore(
            &ubuntu,
            PiRuntimeKind::Wsl,
            Some("Ubuntu")
        ));
    }

    #[tokio::test]
    #[serial]
    async fn starting_the_local_proxy_does_not_project_pi_without_takeover() {
        let _agent = TestAgentDir::new();
        let state = state();
        seed_managed_openai_provider(&state);
        use_ephemeral_listen_port(&state).await;

        assert_eq!(live_base_url(), "https://api.example.com/v1");
        state
            .proxy_service
            .start()
            .await
            .expect("start local proxy");
        assert_eq!(live_base_url(), "https://api.example.com/v1");
        state.proxy_service.stop().await.expect("stop local proxy");
    }

    #[tokio::test]
    #[serial]
    async fn enabling_pi_takeover_starts_the_proxy_and_projects_models() {
        let _agent = TestAgentDir::new();
        let state = state();
        seed_managed_openai_provider(&state);
        use_ephemeral_listen_port(&state).await;

        assert_eq!(live_base_url(), "https://api.example.com/v1");
        state
            .proxy_service
            .set_takeover_for_app("pi", true)
            .await
            .expect("enable Pi takeover");
        assert!(state.proxy_service.is_running().await);
        let status = state
            .proxy_service
            .get_status()
            .await
            .expect("proxy status");
        assert!(
            is_pi_proxy_base_url(&live_base_url()),
            "live baseUrl should be projected through /pi/<id>"
        );
        assert!(
            live_base_url().starts_with(&format!("http://127.0.0.1:{}", status.port)),
            "projected origin should match the running listener"
        );

        let takeover = state
            .proxy_service
            .get_takeover_status()
            .await
            .expect("takeover status");
        assert!(takeover.pi);

        state
            .proxy_service
            .set_takeover_for_app("pi", false)
            .await
            .expect("disable Pi takeover");
        assert_eq!(live_base_url(), "https://api.example.com/v1");
    }

    #[tokio::test]
    #[serial]
    async fn apply_for_state_follows_proxy_without_starting_it() {
        let _agent = TestAgentDir::new();
        let _projection = ProjectionOverride::set(true);
        let state = state();
        seed_managed_openai_provider(&state);

        let origin = apply_for_state(&state)
            .await
            .expect("apply while proxy is stopped");
        assert!(origin.is_none());
        assert!(!state.proxy_service.is_running().await);
        assert_eq!(live_base_url(), "https://api.example.com/v1");
    }

    #[tokio::test]
    #[serial]
    async fn explicit_opt_out_restores_upstream_when_the_proxy_starts() {
        let _agent = TestAgentDir::new();
        let state = state();
        seed_managed_openai_provider(&state);
        use_ephemeral_listen_port(&state).await;

        sync_live_providers(state.db.as_ref(), Some("http://127.0.0.1:15721")).expect("project");
        assert!(is_pi_proxy_base_url(&live_base_url()));

        let _opt_out = ProjectionOverride::set(false);
        state
            .proxy_service
            .start()
            .await
            .expect("start local proxy");
        assert_eq!(live_base_url(), "https://api.example.com/v1");
        state.proxy_service.stop().await.expect("stop local proxy");
    }
}
