//! Project Pi `models.json` onto the existing CC Switch local proxy listen.
//!
//! SCHEMA 18 cannot store `proxy_config.app_type = 'pi'`, so Pi reuses Claude's
//! listen address/port (`Database::get_proxy_config`) and the same host rewrite
//! (`0.0.0.0` → `127.0.0.1`). There is no separate Pi gateway or port.
//!
//! Writes are atomic, unknown JSON fields are preserved, `auth.json` is never
//! touched, and `defaultProvider` / `defaultModel` are never written.

use super::{
    get_pi_models_path, lock_models_file, read_models_document_with_revision, write_models_document,
};
use crate::error::AppError;
use serde_json::{Map, Value};
use std::path::Path;

pub(crate) const PI_PROXY_API_KEY_PLACEHOLDER: &str = "PROXY_MANAGED";
/// Injected into projected `models.json` headers so shared-listen logs can be
/// filtered as Pi without a `/pi/` gateway or SCHEMA bump.
pub(crate) const PI_CLIENT_APP_HEADER: &str = "x-cc-switch-app";
pub(crate) const PI_CLIENT_APP_VALUE: &str = "pi";

/// Same Claude-style connect-host rewrite used by `ProxyService::build_proxy_urls`.
pub(crate) fn rewrite_listen_host_for_clients(listen_address: &str) -> String {
    match listen_address {
        "0.0.0.0" => "127.0.0.1".to_string(),
        "::" => "::1".to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn proxy_origin_from_listen(listen_address: &str, listen_port: u16) -> String {
    let host = rewrite_listen_host_for_clients(listen_address);
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host
    };
    format!("http://{host}:{listen_port}")
}

/// Map a Pi provider API onto the existing proxy routes (no `/pi/` prefix).
///
/// - Anthropic / Gemini / Bedrock → Claude/Gemini origin (`http://host:port`)
/// - OpenAI completions / responses → Codex `/v1` (`http://host:port/v1`)
pub(crate) fn proxy_base_url_for_api(api: Option<&str>, proxy_origin: &str) -> String {
    let origin = proxy_origin.trim_end_matches('/');
    match api {
        Some("openai-completions") | Some("openai-responses") => format!("{origin}/v1"),
        _ => origin.to_string(),
    }
}

pub(crate) fn is_local_proxy_url(url: &str) -> bool {
    let url = url.trim();
    if !url.starts_with("http://") {
        return false;
    }
    let rest = &url["http://".len()..];
    rest.starts_with("127.0.0.1")
        || rest.starts_with("localhost")
        || rest.starts_with("0.0.0.0")
        || rest.starts_with("[::1]")
        || rest.starts_with("[::]")
        || rest.starts_with("::1")
        || rest.starts_with("::")
}

pub(crate) fn is_projected_provider_node(node: &Value) -> bool {
    if node
        .get("apiKey")
        .and_then(Value::as_str)
        .is_some_and(|key| key == PI_PROXY_API_KEY_PLACEHOLDER)
    {
        return true;
    }
    if node
        .get("baseUrl")
        .and_then(Value::as_str)
        .is_some_and(is_local_proxy_url)
    {
        return true;
    }
    node.get("models")
        .and_then(Value::as_array)
        .is_some_and(|models| {
            models.iter().any(|model| {
                model
                    .get("baseUrl")
                    .and_then(Value::as_str)
                    .is_some_and(is_local_proxy_url)
            })
        })
}

pub(crate) fn document_has_proxy_projection(document: &Value) -> bool {
    match document.get("providers").and_then(Value::as_object) {
        Some(providers) => providers.values().any(is_projected_provider_node),
        None => false,
    }
}

/// Pi defaults `supportsDeveloperRole` to true for openai-completions reasoning
/// models, which emits the system/custom prompt as `role=developer`. Strict
/// OpenAI-compatible gateways (Zhipu GLM 1214「角色信息不正确」, intranet yum /
/// baisheng, vLLM, Ollama) only accept `system` / `user` / `assistant` / `tool`.
///
/// OpenCode Go presets already pin this per model; custom/yum cards edited in
/// CC Switch often omit it. Write the flag on both the provider and every
/// model so Pi inherits it regardless of merge-vs-replace compat semantics.
///
/// SCHEMA 18: write the existing `compat` field only; never invent a new column.
/// Live file is `~/.pi/agent/models.json` (not `~/.pi/models.json`).
/// An explicit `true` is left untouched so official OpenAI cards can opt in.
pub(crate) fn ensure_openai_completions_system_role(node: &mut Value) -> bool {
    let Some(object) = node.as_object_mut() else {
        return false;
    };
    if object.get("api").and_then(Value::as_str) != Some("openai-completions") {
        return false;
    }

    let mut changed = false;
    changed |= insert_supports_developer_role_false(object);

    if let Some(Value::Array(models)) = object.get_mut("models") {
        for model in models {
            let Some(model_object) = model.as_object_mut() else {
                continue;
            };
            changed |= insert_supports_developer_role_false(model_object);
        }
    }
    changed
}

fn insert_supports_developer_role_false(object: &mut Map<String, Value>) -> bool {
    let compat = object
        .entry("compat")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(compat) = compat.as_object_mut() else {
        return false;
    };
    if compat.contains_key("supportsDeveloperRole") {
        return false;
    }
    compat.insert("supportsDeveloperRole".to_string(), Value::Bool(false));
    true
}

pub(crate) fn project_provider_node(node: &Value, proxy_origin: &str) -> Value {
    let mut projected = node.clone();
    ensure_openai_completions_system_role(&mut projected);
    let Some(object) = projected.as_object_mut() else {
        return projected;
    };
    let api = object
        .get("api")
        .and_then(Value::as_str)
        .map(str::to_string);
    let proxy_base = proxy_base_url_for_api(api.as_deref(), proxy_origin);
    object.insert("baseUrl".to_string(), Value::String(proxy_base.clone()));
    object.insert(
        "apiKey".to_string(),
        Value::String(PI_PROXY_API_KEY_PLACEHOLDER.to_string()),
    );
    let headers = object
        .entry("headers")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(headers) = headers.as_object_mut() {
        headers.insert(
            PI_CLIENT_APP_HEADER.to_string(),
            Value::String(PI_CLIENT_APP_VALUE.to_string()),
        );
    }
    if let Some(Value::Array(models)) = object.get_mut("models") {
        for model in models {
            if let Some(model_object) = model.as_object_mut() {
                if model_object.contains_key("baseUrl") {
                    model_object.insert("baseUrl".to_string(), Value::String(proxy_base.clone()));
                }
            }
        }
    }
    projected
}

pub(crate) fn project_models_document(document: &mut Value, proxy_origin: &str) -> bool {
    let Some(providers) = document.get_mut("providers").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    let keys: Vec<String> = providers.keys().cloned().collect();
    for key in keys {
        let Some(current) = providers.get(&key).cloned() else {
            continue;
        };
        if !current.is_object() {
            continue;
        }
        let projected = project_provider_node(&current, proxy_origin);
        if projected != current {
            providers.insert(key, projected);
            changed = true;
        }
    }
    changed
}

pub(crate) fn read_pi_models_document() -> Result<Value, AppError> {
    let _guard = lock_models_file()?;
    let path = get_pi_models_path()?;
    read_models_document_with_revision(&path).map(|(document, _)| document)
}

pub(crate) fn write_pi_models_document(document: &Value) -> Result<(), AppError> {
    let _guard = lock_models_file()?;
    write_pi_models_document_locked(document)
}

fn write_pi_models_document_locked(document: &Value) -> Result<(), AppError> {
    let path = get_pi_models_path()?;
    let (_, expected_revision) = read_models_document_with_revision(&path)?;
    write_models_document(&path, document, &expected_revision)
}

/// Rewrite every explicit provider `baseUrl` onto the existing proxy listen.
/// Preserves unknown fields and does not create `defaultProvider`/`defaultModel`.
pub(crate) fn project_live_models_to_proxy(proxy_origin: &str) -> Result<(), AppError> {
    let _guard = lock_models_file()?;
    let path = get_pi_models_path()?;
    let (mut document, expected_revision) = read_models_document_with_revision(&path)?;
    if !document.is_object() {
        document = Value::Object(Map::new());
    }
    let _ = project_models_document(&mut document, proxy_origin);
    write_models_document(&path, &document, &expected_revision)
}

pub(crate) fn restore_live_models_document(document: &Value) -> Result<(), AppError> {
    write_pi_models_document(document)
}

/// Project a single provider node when takeover is already active (enable/update).
pub(crate) fn project_node_if_object(node: &Value, proxy_origin: &str) -> Value {
    if node.is_object() {
        project_provider_node(node, proxy_origin)
    } else {
        node.clone()
    }
}

/// True when `path` is a usable Pi agent directory on this host or a Windows UNC path.
pub(crate) fn is_usable_pi_agent_dir(path: &Path) -> bool {
    path.is_absolute() || is_windows_unc_path(path)
}

pub(crate) fn is_windows_unc_path(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    let normalized = raw.replace('/', r"\");
    normalized.starts_with(r"\\")
}

pub(crate) fn request_is_pi_client(headers: &http::HeaderMap) -> bool {
    headers
        .get(PI_CLIENT_APP_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case(PI_CLIENT_APP_VALUE))
}

pub(crate) fn usage_app_type_from_headers(
    headers: &http::HeaderMap,
    routed_app: &'static str,
) -> &'static str {
    if request_is_pi_client(headers) {
        "pi"
    } else {
        routed_app
    }
}

pub(crate) fn pi_provider_api(config: &Value) -> Option<&str> {
    config.get("api").and_then(Value::as_str)
}

/// Shared Claude listen still uses Claude / Codex / Gemini handlers.
/// Pick the Pi `api` family that belongs on that handler so we do not
/// forward an Anthropic Pi card through Codex (or the reverse).
pub(crate) fn pi_api_matches_listen_app(api: Option<&str>, listen_app: &str) -> bool {
    match listen_app {
        "claude" => matches!(
            api,
            Some("anthropic-messages") | Some("bedrock-converse-stream") | None
        ),
        "codex" => matches!(api, Some("openai-completions") | Some("openai-responses")),
        "gemini" => matches!(api, Some("google-generative-ai")),
        _ => false,
    }
}

pub(crate) fn pi_provider_has_model(config: &Value, model: &str) -> bool {
    let model = model.trim();
    if model.is_empty() || model == "unknown" {
        return false;
    }
    config
        .get("models")
        .and_then(Value::as_array)
        .is_some_and(|models| {
            models.iter().any(|entry| {
                entry
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id == model)
                    || entry.as_str() == Some(model)
            })
        })
}

/// Real upstream credentials live in the Pi provider catalog. Projected
/// `models.json` nodes (`PROXY_MANAGED` / local listen URL) cannot be forwarded.
pub(crate) fn pi_provider_is_forwardable(config: &Value) -> bool {
    if is_projected_provider_node(config) {
        return false;
    }
    config
        .get("apiKey")
        .or_else(|| config.get("api_key"))
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|key| !key.is_empty() && key != PI_PROXY_API_KEY_PLACEHOLDER)
}

/// Claude / Codex / Gemini adapters look up `base_url` / `baseURL`.
/// Pi's native schema uses `baseUrl`; copy it so the shared adapters work.
pub(crate) fn copy_pi_base_url_for_adapters(config: &mut Value) {
    let Some(object) = config.as_object_mut() else {
        return;
    };
    if object
        .get("base_url")
        .and_then(Value::as_str)
        .is_some_and(|url| !url.trim().is_empty())
    {
        return;
    }
    if let Some(url) = object
        .get("baseUrl")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
    {
        object.insert("base_url".to_string(), Value::String(url));
        return;
    }
    let model_url = object
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| {
            models.iter().find_map(|model| {
                model
                    .get("baseUrl")
                    .or_else(|| model.get("base_url"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|url| !url.is_empty())
                    .map(str::to_string)
            })
        });
    if let Some(url) = model_url {
        object.insert("base_url".to_string(), Value::String(url));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_config::test_support::TestAgentDir;
    use serde_json::json;
    use serial_test::serial;
    use std::fs;
    use std::path::PathBuf;

    const FIXTURE: &str = include_str!("fixtures/models_with_unknown_fields.json");

    fn fixture_document() -> Value {
        serde_json::from_str(FIXTURE).expect("fixture json")
    }

    #[test]
    fn wsl_localhost_agent_dir_is_accepted() {
        let unc = PathBuf::from(r"\\wsl.localhost\Ubuntu-22.04\home\user\.pi\agent");
        assert!(
            is_windows_unc_path(&unc),
            "UNC agent dir must be recognized: {}",
            unc.display()
        );
        assert!(is_usable_pi_agent_dir(&unc));
        assert_eq!(
            crate::pi_config::resolve_pi_agent_dir(
                Some(unc.clone()),
                None,
                PathBuf::from("/unused")
            )
            .expect("UNC override must resolve"),
            unc
        );
    }

    #[test]
    fn wsl_localhost_dot_pi_override_canonicalizes_to_agent() {
        let unc = PathBuf::from(r"\\wsl.localhost\Ubuntu-22.04\home\user\.pi");
        let resolved =
            crate::pi_config::resolve_pi_agent_dir(Some(unc), None, PathBuf::from("/unused"))
                .expect("UNC .pi override must resolve");
        let normalized = resolved.to_string_lossy().replace('/', r"\");
        assert!(
            normalized.ends_with(r"\.pi\agent"),
            "session root must be under .pi/agent, not a C: mirror: {normalized}"
        );
        assert!(!normalized.to_ascii_lowercase().contains("pi-wsl-sessions"));
    }

    #[test]
    fn wsl_dollar_agent_dir_is_accepted() {
        let unc = PathBuf::from(r"\\wsl$\Ubuntu-22.04\home\user\.pi\agent");
        assert!(is_usable_pi_agent_dir(&unc));
        assert_eq!(
            crate::pi_config::resolve_pi_agent_dir(
                Some(unc.clone()),
                None,
                PathBuf::from("/unused")
            )
            .expect("wsl$ override must resolve"),
            unc
        );
    }

    #[test]
    fn listen_host_rewrite_matches_claude() {
        assert_eq!(rewrite_listen_host_for_clients("0.0.0.0"), "127.0.0.1");
        assert_eq!(rewrite_listen_host_for_clients("::"), "::1");
        assert_eq!(rewrite_listen_host_for_clients("127.0.0.1"), "127.0.0.1");
        assert_eq!(
            proxy_origin_from_listen("0.0.0.0", 15721),
            "http://127.0.0.1:15721"
        );
        assert_eq!(
            proxy_origin_from_listen("192.168.1.8", 15721),
            "http://192.168.1.8:15721"
        );
    }

    #[test]
    fn projection_maps_apis_onto_existing_proxy_routes() {
        let origin = "http://127.0.0.1:15721";
        assert_eq!(
            proxy_base_url_for_api(Some("anthropic-messages"), origin),
            origin
        );
        assert_eq!(
            proxy_base_url_for_api(Some("google-generative-ai"), origin),
            origin
        );
        assert_eq!(
            proxy_base_url_for_api(Some("openai-completions"), origin),
            "http://127.0.0.1:15721/v1"
        );
        assert_eq!(
            proxy_base_url_for_api(Some("openai-responses"), origin),
            "http://127.0.0.1:15721/v1"
        );
    }

    #[test]
    fn projection_preserves_unknown_fields_and_skips_defaults() {
        let mut document = fixture_document();
        assert!(project_models_document(
            &mut document,
            "http://127.0.0.1:15721"
        ));
        assert_eq!(
            document.get("customTopLevel").and_then(Value::as_str),
            Some("keep-me")
        );
        assert!(document.get("defaultProvider").is_none());
        assert!(document.get("defaultModel").is_none());

        let anthropic = &document["providers"]["anthropic"];
        assert_eq!(anthropic["baseUrl"], json!("http://127.0.0.1:15721"));
        assert_eq!(anthropic["apiKey"], json!(PI_PROXY_API_KEY_PLACEHOLDER));
        assert_eq!(anthropic["headers"]["X-Custom"], json!("hdr"));
        assert_eq!(
            anthropic["headers"][PI_CLIENT_APP_HEADER],
            json!(PI_CLIENT_APP_VALUE)
        );
        assert_eq!(anthropic["sdkOption"]["timeout"], json!(30));
        assert_eq!(
            anthropic["models"][0]["compat"]["supportsDeveloperRole"],
            json!(true)
        );

        let openai = &document["providers"]["openai"];
        assert_eq!(openai["baseUrl"], json!("http://127.0.0.1:15721/v1"));
        assert_eq!(
            openai["models"][0]["baseUrl"],
            json!("http://127.0.0.1:15721/v1")
        );
        assert!(is_projected_provider_node(anthropic));
        assert!(document_has_proxy_projection(&document));
    }

    #[test]
    fn openai_completions_default_compat_writes_system_not_developer() {
        let mut node = json!({
            "name": "Intranet",
            "api": "openai-completions",
            "baseUrl": "https://llm.intranet.example/v1",
            "models": [
                { "id": "glm-5.1", "reasoning": true },
                {
                    "id": "kimi-k3",
                    "compat": { "thinkingFormat": "openai" }
                }
            ]
        });
        assert!(ensure_openai_completions_system_role(&mut node));
        assert_eq!(node["compat"]["supportsDeveloperRole"], json!(false));
        assert_eq!(
            node["models"][0]["compat"]["supportsDeveloperRole"],
            json!(false)
        );
        assert_eq!(
            node["models"][1]["compat"]["supportsDeveloperRole"],
            json!(false)
        );
        assert_eq!(
            node["models"][1]["compat"]["thinkingFormat"],
            json!("openai")
        );
        assert!(!ensure_openai_completions_system_role(&mut node));
    }

    #[test]
    fn openai_completions_preserves_explicit_developer_role_opt_in() {
        let mut node = json!({
            "api": "openai-completions",
            "compat": { "supportsDeveloperRole": true },
            "models": [{
                "id": "gpt-5.4",
                "compat": { "supportsDeveloperRole": true }
            }]
        });
        assert!(!ensure_openai_completions_system_role(&mut node));
        assert_eq!(node["compat"]["supportsDeveloperRole"], json!(true));
        assert_eq!(
            node["models"][0]["compat"]["supportsDeveloperRole"],
            json!(true)
        );
    }

    #[test]
    fn anthropic_and_responses_apis_are_not_rewritten() {
        for api in ["anthropic-messages", "openai-responses"] {
            let mut node = json!({ "api": api, "models": [{ "id": "m" }] });
            assert!(!ensure_openai_completions_system_role(&mut node));
            assert!(node.get("compat").is_none());
        }
    }

    #[test]
    fn baisheng_yum_style_card_gets_system_role_compat_like_opencode_go() {
        let mut node = json!({
            "name": "baisheng",
            "api": "openai-completions",
            "baseUrl": "http://api.llm.prd.yumc.local/v1",
            "models": [
                { "id": "glm-5.2", "reasoning": true },
                { "id": "kimi-k2.7-code" }
            ]
        });
        assert!(ensure_openai_completions_system_role(&mut node));
        assert_eq!(node["compat"]["supportsDeveloperRole"], json!(false));
        for model in node["models"].as_array().unwrap() {
            assert_eq!(model["compat"]["supportsDeveloperRole"], json!(false));
        }
    }

    #[test]
    fn projection_injects_system_role_compat_for_openai_completions() {
        let node = project_provider_node(
            &json!({
                "api": "openai-completions",
                "baseUrl": "https://open.bigmodel.cn/api/coding/paas/v4",
                "apiKey": "sk-live"
            }),
            "http://127.0.0.1:15721",
        );
        assert_eq!(node["compat"]["supportsDeveloperRole"], json!(false));
        assert_eq!(node["baseUrl"], json!("http://127.0.0.1:15721/v1"));
    }

    #[test]
    fn projection_injects_base_url_when_missing() {
        let mut node = json!({
            "name": "Minimal",
            "api": "anthropic-messages",
            "futureField": true
        });
        node = project_provider_node(&node, "http://127.0.0.1:15721");
        assert_eq!(node["baseUrl"], json!("http://127.0.0.1:15721"));
        assert_eq!(node["apiKey"], json!(PI_PROXY_API_KEY_PLACEHOLDER));
        assert_eq!(
            node["headers"][PI_CLIENT_APP_HEADER],
            json!(PI_CLIENT_APP_VALUE)
        );
        assert_eq!(node["futureField"], json!(true));
    }

    #[test]
    fn usage_app_type_from_headers_tags_pi_without_changing_routed_app() {
        let mut headers = http::HeaderMap::new();
        assert_eq!(usage_app_type_from_headers(&headers, "claude"), "claude");
        headers.insert(
            PI_CLIENT_APP_HEADER,
            http::HeaderValue::from_static(PI_CLIENT_APP_VALUE),
        );
        assert!(request_is_pi_client(&headers));
        assert_eq!(usage_app_type_from_headers(&headers, "claude"), "pi");
        assert_eq!(usage_app_type_from_headers(&headers, "codex"), "pi");
    }

    #[test]
    fn pi_api_matches_shared_listen_handlers() {
        assert!(pi_api_matches_listen_app(
            Some("anthropic-messages"),
            "claude"
        ));
        assert!(pi_api_matches_listen_app(
            Some("bedrock-converse-stream"),
            "claude"
        ));
        assert!(pi_api_matches_listen_app(None, "claude"));
        assert!(!pi_api_matches_listen_app(
            Some("openai-completions"),
            "claude"
        ));
        assert!(pi_api_matches_listen_app(
            Some("openai-completions"),
            "codex"
        ));
        assert!(pi_api_matches_listen_app(Some("openai-responses"), "codex"));
        assert!(!pi_api_matches_listen_app(
            Some("anthropic-messages"),
            "codex"
        ));
        assert!(pi_api_matches_listen_app(
            Some("google-generative-ai"),
            "gemini"
        ));
        assert!(!pi_api_matches_listen_app(
            Some("anthropic-messages"),
            "gemini"
        ));
    }

    #[test]
    fn pi_forwardable_skips_projected_placeholder() {
        let live = json!({
            "api": "anthropic-messages",
            "baseUrl": "https://api.anthropic.com",
            "apiKey": "sk-ant-live"
        });
        assert!(pi_provider_is_forwardable(&live));
        assert!(pi_provider_has_model(
            &json!({"models":[{"id":"claude-sonnet-4"}]}),
            "claude-sonnet-4"
        ));

        let projected = project_provider_node(&live, "http://127.0.0.1:15721");
        assert!(!pi_provider_is_forwardable(&projected));
        assert_eq!(projected["apiKey"], json!(PI_PROXY_API_KEY_PLACEHOLDER));
    }

    #[test]
    fn copy_pi_base_url_exposes_native_field_to_adapters() {
        let mut config = json!({
            "api": "anthropic-messages",
            "baseUrl": "https://api.anthropic.com",
            "apiKey": "sk-ant-live"
        });
        copy_pi_base_url_for_adapters(&mut config);
        assert_eq!(config["base_url"], json!("https://api.anthropic.com"));
        assert_eq!(config["baseUrl"], json!("https://api.anthropic.com"));
    }

    #[test]
    #[serial]
    fn project_live_models_is_atomic_and_does_not_touch_auth_or_settings() {
        let _agent = TestAgentDir::new();
        let models_path = crate::pi_config::get_pi_models_path().expect("models path");
        fs::create_dir_all(models_path.parent().expect("agent dir")).expect("agent dir");
        fs::write(&models_path, FIXTURE).expect("write fixture models");

        let settings_path = crate::pi_config::get_pi_settings_path().expect("settings path");
        fs::write(
            &settings_path,
            r#"{"defaultProvider":"anthropic","defaultModel":"claude-sonnet-4"}"#,
        )
        .expect("write settings");
        let auth_path = crate::pi_config::get_pi_agent_dir()
            .expect("agent dir")
            .parent()
            .expect("pi root")
            .join("auth.json");
        fs::write(&auth_path, r#"{"anthropic":{"type":"oauth"}}"#).expect("write auth");
        let settings_before = fs::read_to_string(&settings_path).expect("read settings");
        let auth_before = fs::read_to_string(&auth_path).expect("read auth");

        project_live_models_to_proxy("http://127.0.0.1:15721").expect("project");

        let written: Value =
            serde_json::from_str(&fs::read_to_string(&models_path).expect("read models"))
                .expect("parse models");
        assert_eq!(
            written["providers"]["anthropic"]["baseUrl"],
            json!("http://127.0.0.1:15721")
        );
        assert_eq!(
            written["providers"]["openai"]["baseUrl"],
            json!("http://127.0.0.1:15721/v1")
        );
        assert_eq!(written["customTopLevel"], json!("keep-me"));
        assert!(written.get("defaultProvider").is_none());
        assert_eq!(
            fs::read_to_string(&settings_path).expect("settings after"),
            settings_before,
            "settings.json must be untouched"
        );
        assert_eq!(
            fs::read_to_string(&auth_path).expect("auth after"),
            auth_before,
            "auth.json must be untouched"
        );
    }
}
