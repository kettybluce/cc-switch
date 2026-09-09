//! Default WSL topology fixture for this fork's self-tests.
//!
//! Encodes the production host (WSL 2.7.10, Ubuntu-22.04) as reported by the
//! user. This is **mirrored + firewall + dnsTunneling**, not generic NAT.
//!
//! `%USERPROFILE%\.wslconfig`:
//! ```text
//! [experimental]
//! sparseVhd=false
//! [wsl2]
//! networkingMode=mirrored
//! dnsTunneling=true
//! firewall=true
//! autoProxy=false
//! ```
//!
//! Measured from inside WSL while CC Switch listens on `127.0.0.1:15721` only:
//! - `curl http://127.0.0.1:15721/` → HTTP 404 (SUCCESS — listener answered)
//! - `172.30.213.1:15721` → connection refused
//! - `10.255.255.254:15721` → connection refused
//!
//! `ip route` still shows a default via eth1 and `resolv.conf` lists the
//! dnsTunneling nameserver. Those addresses are fallbacks, not evidence of NAT.

#![cfg(test)]

use super::wsl::test_support::LocalBashRunner;
use super::wsl::{is_usable_wsl_script, OUTPUT_SENTINEL};
use super::wsl::{WslExecResult, WslRequest, WslRunner};

/// Production profile captured from the blocked user's machine.
pub const USER_MIRRORED: MirroredProfile = MirroredProfile {
    distro: "Ubuntu-22.04",
    wsl_version: "2.7.10",
    listen_host: "127.0.0.1",
    proxy_port: 15721,
    eth1_gateway: "172.30.213.1",
    dns_tunnel: "10.255.255.254",
    docker0: "172.17.0.0/16",
    networking_mode: "mirrored",
    dns_tunneling: true,
    firewall: true,
    auto_proxy: false,
    sparse_vhd: false,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirroredProfile {
    pub distro: &'static str,
    pub wsl_version: &'static str,
    pub listen_host: &'static str,
    pub proxy_port: u16,
    pub eth1_gateway: &'static str,
    pub dns_tunnel: &'static str,
    pub docker0: &'static str,
    pub networking_mode: &'static str,
    pub dns_tunneling: bool,
    pub firewall: bool,
    pub auto_proxy: bool,
    pub sparse_vhd: bool,
}

impl MirroredProfile {
    pub fn wslconfig(&self) -> String {
        format!(
            "[experimental]\nsparseVhd={}\n[wsl2]\nnetworkingMode={}\ndnsTunneling={}\nfirewall={}\nautoProxy={}\n",
            self.sparse_vhd,
            self.networking_mode,
            self.dns_tunneling,
            self.firewall,
            self.auto_proxy,
        )
    }

    pub fn host_discovery_payload(&self) -> String {
        format!(
            "gateway={}\nnameserver={}\n",
            self.eth1_gateway, self.dns_tunnel
        )
    }

    /// Probe lines matching the user's measured host matrix.
    ///
    /// Localhost `/` returns 404 (reachable). eth1 / dnsTunnel hosts are
    /// connection-refused. The resolver must stop at localhost and must not
    /// toast "no route".
    pub fn probe_payload_localhost_404(&self) -> String {
        format!(
            "host={} code=404 latency=12\nhost={} code=000 latency=1\nhost={} code=000 latency=1\n",
            self.listen_host, self.eth1_gateway, self.dns_tunnel
        )
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}:{}", self.listen_host, self.proxy_port)
    }
}

/// WSL double that replays the user's mirrored host matrix.
///
/// Host discovery and the proxy probe are canned from [`USER_MIRRORED`].
/// File writes go through [`LocalBashRunner`]. When `drop_write_args` is
/// set, positional `$1…` are cleared so the 3.0.1 stdin→stage→sha256→`mv`
/// contract must survive via `set --` fallback — never `bash -c ''`.
#[derive(Debug)]
pub struct UserMirroredTopologyRunner {
    pub inner: LocalBashRunner,
    pub drop_write_args: bool,
}

impl UserMirroredTopologyRunner {
    pub fn new(inner: LocalBashRunner) -> Self {
        Self {
            inner,
            drop_write_args: true,
        }
    }
}

impl WslRunner for UserMirroredTopologyRunner {
    fn run(&self, request: &WslRequest) -> crate::pi_runtime::PiResult<WslExecResult> {
        assert!(
            is_usable_wsl_script(&request.script),
            "Pi models.json / probe must never invoke empty bash: {:?}",
            request.script
        );
        assert!(
            request.script.trim() != "''" && !request.script.trim().is_empty(),
            "script collapsed to empty quotes"
        );

        let script = &request.script;
        if script.contains("ip route show default") {
            return Ok(canned_stdout(&USER_MIRRORED.host_discovery_payload()));
        }
        if script.contains("for host in") && script.contains("/health") {
            return Ok(canned_stdout(&USER_MIRRORED.probe_payload_localhost_404()));
        }

        let mut next = request.clone();
        if self.drop_write_args {
            next.args.clear();
        }
        self.inner.run(&next)
    }

    fn list_distros(&self) -> crate::pi_runtime::PiResult<Vec<String>> {
        self.inner.list_distros()
    }
}

fn canned_stdout(payload: &str) -> WslExecResult {
    WslExecResult {
        exit_code: Some(0),
        stdout: format!("\n{OUTPUT_SENTINEL}\n{payload}").into_bytes(),
        stderr: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_the_user_mirrored_firewall_dnstunnel_matrix() {
        let profile = USER_MIRRORED;
        let config = profile.wslconfig();
        assert_eq!(profile.distro, "Ubuntu-22.04");
        assert_eq!(profile.wsl_version, "2.7.10");
        assert_eq!(profile.networking_mode, "mirrored");
        assert!(profile.firewall);
        assert!(profile.dns_tunneling);
        assert!(!profile.auto_proxy);
        assert!(!profile.sparse_vhd);
        assert!(config.contains("networkingMode=mirrored"));
        assert!(config.contains("dnsTunneling=true"));
        assert!(config.contains("firewall=true"));
        assert!(config.contains("autoProxy=false"));
        assert_eq!(profile.eth1_gateway, "172.30.213.1");
        assert_eq!(profile.dns_tunnel, "10.255.255.254");
        assert_eq!(profile.docker0, "172.17.0.0/16");
        assert_eq!(profile.proxy_port, 15721);
        assert_eq!(profile.listen_host, "127.0.0.1");
        assert_eq!(profile.endpoint(), "http://127.0.0.1:15721");
    }
}
