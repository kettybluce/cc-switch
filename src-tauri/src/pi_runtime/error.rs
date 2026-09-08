//! Error taxonomy for the Pi runtime adapter.
//!
//! Callers (and the UI) must be able to tell "WSL is missing" apart from
//! "Pi is missing" apart from "the provider rejected our credentials", so the
//! runtime never collapses those into a single opaque failure string.

use serde::Serialize;
use std::fmt;

use crate::error::AppError;

/// Stable machine-readable failure codes surfaced to the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PiRuntimeErrorCode {
    WslNotFound,
    WslDistroNotFound,
    WslCommandFailed,
    PiNotFound,
    PiConfigNotFound,
    PiConfigConflict,
    ProxyUnreachable,
    ProxyProtocolError,
    SessionReadFailed,
    SessionParseFailed,
    InvalidInput,
}

impl PiRuntimeErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WslNotFound => "WSL_NOT_FOUND",
            Self::WslDistroNotFound => "WSL_DISTRO_NOT_FOUND",
            Self::WslCommandFailed => "WSL_COMMAND_FAILED",
            Self::PiNotFound => "PI_NOT_FOUND",
            Self::PiConfigNotFound => "PI_CONFIG_NOT_FOUND",
            Self::PiConfigConflict => "PI_CONFIG_CONFLICT",
            Self::ProxyUnreachable => "PROXY_UNREACHABLE",
            Self::ProxyProtocolError => "PROXY_PROTOCOL_ERROR",
            Self::SessionReadFailed => "SESSION_READ_FAILED",
            Self::SessionParseFailed => "SESSION_PARSE_FAILED",
            Self::InvalidInput => "INVALID_INPUT",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiRuntimeError {
    pub code: PiRuntimeErrorCode,
    pub message: String,
}

impl PiRuntimeError {
    pub fn new(code: PiRuntimeErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: super::redact(&message.into()),
        }
    }

    pub fn wsl_not_found(message: impl Into<String>) -> Self {
        Self::new(PiRuntimeErrorCode::WslNotFound, message)
    }

    pub fn distro_not_found(distro: &str) -> Self {
        Self::new(
            PiRuntimeErrorCode::WslDistroNotFound,
            format!("WSL distribution '{distro}' was not found"),
        )
    }

    pub fn command_failed(message: impl Into<String>) -> Self {
        Self::new(PiRuntimeErrorCode::WslCommandFailed, message)
    }

    pub fn pi_not_found(message: impl Into<String>) -> Self {
        Self::new(PiRuntimeErrorCode::PiNotFound, message)
    }

    pub fn config_not_found(message: impl Into<String>) -> Self {
        Self::new(PiRuntimeErrorCode::PiConfigNotFound, message)
    }

    pub fn config_conflict(message: impl Into<String>) -> Self {
        Self::new(PiRuntimeErrorCode::PiConfigConflict, message)
    }

    pub fn proxy_unreachable(message: impl Into<String>) -> Self {
        Self::new(PiRuntimeErrorCode::ProxyUnreachable, message)
    }

    pub fn proxy_protocol(message: impl Into<String>) -> Self {
        Self::new(PiRuntimeErrorCode::ProxyProtocolError, message)
    }

    pub fn session_read(message: impl Into<String>) -> Self {
        Self::new(PiRuntimeErrorCode::SessionReadFailed, message)
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(PiRuntimeErrorCode::InvalidInput, message)
    }
}

impl fmt::Display for PiRuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for PiRuntimeError {}

impl From<PiRuntimeError> for AppError {
    fn from(error: PiRuntimeError) -> Self {
        match error.code {
            PiRuntimeErrorCode::PiConfigConflict => AppError::Conflict(error.to_string()),
            PiRuntimeErrorCode::InvalidInput => AppError::InvalidInput(error.to_string()),
            _ => AppError::Config(error.to_string()),
        }
    }
}

pub type PiResult<T> = Result<T, PiRuntimeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_prefixes_the_stable_code() {
        let error = PiRuntimeError::distro_not_found("Ubuntu-22.04");
        assert_eq!(
            error.to_string(),
            "[WSL_DISTRO_NOT_FOUND] WSL distribution 'Ubuntu-22.04' was not found"
        );
    }

    #[test]
    fn conflicts_map_to_conflict_app_errors() {
        let app_error: AppError =
            PiRuntimeError::config_conflict("changed outside CC Switch").into();
        assert!(matches!(app_error, AppError::Conflict(_)));
    }

    #[test]
    fn messages_are_redacted_at_construction() {
        let error = PiRuntimeError::command_failed("apiKey=sk-ant-012345678901234567890123 failed");
        assert!(!error.message.contains("sk-ant-012345678901234567890123"));
    }
}
