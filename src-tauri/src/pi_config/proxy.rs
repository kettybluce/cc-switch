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

pub(crate) fn project_provider_node(node: &Value, proxy_origin: &str) -> Value {
    let mut projected = node.clone();
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
    fn projection_injects_base_url_when_missing() {
        let mut node = json!({
            "name": "Minimal",
            "api": "anthropic-messages",
            "futureField": true
        });
        node = project_provider_node(&node, "http://127.0.0.1:15721");
        assert_eq!(node["baseUrl"], json!("http://127.0.0.1:15721"));
        assert_eq!(node["apiKey"], json!(PI_PROXY_API_KEY_PLACEHOLDER));
        assert_eq!(node["futureField"], json!(true));
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
