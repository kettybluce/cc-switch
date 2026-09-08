//! Reaching a Windows-side proxy from inside WSL.
//!
//! `127.0.0.1` does not mean the same thing on both sides of the WSL boundary.
//! Under mirrored networking (Windows 11 22H2+, `networkingMode=mirrored`) the
//! loopback address is shared, so a listener bound to `127.0.0.1` on Windows is
//! reachable from Linux. Under the default NAT mode it is not: the Windows host
//! appears as the default gateway of the distribution's virtual switch instead.
//!
//! Rather than guess, the resolver probes the candidates in priority order and
//! reports which one answered (design document §14–§16).
//!
//! # Scope
//!
//! Proxy settings are applied to the Pi process only, by prefixing its launch
//! with `env`. CC Switch never edits `/etc/environment`, `/etc/profile` or
//! `~/.bashrc`, so `git`, `npm`, `maven` and everything else in the
//! distribution keep their own network configuration (design document §44–§45).

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use serde::Serialize;

use super::error::{PiResult, PiRuntimeError};
use super::wsl::{self, WslRequest};

/// One round trip that collects both host-discovery inputs.
const HOST_SCRIPT: &str = r#"
set -u
printf 'gateway=%s\n' "$(ip route show default 2>/dev/null | awk '/^default/ {print $3; exit}')"
printf 'nameserver=%s\n' "$(awk '/^nameserver/ {print $2; exit}' /etc/resolv.conf 2>/dev/null)"
"#;

/// `$1` port, `$2` per-probe timeout in seconds, `$3..` candidate hosts.
///
/// Probing every candidate in one call keeps the whole resolution to a single
/// `wsl.exe` invocation.
const PROBE_SCRIPT: &str = r#"
set -u
port="$1"; timeout="$2"; shift 2
if ! command -v curl >/dev/null 2>&1; then printf 'no-curl\n'; exit 0; fi
for host in "$@"; do
  start=$(date +%s%3N)
  code=$(curl -s -o /dev/null -m "$timeout" -w '%{http_code}' "http://$host:$port/health" 2>/dev/null || printf '000')
  end=$(date +%s%3N)
  printf 'host=%s code=%s latency=%s\n' "$host" "$code" "$((end - start))"
done
"#;

/// `$1` proxy URL, `$2` target URL, `$3` timeout in seconds.
///
/// Verifies the whole path a real request takes: TCP to the proxy, CONNECT,
/// TLS to the origin and an HTTP response (design document §50).
const FORWARD_PROBE_SCRIPT: &str = r#"
set -u
proxy="$1"; target="$2"; timeout="$3"
if ! command -v curl >/dev/null 2>&1; then printf 'no-curl\n'; exit 0; fi
start=$(date +%s%3N)
code=$(curl -s -o /dev/null -m "$timeout" --proxy "$proxy" -w '%{http_code}' "$target" 2>/tmp/cc-switch-proxy-probe || printf '000')
end=$(date +%s%3N)
printf 'code=%s latency=%s\n' "$code" "$((end - start))"
if [ "$code" = "000" ]; then head -c 400 /tmp/cc-switch-proxy-probe; fi
rm -f /tmp/cc-switch-proxy-probe
"#;

const PROBE_TIMEOUT_SECONDS: u32 = 3;
const FORWARD_PROBE_TARGET: &str = "https://example.com";

/// Proxy protocols CC Switch can hand to Pi.
///
/// Keeping these distinct is what prevents the `UnsupportedProxyProtocol`
/// class of failure: a SOCKS proxy written as `HTTP_PROXY=http://…` fails at
/// request time, far away from the setting that caused it (§47).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyProtocol {
    Http,
    Https,
    Socks5,
    Socks5h,
}

impl ProxyProtocol {
    pub fn scheme(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Socks5 => "socks5",
            Self::Socks5h => "socks5h",
        }
    }

    pub fn from_scheme(scheme: &str) -> Option<Self> {
        match scheme.to_ascii_lowercase().as_str() {
            "http" => Some(Self::Http),
            "https" => Some(Self::Https),
            "socks5" => Some(Self::Socks5),
            // `socks5h` resolves DNS at the proxy, which is what a tunnel to a
            // Windows-side client almost always wants.
            "socks5h" | "socks" => Some(Self::Socks5h),
            _ => None,
        }
    }

    pub fn is_socks(self) -> bool {
        matches!(self, Self::Socks5 | Self::Socks5h)
    }
}

/// How the WSL-reachable host was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HostStrategy {
    /// WSL mirrored networking shares the loopback address.
    MirroredLoopback,
    /// NAT mode: the Windows host is the distribution's default gateway.
    DefaultGateway,
    /// Fallback for setups where `ip route` is unavailable but WSL still
    /// publishes the host address as the resolver.
    ResolvConf,
}

impl HostStrategy {
    pub fn describe(self) -> &'static str {
        match self {
            Self::MirroredLoopback => "WSL mirrored networking (shared loopback)",
            Self::DefaultGateway => "WSL NAT gateway",
            Self::ResolvConf => "/etc/resolv.conf nameserver",
        }
    }
}

/// A candidate address for the Windows host, as seen from WSL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCandidate {
    pub host: String,
    pub strategy: HostStrategy,
}

/// Result of probing CC Switch's local endpoint from inside WSL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyHealth {
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<HostStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ProxyProtocol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ProxyHealth {
    fn unreachable(error: impl Into<String>) -> Self {
        Self {
            reachable: false,
            endpoint: None,
            host: None,
            strategy: None,
            latency_ms: None,
            protocol: None,
            error: Some(super::redact(&error.into())),
        }
    }
}

/// A proxy endpoint Pi can be pointed at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyEndpoint {
    pub protocol: ProxyProtocol,
    pub host: String,
    pub port: u16,
    /// Credentials, if the configured proxy URL carried any. Never logged.
    #[serde(skip)]
    pub userinfo: Option<String>,
}

impl ProxyEndpoint {
    pub fn url(&self) -> String {
        match &self.userinfo {
            Some(userinfo) => format!(
                "{}://{userinfo}@{}:{}",
                self.protocol.scheme(),
                self.host,
                self.port
            ),
            None => format!("{}://{}:{}", self.protocol.scheme(), self.host, self.port),
        }
    }

    /// URL with credentials replaced, safe for logs and the UI.
    pub fn display_url(&self) -> String {
        let userinfo = if self.userinfo.is_some() { "***@" } else { "" };
        format!(
            "{}://{userinfo}{}:{}",
            self.protocol.scheme(),
            self.host,
            self.port
        )
    }

    /// Parse a proxy URL as stored in CC Switch's global proxy setting.
    pub fn parse(url: &str) -> PiResult<Self> {
        let url = url.trim();
        let (scheme, rest) = url.split_once("://").ok_or_else(|| {
            PiRuntimeError::proxy_protocol(format!(
                "proxy URL is missing a scheme: {}",
                super::redact(url)
            ))
        })?;
        let protocol = ProxyProtocol::from_scheme(scheme).ok_or_else(|| {
            PiRuntimeError::proxy_protocol(format!("unsupported proxy protocol: {scheme}"))
        })?;

        let rest = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let (userinfo, authority) = match rest.rsplit_once('@') {
            Some((userinfo, authority)) => (Some(userinfo.to_string()), authority),
            None => (None, rest),
        };
        let (host, port) = authority.rsplit_once(':').ok_or_else(|| {
            PiRuntimeError::proxy_protocol("proxy URL is missing a port".to_string())
        })?;
        let port: u16 = port.parse().map_err(|_| {
            PiRuntimeError::proxy_protocol(format!("proxy URL has an invalid port: {port}"))
        })?;
        if host.is_empty() {
            return Err(PiRuntimeError::proxy_protocol(
                "proxy URL is missing a host".to_string(),
            ));
        }

        Ok(Self {
            protocol,
            host: host.trim_matches(['[', ']']).to_string(),
            port,
            userinfo,
        })
    }

    /// Whether this endpoint points at the machine CC Switch runs on.
    pub fn is_loopback(&self) -> bool {
        matches!(
            self.host.as_str(),
            "127.0.0.1" | "localhost" | "::1" | "0.0.0.0"
        ) || self.host.starts_with("127.")
    }

    /// Replace a loopback host with an address WSL can actually route to.
    pub fn rehost(&self, host: &str) -> Self {
        Self {
            host: host.to_string(),
            ..self.clone()
        }
    }
}

/// Everything the Pi Proxy panel needs, and everything a Pi launch needs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiProxyPlan {
    pub enabled: bool,
    /// CC Switch's own local endpoint as seen from WSL.
    pub gateway: ProxyHealth,
    /// The upstream forward proxy Pi should use, if one is configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_proxy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_proxy_health: Option<ProxyHealth>,
    /// Environment applied to the Pi process only.
    pub environment: BTreeMap<String, String>,
}

/// Parse the third field of `ip route show default`.
pub fn parse_default_gateway(output: &str) -> Option<Ipv4Addr> {
    output
        .lines()
        .map(str::trim)
        .find_map(|line| line.parse::<Ipv4Addr>().ok())
        .filter(|address| !address.is_loopback() && !address.is_unspecified())
}

/// Parse the first `nameserver` entry of `/etc/resolv.conf`.
pub fn parse_resolv_nameserver(output: &str) -> Option<Ipv4Addr> {
    output
        .lines()
        .map(str::trim)
        .find_map(|line| line.parse::<Ipv4Addr>().ok())
        .filter(|address| !address.is_loopback() && !address.is_unspecified())
}

/// Candidate host addresses for the Windows side, in probe order.
pub fn host_candidates(distro: &str) -> PiResult<Vec<HostCandidate>> {
    let output = wsl::run(
        &WslRequest::guarded(distro, HOST_SCRIPT).timeout(wsl::PROBE_TIMEOUT),
    )?;
    output.require_success("resolving the WSL host address")?;

    let payload = output.payload_lossy();
    let mut gateway = None;
    let mut nameserver = None;
    for line in payload.lines() {
        match line.split_once('=') {
            Some(("gateway", value)) => gateway = parse_default_gateway(value),
            Some(("nameserver", value)) => nameserver = parse_resolv_nameserver(value),
            _ => {}
        }
    }
    Ok(build_candidates(gateway, nameserver))
}

fn build_candidates(
    gateway: Option<Ipv4Addr>,
    nameserver: Option<Ipv4Addr>,
) -> Vec<HostCandidate> {
    let mut candidates = vec![HostCandidate {
        host: "127.0.0.1".to_string(),
        strategy: HostStrategy::MirroredLoopback,
    }];
    if let Some(gateway) = gateway {
        candidates.push(HostCandidate {
            host: gateway.to_string(),
            strategy: HostStrategy::DefaultGateway,
        });
    }
    if let Some(nameserver) = nameserver {
        // Under NAT the resolver and the gateway are frequently the same
        // address; probing it twice would only slow resolution down.
        if !candidates
            .iter()
            .any(|candidate| candidate.host == nameserver.to_string())
        {
            candidates.push(HostCandidate {
                host: nameserver.to_string(),
                strategy: HostStrategy::ResolvConf,
            });
        }
    }
    candidates
}

/// One probe result line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeResult {
    host: String,
    code: u32,
    latency_ms: u32,
}

fn parse_probe_output(payload: &str) -> Vec<ProbeResult> {
    let mut results = Vec::new();
    for line in payload.lines() {
        let mut host = None;
        let mut code = None;
        let mut latency = None;
        for field in line.split_whitespace() {
            match field.split_once('=') {
                Some(("host", value)) => host = Some(value.to_string()),
                Some(("code", value)) => code = value.parse::<u32>().ok(),
                Some(("latency", value)) => latency = value.parse::<u32>().ok(),
                _ => {}
            }
        }
        if let (Some(host), Some(code)) = (host, code) {
            results.push(ProbeResult {
                host,
                code,
                latency_ms: latency.unwrap_or(0),
            });
        }
    }
    results
}

/// Any HTTP status means the listener answered; only `000` (curl's "no
/// response") means unreachable. A 404 still proves the port is ours.
fn is_reachable(code: u32) -> bool {
    code != 0
}

/// Find the address WSL can use to reach CC Switch's local endpoint on `port`.
pub fn resolve_gateway(distro: &str, port: u16) -> PiResult<ProxyHealth> {
    let candidates = host_candidates(distro)?;
    let mut request = WslRequest::guarded(distro, PROBE_SCRIPT)
        .arg(port.to_string())
        .arg(PROBE_TIMEOUT_SECONDS.to_string())
        .timeout(wsl::DEFAULT_TIMEOUT);
    for candidate in &candidates {
        request = request.arg(&candidate.host);
    }

    let output = wsl::run(&request)?;
    output.require_success("probing the CC Switch endpoint")?;
    let payload = output.payload_lossy();
    if wsl::first_line(&payload) == Some("no-curl") {
        return Ok(ProxyHealth::unreachable(format!(
            "curl is not installed in WSL '{distro}', so reachability cannot be verified"
        )));
    }

    let results = parse_probe_output(&payload);
    for candidate in &candidates {
        let Some(result) = results
            .iter()
            .find(|result| result.host == candidate.host && is_reachable(result.code))
        else {
            continue;
        };
        log::info!(
            "[PiProxy] CC Switch is reachable from WSL '{distro}' at {}:{port} via {}",
            candidate.host,
            candidate.strategy.describe()
        );
        return Ok(ProxyHealth {
            reachable: true,
            endpoint: Some(format!("http://{}:{port}", candidate.host)),
            host: Some(candidate.host.clone()),
            strategy: Some(candidate.strategy),
            latency_ms: Some(result.latency_ms),
            protocol: Some(ProxyProtocol::Http),
            error: None,
        });
    }

    let attempted = candidates
        .iter()
        .map(|candidate| candidate.host.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(ProxyHealth::unreachable(format!(
        "no route from WSL '{distro}' to CC Switch on port {port} (tried {attempted}). Check that the local proxy is running and that Windows Firewall allows the WSL subnet."
    )))
}

/// Verify that Pi can actually reach the internet through `endpoint` (§50–51).
pub fn validate_forward_proxy(distro: &str, endpoint: &ProxyEndpoint) -> PiResult<ProxyHealth> {
    let output = wsl::run(
        &WslRequest::guarded(distro, FORWARD_PROBE_SCRIPT)
            .arg(endpoint.url())
            .arg(FORWARD_PROBE_TARGET)
            .arg(PROBE_TIMEOUT_SECONDS.to_string())
            .timeout(wsl::DEFAULT_TIMEOUT),
    )?;
    output.require_success("probing the Pi proxy")?;

    let payload = output.payload_lossy();
    if wsl::first_line(&payload) == Some("no-curl") {
        return Ok(ProxyHealth::unreachable(format!(
            "curl is not installed in WSL '{distro}', so the proxy cannot be verified"
        )));
    }

    let results = parse_probe_output(&payload);
    let code = results.first().map(|result| result.code).unwrap_or(0);
    let latency = results.first().map(|result| result.latency_ms);

    if is_reachable(code) {
        return Ok(ProxyHealth {
            reachable: true,
            endpoint: Some(endpoint.display_url()),
            host: Some(endpoint.host.clone()),
            strategy: None,
            latency_ms: latency,
            protocol: Some(endpoint.protocol),
            error: None,
        });
    }

    let detail = payload
        .lines()
        .skip(1)
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no response through the proxy");
    Ok(ProxyHealth {
        reachable: false,
        endpoint: Some(endpoint.display_url()),
        host: Some(endpoint.host.clone()),
        strategy: None,
        latency_ms: None,
        protocol: Some(endpoint.protocol),
        error: Some(super::redact(detail.trim())),
    })
}

/// Environment for the Pi process, honouring the endpoint's real protocol.
///
/// The scheme is never rewritten: a SOCKS proxy stays `socks5h://` in every
/// variable, because silently downgrading it to `http://` is what produces
/// unsupported-protocol errors at request time.
pub fn pi_process_env(endpoint: &ProxyEndpoint) -> BTreeMap<String, String> {
    let url = endpoint.url();
    let mut environment = BTreeMap::new();
    environment.insert("ALL_PROXY".to_string(), url.clone());
    environment.insert("all_proxy".to_string(), url.clone());
    if !endpoint.protocol.is_socks() {
        environment.insert("HTTP_PROXY".to_string(), url.clone());
        environment.insert("http_proxy".to_string(), url.clone());
        environment.insert("HTTPS_PROXY".to_string(), url.clone());
        environment.insert("https_proxy".to_string(), url);
    }
    // Keep Pi's own loopback traffic (and the CC Switch endpoint) direct.
    environment.insert(
        "NO_PROXY".to_string(),
        "localhost,127.0.0.1,::1".to_string(),
    );
    environment.insert(
        "no_proxy".to_string(),
        "localhost,127.0.0.1,::1".to_string(),
    );
    environment
}

/// Build the command that launches Pi with the proxy applied to that process
/// only (design document §46).
pub fn build_pi_launch_argv(
    distro: &str,
    environment: &BTreeMap<String, String>,
    pi_args: &[String],
) -> PiResult<Vec<String>> {
    let mut script = String::from("exec env");
    let mut args = Vec::new();
    let mut index = 1;

    for _ in environment.keys() {
        script.push_str(&format!(" \"${index}\""));
        index += 1;
    }
    script.push_str(" pi");
    for _ in pi_args {
        script.push_str(&format!(" \"${index}\""));
        index += 1;
    }

    for (key, value) in environment {
        if key.is_empty() || key.contains('=') || key.chars().any(char::is_whitespace) {
            return Err(PiRuntimeError::invalid_input(format!(
                "invalid environment variable name: {key}"
            )));
        }
        args.push(format!("{key}={value}"));
    }
    args.extend(pi_args.iter().cloned());

    let request = WslRequest::new(distro, script).args(args).login(true);
    wsl::build_wsl_argv(&request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_runtime::wsl::test_support::{LocalBashRunner, RunnerGuard};
    use std::sync::Arc;

    #[test]
    fn the_wsl_nat_gateway_is_read_from_ip_route() {
        assert_eq!(
            parse_default_gateway("172.30.208.1"),
            Some(Ipv4Addr::new(172, 30, 208, 1))
        );
        assert_eq!(parse_default_gateway(""), None);
        assert_eq!(parse_default_gateway("127.0.0.1"), None);
        assert_eq!(parse_default_gateway("not-an-address"), None);
    }

    #[test]
    fn the_resolver_address_is_read_from_resolv_conf() {
        assert_eq!(
            parse_resolv_nameserver("10.255.255.254"),
            Some(Ipv4Addr::new(10, 255, 255, 254))
        );
        assert_eq!(parse_resolv_nameserver("0.0.0.0"), None);
    }

    #[test]
    fn candidates_follow_the_documented_priority_order() {
        let candidates = build_candidates(
            Some(Ipv4Addr::new(172, 30, 208, 1)),
            Some(Ipv4Addr::new(10, 255, 255, 254)),
        );

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| (candidate.host.as_str(), candidate.strategy))
                .collect::<Vec<_>>(),
            vec![
                ("127.0.0.1", HostStrategy::MirroredLoopback),
                ("172.30.208.1", HostStrategy::DefaultGateway),
                ("10.255.255.254", HostStrategy::ResolvConf),
            ]
        );
    }

    #[test]
    fn a_shared_gateway_and_resolver_are_probed_once() {
        let address = Ipv4Addr::new(172, 30, 208, 1);
        let candidates = build_candidates(Some(address), Some(address));
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn loopback_only_setups_still_yield_a_candidate() {
        assert_eq!(build_candidates(None, None).len(), 1);
    }

    #[test]
    fn proxy_urls_are_parsed_with_their_protocol_preserved() {
        let http = ProxyEndpoint::parse("http://127.0.0.1:7890").expect("parse http proxy");
        assert_eq!(http.protocol, ProxyProtocol::Http);
        assert_eq!(http.port, 7890);
        assert!(http.is_loopback());

        let socks = ProxyEndpoint::parse("socks5://127.0.0.1:7891").expect("parse socks proxy");
        assert_eq!(socks.protocol, ProxyProtocol::Socks5);
        assert!(socks.protocol.is_socks());

        let socks_h = ProxyEndpoint::parse("socks5h://proxy.internal:1080").expect("parse socks5h");
        assert_eq!(socks_h.protocol, ProxyProtocol::Socks5h);
        assert!(!socks_h.is_loopback());
    }

    #[test]
    fn unsupported_protocols_are_rejected_rather_than_coerced() {
        for url in ["ftp://127.0.0.1:21", "127.0.0.1:7890", "http://127.0.0.1"] {
            assert!(
                ProxyEndpoint::parse(url).is_err(),
                "expected '{url}' to be rejected"
            );
        }
    }

    #[test]
    fn credentials_never_appear_in_the_display_url() {
        let endpoint =
            ProxyEndpoint::parse("http://user:s3cret@127.0.0.1:7890").expect("parse proxy");
        assert_eq!(endpoint.display_url(), "http://***@127.0.0.1:7890");
        assert!(endpoint.url().contains("user:s3cret"));
        assert!(!serde_json::to_string(&endpoint)
            .expect("serialize endpoint")
            .contains("s3cret"));
    }

    #[test]
    fn loopback_endpoints_are_rehosted_for_wsl() {
        let endpoint = ProxyEndpoint::parse("http://127.0.0.1:7890").expect("parse proxy");
        let rehosted = endpoint.rehost("172.30.208.1");
        assert_eq!(rehosted.url(), "http://172.30.208.1:7890");
        assert_eq!(rehosted.protocol, ProxyProtocol::Http);
    }

    #[test]
    fn http_proxies_populate_every_conventional_variable() {
        let endpoint = ProxyEndpoint::parse("http://172.30.208.1:7890").expect("parse proxy");
        let environment = pi_process_env(&endpoint);

        assert_eq!(
            environment.get("HTTP_PROXY").map(String::as_str),
            Some("http://172.30.208.1:7890")
        );
        assert_eq!(
            environment.get("HTTPS_PROXY").map(String::as_str),
            Some("http://172.30.208.1:7890")
        );
        assert_eq!(
            environment.get("ALL_PROXY").map(String::as_str),
            Some("http://172.30.208.1:7890")
        );
        assert!(environment.get("NO_PROXY").is_some());
    }

    #[test]
    fn socks_proxies_are_never_advertised_as_http() {
        let endpoint = ProxyEndpoint::parse("socks5://172.30.208.1:7891").expect("parse proxy");
        let environment = pi_process_env(&endpoint);

        assert_eq!(
            environment.get("ALL_PROXY").map(String::as_str),
            Some("socks5://172.30.208.1:7891")
        );
        assert!(
            !environment.contains_key("HTTP_PROXY"),
            "a SOCKS proxy must not be exported as HTTP_PROXY"
        );
    }

    #[test]
    fn probe_output_is_parsed_per_candidate() {
        let results = parse_probe_output(
            "host=127.0.0.1 code=000 latency=3005\nhost=172.30.208.1 code=200 latency=12\n",
        );

        assert_eq!(results.len(), 2);
        assert!(!is_reachable(results[0].code));
        assert!(is_reachable(results[1].code));
        assert_eq!(results[1].latency_ms, 12);
    }

    #[test]
    fn any_http_status_counts_as_reachable() {
        // A 404 still proves something is listening on the port.
        assert!(is_reachable(404));
        assert!(is_reachable(500));
        assert!(!is_reachable(0));
    }

    #[test]
    fn the_launch_command_keeps_the_proxy_scoped_to_the_pi_process() {
        let endpoint = ProxyEndpoint::parse("http://172.30.208.1:7890").expect("parse proxy");
        let environment = pi_process_env(&endpoint);

        let argv = build_pi_launch_argv(
            "Ubuntu-22.04",
            &environment,
            &["--session".to_string(), "/home/me/s.jsonl".to_string()],
        )
        .expect("build launch argv");

        assert_eq!(&argv[..5], &["-d", "Ubuntu-22.04", "--", "bash", "-lc"]);
        // Values are positional parameters, so no proxy URL is ever spliced
        // into the script text.
        assert!(!argv[5].contains("172.30.208.1"));
        assert!(argv[5].starts_with("exec env "));
        assert!(argv.contains(&"HTTP_PROXY=http://172.30.208.1:7890".to_string()));
        assert!(argv.contains(&"/home/me/s.jsonl".to_string()));

        // One placeholder per value, in order: the environment assignments,
        // then `pi`, then Pi's own arguments.
        let placeholders = argv[5].matches("\"$").count();
        let values = &argv[6..];
        assert_eq!(placeholders, environment.len() + 2);
        assert_eq!(values.len(), environment.len() + 3);
        assert!(argv[5].ends_with(&format!(
            " pi \"${}\" \"${}\"",
            environment.len() + 1,
            environment.len() + 2
        )));
    }

    #[test]
    fn the_launch_command_works_without_a_proxy() {
        let argv = build_pi_launch_argv("Ubuntu-22.04", &BTreeMap::new(), &[])
            .expect("build launch argv");
        assert_eq!(argv[5], "exec env pi");
    }

    #[test]
    fn host_discovery_runs_against_a_real_shell() {
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = RunnerGuard::install(Arc::new(LocalBashRunner::new(
            "Ubuntu-22.04",
            home.path().to_path_buf(),
        )));

        // `ip` and `/etc/resolv.conf` may or may not exist on the test host;
        // either way discovery must return the loopback candidate and never
        // fail.
        let candidates = host_candidates("Ubuntu-22.04").expect("resolve candidates");
        assert_eq!(candidates[0].strategy, HostStrategy::MirroredLoopback);
    }
}
