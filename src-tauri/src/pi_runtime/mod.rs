//! Pi runtime adapter.
//!
//! CC Switch adapts to Pi rather than the other way around: Pi owns its CLI,
//! its config format and its session format, and this module is the boundary
//! that lets those artefacts live somewhere other than the local filesystem.
//!
//! Today there are two runtimes:
//!
//! ```text
//! PiRuntimeTarget
//!   ├── Local  — Pi installed for the CC Switch user (Windows, macOS, Linux)
//!   └── Wsl    — Pi installed inside a WSL2 distribution, reached via wsl.exe
//! ```
//!
//! A future `Docker` or `Ssh` target only has to add a variant here; nothing
//! in the provider, session or usage code learns about transports.

pub mod detect;
pub mod error;
pub mod files;
pub mod proxy;
pub mod sessions;
pub mod wsl;

use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub use detect::WslPiProbe;
pub use error::{PiResult, PiRuntimeError};

/// Which runtime hosts Pi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PiRuntimeKind {
    /// Pi runs as the CC Switch user on this machine.
    ///
    /// Serialized as `local`; `windows` is accepted so a settings file written
    /// against the design document's `● Windows / ○ WSL` wording still loads.
    #[default]
    #[serde(alias = "windows", alias = "native", alias = "host")]
    Local,
    /// Pi runs inside a WSL2 distribution.
    Wsl,
}

impl PiRuntimeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Wsl => "wsl",
        }
    }
}

/// Gradual-rollout switches (design document §65).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiFeatureFlags {
    /// `pi.wsl.enabled` — master switch for the WSL runtime.
    #[serde(default = "enabled")]
    pub wsl: bool,
    /// `pi.wsl.proxy.enabled` — inject CC Switch's upstream proxy into the Pi
    /// process. Off by default because it changes where Pi's traffic goes.
    #[serde(default)]
    pub wsl_proxy: bool,
    /// `pi.session.enabled` — mirror WSL session files into the local cache.
    #[serde(default = "enabled")]
    pub session: bool,
    /// `pi.session.incremental.enabled` — only re-fetch changed session files.
    #[serde(default = "enabled")]
    pub session_incremental: bool,
    /// `pi.usage.enabled` — import token/cost/duration statistics.
    #[serde(default = "enabled")]
    pub usage: bool,
}

fn enabled() -> bool {
    true
}

impl Default for PiFeatureFlags {
    fn default() -> Self {
        Self {
            wsl: true,
            wsl_proxy: false,
            session: true,
            session_incremental: true,
            usage: true,
        }
    }
}

/// Device-level Pi runtime selection, persisted in `AppSettings`.
///
/// Absent settings mean the local runtime, so configurations written before
/// this feature existed keep working unchanged (design document §67).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PiRuntimeSettings {
    #[serde(default)]
    pub kind: PiRuntimeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distro: Option<String>,
    #[serde(default)]
    pub flags: PiFeatureFlags,
}

impl PiRuntimeSettings {
    /// The runtime is only WSL when the user selected it, the flag is on and
    /// a distribution was chosen.
    pub fn wants_wsl(&self) -> bool {
        self.kind == PiRuntimeKind::Wsl && self.flags.wsl && self.distro.is_some()
    }
}

/// A resolved runtime, with every path the rest of the code needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PiRuntimeTarget {
    Local,
    #[serde(rename_all = "camelCase")]
    Wsl {
        distro: String,
        home: String,
        agent_dir: String,
    },
}

impl PiRuntimeTarget {
    pub fn is_wsl(&self) -> bool {
        matches!(self, Self::Wsl { .. })
    }

    pub fn distro(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Wsl { distro, .. } => Some(distro),
        }
    }

    /// Absolute Linux path of the Pi agent directory, for WSL targets.
    pub fn agent_dir(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Wsl { agent_dir, .. } => Some(agent_dir),
        }
    }

    pub fn sessions_path(&self) -> Option<String> {
        self.agent_dir().map(|dir| format!("{dir}/sessions"))
    }
}

/// Full runtime state for the Runtime panel (design document §35).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiRuntimeStatus {
    pub settings: PiRuntimeSettings,
    pub target: PiRuntimeTarget,
    /// Only populated for WSL targets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<WslPiProbe>,
    /// A WSL selection that could not be honoured, reported without breaking
    /// the rest of the Pi UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<PiRuntimeError>,
    pub wsl_available: bool,
}

/// Resolving a WSL target costs a `wsl.exe` round trip, and provider reads hit
/// it repeatedly, so the resolution is memoized for a short window.
const TARGET_TTL: Duration = Duration::from_secs(60);

struct CachedTarget {
    resolved_at: Instant,
    settings: PiRuntimeSettings,
    target: PiRuntimeTarget,
}

static TARGET_CACHE: LazyLock<Mutex<Option<CachedTarget>>> = LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
static TEST_TARGET: LazyLock<Mutex<Option<PiRuntimeTarget>>> = LazyLock::new(|| Mutex::new(None));

/// Read the persisted runtime selection.
pub fn settings() -> PiRuntimeSettings {
    crate::settings::get_pi_runtime_settings()
}

/// Resolve the runtime that Pi artefacts should be read from and written to.
///
/// Never fails: a WSL selection that cannot be resolved logs and degrades to
/// the local runtime, because failing here would take the whole Pi UI down.
pub fn target() -> PiRuntimeTarget {
    match try_target() {
        Ok(target) => target,
        Err(error) => {
            log::warn!("[PiRuntime] falling back to the local runtime: {error}");
            PiRuntimeTarget::Local
        }
    }
}

/// Resolve the runtime, surfacing why a WSL selection could not be honoured.
pub fn try_target() -> PiResult<PiRuntimeTarget> {
    #[cfg(test)]
    if let Some(target) = TEST_TARGET.lock().expect("lock Pi test target").clone() {
        return Ok(target);
    }

    let settings = settings();
    if !settings.wants_wsl() {
        return Ok(PiRuntimeTarget::Local);
    }

    if let Some(cached) = cached_target(&settings) {
        return Ok(cached);
    }

    let distro = settings
        .distro
        .clone()
        .expect("wants_wsl guarantees a distribution");
    let probe = detect::probe(&distro)?;
    let target = PiRuntimeTarget::Wsl {
        distro: probe.distro.clone(),
        home: probe.home.clone(),
        agent_dir: probe.agent_dir.clone(),
    };
    store_target(&settings, &target);
    Ok(target)
}

fn cached_target(settings: &PiRuntimeSettings) -> Option<PiRuntimeTarget> {
    let guard = TARGET_CACHE.lock().expect("lock Pi runtime target cache");
    let cached = guard.as_ref()?;
    (cached.settings == *settings && cached.resolved_at.elapsed() < TARGET_TTL)
        .then(|| cached.target.clone())
}

fn store_target(settings: &PiRuntimeSettings, target: &PiRuntimeTarget) {
    *TARGET_CACHE.lock().expect("lock Pi runtime target cache") = Some(CachedTarget {
        resolved_at: Instant::now(),
        settings: settings.clone(),
        target: target.clone(),
    });
}

/// Drop the memoized runtime. Call after the selection changes.
pub fn invalidate_target_cache() {
    *TARGET_CACHE.lock().expect("lock Pi runtime target cache") = None;
}

/// Whether this platform can host a WSL runtime at all.
pub fn wsl_supported() -> bool {
    cfg!(target_os = "windows")
}

/// Collect the runtime state the Pi Runtime panel renders.
pub fn status() -> PiRuntimeStatus {
    let settings = settings();
    let wsl_available = wsl_supported();

    if !settings.wants_wsl() {
        return PiRuntimeStatus {
            settings,
            target: PiRuntimeTarget::Local,
            probe: None,
            error: None,
            wsl_available,
        };
    }

    let distro = settings.distro.clone().unwrap_or_default();
    match detect::probe(&distro) {
        Ok(probe) => {
            let target = PiRuntimeTarget::Wsl {
                distro: probe.distro.clone(),
                home: probe.home.clone(),
                agent_dir: probe.agent_dir.clone(),
            };
            store_target(&settings, &target);
            let error = (!probe.pi_installed()).then(|| {
                PiRuntimeError::pi_not_found(format!(
                    "Pi is not on PATH in WSL '{}'. Install it with `npm i -g @earendil-works/pi-coding-agent`.",
                    probe.distro
                ))
            });
            PiRuntimeStatus {
                settings,
                target,
                probe: Some(probe),
                error,
                wsl_available,
            }
        }
        Err(error) => PiRuntimeStatus {
            settings,
            target: PiRuntimeTarget::Local,
            probe: None,
            error: Some(error),
            wsl_available,
        },
    }
}

/// Remove credentials from a string before it reaches a log, a toast or an
/// error message (design document §41, §54).
pub fn redact(text: &str) -> String {
    static SECRET_KEYS: &[&str] = &[
        "apikey",
        "api_key",
        "api-key",
        "authorization",
        "x-api-key",
        "token",
        "secret",
        "password",
        "bearer",
    ];

    let mut output = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let lowered = text.to_ascii_lowercase();
    let mut index = 0usize;

    while index < bytes.len() {
        if let Some(key) = SECRET_KEYS
            .iter()
            .find(|key| lowered[index..].starts_with(**key))
        {
            output.push_str(&text[index..index + key.len()]);
            index += key.len();
            // Keep the separator run (`": "`, `=`, whitespace) so the shape of
            // the message survives, then swallow the value itself.
            let value_start = index
                + bytes[index..]
                    .iter()
                    .take_while(|byte| matches!(byte, b'"' | b'\'' | b':' | b'=' | b' ' | b'\t'))
                    .count();
            output.push_str(&text[index..value_start]);
            index = value_start;

            let mut value_end = index + value_token_length(&text[index..]);
            // `Authorization: Bearer <token>` puts the credential in a second
            // token, so redacting only the first would leak it.
            if is_auth_scheme(&text[index..value_end]) {
                let after_scheme = value_end
                    + bytes[value_end..]
                        .iter()
                        .take_while(|byte| matches!(byte, b' ' | b'\t'))
                        .count();
                value_end = after_scheme + value_token_length(&text[after_scheme..]);
            }

            if value_end > index {
                output.push_str("***");
                index = value_end;
            }
            continue;
        }

        if let Some(length) = secret_token_length(&text[index..]) {
            output.push_str("***");
            index += length;
            continue;
        }

        let char_length = text[index..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
        output.push_str(&text[index..index + char_length]);
        index += char_length;
    }

    output
}

/// Length of the credential value at the start of `text`, stopping at the
/// delimiters that end a value in JSON, in a URL query or in a log line.
fn value_token_length(text: &str) -> usize {
    text.bytes()
        .take_while(|byte| {
            !matches!(
                byte,
                b'"' | b'\'' | b',' | b'}' | b' ' | b'\t' | b'\n' | b'\r' | b'&'
            )
        })
        .count()
}

fn is_auth_scheme(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "bearer" | "basic" | "token" | "digest"
    )
}

/// Length of a bare provider key (`sk-...`, `sk-ant-...`, `ghp_...`) at the
/// start of `text`, if there is one.
fn secret_token_length(text: &str) -> Option<usize> {
    const PREFIXES: &[&str] = &["sk-", "sk_", "ghp_", "gho_", "xai-", "pi-"];
    let prefix = PREFIXES.iter().find(|prefix| text.starts_with(**prefix))?;
    let length = prefix.len()
        + text[prefix.len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .count();
    // Short values are far more likely to be prose ("sk-" in a sentence) than
    // a real credential.
    (length >= prefix.len() + 12).then_some(length)
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    pub struct TestTarget {
        previous: Option<PiRuntimeTarget>,
    }

    impl TestTarget {
        pub fn install(target: PiRuntimeTarget) -> Self {
            invalidate_target_cache();
            Self {
                previous: TEST_TARGET
                    .lock()
                    .expect("lock Pi test target")
                    .replace(target),
            }
        }

        pub fn wsl(distro: &str, home: &str) -> Self {
            Self::install(PiRuntimeTarget::Wsl {
                distro: distro.to_string(),
                home: home.to_string(),
                agent_dir: format!("{home}/.pi/agent"),
            })
        }
    }

    impl Drop for TestTarget {
        fn drop(&mut self) {
            *TEST_TARGET.lock().expect("lock Pi test target") = self.previous.take();
            invalidate_target_cache();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_settings_mean_the_local_runtime() {
        let settings = serde_json::from_str::<PiRuntimeSettings>("{}").expect("deserialize");
        assert_eq!(settings.kind, PiRuntimeKind::Local);
        assert!(!settings.wants_wsl());
        assert!(settings.flags.session);
        assert!(!settings.flags.wsl_proxy);
    }

    #[test]
    fn the_design_documents_windows_wording_still_deserializes() {
        let settings = serde_json::from_str::<PiRuntimeSettings>(r#"{"kind":"windows"}"#)
            .expect("deserialize");
        assert_eq!(settings.kind, PiRuntimeKind::Local);
    }

    #[test]
    fn wsl_requires_both_a_distro_and_the_feature_flag() {
        let mut settings = PiRuntimeSettings {
            kind: PiRuntimeKind::Wsl,
            distro: None,
            flags: PiFeatureFlags::default(),
        };
        assert!(!settings.wants_wsl(), "a distro is required");

        settings.distro = Some("Ubuntu-22.04".to_string());
        assert!(settings.wants_wsl());

        settings.flags.wsl = false;
        assert!(!settings.wants_wsl(), "the flag must gate the runtime");
    }

    #[test]
    fn wsl_targets_expose_pi_paths() {
        let target = PiRuntimeTarget::Wsl {
            distro: "Ubuntu-22.04".to_string(),
            home: "/home/tfdx8045".to_string(),
            agent_dir: "/home/tfdx8045/.pi/agent".to_string(),
        };

        assert_eq!(
            target.sessions_path().as_deref(),
            Some("/home/tfdx8045/.pi/agent/sessions")
        );
        assert_eq!(target.distro(), Some("Ubuntu-22.04"));
        assert_eq!(PiRuntimeTarget::Local.sessions_path(), None);
        assert_eq!(PiRuntimeTarget::Local.distro(), None);
    }

    #[test]
    fn redaction_hides_keyed_credentials_but_keeps_the_message_shape() {
        assert_eq!(
            redact(r#"{"apiKey": "sk-ant-api03-abcdefghijklmnop"}"#),
            r#"{"apiKey": "***"}"#
        );
        assert_eq!(
            redact("Authorization: Bearer abcdef123456"),
            "Authorization: ***"
        );
        assert_eq!(
            redact("HTTPS_PROXY=http://host:7890 token=abc123def456"),
            "HTTPS_PROXY=http://host:7890 token=***"
        );
    }

    #[test]
    fn redaction_hides_bare_provider_keys() {
        assert_eq!(
            redact("request failed for sk-ant-0123456789abcdef in WSL"),
            "request failed for *** in WSL"
        );
        // Too short to be a credential; leaving prose alone matters more than
        // catching a hypothetical 4-character key.
        assert_eq!(redact("see sk-docs"), "see sk-docs");
    }

    #[test]
    fn redaction_leaves_ordinary_diagnostics_untouched() {
        let message =
            "Pi provider 'anthropic' changed outside CC Switch: /home/me/.pi/agent/models.json";
        assert_eq!(redact(message), message);
    }
}
