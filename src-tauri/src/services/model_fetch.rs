//! 模型列表获取服务
//!
//! 通过 OpenAI 兼容的 GET /v1/models 端点获取供应商可用模型列表。
//! 主要面向第三方聚合站（硅基流动、OpenRouter 等），以及把 Anthropic
//! 协议挂在兼容子路径上的官方供应商（DeepSeek、Kimi、智谱 GLM 等）。

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, USER_AGENT};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// 获取到的模型信息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchedModel {
    pub id: String,
    pub owned_by: Option<String>,
}

/// 模型列表响应的兼容格式。
///
/// OpenAI 兼容接口和 Anthropic 接口使用 `data` 字段，智谱 OpenAI Responses
/// 接口使用 `models` 字段，此结构同时兼容这两种格式。
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Option<Vec<ModelEntry>>,
    models: Option<Vec<ZhipuModelEntry>>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    id: String,
    owned_by: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZhipuModelEntry {
    slug: String,
}

const FETCH_TIMEOUT_SECS: u64 = 15;
const MAX_REQUEST_HEADERS: usize = 64;
const MAX_HEADER_NAME_BYTES: usize = 256;
const MAX_HEADER_VALUE_BYTES: usize = 16 * 1024;

/// 404/405 响应体截断长度：避免把几十 KB HTML 404 页整页保留到错误串里。
const ERROR_BODY_MAX_CHARS: usize = 512;

/// 已知的「Anthropic 协议兼容子路径」后缀；按长度降序，最长前缀优先匹配。
/// baseURL 命中这些后缀时，候选列表会追加「剥离后缀再拼 /v1/models / /models」的版本。
const KNOWN_COMPAT_SUFFIXES: &[&str] = &[
    "/api/claudecode",
    "/api/anthropic",
    "/apps/anthropic",
    "/api/coding",
    "/claudecode",
    "/anthropic",
    "/step_plan",
    "/coding",
    "/claude",
];

/// 获取供应商的可用模型列表
///
/// 使用 OpenAI 兼容的 GET /v1/models 端点，按候选列表顺序尝试。
pub async fn fetch_models(
    base_url: &str,
    api_key: &str,
    is_full_url: bool,
    models_url_override: Option<&str>,
    user_agent: Option<HeaderValue>,
    api_format: Option<&str>,
    request_headers: Option<&BTreeMap<String, String>>,
) -> Result<Vec<FetchedModel>, String> {
    fetch_models_with_client(
        model_fetch_client(),
        base_url,
        api_key,
        is_full_url,
        models_url_override,
        user_agent,
        api_format,
        request_headers,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn fetch_models_with_client(
    client: reqwest::Client,
    base_url: &str,
    api_key: &str,
    is_full_url: bool,
    models_url_override: Option<&str>,
    user_agent: Option<HeaderValue>,
    api_format: Option<&str>,
    request_headers: Option<&BTreeMap<String, String>>,
) -> Result<Vec<FetchedModel>, String> {
    let candidates = build_models_url_candidates(base_url, is_full_url, models_url_override)?;
    let headers =
        build_model_fetch_headers(api_key, api_format, user_agent.as_ref(), request_headers)?;
    let mut last_err: Option<String> = None;
    let mut known_secrets = vec![api_key.to_string()];
    if let Some(request_headers) = request_headers {
        known_secrets.extend(request_headers.values().cloned());
    }

    for url in &candidates {
        log::debug!(
            "[ModelFetch] Trying endpoint: {}",
            crate::url_for_log_with_secrets(url, &known_secrets)
        );
        let request = client
            .get(url)
            .headers(headers.clone())
            .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS));
        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                return Err(format!("Request failed: {e}"));
            }
        };

        let status = response.status();

        if status.is_success() {
            let resp: ModelsResponse = response
                .json()
                .await
                .map_err(|e| format!("Failed to parse response: {e}"))?;

            let mut models: Vec<FetchedModel> = if let Some(data) = resp.data {
                data.into_iter()
                    .map(|m| FetchedModel {
                        id: m.id,
                        owned_by: m.owned_by,
                    })
                    .collect()
            } else {
                resp.models
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| FetchedModel {
                        id: m.slug,
                        owned_by: None,
                    })
                    .collect()
            };

            models.sort_by(|a, b| a.id.cmp(&b.id));
            return Ok(models);
        }

        if status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED {
            let body = redact_model_fetch_error_body(
                response.text().await.unwrap_or_default(),
                &known_secrets,
            );
            last_err = Some(format!("HTTP {status}: {body}"));
            continue;
        }

        let body = redact_model_fetch_error_body(
            response.text().await.unwrap_or_default(),
            &known_secrets,
        );
        return Err(format!("HTTP {status}: {body}"));
    }

    Err(format!(
        "All candidates failed: {}",
        last_err.unwrap_or_else(|| "no candidates".to_string())
    ))
}

/// Shared outbound client (Pi Option A / Claude fetch-models).
///
/// A saved `global_proxy_url` is applied to this process-wide client at
/// startup and when the setting changes. Empty/unset follows system proxy
/// or direct connect. Candidates stay on the provider URL / `modelsUrl` and
/// are never rewritten to the Claude listen port (`15721`).
fn model_fetch_client() -> reqwest::Client {
    crate::proxy::http_client::get()
}

fn redact_model_fetch_error_body(body: String, known_secrets: &[String]) -> String {
    truncate_body(crate::redact_known_secrets_strict(&body, known_secrets))
}

fn build_model_fetch_headers(
    api_key: &str,
    api_format: Option<&str>,
    user_agent: Option<&HeaderValue>,
    request_headers: Option<&BTreeMap<String, String>>,
) -> Result<HeaderMap, String> {
    let custom_count = request_headers.map_or(0, BTreeMap::len);
    if api_key.is_empty() && custom_count == 0 {
        return Err("API Key or request headers are required to fetch models".to_string());
    }
    if custom_count > MAX_REQUEST_HEADERS {
        return Err(format!(
            "Too many model-fetch request headers (maximum {MAX_REQUEST_HEADERS})"
        ));
    }

    let mut headers = HeaderMap::new();
    if !api_key.is_empty() {
        let (name, value) = match api_format {
            Some("anthropic-messages") => (
                HeaderName::from_static("x-api-key"),
                HeaderValue::from_str(api_key)
                    .map_err(|error| format!("Invalid API Key header value: {error}"))?,
            ),
            Some("google-generative-ai") => (
                HeaderName::from_static("x-goog-api-key"),
                HeaderValue::from_str(api_key)
                    .map_err(|error| format!("Invalid API Key header value: {error}"))?,
            ),
            _ => (
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {api_key}"))
                    .map_err(|error| format!("Invalid API Key header value: {error}"))?,
            ),
        };
        headers.insert(name, value);
    }

    if let Some(user_agent) = user_agent {
        headers.insert(USER_AGENT, user_agent.clone());
    }

    if let Some(request_headers) = request_headers {
        for (raw_name, raw_value) in request_headers {
            let name = raw_name.trim();
            if name.is_empty() || name.len() > MAX_HEADER_NAME_BYTES {
                return Err(format!("Invalid model-fetch header name: {raw_name}"));
            }
            if raw_value.len() > MAX_HEADER_VALUE_BYTES {
                return Err(format!("Model-fetch header value is too large: {name}"));
            }
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| format!("Invalid model-fetch header name {name}: {error}"))?;
            let value = HeaderValue::from_str(raw_value)
                .map_err(|error| format!("Invalid model-fetch header value for {name}: {error}"))?;
            headers.insert(name, value);
        }
    }

    Ok(headers)
}

/// 构造「模型列表端点」的候选 URL 列表
///
/// 候选顺序：
/// 1. `models_url_override` 非空 → 只返回它
/// 2. baseURL 拼 `/v1/models`；若已以版本段 `/v{N}` 结尾（`/v1`、智谱
///    `/api/coding/paas/v4` 等），版本号已在路径里，改拼 `/models`
/// 3. 版本段非 `/v1`（如 `/v4`）时再追加 `/v1/models` 作为兜底次候选
/// 4. 若 baseURL 命中 [`KNOWN_COMPAT_SUFFIXES`]，剥离后缀再拼 `/v1/models`、`/models`
///
/// 结果已去重且保持首次出现顺序。
pub fn build_models_url_candidates(
    base_url: &str,
    is_full_url: bool,
    models_url_override: Option<&str>,
) -> Result<Vec<String>, String> {
    if let Some(raw) = models_url_override {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(vec![trimmed.to_string()]);
        }
    }

    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("Base URL is empty".to_string());
    }

    let mut candidates: Vec<String> = Vec::new();

    if is_full_url {
        if let Some(idx) = trimmed.find("/v1/") {
            candidates.push(format!("{}/v1/models", &trimmed[..idx]));
        } else if let Some(idx) = trimmed.rfind('/') {
            let root = &trimmed[..idx];
            if root.contains("://") && root.len() > root.find("://").unwrap() + 3 {
                candidates.push(format!("{root}/v1/models"));
            }
        }
        if candidates.is_empty() {
            return Err("Cannot derive models endpoint from full URL".to_string());
        }
        return Ok(candidates);
    }

    // baseURL 已以版本段 /v{N} 结尾时（如 `/v1`、智谱 `/api/coding/paas/v4`），
    // OpenAI 惯例的模型端点是 `{base}/models`，不能再补 `/v1`
    // （否则 .../coding/paas/v4/v1/models → 404）。
    if ends_with_version_segment(trimmed) {
        candidates.push(format!("{trimmed}/models"));
        // 版本段非 /v1 时，保留旧的 /v1/models 作为兜底次候选（正确路径已在前）。
        if !trimmed.ends_with("/v1") {
            candidates.push(format!("{trimmed}/v1/models"));
        }
    } else {
        candidates.push(format!("{trimmed}/v1/models"));
    }

    if let Some(stripped) = strip_compat_suffix(trimmed) {
        let root = stripped.trim_end_matches('/');
        if !root.is_empty() && root.contains("://") {
            candidates.push(format!("{root}/v1/models"));
            candidates.push(format!("{root}/models"));
        }
    }

    // 候选最多 3 条，线性去重即可，不值得上 HashSet。
    let mut unique: Vec<String> = Vec::with_capacity(candidates.len());
    for url in candidates {
        if !unique.iter().any(|u| u == &url) {
            unique.push(url);
        }
    }

    Ok(unique)
}

/// 截断响应体到 [`ERROR_BODY_MAX_CHARS`] 字符，避免 HTML 404 页占用错误串。
fn truncate_body(body: String) -> String {
    if body.chars().count() <= ERROR_BODY_MAX_CHARS {
        body
    } else {
        let mut s: String = body.chars().take(ERROR_BODY_MAX_CHARS).collect();
        s.push('…');
        s
    }
}

/// 若 baseURL 以任一已知兼容子路径结尾，返回剥离后的剩余部分；否则 `None`。
///
/// 依赖 [`KNOWN_COMPAT_SUFFIXES`] 按长度降序排列，确保最长前缀优先命中
/// （否则 `/anthropic` 会提前匹配掉 `/api/anthropic` 的场景）。
fn strip_compat_suffix(base_url: &str) -> Option<&str> {
    for suffix in KNOWN_COMPAT_SUFFIXES {
        if base_url.ends_with(*suffix) {
            return Some(&base_url[..base_url.len() - suffix.len()]);
        }
    }
    None
}

/// 判断 baseURL 是否以 OpenAI 风格的版本段 `/v{N}` 结尾（`N` 为一个或多个数字），
/// 例如 `/v1`、`.../paas/v4`。这类 URL 版本号已在路径中，模型端点应为
/// `{base}/models`，不能再补 `/v1`（智谱 Coding Plan 即 `.../coding/paas/v4`）。
fn ends_with_version_segment(url: &str) -> bool {
    let last = url.rsplit('/').next().unwrap_or("");
    last.strip_prefix('v')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_fetch_headers_follow_pi_api_format() {
        let anthropic =
            build_model_fetch_headers("anthropic-key", Some("anthropic-messages"), None, None)
                .unwrap();
        assert_eq!(anthropic["x-api-key"], "anthropic-key");
        assert!(!anthropic.contains_key(AUTHORIZATION));

        let google =
            build_model_fetch_headers("google-key", Some("google-generative-ai"), None, None)
                .unwrap();
        assert_eq!(google["x-goog-api-key"], "google-key");
        assert!(!google.contains_key(AUTHORIZATION));

        let openai =
            build_model_fetch_headers("openai-key", Some("openai-responses"), None, None).unwrap();
        assert_eq!(openai[AUTHORIZATION], "Bearer openai-key");
    }

    #[test]
    fn model_fetch_headers_allow_validated_header_only_auth_and_overrides() {
        let custom = BTreeMap::from([
            ("Authorization".to_string(), "Token literal".to_string()),
            ("X-Tenant".to_string(), "tenant-a".to_string()),
        ]);
        let headers =
            build_model_fetch_headers("", Some("openai-completions"), None, Some(&custom)).unwrap();
        assert_eq!(headers[AUTHORIZATION], "Token literal");
        assert_eq!(headers["x-tenant"], "tenant-a");

        let override_default =
            BTreeMap::from([("x-api-key".to_string(), "header-managed-key".to_string())]);
        let headers = build_model_fetch_headers(
            "provider-key",
            Some("anthropic-messages"),
            None,
            Some(&override_default),
        )
        .unwrap();
        assert_eq!(headers["x-api-key"], "header-managed-key");
    }

    #[test]
    fn model_fetch_headers_reject_invalid_or_missing_credentials() {
        assert!(build_model_fetch_headers("", None, None, None).is_err());
        let invalid = BTreeMap::from([("bad header".to_string(), "literal-value".to_string())]);
        assert!(build_model_fetch_headers("", None, None, Some(&invalid)).is_err());
    }

    #[test]
    fn model_fetch_error_body_redacts_known_header_credentials() {
        let secrets = vec![
            "short".to_string(),
            "Bearer literal-header-secret".to_string(),
        ];
        let body = redact_model_fetch_error_body(
            "invalid short / Bearer literal-header-secret".to_string(),
            &secrets,
        );
        assert_eq!(body, "invalid [REDACTED] / [REDACTED]");
    }

    #[test]
    fn test_candidates_plain_root() {
        let c = build_models_url_candidates("https://api.siliconflow.cn", false, None).unwrap();
        assert_eq!(c, vec!["https://api.siliconflow.cn/v1/models"]);
    }

    #[test]
    fn test_candidates_trailing_slash() {
        let c = build_models_url_candidates("https://api.example.com/", false, None).unwrap();
        assert_eq!(c, vec!["https://api.example.com/v1/models"]);
    }

    #[test]
    fn test_candidates_with_v1() {
        let c = build_models_url_candidates("https://api.example.com/v1", false, None).unwrap();
        assert_eq!(c, vec!["https://api.example.com/v1/models"]);
    }

    #[test]
    fn test_candidates_zhipu_coding_paas_v4() {
        // 智谱 Coding Plan 端点以 /v4 版本段结尾：模型端点是 {base}/models，
        // 正确路径必须排在 .../v4/v1/models（404）之前。
        let c =
            build_models_url_candidates("https://open.bigmodel.cn/api/coding/paas/v4", false, None)
                .unwrap();
        assert_eq!(
            c,
            vec![
                "https://open.bigmodel.cn/api/coding/paas/v4/models",
                "https://open.bigmodel.cn/api/coding/paas/v4/v1/models",
            ]
        );
    }

    #[test]
    fn test_candidates_zai_coding_paas_v4() {
        let c = build_models_url_candidates("https://api.z.ai/api/coding/paas/v4", false, None)
            .unwrap();
        assert_eq!(
            c,
            vec![
                "https://api.z.ai/api/coding/paas/v4/models",
                "https://api.z.ai/api/coding/paas/v4/v1/models",
            ]
        );
    }

    #[test]
    fn test_ends_with_version_segment() {
        assert!(ends_with_version_segment("https://x.com/v1"));
        assert!(ends_with_version_segment(
            "https://open.bigmodel.cn/api/coding/paas/v4"
        ));
        assert!(ends_with_version_segment("https://x.com/v10"));
        assert!(!ends_with_version_segment("https://x.com/api"));
        assert!(!ends_with_version_segment("https://x.com/vX"));
        assert!(!ends_with_version_segment("https://x.com/models"));
        assert!(!ends_with_version_segment("https://api.siliconflow.cn"));
    }

    #[test]
    fn test_candidates_full_url() {
        let c = build_models_url_candidates(
            "https://proxy.example.com/v1/chat/completions",
            true,
            None,
        )
        .unwrap();
        assert_eq!(c, vec!["https://proxy.example.com/v1/models"]);
    }

    #[test]
    fn test_candidates_empty() {
        assert!(build_models_url_candidates("", false, None).is_err());
    }

    #[test]
    fn test_candidates_override_returns_single() {
        let c = build_models_url_candidates(
            "https://api.deepseek.com/anthropic",
            false,
            Some("https://api.deepseek.com/models"),
        )
        .unwrap();
        assert_eq!(c, vec!["https://api.deepseek.com/models"]);
    }

    #[test]
    fn test_candidates_override_empty_falls_through() {
        let c =
            build_models_url_candidates("https://api.siliconflow.cn", false, Some("   ")).unwrap();
        assert_eq!(c, vec!["https://api.siliconflow.cn/v1/models"]);
    }

    #[test]
    fn test_candidates_deepseek_strip_anthropic() {
        let c =
            build_models_url_candidates("https://api.deepseek.com/anthropic", false, None).unwrap();
        assert_eq!(
            c,
            vec![
                "https://api.deepseek.com/anthropic/v1/models",
                "https://api.deepseek.com/v1/models",
                "https://api.deepseek.com/models",
            ]
        );
    }

    #[test]
    fn test_candidates_zhipu_strip_api_anthropic() {
        let c = build_models_url_candidates("https://open.bigmodel.cn/api/anthropic", false, None)
            .unwrap();
        assert_eq!(
            c,
            vec![
                "https://open.bigmodel.cn/api/anthropic/v1/models",
                "https://open.bigmodel.cn/v1/models",
                "https://open.bigmodel.cn/models",
            ]
        );
    }

    #[test]
    fn test_candidates_bailian_strip_apps_anthropic() {
        let c = build_models_url_candidates(
            "https://dashscope.aliyuncs.com/apps/anthropic",
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            c,
            vec![
                "https://dashscope.aliyuncs.com/apps/anthropic/v1/models",
                "https://dashscope.aliyuncs.com/v1/models",
                "https://dashscope.aliyuncs.com/models",
            ]
        );
    }

    #[test]
    fn test_candidates_stepfun_strip_step_plan() {
        let c =
            build_models_url_candidates("https://api.stepfun.com/step_plan", false, None).unwrap();
        assert_eq!(
            c,
            vec![
                "https://api.stepfun.com/step_plan/v1/models",
                "https://api.stepfun.com/v1/models",
                "https://api.stepfun.com/models",
            ]
        );
    }

    #[test]
    fn test_candidates_doubao_strip_api_coding() {
        let c = build_models_url_candidates(
            "https://ark.cn-beijing.volces.com/api/coding",
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            c,
            vec![
                "https://ark.cn-beijing.volces.com/api/coding/v1/models",
                "https://ark.cn-beijing.volces.com/v1/models",
                "https://ark.cn-beijing.volces.com/models",
            ]
        );
    }

    #[test]
    fn test_candidates_rightcode_strip_claude() {
        let c = build_models_url_candidates("https://www.right.codes/claude", false, None).unwrap();
        assert_eq!(
            c,
            vec![
                "https://www.right.codes/claude/v1/models",
                "https://www.right.codes/v1/models",
                "https://www.right.codes/models",
            ]
        );
    }

    #[test]
    fn test_candidates_longer_suffix_wins() {
        // baseURL 以 /api/anthropic 结尾时，应剥离整个 /api/anthropic，
        // 而不是只剥离 /anthropic（那样会得到残缺的 https://.../api 根）。
        let c = build_models_url_candidates("https://api.z.ai/api/anthropic", false, None).unwrap();
        assert_eq!(
            c,
            vec![
                "https://api.z.ai/api/anthropic/v1/models",
                "https://api.z.ai/v1/models",
                "https://api.z.ai/models",
            ]
        );
    }

    #[test]
    fn test_candidates_no_suffix_no_strip() {
        let c = build_models_url_candidates("https://openrouter.ai/api", false, None).unwrap();
        assert_eq!(c, vec!["https://openrouter.ai/api/v1/models"]);
    }

    #[test]
    fn test_candidates_deduplicate() {
        // 虚构 case：baseURL 就是 "scheme://host"，剥不出子路径，应只有一个候选。
        let c = build_models_url_candidates("https://host.example.com", false, None).unwrap();
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn test_parse_response() {
        let json = r#"{"object":"list","data":[{"id":"gpt-4","object":"model","owned_by":"openai"},{"id":"claude-3-sonnet","object":"model","owned_by":"anthropic"}]}"#;
        let resp: ModelsResponse = serde_json::from_str(json).unwrap();
        let data = resp.data.unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0].id, "gpt-4");
        assert_eq!(data[0].owned_by.as_deref(), Some("openai"));
        assert_eq!(data[1].id, "claude-3-sonnet");
    }

    #[test]
    fn test_parse_response_no_owned_by() {
        let json = r#"{"object":"list","data":[{"id":"my-model","object":"model"}]}"#;
        let resp: ModelsResponse = serde_json::from_str(json).unwrap();
        let data = resp.data.unwrap();
        assert_eq!(data[0].id, "my-model");
        assert!(data[0].owned_by.is_none());
    }

    #[test]
    fn test_parse_response_empty_data() {
        let json = r#"{"object":"list","data":[]}"#;
        let resp: ModelsResponse = serde_json::from_str(json).unwrap();
        assert!(resp.data.unwrap().is_empty());
    }

    #[test]
    fn fetch_models_candidates_do_not_rewrite_to_claude_listen() {
        let c = build_models_url_candidates(
            "https://api.llm.prd.yumc.local/v1",
            false,
            Some("https://api.deepseek.com/models"),
        )
        .unwrap();
        assert_eq!(c, vec!["https://api.deepseek.com/models"]);
        assert!(
            c.iter()
                .all(|url| !url.contains("15721") && !url.contains("127.0.0.1")),
            "Option A must keep the provider/modelsUrl host, not Claude listen: {c:?}"
        );
    }

    /// Claude / Codex / Pi 预设形态的候选 URL 矩阵。任何一条都不得改写到 Claude listen 15721。
    #[test]
    fn fetch_models_candidate_matrix_for_claude_codex_pi_never_rewrites_15721() {
        struct Case {
            name: &'static str,
            base_url: &'static str,
            is_full_url: bool,
            models_url: Option<&'static str>,
            expected: &'static [&'static str],
        }

        let cases = [
            Case {
                name: "claude-official",
                base_url: "https://api.anthropic.com",
                is_full_url: false,
                models_url: None,
                expected: &["https://api.anthropic.com/v1/models"],
            },
            Case {
                name: "claude-deepseek-modelsUrl",
                base_url: "https://api.deepseek.com/anthropic",
                is_full_url: false,
                models_url: Some("https://api.deepseek.com/models"),
                expected: &["https://api.deepseek.com/models"],
            },
            Case {
                name: "claude-full-url-messages",
                base_url: "https://api.anthropic.com/v1/messages",
                is_full_url: true,
                models_url: None,
                expected: &["https://api.anthropic.com/v1/models"],
            },
            Case {
                name: "claude-tencent-plan-modelsUrl",
                base_url: "https://api.lkeap.cloud.tencent.com/plan/anthropic",
                is_full_url: false,
                models_url: Some("https://api.lkeap.cloud.tencent.com/plan/v3/models"),
                expected: &["https://api.lkeap.cloud.tencent.com/plan/v3/models"],
            },
            Case {
                name: "codex-openai-v1",
                base_url: "https://api.openai.com/v1",
                is_full_url: false,
                models_url: None,
                expected: &["https://api.openai.com/v1/models"],
            },
            Case {
                name: "codex-full-url-responses",
                base_url: "https://api.openai.com/v1/responses",
                is_full_url: true,
                models_url: None,
                expected: &["https://api.openai.com/v1/models"],
            },
            Case {
                name: "codex-full-url-chat-completions",
                base_url: "https://api.openai.com/v1/chat/completions",
                is_full_url: true,
                models_url: None,
                expected: &["https://api.openai.com/v1/models"],
            },
            Case {
                name: "pi-deepseek-aggregator",
                base_url: "https://api.deepseek.com/v1",
                is_full_url: false,
                models_url: Some("https://api.deepseek.com/models"),
                expected: &["https://api.deepseek.com/models"],
            },
            Case {
                name: "pi-ppio-modelsUrl",
                base_url: "https://api.ppio.com/openai/v1",
                is_full_url: false,
                models_url: Some("https://api.ppio.com/openai/v1/models"),
                expected: &["https://api.ppio.com/openai/v1/models"],
            },
            Case {
                name: "pi-novita-modelsUrl",
                base_url: "https://api.novita.ai/openai/v1",
                is_full_url: false,
                models_url: Some("https://api.novita.ai/openai/v1/models"),
                expected: &["https://api.novita.ai/openai/v1/models"],
            },
            Case {
                name: "pi-openai-completions-v1",
                base_url: "https://api.llm.prd.yumc.local/v1",
                is_full_url: false,
                models_url: None,
                expected: &["https://api.llm.prd.yumc.local/v1/models"],
            },
            Case {
                name: "pi-full-url-chat-completions",
                base_url: "https://api.llm.prd.yumc.local/v1/chat/completions",
                is_full_url: true,
                models_url: None,
                expected: &["https://api.llm.prd.yumc.local/v1/models"],
            },
        ];

        for case in cases {
            let got = build_models_url_candidates(case.base_url, case.is_full_url, case.models_url)
                .unwrap_or_else(|e| panic!("{}: {e}", case.name));
            assert_eq!(
                got,
                case.expected
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
                "{}",
                case.name
            );
            assert!(
                got.iter()
                    .all(|url| !url.contains("15721") && !url.contains("127.0.0.1:15721")),
                "{} must never rewrite to Claude listen 15721: {got:?}",
                case.name
            );
        }
    }

    #[derive(Clone, Copy)]
    enum MockMode {
        Origin,
        HttpProxy,
        Hang,
    }

    struct CapturingServer {
        url: String,
        hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        captured: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        shutdown: std::sync::mpsc::Sender<()>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for CapturingServer {
        fn drop(&mut self) {
            let _ = self.shutdown.send(());
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    impl CapturingServer {
        fn last_raw(&self) -> String {
            self.captured
                .lock()
                .expect("capture lock")
                .last()
                .cloned()
                .unwrap_or_default()
        }

        fn header(&self, name: &str) -> Option<String> {
            let raw = self.last_raw();
            let needle = format!("{}:", name.to_ascii_lowercase());
            raw.lines().find_map(|line| {
                let lower = line.to_ascii_lowercase();
                lower
                    .starts_with(&needle)
                    .then(|| line.split_once(':').map(|(_, v)| v.trim().to_string()))
                    .flatten()
            })
        }

        fn request_target(&self) -> String {
            self.last_raw()
                .lines()
                .next()
                .unwrap_or("")
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .to_string()
        }
    }

    fn http_json_response(status: u16, reason: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn spawn_capturing_server(mode: MockMode) -> CapturingServer {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock http");
        listener
            .set_nonblocking(true)
            .expect("nonblocking mock http");
        let port = listener.local_addr().expect("mock addr").port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let hits_clone = hits.clone();
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_clone = captured.clone();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
        let ok_body = r#"{"object":"list","data":[{"id":"glm-5.1","owned_by":"test"}]}"#;
        let handle = std::thread::spawn(move || {
            loop {
                if shutdown_rx.try_recv().is_ok() {
                    break;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Windows inherits nonblocking from the listener; reqwest then
                        // sees "error sending request" if write_all WouldBlock-drops.
                        let _ = stream.set_nonblocking(false);
                        hits_clone.fetch_add(1, Ordering::SeqCst);
                        if matches!(mode, MockMode::Hang) {
                            // 保持连接但不写响应，让 fetch_models 的 15s 请求超时生效。
                            for _ in 0..200 {
                                if shutdown_rx.try_recv().is_ok() {
                                    return;
                                }
                                std::thread::sleep(Duration::from_millis(100));
                            }
                            continue;
                        }
                        let mut buf = [0u8; 8192];
                        let n = stream.read(&mut buf).unwrap_or(0);
                        let mut raw = String::from_utf8_lossy(&buf[..n]).into_owned();
                        let first = raw.lines().next().unwrap_or("").to_string();
                        let method = first.split_whitespace().next().unwrap_or("");
                        if method.eq_ignore_ascii_case("CONNECT") {
                            let _ =
                                stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
                            let n2 = stream.read(&mut buf).unwrap_or(0);
                            raw.push_str(&String::from_utf8_lossy(&buf[..n2]));
                        }
                        captured_clone.lock().expect("capture").push(raw.clone());
                        assert!(
                            !raw.contains("15721"),
                            "captured request must not target Claude listen 15721: {raw}"
                        );
                        let target = raw
                            .lines()
                            .find(|line| {
                                let m = line.split_whitespace().next().unwrap_or("");
                                m.eq_ignore_ascii_case("GET")
                            })
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or("");
                        let response = if target.contains("/missing") {
                            http_json_response(404, "Not Found", r#"{"error":"missing"}"#)
                        } else if target.contains("/unauthorized") {
                            http_json_response(401, "Unauthorized", r#"{"error":"nope"}"#)
                        } else {
                            http_json_response(200, "OK", ok_body)
                        };
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.flush();
                    }
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        });
        CapturingServer {
            url: format!("http://127.0.0.1:{port}"),
            hits,
            captured,
            shutdown: shutdown_tx,
            handle: Some(handle),
        }
    }

    fn client_for_optional_proxy(proxy_url: Option<&str>) -> reqwest::Client {
        let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(5));
        builder = match proxy_url {
            Some(url) => builder.proxy(reqwest::Proxy::all(url).expect("proxy url")),
            None => builder.no_proxy(),
        };
        builder.build().expect("build isolated fetch-models client")
    }

    fn assert_not_claude_listen(value: &str) {
        assert!(
            !value.contains("15721"),
            "must never rewrite to Claude listen 15721: {value}"
        );
    }

    #[tokio::test]
    async fn fetch_models_respects_configured_global_proxy_url() {
        use std::sync::atomic::Ordering;

        // Isolated clients only — never apply_proxy on GLOBAL_CLIENT (races --lib).
        // Production fetch_models() uses model_fetch_client() → http_client::get()
        // after init/apply_proxy from global_proxy_url.
        let origin = spawn_capturing_server(MockMode::Origin);
        let dead_listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("reserve dead proxy port");
        let dead_port = dead_listener.local_addr().expect("dead proxy addr").port();
        drop(dead_listener);
        let dead_proxy = format!("http://127.0.0.1:{dead_port}");
        assert_not_claude_listen(&origin.url);
        assert_not_claude_listen(&dead_proxy);

        let err = fetch_models_with_client(
            client_for_optional_proxy(Some(&dead_proxy)),
            &origin.url,
            "test-key",
            false,
            None,
            None,
            None,
            None,
        )
        .await
        .expect_err("dead global_proxy_url-style client must fail the fetch");
        assert!(
            err.contains("Request failed"),
            "fetch-models must go through the configured proxy: {err}"
        );
        assert_eq!(
            origin.hits.load(Ordering::SeqCst),
            0,
            "origin must not be reached when a proxy is configured"
        );

        let models = fetch_models_with_client(
            client_for_optional_proxy(None),
            &origin.url,
            "test-key",
            false,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("direct fetch-models after clearing proxy");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "glm-5.1");
        assert!(
            origin.hits.load(Ordering::SeqCst) >= 1,
            "cleared proxy must reach the provider URL, not 15721"
        );
        assert_not_claude_listen(&origin.request_target());
    }

    #[tokio::test]
    async fn fetch_models_working_proxy_records_request_and_skips_origin() {
        use std::sync::atomic::Ordering;

        let origin = spawn_capturing_server(MockMode::Origin);
        let proxy = spawn_capturing_server(MockMode::HttpProxy);
        assert_not_claude_listen(&origin.url);
        assert_not_claude_listen(&proxy.url);

        let models = fetch_models_with_client(
            client_for_optional_proxy(Some(&proxy.url)),
            &origin.url,
            "proxy-key",
            false,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("working HTTP proxy should serve models");
        assert_eq!(models[0].id, "glm-5.1");
        assert_eq!(
            origin.hits.load(Ordering::SeqCst),
            0,
            "recording proxy answers itself; origin must stay at 0 hits"
        );
        assert!(
            proxy.hits.load(Ordering::SeqCst) >= 1,
            "request must go through the configured proxy"
        );
        let target = proxy.request_target();
        assert!(
            target.contains(&origin.url) || target.contains("/v1/models"),
            "proxy target should be the provider models URL, not 15721: {target}"
        );
        assert_not_claude_listen(&target);
        assert_eq!(
            proxy.header("authorization").as_deref(),
            Some("Bearer proxy-key")
        );
    }

    #[tokio::test]
    async fn fetch_models_auth_header_matrix_for_claude_codex_pi() {
        struct Case {
            name: &'static str,
            api_key: &'static str,
            api_format: Option<&'static str>,
            request_headers: Option<BTreeMap<String, String>>,
            expect_name: &'static str,
            expect_value: &'static str,
            forbid: Option<&'static str>,
        }

        let cases = [
            Case {
                name: "claude-anthropic-messages",
                api_key: "sk-ant-test",
                api_format: Some("anthropic-messages"),
                request_headers: None,
                expect_name: "x-api-key",
                expect_value: "sk-ant-test",
                forbid: Some("authorization"),
            },
            Case {
                name: "codex-openai-responses",
                api_key: "sk-codex",
                api_format: Some("openai-responses"),
                request_headers: None,
                expect_name: "authorization",
                expect_value: "Bearer sk-codex",
                forbid: Some("x-api-key"),
            },
            Case {
                name: "pi-aggregator-default-bearer",
                api_key: "sk-pi-agg",
                api_format: None,
                request_headers: None,
                expect_name: "authorization",
                expect_value: "Bearer sk-pi-agg",
                forbid: Some("x-api-key"),
            },
            Case {
                name: "pi-header-only-token",
                api_key: "",
                api_format: Some("openai-completions"),
                request_headers: Some(BTreeMap::from([(
                    "Authorization".to_string(),
                    "Token literal-header".to_string(),
                )])),
                expect_name: "authorization",
                expect_value: "Token literal-header",
                forbid: None,
            },
            Case {
                name: "google-generative-ai",
                api_key: "goog-key",
                api_format: Some("google-generative-ai"),
                request_headers: None,
                expect_name: "x-goog-api-key",
                expect_value: "goog-key",
                forbid: Some("authorization"),
            },
        ];

        for case in cases {
            let origin = spawn_capturing_server(MockMode::Origin);
            let models = fetch_models_with_client(
                client_for_optional_proxy(None),
                &origin.url,
                case.api_key,
                false,
                None,
                None,
                case.api_format,
                case.request_headers.as_ref(),
            )
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
            assert_eq!(models[0].id, "glm-5.1", "{}", case.name);
            assert_eq!(
                origin.header(case.expect_name).as_deref(),
                Some(case.expect_value),
                "{}",
                case.name
            );
            if let Some(forbid) = case.forbid {
                assert!(
                    origin.header(forbid).is_none(),
                    "{} must not send {forbid}: {}",
                    case.name,
                    origin.last_raw()
                );
            }
            assert_not_claude_listen(&origin.request_target());
        }
    }

    #[tokio::test]
    async fn fetch_models_is_full_url_and_models_url_hit_derived_paths() {
        let origin = spawn_capturing_server(MockMode::Origin);

        // isFullUrl：从 /v1/chat/completions 推导 /v1/models，不改写 15721。
        let full_url = format!("{}/v1/chat/completions", origin.url);
        let models = fetch_models_with_client(
            client_for_optional_proxy(None),
            &full_url,
            "full-url-key",
            true,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("isFullUrl fetch");
        assert_eq!(models[0].id, "glm-5.1");
        let target = origin.request_target();
        assert!(
            target.ends_with("/v1/models") || target.contains("/v1/models"),
            "isFullUrl should request /v1/models: {target}"
        );
        assert!(!target.contains("chat/completions"), "{target}");
        assert_not_claude_listen(&target);

        // modelsUrl 覆写：只打精确路径（Pi DeepSeek / 腾讯 Token Plan 形态）。
        let origin2 = spawn_capturing_server(MockMode::Origin);
        let models_url = format!("{}/plan/v3/models", origin2.url);
        let models = fetch_models_with_client(
            client_for_optional_proxy(None),
            &format!("{}/plan/anthropic", origin2.url),
            "plan-key",
            false,
            Some(models_url.as_str()),
            None,
            Some("anthropic-messages"),
            None,
        )
        .await
        .expect("modelsUrl override fetch");
        assert_eq!(models[0].id, "glm-5.1");
        let target = origin2.request_target();
        assert!(
            target.contains("/plan/v3/models"),
            "modelsUrl must win: {target}"
        );
        assert!(
            !target.contains("/anthropic/"),
            "must not fall back to baseURL candidates when modelsUrl is set: {target}"
        );
        assert_not_claude_listen(&target);
        assert_eq!(origin2.header("x-api-key").as_deref(), Some("plan-key"));
    }

    #[tokio::test]
    async fn fetch_models_hanging_origin_times_out() {
        let origin = spawn_capturing_server(MockMode::Hang);
        let started = std::time::Instant::now();
        let err = fetch_models_with_client(
            client_for_optional_proxy(None),
            &origin.url,
            "timeout-key",
            false,
            None,
            None,
            None,
            None,
        )
        .await
        .expect_err("hanging origin must time out");
        let elapsed = started.elapsed();
        assert!(
            err.contains("Request failed"),
            "expected request failure on hanging origin, got: {err}"
        );
        assert!(
            elapsed >= std::time::Duration::from_secs(14),
            "FETCH_TIMEOUT_SECS is 15s (reqwest Display may omit the word timeout): {elapsed:?} {err}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(25),
            "timeout should not wait for the client-level 600s: {elapsed:?}"
        );
        assert_not_claude_listen(&origin.url);
    }
}
