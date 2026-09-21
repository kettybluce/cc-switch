//! Shape-based secret redaction for logs, request-log rows, and diagnostic UI.
//!
//! Known-value redaction (`redact_known_secrets`) is preferred when we hold the
//! exact credential. This module is the fallback for bodies, error text, and
//! Display output where the key is echoed but not passed in as a known value.

use regex::Regex;
use std::sync::LazyLock;

static URL_CREDENTIAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(https?://)[^/@\s]+@").expect("url credential regex"));

static SENSITIVE_HEADER_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?im)(^|[\r\n])([ \t]*(?:(?:proxy-)?authorization|cookie|set-cookie|x-api-key|api-key|x-goog-api-key)\s*[:=]\s*)[^\r\n]+",
    )
    .expect("sensitive header regex")
});

static AUTH_SCHEME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(Bearer|Basic|Token|ApiKey|Digest|Negotiate|AWS4-HMAC-SHA256)\s+[^\s"',}\]]+"#,
    )
    .expect("auth scheme regex")
});

static SECRET_SHAPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(^|[^A-Za-z0-9])(?:sk-[A-Za-z0-9._~+/=-]{6,}|AIza[A-Za-z0-9_-]{8,}|github_pat_[A-Za-z0-9_]{6,}|gh[pousr]_[A-Za-z0-9_]{6,}|xox[baprs]-[A-Za-z0-9-]{6,}|ya29\.[A-Za-z0-9._-]{6,}|(?:AKIA|ASIA)[A-Z0-9]{12,}|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)",
    )
    .expect("secret shape regex")
});

static NAMED_SECRET_CONTAINER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)((?:["']?\b(?:api[_-]?key|access[_-]?key|secret[_-]?key|private[_-]?key|client[_-]?secret|auth[_-]?token|access[_-]?token|refresh[_-]?token|id[_-]?token|session[_-]?token|authorization|credential|password|passwd|bearer|cookie|secret|token|auth|pwd)s?["']?\s*[:=]\s*))(\[[^\]]*\]|\{[^{}]*\})"#,
    )
    .expect("named secret container regex")
});

static QUOTED_NAMED_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)((?:api[_-]?key|access[_-]?token|refresh[_-]?token|token|authorization|auth|password|passwd|pwd|secret|cookie)\s*["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*')"#,
    )
    .expect("quoted named secret regex")
});

static NAMED_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)((?:api[_-]?key|access[_-]?token|refresh[_-]?token|token|authorization|auth|password|passwd|pwd|secret|cookie)\s*["']?\s*[:=]\s*["']?)([^\s"',}]+)"#,
    )
    .expect("named secret regex")
});

/// Replace credential-shaped values in text destined for logs or request-log rows.
pub(crate) fn redact_secret_text(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }

    let mut output = SENSITIVE_HEADER_LINE
        .replace_all(input, "${1}${2}[REDACTED]")
        .into_owned();
    output = URL_CREDENTIAL
        .replace_all(&output, "${1}[REDACTED]@")
        .into_owned();
    output = AUTH_SCHEME
        .replace_all(&output, "${1} [REDACTED]")
        .into_owned();
    output = SECRET_SHAPE
        .replace_all(&output, "${1}[REDACTED]")
        .into_owned();
    output = NAMED_SECRET_CONTAINER
        .replace_all(&output, "${1}[REDACTED]")
        .into_owned();
    output = QUOTED_NAMED_SECRET
        .replace_all(&output, "${1}[REDACTED]")
        .into_owned();
    NAMED_SECRET
        .replace_all(&output, "${1}[REDACTED]")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::redact_secret_text;

    const CLAUDE_KEY: &str = "sk-ant-api03-TESTSECRETVALUE99xxxx";
    const CODEX_KEY: &str = "sk-codex-test-secret-aaaaaaa";
    const GEMINI_KEY: &str = "AIzaSyTestSecretValue99xxxx";

    #[test]
    fn redacts_anthropic_openai_and_google_key_shapes() {
        let input = format!(
            "claude={CLAUDE_KEY} codex={CODEX_KEY} gemini={GEMINI_KEY} model=gemini-2.5-pro"
        );
        let redacted = redact_secret_text(&input);
        assert!(!redacted.contains(CLAUDE_KEY), "{redacted}");
        assert!(!redacted.contains(CODEX_KEY), "{redacted}");
        assert!(!redacted.contains(GEMINI_KEY), "{redacted}");
        assert!(!redacted.contains("TESTSECRETVALUE99"), "{redacted}");
        assert!(redacted.contains("[REDACTED]"), "{redacted}");
        assert!(redacted.contains("gemini-2.5-pro"), "{redacted}");
    }

    #[test]
    fn redacts_bearer_headers_and_named_api_keys() {
        let input = concat!(
            "Authorization: Bearer sk-ant-api03-TESTSECRETVALUE99xxxx\n",
            r#"{"api_key":"sk-codex-test-secret-aaaaaaa","model":"gpt-5.4"}"#,
            "\nx-api-key: sk-ant-api03-TESTSECRETVALUE99xxxx"
        );
        let redacted = redact_secret_text(input);
        assert!(!redacted.contains("TESTSECRETVALUE99"), "{redacted}");
        assert!(
            !redacted.contains("sk-codex-test-secret-aaaaaaa"),
            "{redacted}"
        );
        assert!(redacted.contains("gpt-5.4"), "{redacted}");
        assert!(redacted.contains("[REDACTED]"), "{redacted}");
    }

    #[test]
    fn redacts_url_userinfo_without_dropping_host() {
        let redacted = redact_secret_text(
            "https://user:sk-ant-api03-TESTSECRETVALUE99xxxx@api.example.com/v1",
        );
        assert!(!redacted.contains("TESTSECRETVALUE99"), "{redacted}");
        assert!(redacted.contains("api.example.com"), "{redacted}");
    }

    #[test]
    fn does_not_redact_ordinary_provider_errors() {
        let input = "rate limit exceeded for provider anthropic";
        assert_eq!(redact_secret_text(input), input);
    }
}
