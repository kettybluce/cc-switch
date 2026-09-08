//! Option B: project Pi `models.json` `baseUrl`s through the local proxy.
//!
//! The CC Switch database keeps the real upstream URL. When the local proxy
//! is running, the live `models.json` written for Pi points at
//! `http://<reachable-host>:<port>/pi/<provider-id>[/v1|/v1beta]`. Stopping
//! or disabling the local proxy restores the stored upstream URLs.
//!
//! Unknown JSON fields are left untouched; only `baseUrl` (provider-level and
//! per-model) is rewritten.

use serde_json::Value;

/// Path prefix the local proxy uses for Pi-attributed traffic.
pub const PI_PROXY_PATH_PREFIX: &str = "/pi/";

/// Pi `api` values we can route through the existing local proxy handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiApiKind {
    AnthropicMessages,
    OpenaiCompletions,
    OpenaiResponses,
    GoogleGenerativeAi,
    /// Bedrock and any future / unknown protocol. Left on the real upstream.
    Unsupported,
}

impl PiApiKind {
    pub fn from_api_field(api: Option<&str>) -> Self {
        match api.map(str::trim).unwrap_or_default() {
            "anthropic-messages" | "anthropic" => Self::AnthropicMessages,
            "openai-completions" | "openai-chat" | "openai" => Self::OpenaiCompletions,
            "openai-responses" => Self::OpenaiResponses,
            "google-generative-ai" | "gemini" | "gemini-cli" => Self::GoogleGenerativeAi,
            _ => Self::Unsupported,
        }
    }

    pub fn is_supported(self) -> bool {
        !matches!(self, Self::Unsupported)
    }
}

/// Read the `api` field from a Pi provider node.
pub fn provider_api_kind(config: &Value) -> PiApiKind {
    PiApiKind::from_api_field(config.get("api").and_then(Value::as_str))
}

/// Whether `provider_id` is safe to embed as a single URL path segment.
pub fn is_safe_provider_id(provider_id: &str) -> bool {
    let id = provider_id.trim();
    !id.is_empty()
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains('?')
        && !id.contains('#')
        && !id.contains('%')
        && !id.contains("..")
}

/// Build the live `baseUrl` Pi should call when proxy mode is on.
///
/// `origin` is `http://<host>:<port>` with no trailing slash — the address
/// the Pi process can actually reach (loopback on the host, or the WSL
/// gateway / mirrored loopback inside a distribution).
///
/// `upstream` is the stored URL, used only to preserve a `/v1beta` suffix
/// for Google-style providers.
pub fn proxy_base_url(origin: &str, provider_id: &str, kind: PiApiKind, upstream: &str) -> String {
    let origin = origin.trim().trim_end_matches('/');
    let path = match kind {
        PiApiKind::AnthropicMessages => format!("/pi/{provider_id}"),
        PiApiKind::OpenaiCompletions | PiApiKind::OpenaiResponses => {
            format!("/pi/{provider_id}/v1")
        }
        PiApiKind::GoogleGenerativeAi => {
            if strip_query(upstream)
                .trim_end_matches('/')
                .ends_with("/v1beta")
            {
                format!("/pi/{provider_id}/v1beta")
            } else {
                format!("/pi/{provider_id}")
            }
        }
        PiApiKind::Unsupported => {
            return upstream.to_string();
        }
    };
    format!("{origin}{path}")
}

/// True when `url` is a CC Switch Pi proxy projection, not a real upstream.
pub fn is_pi_proxy_base_url(url: &str) -> bool {
    let url = url.trim();
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let path = match rest.find('/') {
        Some(index) => &rest[index..],
        None => return false,
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    path == "/pi" || path.starts_with(PI_PROXY_PATH_PREFIX)
}

/// Extract the provider id encoded in a projected proxy `baseUrl`.
#[allow(dead_code)]
pub fn provider_id_from_proxy_url(url: &str) -> Option<&str> {
    let url = url.trim();
    let rest = url.strip_prefix("http://")?;
    let path = rest.find('/').map(|index| &rest[index..])?;
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let after = path.strip_prefix(PI_PROXY_PATH_PREFIX)?;
    let id = after.split('/').next().unwrap_or_default();
    (!id.is_empty()).then_some(id)
}

/// Rewrite provider-level and per-model `baseUrl`s onto the local proxy.
///
/// Returns `true` when at least one URL changed. Unsupported APIs and
/// unusable provider ids are left alone.
pub fn project_provider_base_urls(config: &mut Value, provider_id: &str, origin: &str) -> bool {
    if !is_safe_provider_id(provider_id) {
        return false;
    }
    let kind = provider_api_kind(config);
    if !kind.is_supported() {
        return false;
    }

    let mut changed = false;
    if let Some(current) = config
        .get("baseUrl")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        let projected = proxy_base_url(origin, provider_id, kind, &current);
        if current != projected {
            config["baseUrl"] = Value::String(projected);
            changed = true;
        }
    }

    let provider_api = config
        .get("api")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(models) = config.get_mut("models").and_then(Value::as_array_mut) {
        for model in models {
            let Some(current) = model
                .get("baseUrl")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let model_kind = PiApiKind::from_api_field(
                model
                    .get("api")
                    .and_then(Value::as_str)
                    .or(provider_api.as_deref()),
            );
            if !model_kind.is_supported() {
                continue;
            }
            let projected = proxy_base_url(origin, provider_id, model_kind, &current);
            if current != projected {
                model["baseUrl"] = Value::String(projected);
                changed = true;
            }
        }
    }

    changed
}

/// Put stored upstream `baseUrl`s back when the live node currently holds a
/// proxy projection.
///
/// External edits to a *real* URL are kept. Only URLs that look like our
/// `/pi/<id>` projection are restored from `stored`.
pub fn restore_provider_base_urls(live: &mut Value, stored: &Value) -> bool {
    let mut changed = false;
    if live
        .get("baseUrl")
        .and_then(Value::as_str)
        .is_some_and(is_pi_proxy_base_url)
    {
        match stored.get("baseUrl").and_then(Value::as_str) {
            Some(upstream) if !is_pi_proxy_base_url(upstream) => {
                live["baseUrl"] = Value::String(upstream.to_string());
                changed = true;
            }
            _ => {}
        }
    }

    let Some(live_models) = live.get_mut("models").and_then(Value::as_array_mut) else {
        return changed;
    };
    let stored_models = stored.get("models").and_then(Value::as_array);
    for (index, model) in live_models.iter_mut().enumerate() {
        if !model
            .get("baseUrl")
            .and_then(Value::as_str)
            .is_some_and(is_pi_proxy_base_url)
        {
            continue;
        }
        let Some(upstream) = stored_models
            .and_then(|models| models.get(index))
            .and_then(|model| model.get("baseUrl"))
            .and_then(Value::as_str)
            .filter(|url| !is_pi_proxy_base_url(url))
        else {
            continue;
        };
        model["baseUrl"] = Value::String(upstream.to_string());
        changed = true;
    }
    changed
}

/// Copy `stored` and project it for a live write, preserving unknown fields
/// from `live` when that node already exists.
///
/// Prefer this over projecting `stored` alone so a field that only exists in
/// `models.json` is not dropped when we rewrite `baseUrl`.
pub fn live_provider_node(
    stored: &Value,
    live: Option<&Value>,
    provider_id: &str,
    origin: Option<&str>,
) -> Value {
    let mut node = match live {
        Some(live) => live.clone(),
        None => stored.clone(),
    };

    // The database is the source of truth for the real upstream URL and the
    // fields CC Switch manages. Overlay those onto the live node first, then
    // optionally rewrite baseUrl so we never persist a proxy URL in the DB
    // and never lose unknown native fields.
    overlay_managed_fields(&mut node, stored);

    if let Some(origin) = origin {
        project_provider_base_urls(&mut node, provider_id, origin);
    } else {
        restore_provider_base_urls(&mut node, stored);
    }
    node
}

fn overlay_managed_fields(live: &mut Value, stored: &Value) {
    let Some(stored_obj) = stored.as_object() else {
        return;
    };
    let Some(live_obj) = live.as_object_mut() else {
        *live = stored.clone();
        return;
    };
    for (key, value) in stored_obj {
        live_obj.insert(key.clone(), value.clone());
    }
}

fn strip_query(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn api_field_classification() {
        assert_eq!(
            PiApiKind::from_api_field(Some("anthropic-messages")),
            PiApiKind::AnthropicMessages
        );
        assert_eq!(
            PiApiKind::from_api_field(Some("openai-completions")),
            PiApiKind::OpenaiCompletions
        );
        assert_eq!(
            PiApiKind::from_api_field(Some("openai-responses")),
            PiApiKind::OpenaiResponses
        );
        assert_eq!(
            PiApiKind::from_api_field(Some("google-generative-ai")),
            PiApiKind::GoogleGenerativeAi
        );
        assert_eq!(
            PiApiKind::from_api_field(Some("bedrock-converse-stream")),
            PiApiKind::Unsupported
        );
        assert!(!PiApiKind::from_api_field(None).is_supported());
    }

    #[test]
    fn anthropic_providers_point_at_the_messages_prefix() {
        assert_eq!(
            proxy_base_url(
                "http://172.30.208.1:15721",
                "anthropic",
                PiApiKind::AnthropicMessages,
                "https://api.anthropic.com",
            ),
            "http://172.30.208.1:15721/pi/anthropic"
        );
    }

    #[test]
    fn openai_providers_include_the_v1_suffix_the_proxy_expects() {
        assert_eq!(
            proxy_base_url(
                "http://127.0.0.1:15721",
                "openai",
                PiApiKind::OpenaiCompletions,
                "https://api.example.com/v1",
            ),
            "http://127.0.0.1:15721/pi/openai/v1"
        );
        assert_eq!(
            proxy_base_url(
                "http://127.0.0.1:15721/",
                "rightapi",
                PiApiKind::OpenaiResponses,
                "https://www.rightapi.ai/codex/v1",
            ),
            "http://127.0.0.1:15721/pi/rightapi/v1"
        );
    }

    #[test]
    fn google_providers_keep_a_v1beta_suffix_when_the_upstream_had_one() {
        assert_eq!(
            proxy_base_url(
                "http://127.0.0.1:15721",
                "gemini",
                PiApiKind::GoogleGenerativeAi,
                "https://generativelanguage.googleapis.com/v1beta",
            ),
            "http://127.0.0.1:15721/pi/gemini/v1beta"
        );
        assert_eq!(
            proxy_base_url(
                "http://127.0.0.1:15721",
                "gemini",
                PiApiKind::GoogleGenerativeAi,
                "https://generativelanguage.googleapis.com",
            ),
            "http://127.0.0.1:15721/pi/gemini"
        );
    }

    #[test]
    fn unsupported_apis_are_never_rewritten() {
        let upstream = "https://bedrock.amazonaws.com";
        assert_eq!(
            proxy_base_url(
                "http://127.0.0.1:15721",
                "bedrock",
                PiApiKind::Unsupported,
                upstream,
            ),
            upstream
        );
    }

    #[test]
    fn proxy_urls_are_detected_even_when_the_host_is_a_wsl_gateway() {
        assert!(is_pi_proxy_base_url(
            "http://172.30.208.1:15721/pi/anthropic"
        ));
        assert!(is_pi_proxy_base_url("http://127.0.0.1:15721/pi/openai/v1"));
        assert!(!is_pi_proxy_base_url("https://api.anthropic.com"));
        assert!(!is_pi_proxy_base_url("http://127.0.0.1:15721/v1"));
        assert!(!is_pi_proxy_base_url(
            "http://127.0.0.1:15721/claude/v1/messages"
        ));
    }

    #[test]
    fn provider_id_is_read_back_from_the_projected_url() {
        assert_eq!(
            provider_id_from_proxy_url("http://172.30.208.1:15721/pi/cc-switch-test/v1"),
            Some("cc-switch-test")
        );
        assert_eq!(
            provider_id_from_proxy_url("https://api.example.com/v1"),
            None
        );
    }

    #[test]
    fn projecting_rewrites_only_base_url_and_keeps_unknown_fields() {
        let mut config = json!({
            "name": "Example",
            "baseUrl": "https://api.example.com/v1",
            "api": "openai-completions",
            "apiKey": "secret",
            "sdkOption": { "timeout": 30 },
            "models": [
                { "id": "a", "baseUrl": "https://api.example.com/v1" },
                { "id": "b", "compat": { "keep": true } }
            ]
        });

        assert!(project_provider_base_urls(
            &mut config,
            "cc-switch-test",
            "http://172.30.208.1:15721"
        ));
        assert_eq!(
            config["baseUrl"],
            "http://172.30.208.1:15721/pi/cc-switch-test/v1"
        );
        assert_eq!(
            config["models"][0]["baseUrl"],
            "http://172.30.208.1:15721/pi/cc-switch-test/v1"
        );
        assert_eq!(config["sdkOption"]["timeout"], 30);
        assert_eq!(config["models"][1]["compat"]["keep"], true);
        assert_eq!(config["apiKey"], "secret");
    }

    #[test]
    fn restore_puts_the_stored_upstream_back_and_ignores_real_external_edits() {
        let stored = json!({
            "baseUrl": "https://api.example.com/v1",
            "api": "openai-completions",
            "models": [{ "id": "a", "baseUrl": "https://api.example.com/v1" }]
        });
        let mut live = json!({
            "baseUrl": "http://127.0.0.1:15721/pi/cc-switch-test/v1",
            "api": "openai-completions",
            "futureField": { "keep": true },
            "models": [{ "id": "a", "baseUrl": "http://127.0.0.1:15721/pi/cc-switch-test/v1" }]
        });

        assert!(restore_provider_base_urls(&mut live, &stored));
        assert_eq!(live["baseUrl"], "https://api.example.com/v1");
        assert_eq!(live["models"][0]["baseUrl"], "https://api.example.com/v1");
        assert_eq!(live["futureField"]["keep"], true);

        let mut external = json!({
            "baseUrl": "https://user-edited.example/v1",
            "api": "openai-completions"
        });
        assert!(!restore_provider_base_urls(&mut external, &stored));
        assert_eq!(external["baseUrl"], "https://user-edited.example/v1");
    }

    #[test]
    fn live_node_uses_db_as_source_of_truth_then_projects() {
        let stored = json!({
            "name": "Example",
            "baseUrl": "https://api.example.com/v1",
            "api": "anthropic-messages",
            "apiKey": "secret"
        });
        let live = json!({
            "name": "Stale",
            "baseUrl": "https://stale.example",
            "api": "anthropic-messages",
            "sdkOption": { "timeout": 30 }
        });

        let projected = live_provider_node(
            &stored,
            Some(&live),
            "anthropic",
            Some("http://127.0.0.1:15721"),
        );
        assert_eq!(projected["baseUrl"], "http://127.0.0.1:15721/pi/anthropic");
        assert_eq!(projected["name"], "Example");
        assert_eq!(projected["sdkOption"]["timeout"], 30);

        let restored = live_provider_node(&stored, Some(&projected), "anthropic", None);
        assert_eq!(restored["baseUrl"], "https://api.example.com/v1");
        assert_eq!(restored["sdkOption"]["timeout"], 30);
    }

    #[test]
    fn unsafe_provider_ids_are_refused() {
        assert!(!is_safe_provider_id("../etc"));
        assert!(!is_safe_provider_id("a/b"));
        assert!(!is_safe_provider_id(""));
        assert!(is_safe_provider_id("cc-switch-test"));
        assert!(is_safe_provider_id("anthropic"));
    }

    #[test]
    fn contract_documents_option_b_baseurl_routing() {
        let contract = include_str!("../../../docs/pi-native-contract-zh.md");
        assert!(
            contract.contains("/pi/<provider_id>"),
            "contract must describe the per-provider proxy path"
        );
        assert!(
            contract.contains("真实上游地址只保存在 CC Switch 数据库"),
            "contract must keep the database as source of truth"
        );
        assert!(
            contract.contains("HTTP_PROXY"),
            "contract must say process-level env injection is not the primary path"
        );
        assert!(
            !contract.contains("env 前缀"),
            "contract must not still describe the old env-injection design as the Pi proxy path"
        );
        assert!(
            contract.contains("不修改 `/etc/environment`"),
            "contract must keep the no-global-WSL-env boundary"
        );
        assert!(
            contract.contains("同一开关"),
            "contract must tie Pi projection to the existing local-proxy control"
        );
        assert!(
            contract.contains("没有单独的 Pi 代理开关"),
            "contract must not treat a Pi-only toggle as the primary UX"
        );
    }
}
