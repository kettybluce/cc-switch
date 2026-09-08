//! Pi provider adapter.
//!
//! Pi's native `models.json` node is `{ baseUrl, apiKey, api, headers, models }`.
//! Traffic is attributed as `app_type = "pi"`; URL joining and auth headers
//! reuse the Claude / Codex / Gemini adapters matching Pi's `api` field.

use super::{
    adapter::auth_header_value, AuthInfo, AuthStrategy, ClaudeAdapter, CodexAdapter, GeminiAdapter,
    ProviderAdapter,
};
use crate::pi_runtime::rewrite::{provider_api_kind, PiApiKind};
use crate::provider::Provider;
use crate::proxy::error::ProxyError;
use http::{HeaderName, HeaderValue};

pub struct PiAdapter {
    claude: ClaudeAdapter,
    codex: CodexAdapter,
    gemini: GeminiAdapter,
}

impl PiAdapter {
    pub fn new() -> Self {
        Self {
            claude: ClaudeAdapter::new(),
            codex: CodexAdapter::new(),
            gemini: GeminiAdapter::new(),
        }
    }

    fn kind(provider: &Provider) -> PiApiKind {
        provider_api_kind(&provider.settings_config)
    }

    fn extract_key(provider: &Provider) -> Option<String> {
        provider
            .settings_config
            .get("apiKey")
            .or_else(|| provider.settings_config.get("api_key"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }
}

impl Default for PiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for PiAdapter {
    fn name(&self) -> &'static str {
        "Pi"
    }

    fn extract_base_url(&self, provider: &Provider) -> Result<String, ProxyError> {
        crate::pi_config::provider_base_url(&provider.settings_config)
            .map(|url| url.trim_end_matches('/').to_string())
            .map_err(|error| ProxyError::ConfigError(error.to_string()))
    }

    fn extract_auth(&self, provider: &Provider) -> Option<AuthInfo> {
        let key = Self::extract_key(provider)?;
        let strategy = match Self::kind(provider) {
            PiApiKind::AnthropicMessages => AuthStrategy::Anthropic,
            PiApiKind::GoogleGenerativeAi => AuthStrategy::Google,
            PiApiKind::OpenaiCompletions | PiApiKind::OpenaiResponses | PiApiKind::Unsupported => {
                AuthStrategy::Bearer
            }
        };
        Some(AuthInfo::new(key, strategy))
    }

    fn build_url(&self, base_url: &str, endpoint: &str) -> String {
        // The inner adapters only look at base_url + endpoint, not at the
        // provider, so we can reuse their join / de-dupe rules.
        match endpoint {
            path if path.contains("/v1beta") || path.contains(":generateContent") => {
                self.gemini.build_url(base_url, endpoint)
            }
            path if path.contains("/chat/completions")
                || path.contains("/responses")
                || path.contains("/models")
                || path.contains("/images/")
                || path.contains("/alpha/") =>
            {
                self.codex.build_url(base_url, endpoint)
            }
            _ => self.claude.build_url(base_url, endpoint),
        }
    }

    fn get_auth_headers(
        &self,
        auth: &AuthInfo,
    ) -> Result<Vec<(HeaderName, HeaderValue)>, ProxyError> {
        match auth.strategy {
            AuthStrategy::Anthropic => self.claude.get_auth_headers(auth),
            AuthStrategy::Google | AuthStrategy::GoogleOAuth => self.gemini.get_auth_headers(auth),
            _ => {
                let bearer = format!("Bearer {}", auth.api_key);
                Ok(vec![(
                    HeaderName::from_static("authorization"),
                    auth_header_value(&bearer)?,
                )])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;
    use serde_json::json;

    fn provider(api: &str, base_url: &str) -> Provider {
        Provider::with_id(
            "cc-switch-test".to_string(),
            "Test".to_string(),
            json!({
                "name": "Test",
                "baseUrl": base_url,
                "apiKey": "sk-test-key-123456",
                "api": api,
                "models": [{ "id": "model-a" }]
            }),
            None,
        )
    }

    #[test]
    fn reads_pi_native_base_url_and_key() {
        let adapter = PiAdapter::new();
        let provider = provider("anthropic-messages", "https://api.example.com");
        assert_eq!(
            adapter.extract_base_url(&provider).expect("base url"),
            "https://api.example.com"
        );
        let auth = adapter.extract_auth(&provider).expect("auth");
        assert_eq!(auth.api_key, "sk-test-key-123456");
        assert_eq!(auth.strategy, AuthStrategy::Anthropic);
    }

    #[test]
    fn openai_providers_use_bearer_auth() {
        let adapter = PiAdapter::new();
        let provider = provider("openai-completions", "https://api.example.com/v1");
        let auth = adapter.extract_auth(&provider).expect("auth");
        assert_eq!(auth.strategy, AuthStrategy::Bearer);
        let headers = adapter.get_auth_headers(&auth).expect("headers");
        assert_eq!(headers[0].0, HeaderName::from_static("authorization"));
    }

    #[test]
    fn google_providers_use_goog_api_key() {
        let adapter = PiAdapter::new();
        let provider = provider(
            "google-generative-ai",
            "https://generativelanguage.googleapis.com",
        );
        let auth = adapter.extract_auth(&provider).expect("auth");
        assert_eq!(auth.strategy, AuthStrategy::Google);
    }

    #[test]
    fn joining_openai_urls_does_not_double_v1() {
        let adapter = PiAdapter::new();
        assert_eq!(
            adapter.build_url("https://api.example.com/v1", "/chat/completions"),
            "https://api.example.com/v1/chat/completions"
        );
    }
}
