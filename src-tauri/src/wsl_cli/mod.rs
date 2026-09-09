//! Shared WSL home for Claude, Codex, and Pi.
//!
//! Claude/Codex live-file writes still go through `wsl.exe` (takeover /
//! local proxy). Pi **session and usage discovery** reads the distro home
//! the same way open-source CC Switch points Claude at
//! `\\wsl.localhost\<distro>\home\<user>\.claude`:
//! `\\wsl.localhost\<distro>\home\<user>\.pi\...`.
//!
//! Linux CI cannot mount that 9P share, so [`walkable_wsl_path`] falls
//! back to the POSIX path itself (documented equivalent).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::AppError;
use crate::pi_runtime::error::PiRuntimeError;
use crate::pi_runtime::files;
use crate::pi_runtime::sessions;
use crate::pi_runtime::wsl::{self, WslRequest};
use crate::pi_runtime::{self, PiRuntimeTarget};

/// `$1` target, `$2` digest of the incoming payload. Atomic `mv` replace.
const OVERWRITE_SCRIPT: &str = concat!(
    r#"
set -u
target="${1:-}"; payload="${2:-}"
if [ -z "$target" ] || [ "$target" = "/" ]; then
  printf 'empty-target\n' >&2
  exit 1
fi
dir=$(dirname -- "$target")
mkdir -p -- "$dir" || { printf 'mkdir-failed\n' >&2; exit 1; }
chmod 700 -- "$dir" 2>/dev/null || true
umask 077
"#,
    crate::pi_runtime::files::atomic_stage_snippet!(),
    r#"
trap 'rm -f -- "$tmp"' EXIT HUP INT TERM
cat > "$tmp" || { printf 'write-failed\n' >&2; exit 1; }
chmod 600 -- "$tmp"
written=$(sha256sum < "$tmp" | cut -d' ' -f1)
if [ "$written" != "$payload" ]; then printf 'payload-mismatch\n' >&2; exit 1; fi
mv -f -- "$tmp" "$target" || { printf 'replace-failed\n' >&2; exit 1; }
trap - EXIT HUP INT TERM
verify=$(sha256sum < "$target" | cut -d' ' -f1)
if [ "$verify" != "$payload" ]; then printf 'verify-mismatch\n' >&2; exit 1; fi
printf 'ok\n'
"#
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslHome {
    pub distro: String,
    pub home: String,
}

impl WslHome {
    pub fn claude_dir(&self) -> String {
        format!("{}/.claude", self.home.trim_end_matches('/'))
    }

    pub fn claude_settings(&self) -> String {
        format!("{}/settings.json", self.claude_dir())
    }

    pub fn claude_projects(&self) -> String {
        format!("{}/projects", self.claude_dir())
    }

    pub fn codex_dir(&self) -> String {
        format!("{}/.codex", self.home.trim_end_matches('/'))
    }

    pub fn codex_auth(&self) -> String {
        format!("{}/auth.json", self.codex_dir())
    }

    pub fn codex_config(&self) -> String {
        format!("{}/config.toml", self.codex_dir())
    }

    pub fn codex_sessions(&self) -> String {
        format!("{}/sessions", self.codex_dir())
    }

    #[cfg(test)]
    pub fn display(&self, linux_path: &str) -> String {
        format!("wsl:{}:{linux_path}", self.distro)
    }
}

/// True when `raw` is a WSL 9P UNC path (`\\wsl.localhost\…` / `\\wsl$\…`).
pub fn is_wsl_unc_str(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }
    let normalized = trimmed.replace('/', r"\");
    let lower = normalized.to_ascii_lowercase();
    let body = lower
        .strip_prefix(r"\\?\unc\")
        .or_else(|| lower.strip_prefix(r"\\?\unix\"))
        .unwrap_or(&lower);
    body.starts_with(r"\\wsl.localhost\")
        || body.starts_with(r"\\wsl$\")
        || body.starts_with(r"\\wsl.localhost")
        || body.starts_with(r"\\wsl$")
}

pub fn is_wsl_unc_path(path: &Path) -> bool {
    is_wsl_unc_str(&path.to_string_lossy())
}

/// `\\wsl.localhost\<distro>` plus a Linux absolute path as UNC components.
///
/// `/home/tfdx8045/.pi` on `Ubuntu-22.04` becomes
/// `\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi`.
pub fn wsl_localhost_unc(distro: &str, linux_abs: &str) -> String {
    let mut unc = format!(r"\\wsl.localhost\{distro}");
    for component in linux_abs.split(['/', '\\']).filter(|part| !part.is_empty()) {
        unc.push('\\');
        unc.push_str(component);
    }
    unc
}

/// Path CC Switch should `read_dir` for WSL-home files.
///
/// On Windows a Linux absolute path becomes `\\wsl.localhost\…`. Elsewhere
/// (Linux fixtures / CI) the POSIX path is the documented equivalent.
pub fn walkable_wsl_path(distro: &str, linux_abs: &str) -> PathBuf {
    if cfg!(windows) && wsl::is_valid_linux_path(linux_abs) {
        PathBuf::from(wsl_localhost_unc(distro, linux_abs))
    } else {
        PathBuf::from(linux_abs)
    }
}

/// Invert [`wsl_localhost_unc`], including `\\?\UNC\` and `\\wsl$\` shapes.
pub fn linux_path_from_wsl_unc(raw: &str, distro: &str) -> Option<String> {
    let normalized = raw.replace('/', r"\");
    let lower = normalized.to_ascii_lowercase();
    let distro_l = distro.to_ascii_lowercase();
    let prefixes = [
        format!(r"\\?\unc\wsl.localhost\{distro_l}"),
        format!(r"\\?\unc\wsl$\{distro_l}"),
        format!(r"\\wsl.localhost\{distro_l}"),
        format!(r"\\wsl$\{distro_l}"),
    ];
    for prefix in prefixes {
        if lower.starts_with(&prefix) {
            let rest = &normalized[prefix.len()..];
            let linux = rest.replace('\\', "/");
            let linux = if linux.starts_with('/') {
                linux
            } else {
                format!("/{linux}")
            };
            if linux.len() > 1 {
                return Some(linux);
            }
        }
    }
    None
}

/// Drop a Claude/Codex override that would walk WSL through UNC.
///
/// Pi keeps UNC overrides — that is the session/usage discovery path.
pub fn reject_unc_override(path: Option<&str>) -> Option<String> {
    let value = path.map(str::trim).filter(|value| !value.is_empty())?;
    if is_wsl_unc_str(value) {
        log::warn!("[WslCli] ignoring UNC override '{value}'; select the WSL runtime instead");
        return None;
    }
    Some(value.to_string())
}

/// Active WSL home when the Pi runtime target is a distribution.
pub fn active_wsl_home() -> Option<WslHome> {
    match pi_runtime::target() {
        PiRuntimeTarget::Wsl { distro, home, .. } => {
            if !wsl::is_valid_distro_name(&distro) || !wsl::is_valid_linux_path(&home) {
                return None;
            }
            Some(WslHome { distro, home })
        }
        PiRuntimeTarget::Local => None,
    }
}

pub fn is_wsl_runtime() -> bool {
    active_wsl_home().is_some()
}

fn overwrite_linux_file(distro: &str, linux_path: &str, bytes: &[u8]) -> Result<(), AppError> {
    if !wsl::is_valid_linux_path(linux_path) {
        return Err(AppError::InvalidInput(format!(
            "unusable WSL path '{linux_path}'"
        )));
    }
    let digest = files::revision(bytes);
    let script = wsl::with_dropped_arg_fallback(OVERWRITE_SCRIPT, &[linux_path, &digest]);
    let output = wsl::run(
        &WslRequest::guarded(distro, &script)
            .arg(linux_path)
            .arg(&digest)
            .stdin(bytes.to_vec())
            .timeout(wsl::DEFAULT_TIMEOUT),
    )
    .map_err(pi_to_app)?;
    if !output.succeeded() {
        let detail = wsl::first_line(&output.stderr).unwrap_or("no stderr output");
        return Err(AppError::Config(format!(
            "writing {linux_path} in WSL '{distro}' failed: {detail}"
        )));
    }
    match wsl::first_line(&output.payload_lossy()) {
        Some("ok") => {
            log::info!(
                "[WslCli] wrote {} bytes to wsl:{distro}:{linux_path}",
                bytes.len()
            );
            Ok(())
        }
        other => Err(AppError::Config(format!(
            "writing {linux_path} in WSL '{distro}' returned {}",
            other.unwrap_or("no output")
        ))),
    }
}

fn pi_to_app(error: PiRuntimeError) -> AppError {
    AppError::Config(error.to_string())
}

/// Write Claude `settings.json` into WSL `$HOME/.claude`. Returns `true` when
/// the WSL runtime handled the write.
pub fn write_claude_settings(settings: &Value) -> Result<bool, AppError> {
    let Some(home) = active_wsl_home() else {
        return Ok(false);
    };
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| AppError::JsonSerialize { source: error })?;
    overwrite_linux_file(&home.distro, &home.claude_settings(), &bytes)?;
    Ok(true)
}

/// Write Codex live files into WSL `$HOME/.codex` via `mv` (never hard_link).
pub fn write_codex_live(auth: Option<&Value>, config_text: Option<&str>) -> Result<bool, AppError> {
    let Some(home) = active_wsl_home() else {
        return Ok(false);
    };
    if let Some(auth) = auth {
        let bytes = serde_json::to_vec_pretty(auth)
            .map_err(|error| AppError::JsonSerialize { source: error })?;
        overwrite_linux_file(&home.distro, &home.codex_auth(), &bytes)?;
    }
    if let Some(config) = config_text {
        overwrite_linux_file(&home.distro, &home.codex_config(), config.as_bytes())?;
    }
    Ok(true)
}

/// Rewrite a Windows-side proxy origin so a WSL CLI can reach it.
pub fn rewrite_proxy_origin(local_origin: &str, listen_port: u16) -> Option<String> {
    let target = pi_runtime::target();
    if !target.is_wsl() {
        return None;
    }
    let plan = pi_runtime::proxy::plan(&target, true, listen_port, None).ok()?;
    let origin = plan.origin?;
    if origin == local_origin {
        return Some(origin);
    }
    log::info!("[WslCli] rewritten proxy origin {local_origin} → {origin} for WSL Claude/Codex");
    Some(origin)
}

fn app_cache(distro: &str, name: &str) -> PathBuf {
    crate::config::get_app_config_dir()
        .join("wsl-cli-sessions")
        .join(distro)
        .join(name)
}

fn prepare_tree(home: &WslHome, linux_root: &str, cache_name: &str) -> PathBuf {
    let cache = app_cache(&home.distro, cache_name);
    if let Err(error) = sessions::sync_tree(&home.distro, linux_root, &cache) {
        log::warn!("[WslCli] session sync via wsl.exe failed for {linux_root}: {error}");
    }
    cache
}

/// Local directory the Claude session scanner / usage importer should walk.
///
/// `None` means skip this pass (UNC override with no WSL runtime).
pub fn claude_projects_dir() -> Option<PathBuf> {
    if let Some(home) = active_wsl_home() {
        return Some(prepare_tree(
            &home,
            &home.claude_projects(),
            "claude-projects",
        ));
    }
    let dir = crate::config::get_claude_config_dir();
    if is_wsl_unc_path(&dir) {
        return None;
    }
    Some(dir.join("projects"))
}

fn prepare_codex_home(home: &WslHome) -> PathBuf {
    let home_cache = app_cache(&home.distro, "codex-home");
    let sessions = home_cache.join("sessions");
    if let Err(error) = sessions::sync_tree(&home.distro, &home.codex_sessions(), &sessions) {
        log::warn!(
            "[WslCli] session sync via wsl.exe failed for {}: {error}",
            home.codex_sessions()
        );
    }
    home_cache
}

/// Local Codex session roots after an optional WSL mirror.
pub fn codex_session_roots() -> Vec<PathBuf> {
    if let Some(home) = active_wsl_home() {
        return vec![prepare_codex_home(&home).join("sessions")];
    }
    let dir = crate::codex_config::get_codex_config_dir();
    if is_wsl_unc_path(&dir) {
        return Vec::new();
    }
    vec![dir.join("sessions"), dir.join("archived_sessions")]
}

/// Codex home used by the usage importer. Empty when UNC would be walked.
pub fn codex_usage_dir() -> Option<PathBuf> {
    if let Some(home) = active_wsl_home() {
        return Some(prepare_codex_home(&home));
    }
    let dir = crate::codex_config::get_codex_config_dir();
    if is_wsl_unc_path(&dir) {
        return None;
    }
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_wsl_unc_shapes() {
        assert!(is_wsl_unc_str(r"\\wsl.localhost\Ubuntu\home\chen\.claude"));
        assert!(is_wsl_unc_str(r"\\wsl$\Ubuntu\home\lee\.codex"));
        assert!(is_wsl_unc_str("//wsl.localhost/Ubuntu/home/user/.pi"));
        assert!(is_wsl_unc_str(r"\\WSL.LOCALHOST\Debian\root\.codex"));
        assert!(!is_wsl_unc_str(r"C:\Users\alice\.claude"));
        assert!(!is_wsl_unc_str("/home/alice/.claude"));
        assert!(!is_wsl_unc_str(""));
        assert!(is_wsl_unc_path(&PathBuf::from(
            r"\\wsl.localhost\Ubuntu\home\chen\.claude"
        )));
    }

    #[test]
    fn wsl_localhost_unc_matches_open_source_claude_shape() {
        assert_eq!(
            wsl_localhost_unc("Ubuntu-22.04", "/home/tfdx8045/.pi"),
            r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi"
        );
        assert_eq!(
            wsl_localhost_unc("Ubuntu-22.04", "/home/tfdx8045/.pi/agent/sessions"),
            r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi\agent\sessions"
        );
        assert_eq!(
            linux_path_from_wsl_unc(
                r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi\agent\sessions\work\abc.jsonl",
                "Ubuntu-22.04",
            )
            .as_deref(),
            Some("/home/tfdx8045/.pi/agent/sessions/work/abc.jsonl")
        );
        assert_eq!(
            linux_path_from_wsl_unc(
                r"\\?\UNC\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi\agent\sessions\a.jsonl",
                "Ubuntu-22.04",
            )
            .as_deref(),
            Some("/home/tfdx8045/.pi/agent/sessions/a.jsonl")
        );
        let walkable = walkable_wsl_path("Ubuntu-22.04", "/tmp/fixture/.pi/agent/sessions");
        if cfg!(windows) {
            assert_eq!(
                walkable,
                PathBuf::from(r"\\wsl.localhost\Ubuntu-22.04\tmp\fixture\.pi\agent\sessions")
            );
        } else {
            assert_eq!(walkable, PathBuf::from("/tmp/fixture/.pi/agent/sessions"));
        }
    }

    #[test]
    fn reject_unc_override_drops_wsl_paths() {
        assert_eq!(
            reject_unc_override(Some(r"\\wsl.localhost\Ubuntu\home\u\.claude")),
            None
        );
        assert_eq!(
            reject_unc_override(Some("  /home/u/.claude  ")),
            Some("/home/u/.claude".to_string())
        );
        assert_eq!(reject_unc_override(Some("   ")), None);
    }

    #[test]
    fn overwrite_script_stages_under_tmp_never_at_root() {
        assert!(
            OVERWRITE_SCRIPT.contains("mktemp /tmp/cc-switch-XXXXXX")
                || OVERWRITE_SCRIPT.contains("mktemp -p")
        );
        assert!(!OVERWRITE_SCRIPT.contains("$dir/.cc-switch-XXXXXX"));
        assert!(OVERWRITE_SCRIPT.contains("empty-target"));
    }

    #[test]
    fn wsl_home_paths_stay_posix_and_never_unc() {
        let home = WslHome {
            distro: "Ubuntu-22.04".to_string(),
            home: "/home/tfdx8045".to_string(),
        };
        assert_eq!(
            home.claude_settings(),
            "/home/tfdx8045/.claude/settings.json"
        );
        assert_eq!(home.codex_auth(), "/home/tfdx8045/.codex/auth.json");
        assert!(!home.display(&home.claude_settings()).contains(r"\\wsl"));
        assert!(home.display(&home.claude_settings()).starts_with("wsl:"));
    }
}
