//! WSL and Pi discovery.
//!
//! Discovery deliberately runs as a single round trip per distribution: a
//! login shell start costs hundreds of milliseconds, and the Runtime panel
//! needs `$HOME`, the `pi` entry point, its version and the agent directory
//! layout all at once.

use serde::Serialize;

use super::error::{PiResult, PiRuntimeError};
use super::wsl::{self, WslRequest};

/// One round trip that collects everything the Runtime panel renders.
///
/// `PI_CODING_AGENT_DIR` is honoured because that is how Pi itself resolves
/// its agent directory, so a user who exports it in their shell profile keeps
/// a consistent view between Pi and CC Switch.
const PROBE_SCRIPT: &str = r#"
set -u
printf 'home=%s\n' "$HOME"
if command -v pi >/dev/null 2>&1; then
  printf 'piPath=%s\n' "$(command -v pi)"
  printf 'piVersion=%s\n' "$(pi --version 2>/dev/null | head -n 1 | tr -d '\r')"
fi
agent_dir="${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}"
printf 'agentDir=%s\n' "$agent_dir"
if [ -f "$agent_dir/models.json" ]; then printf 'models=1\n'; else printf 'models=0\n'; fi
if [ -d "$agent_dir/sessions" ]; then printf 'sessions=1\n'; else printf 'sessions=0\n'; fi
printf 'sessionCount=%s\n' "$(find "$agent_dir/sessions" -maxdepth 2 -type f -name '*.jsonl' 2>/dev/null | wc -l | tr -d ' \r')"
"#;

/// What a discovered runtime target can actually do.
///
/// Capabilities are probed rather than derived from a Pi version number, so a
/// new Pi release never needs a CC Switch allowlist update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PiCapabilities {
    pub models: bool,
    pub sessions: bool,
    pub usage: bool,
    pub duration: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WslPiProbe {
    pub distro: String,
    pub home: String,
    pub agent_dir: String,
    pub pi_path: Option<String>,
    pub pi_version: Option<String>,
    pub has_models: bool,
    pub has_sessions: bool,
    pub session_count: u32,
    pub capabilities: PiCapabilities,
}

impl WslPiProbe {
    pub fn pi_installed(&self) -> bool {
        self.pi_path.is_some()
    }
}

/// List installed WSL distributions, filtering out the internal Docker images
/// that never host a user's Pi installation.
pub fn list_distros() -> PiResult<Vec<String>> {
    let distros = wsl::runner().list_distros()?;
    Ok(distros
        .into_iter()
        .filter(|name| !is_infrastructure_distro(name))
        .collect())
}

/// `docker-desktop` and its data volume are WSL distributions but are managed
/// by Docker and have no login shell a user would install Pi into.
pub fn is_infrastructure_distro(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered == "docker-desktop"
        || lowered == "docker-desktop-data"
        || lowered.starts_with("rancher-desktop")
        || lowered == "podman-machine-default"
}

/// Probe one distribution for its Pi installation.
pub fn probe(distro: &str) -> PiResult<WslPiProbe> {
    let output = wsl::run(
        &WslRequest::guarded(distro, PROBE_SCRIPT)
            // `pi` is usually installed by nvm/asdf/mise, which only put it on
            // PATH from a login shell.
            .login(true)
            .timeout(wsl::DEFAULT_TIMEOUT),
    )?;
    if !output.succeeded() {
        let detail = wsl::first_line(&output.stderr).unwrap_or("no stderr output");
        if detail.contains("no distribution") || detail.contains("not found") {
            return Err(PiRuntimeError::distro_not_found(distro));
        }
        return output
            .require_success("Pi discovery")
            .map(|()| unreachable!("require_success returns Err for a failed exit"));
    }
    parse_probe(distro, &output.payload_lossy())
}

/// Probe every distribution and keep the ones that have Pi installed.
pub fn probe_all() -> PiResult<Vec<WslPiProbe>> {
    let mut probes = Vec::new();
    for distro in list_distros()? {
        match probe(&distro) {
            Ok(probe) => probes.push(probe),
            Err(error) => {
                log::debug!("[PiRuntime] skipping distro {distro}: {error}");
            }
        }
    }
    Ok(probes)
}

/// Pick the distribution to default to: the first one that has both Pi and an
/// existing agent directory, otherwise the first one that merely has Pi.
pub fn preferred_distro(probes: &[WslPiProbe]) -> Option<&WslPiProbe> {
    probes
        .iter()
        .find(|probe| probe.pi_installed() && probe.has_models)
        .or_else(|| probes.iter().find(|probe| probe.pi_installed()))
}

fn parse_probe(distro: &str, stdout: &str) -> PiResult<WslPiProbe> {
    let mut home = None;
    let mut agent_dir = None;
    let mut pi_path = None;
    let mut pi_version = None;
    let mut has_models = false;
    let mut has_sessions = false;
    let mut session_count = 0u32;

    for line in stdout.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim_end_matches('\r').trim();
        match key {
            "home" if !value.is_empty() => home = Some(value.to_string()),
            "agentDir" if !value.is_empty() => agent_dir = Some(value.to_string()),
            "piPath" if !value.is_empty() => pi_path = Some(value.to_string()),
            "piVersion" if !value.is_empty() => pi_version = Some(value.to_string()),
            "models" => has_models = value == "1",
            "sessions" => has_sessions = value == "1",
            "sessionCount" => session_count = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    let home = home.ok_or_else(|| {
        PiRuntimeError::command_failed(format!("could not resolve $HOME inside WSL '{distro}'"))
    })?;
    if !wsl::is_valid_linux_path(&home) {
        return Err(PiRuntimeError::command_failed(format!(
            "WSL '{distro}' reported an unusable home directory"
        )));
    }
    let agent_dir = agent_dir.unwrap_or_else(|| format!("{home}/.pi/agent"));
    if !wsl::is_valid_linux_path(&agent_dir) {
        return Err(PiRuntimeError::command_failed(format!(
            "WSL '{distro}' reported an unusable Pi agent directory"
        )));
    }

    let capabilities = PiCapabilities {
        models: has_models,
        sessions: has_sessions,
        // Usage and duration are both derived from the session JSONL stream,
        // so they are available exactly when sessions are.
        usage: has_sessions,
        duration: has_sessions,
    };

    Ok(WslPiProbe {
        distro: distro.to_string(),
        home,
        agent_dir,
        pi_path,
        pi_version: pi_version.map(|version| normalize_version(&version)),
        has_models,
        has_sessions,
        session_count,
        capabilities,
    })
}

/// `pi --version` output varies between bare versions and `pi/0.4.1 ...`
/// banners; keep the first token that looks like a version.
fn normalize_version(raw: &str) -> String {
    raw.split_whitespace()
        .find(|token| token.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .or_else(|| raw.split_whitespace().last())
        .unwrap_or(raw)
        .trim_start_matches('v')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_runtime::wsl::test_support::{
        posix_temp_home_available, LocalBashRunner, RunnerGuard,
    };
    use serial_test::serial;
    use std::sync::Arc;

    #[test]
    fn probe_output_is_parsed_into_paths_and_capabilities() {
        let probe = parse_probe(
            "Ubuntu-22.04",
            "home=/home/tfdx8045\npiPath=/home/tfdx8045/.nvm/versions/node/v22.0.0/bin/pi\npiVersion=v0.4.1\nagentDir=/home/tfdx8045/.pi/agent\nmodels=1\nsessions=1\nsessionCount=137\n",
        )
        .expect("parse probe");

        assert_eq!(probe.home, "/home/tfdx8045");
        assert_eq!(probe.agent_dir, "/home/tfdx8045/.pi/agent");
        assert_eq!(probe.pi_version.as_deref(), Some("0.4.1"));
        assert_eq!(probe.session_count, 137);
        assert!(probe.pi_installed());
        assert_eq!(
            probe.capabilities,
            PiCapabilities {
                models: true,
                sessions: true,
                usage: true,
                duration: true
            }
        );
    }

    #[test]
    fn a_missing_pi_binary_is_reported_without_failing_discovery() {
        let probe = parse_probe(
            "Ubuntu-22.04",
            "home=/home/me\nagentDir=/home/me/.pi/agent\nmodels=0\nsessions=0\nsessionCount=0\n",
        )
        .expect("parse probe");

        assert!(!probe.pi_installed());
        assert_eq!(probe.pi_version, None);
        assert!(!probe.capabilities.sessions);
    }

    #[test]
    fn a_missing_home_is_a_hard_failure() {
        let error = parse_probe("Ubuntu-22.04", "models=0\n").expect_err("expected failure");
        assert!(error.to_string().contains("$HOME"));
    }

    #[test]
    fn version_banners_are_reduced_to_a_version() {
        assert_eq!(normalize_version("v0.4.1"), "0.4.1");
        assert_eq!(normalize_version("pi 0.4.1 (linux-x64)"), "0.4.1");
        assert_eq!(normalize_version("0.4.1"), "0.4.1");
    }

    #[test]
    fn docker_managed_distros_are_not_offered_as_runtime_targets() {
        assert!(is_infrastructure_distro("docker-desktop"));
        assert!(is_infrastructure_distro("docker-desktop-data"));
        assert!(!is_infrastructure_distro("Ubuntu-22.04"));
    }

    #[test]
    #[serial]
    fn the_probe_script_runs_against_a_real_shell() {
        if !posix_temp_home_available() {
            return;
        }
        let home = tempfile::tempdir().expect("tempdir");
        let agent = home.path().join(".pi/agent/sessions/project-a");
        std::fs::create_dir_all(&agent).expect("create sessions");
        std::fs::write(agent.join("one.jsonl"), "{}\n").expect("write session");
        std::fs::write(agent.join("two.jsonl"), "{}\n").expect("write session");
        std::fs::write(
            home.path().join(".pi/agent/models.json"),
            "{\"providers\":{}}\n",
        )
        .expect("write models");

        let _guard = RunnerGuard::install(Arc::new(LocalBashRunner::new(
            "Ubuntu-22.04",
            home.path().to_path_buf(),
        )));

        let probe = probe("Ubuntu-22.04").expect("probe distro");
        assert_eq!(probe.home, home.path().to_string_lossy());
        assert!(probe.has_models);
        assert!(probe.has_sessions);
        assert_eq!(probe.session_count, 2);
    }

    #[test]
    fn preferred_distro_prefers_a_configured_pi() {
        let bare = WslPiProbe {
            distro: "Debian".to_string(),
            home: "/home/me".to_string(),
            agent_dir: "/home/me/.pi/agent".to_string(),
            pi_path: Some("/usr/bin/pi".to_string()),
            pi_version: None,
            has_models: false,
            has_sessions: false,
            session_count: 0,
            capabilities: PiCapabilities::default(),
        };
        let configured = WslPiProbe {
            distro: "Ubuntu-22.04".to_string(),
            has_models: true,
            ..bare.clone()
        };

        let probes = vec![bare, configured];
        assert_eq!(
            preferred_distro(&probes).map(|probe| probe.distro.as_str()),
            Some("Ubuntu-22.04")
        );
    }
}
