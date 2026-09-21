use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::fmt;
use thiserror::Error;

#[derive(Error)]
pub enum ProxyError {
    #[error("上游响应体超过大小上限: {0} 字节")]
    ResponseBodyTooLarge(usize),

    #[error("服务器已在运行")]
    AlreadyRunning,

    #[error("服务器未运行")]
    NotRunning,

    #[error("地址绑定失败: {0}")]
    BindFailed(String),

    #[error("停止超时")]
    StopTimeout,

    #[error("停止失败: {0}")]
    StopFailed(String),

    #[error("请求转发失败: {0}")]
    ForwardFailed(String),

    #[error("无可用的Provider")]
    NoAvailableProvider,

    #[error("所有供应商已熔断，无可用渠道")]
    AllProvidersCircuitOpen,

    #[error("未配置供应商")]
    NoProvidersConfigured,

    #[allow(dead_code)]
    #[error("Provider不健康: {0}")]
    ProviderUnhealthy(String),

    /// Display omits the raw body; logs and request-log rows must use
    /// `get_error_message` / `summarize_proxy_error`, which redact first.
    #[error("上游错误 (状态码 {status})")]
    UpstreamError { status: u16, body: Option<String> },

    #[error("超过最大重试次数")]
    MaxRetriesExceeded,

    #[error("数据库错误: {0}")]
    DatabaseError(String),

    #[error("配置错误: {0}")]
    ConfigError(String),

    #[allow(dead_code)]
    #[error("格式转换错误: {0}")]
    TransformError(String),

    #[allow(dead_code)]
    #[error("无效的请求: {0}")]
    InvalidRequest(String),

    #[error("超时: {0}")]
    Timeout(String),

    /// 流式响应空闲超时
    #[allow(dead_code)]
    #[error("流式响应空闲超时: {0}秒无数据")]
    StreamIdleTimeout(u64),

    /// 认证错误
    #[error("认证失败: {0}")]
    AuthError(String),

    #[allow(dead_code)]
    #[error("内部错误: {0}")]
    Internal(String),
}

impl fmt::Debug for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UpstreamError { status, body } => f
                .debug_struct("UpstreamError")
                .field("status", status)
                .field("body", &body.as_deref().map(crate::redact_secret_text))
                .finish(),
            other => write!(f, "{}", crate::redact_secret_text(&other.to_string())),
        }
    }
}

fn proxy_error_json(status: StatusCode, message: String) -> (StatusCode, serde_json::Value) {
    (
        status,
        json!({
            "error": {
                "message": crate::redact_secret_text(&message),
                "type": "proxy_error",
            }
        }),
    )
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        let (status, body) = match &self {
            ProxyError::UpstreamError {
                status: upstream_status,
                body: upstream_body,
            } => {
                let http_status =
                    StatusCode::from_u16(*upstream_status).unwrap_or(StatusCode::BAD_GATEWAY);

                // 尝试解析上游响应体为 JSON，如果失败则包装为字符串。
                // 非 loopback 监听时调用方不可信，必须先抹掉被上游回显的密钥。
                let error_body = if let Some(body_str) = upstream_body {
                    let body_str = crate::redact_secret_text(body_str);
                    if let Ok(json_body) = serde_json::from_str::<serde_json::Value>(&body_str) {
                        json_body
                    } else {
                        json!({
                            "error": {
                                "message": body_str,
                                "type": "upstream_error",
                            }
                        })
                    }
                } else {
                    json!({
                        "error": {
                            "message": format!("Upstream error (status {})", upstream_status),
                            "type": "upstream_error",
                        }
                    })
                };

                (http_status, error_body)
            }
            ProxyError::AlreadyRunning => proxy_error_json(StatusCode::CONFLICT, self.to_string()),
            ProxyError::NotRunning => {
                proxy_error_json(StatusCode::SERVICE_UNAVAILABLE, self.to_string())
            }
            ProxyError::BindFailed(_)
            | ProxyError::StopTimeout
            | ProxyError::StopFailed(_)
            | ProxyError::DatabaseError(_)
            | ProxyError::Internal(_) => {
                proxy_error_json(StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
            }
            ProxyError::ForwardFailed(_) | ProxyError::ResponseBodyTooLarge(_) => {
                proxy_error_json(StatusCode::BAD_GATEWAY, self.to_string())
            }
            ProxyError::NoAvailableProvider
            | ProxyError::AllProvidersCircuitOpen
            | ProxyError::NoProvidersConfigured
            | ProxyError::ProviderUnhealthy(_)
            | ProxyError::MaxRetriesExceeded => {
                proxy_error_json(StatusCode::SERVICE_UNAVAILABLE, self.to_string())
            }
            ProxyError::ConfigError(_) | ProxyError::InvalidRequest(_) => {
                proxy_error_json(StatusCode::BAD_REQUEST, self.to_string())
            }
            ProxyError::TransformError(_) => {
                proxy_error_json(StatusCode::UNPROCESSABLE_ENTITY, self.to_string())
            }
            ProxyError::Timeout(_) | ProxyError::StreamIdleTimeout(_) => {
                proxy_error_json(StatusCode::GATEWAY_TIMEOUT, self.to_string())
            }
            ProxyError::AuthError(_) => {
                proxy_error_json(StatusCode::UNAUTHORIZED, self.to_string())
            }
        };

        (status, Json(body)).into_response()
    }
}

/// 错误分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// 可重试错误（网络问题、5xx）
    Retryable, // 网络超时、5xx 错误
    /// 不可重试错误（4xx、认证失败）
    NonRetryable, // 认证失败、参数错误、4xx 错误
    #[allow(dead_code)]
    ClientAbort, // 客户端主动中断
}

/// 判断错误是否可重试
#[allow(dead_code)]
pub fn categorize_error(error: &reqwest::Error) -> ErrorCategory {
    if error.is_timeout() || error.is_connect() {
        return ErrorCategory::Retryable;
    }

    if let Some(status) = error.status() {
        if status.is_server_error() {
            ErrorCategory::Retryable
        } else if status.is_client_error() {
            ErrorCategory::NonRetryable
        } else {
            ErrorCategory::Retryable
        }
    } else {
        ErrorCategory::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::response::IntoResponse;
    use serde_json::Value;

    async fn response_json(error: ProxyError) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("read error body");
        let json: Value = serde_json::from_slice(&body).expect("parse error json");
        (status, json)
    }

    #[tokio::test]
    async fn into_response_does_not_print_raw_keys() {
        let key = "sk-ant-api03-TESTSECRETVALUE99xxxx";
        let upstream = ProxyError::UpstreamError {
            status: 401,
            body: Some(format!(
                r#"{{"error":{{"message":"invalid x-api-key: {key}"}}}}"#
            )),
        };
        let response = upstream.into_response();
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("upstream body");
        let text = String::from_utf8(bytes.to_vec()).expect("utf8");
        assert!(!text.contains(key), "{text}");
        assert!(!text.contains("TESTSECRETVALUE99"), "{text}");
        assert!(text.contains("[REDACTED]"), "{text}");

        let forward = ProxyError::ForwardFailed(format!("upstream rejected api_key {key}"));
        let response = forward.into_response();
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("forward body");
        let text = String::from_utf8(bytes.to_vec()).expect("utf8");
        assert!(!text.contains(key), "{text}");
        assert!(!text.contains("TESTSECRETVALUE99"), "{text}");
        assert!(text.contains("[REDACTED]"), "{text}");
    }

    #[tokio::test]
    async fn upstream_400_json_passthrough_preserves_1214_payload() {
        let error = ProxyError::UpstreamError {
            status: 400,
            body: Some(r#"{"error":{"code":"1214","message":"角色信息不正确"}}"#.to_string()),
        };
        let (status, json) = response_json(error).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["code"], "1214");
        assert_eq!(json["error"]["message"], "角色信息不正确");
    }

    #[tokio::test]
    async fn upstream_401_and_429_preserve_status_and_json() {
        let (status, json) = response_json(ProxyError::UpstreamError {
            status: 401,
            body: Some(r#"{"error":{"message":"invalid api key","type":"auth"}}"#.to_string()),
        })
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(json["error"]["type"], "auth");

        let (status, json) = response_json(ProxyError::UpstreamError {
            status: 429,
            body: Some(r#"{"error":{"message":"rate limited"}}"#.to_string()),
        })
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(json["error"]["message"], "rate limited");
    }

    #[tokio::test]
    async fn upstream_500_html_is_wrapped_not_dropped() {
        let (status, json) = response_json(ProxyError::UpstreamError {
            status: 502,
            body: Some("<html>Bad Gateway</html>".to_string()),
        })
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(json["error"]["type"], "upstream_error");
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Bad Gateway"));
    }

    #[tokio::test]
    async fn response_body_too_large_is_bad_gateway() {
        let (status, json) = response_json(ProxyError::ResponseBodyTooLarge(12)).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(json["error"]["type"], "proxy_error");
    }
}
