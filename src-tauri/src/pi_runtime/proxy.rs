//! Reaching the CC Switch local proxy from Pi.
//!
//! Pi takeover points the live `models.json` `baseUrl`s at the local proxy.
//! Process-level `HTTP_PROXY` injection is not used.
//!
//! `127.0.0.1` does not mean the same thing on both sides of the WSL boundary.
//! Under mirrored networking (Windows 11 22H2+, `networkingMode=mirrored`) the
//! loopback address is shared, so a listener bound to `127.0.0.1` on Windows is
//! reachable from Linux. `firewall=true` does not change that: a curl to
//! `http://127.0.0.1:<port>/` that returns any HTTP status (including 404)
//! already proves Hyper-V/WSL firewall is not blocking loopback.
//!
//! Mirrored is **not** NAT, even when `ip route` still shows a default via
//! `172.30.x.x` on eth1 and `dnsTunneling=true` publishes nameserver
//! `10.255.255.254`. Those addresses are fallbacks. They are often
//! connection-refused when CC Switch listens on loopback only, and that is
//! fine — do not require them, and do not treat their presence as NAT mode.
//!
//! Under the default NAT mode, loopback is not shared: the Windows host
//! appears as the default gateway of the distribution's virtual switch.
//! Switching the listen address to `0.0.0.0` is only a NAT fallback, not the
//! mirrored fix.
//!
//! Rather than guess, the resolver probes the candidates in priority order and
//! reports which one answered. The first success (usually `127.0.0.1`) wins.
//! The rewritten `baseUrl` uses that host so Pi inside WSL can reach CC Switch.
//!
//! CC Switch never edits `/etc/environment`, `/etc/profile` or `~/.bashrc`.

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

/// Shown when the local proxy is stopped (port 0) or every probe is
/// connection-refused. The settings toast maps this to 「请先开启本地代理」.
pub const PROXY_NOT_RUNNING: &str = "please start the local proxy";

/// `$1` port, `$2` per-probe timeout in seconds, `$3..` candidate hosts.
///
/// One `wsl.exe` call. `/` is probed first: mirrored WSL often gets HTTP 404
/// on the root (the listener answered) while `/health` can hang or be
/// mis-parsed. `/ping` is the same listener (Windows-side health alias).
/// Any 1xx–5xx is success; `000` is timeout / refused / reset.
/// The first reachable host (usually `127.0.0.1`) stops the loop so NAT
/// candidates are not required when localhost already works.
///
/// Port / hosts are also embedded as fallbacks: some `wsl.exe -- bash -c`
/// wrappers drop `$1` so `curl http://127.0.0.1:` never hits the listener
/// and the UI toasts "no route" while Settings already shows green.
const PROBE_SCRIPT_BODY: &str = r#"
set +e
if ! command -v curl >/dev/null 2>&1; then printf 'no-curl\n'; exit 0; fi
for host in "$@"; do
  code="000"
  start=$(date +%s 2>/dev/null || printf '0')
  for path in / /health /ping; do
    raw=$(curl -sS -o /dev/null -m "$timeout" -w '%{http_code}' "http://$host:$port$path" 2>/dev/null)
    raw=$(printf '%s' "$raw" | tr -cd '0-9')
    case "$raw" in
      [1-5][0-9][0-9]*)
        code=$(printf '%s' "$raw" | cut -c1-3)
        break
        ;;
    esac
  done
  end=$(date +%s 2>/dev/null || printf '0')
  latency=0
  if [ -n "$start" ] && [ -n "$end" ]; then
    latency=$((end - start))
    latency=$((latency * 1000))
  fi
  printf 'host=%s code=%s latency=%s\n' "$host" "$code" "$latency"
  case "$code" in
    [1-5][0-9][0-9]) exit 0 ;;
  esac
done
exit 0
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
/// Option B does not inject process-level proxy env vars. The non-HTTP
/// variants and the parse/env helpers below are kept for an explicit
/// fallback that is off by default.
///
/// Keeping these distinct is what prevents the `UnsupportedProxyProtocol`
/// class of failure: a SOCKS proxy written as `HTTP_PROXY=http://…` fails at
/// request time, far away from the setting that caused it (§47).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyProtocol {
    Http,
    Https,
    Socks5,
    Socks5h,
}

#[allow(dead_code)]
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
            Self::DefaultGateway => "WSL default route (NAT gateway or mirrored eth1)",
            Self::ResolvConf => "/etc/resolv.conf nameserver (dnsTunneling uses 10.255.255.254)",
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
#[allow(dead_code)]
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

#[allow(dead_code)]
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

/// Everything the Pi Proxy panel needs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiProxyPlan {
    pub enabled: bool,
    /// Whether live `models.json` currently points at the local proxy.
    pub projected: bool,
    /// CC Switch's own local endpoint as seen from this runtime.
    pub gateway: ProxyHealth,
    /// `http://host:port` used as the origin of rewritten `baseUrl`s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Local proxy listen address (e.g. `127.0.0.1` or `0.0.0.0`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen_address: Option<String>,
    /// Unused: Option B does not inject process-level proxy env vars.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_proxy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_proxy_health: Option<ProxyHealth>,
    /// Kept empty. Env injection is not the default path.
    pub environment: BTreeMap<String, String>,
}

/// Build the proxy plan for the Pi runtime.
///
/// `enabled` is whether Pi should follow the local proxy (default on;
/// `flags.wsl_proxy` is only an advanced opt-out). The rewritten `baseUrl`
/// host is the address *this runtime* can reach: loopback on the machine CC
/// Switch runs on, or the probed WSL host inside a distribution.
pub fn plan(
    target: &super::PiRuntimeTarget,
    enabled: bool,
    local_proxy_port: u16,
    _configured: Option<&str>,
) -> PiResult<PiProxyPlan> {
    let gateway = local_gateway(target, local_proxy_port)?;
    let origin = gateway.endpoint.clone();
    let projected = enabled && origin.is_some();

    Ok(PiProxyPlan {
        enabled: enabled && local_proxy_port != 0,
        projected,
        gateway,
        origin,
        listen_address: None,
        forward_proxy: None,
        forward_proxy_health: None,
        environment: BTreeMap::new(),
    })
}

/// Settings-panel snapshot. Does **not** curl WSL.
///
/// `plan()` / `resolve_gateway()` take 1–9s when the old probe timed out
/// three hosts. React Query used to re-run that on every window focus, which
/// felt like a ~1s click delay. Live reachability stays on 「检测代理」 and
/// on projection (`resolve_origin`).
pub fn plan_ui_snapshot(
    target: &super::PiRuntimeTarget,
    enabled: bool,
    local_proxy_port: u16,
) -> PiProxyPlan {
    let origin = (local_proxy_port != 0).then(|| format!("http://127.0.0.1:{local_proxy_port}"));
    let gateway = if local_proxy_port == 0 {
        ProxyHealth::unreachable(PROXY_NOT_RUNNING)
    } else {
        ProxyHealth {
            reachable: true,
            endpoint: origin.clone(),
            host: Some("127.0.0.1".to_string()),
            strategy: target.is_wsl().then_some(HostStrategy::MirroredLoopback),
            latency_ms: None,
            protocol: Some(ProxyProtocol::Http),
            error: None,
        }
    };

    PiProxyPlan {
        enabled: enabled && local_proxy_port != 0,
        projected: enabled && origin.is_some(),
        gateway,
        origin,
        listen_address: None,
        forward_proxy: None,
        forward_proxy_health: None,
        environment: BTreeMap::new(),
    }
}

fn local_gateway(target: &super::PiRuntimeTarget, local_proxy_port: u16) -> PiResult<ProxyHealth> {
    if local_proxy_port == 0 {
        return Ok(ProxyHealth::unreachable(PROXY_NOT_RUNNING));
    }
    match target.distro() {
        Some(distro) => resolve_gateway(distro, local_proxy_port),
        None => Ok(ProxyHealth {
            reachable: true,
            endpoint: Some(format!("http://127.0.0.1:{local_proxy_port}")),
            host: Some("127.0.0.1".to_string()),
            strategy: None,
            latency_ms: None,
            protocol: Some(ProxyProtocol::Http),
            error: None,
        }),
    }
}

/// Verify that this runtime can reach the CC Switch local proxy `/health`.
///
/// This is proxy reachability, not provider health: a 200 here does not
/// mean the upstream API accepted a key.
pub async fn verify(
    target: &super::PiRuntimeTarget,
    local_proxy_port: u16,
) -> PiResult<ProxyHealth> {
    if local_proxy_port == 0 {
        return Ok(ProxyHealth::unreachable(PROXY_NOT_RUNNING));
    }
    match target.distro() {
        Some(distro) => resolve_gateway(distro, local_proxy_port),
        None => probe_local_health(local_proxy_port).await,
    }
}

async fn probe_local_health(port: u16) -> PiResult<ProxyHealth> {
    // Same success rule as the WSL probe: any HTTP status means the
    // listener answered. Do not require `/ping` (Windows-only 404/refused
    // used to disagree with a working WSL curl to `/`).
    let url = format!("http://127.0.0.1:{port}/");
    let start = std::time::Instant::now();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .no_proxy()
        .build()
        .map_err(|error| {
            PiRuntimeError::proxy_unreachable(format!("cannot build health client: {error}"))
        })?;
    match client.get(&url).send().await {
        Ok(response) => {
            let latency = start.elapsed().as_millis() as u32;
            if is_reachable(response.status().as_u16() as u32) {
                Ok(ProxyHealth {
                    reachable: true,
                    endpoint: Some(format!("http://127.0.0.1:{port}")),
                    host: Some("127.0.0.1".to_string()),
                    strategy: None,
                    latency_ms: Some(latency),
                    protocol: Some(ProxyProtocol::Http),
                    error: None,
                })
            } else {
                Ok(ProxyHealth::unreachable(format!(
                    "local proxy health check returned {}",
                    response.status()
                )))
            }
        }
        Err(error) => {
            let detail = error.to_string();
            if looks_like_proxy_off(&detail) {
                Ok(ProxyHealth::unreachable(PROXY_NOT_RUNNING))
            } else {
                Ok(ProxyHealth::unreachable(format!(
                    "local proxy health check failed: {error}"
                )))
            }
        }
    }
}

/// Connection-refused / `/ping` / "not running" all mean the listener is off.
pub fn looks_like_proxy_off(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    lower.contains("please start the local proxy")
        || lower.contains("local proxy is not running")
        || lower.contains("connection refused")
        || lower.contains("error sending request")
        || lower.contains("/ping")
        || lower.contains("请先开启本地代理")
}

/// A loopback proxy has to be rewritten to the address WSL can route to.
///
/// Kept for the optional env-injection fallback (`pi_process_env`); Option B
/// does not use it on the default path.
#[allow(dead_code)]
fn rehost_for_wsl(endpoint: ProxyEndpoint, gateway: &ProxyHealth) -> PiResult<ProxyEndpoint> {
    if !endpoint.is_loopback() {
        return Ok(endpoint);
    }
    let Some(host) = gateway.host.as_deref() else {
        return Err(PiRuntimeError::proxy_unreachable(
            "cannot reach a loopback proxy from WSL: no route to the Windows host was found",
        ));
    };
    Ok(endpoint.rehost(host))
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
    let output = wsl::run(&WslRequest::guarded(distro, HOST_SCRIPT).timeout(wsl::PROBE_TIMEOUT))?;
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

fn build_candidates(gateway: Option<Ipv4Addr>, nameserver: Option<Ipv4Addr>) -> Vec<HostCandidate> {
    // Localhost first: mirrored + firewall=true reaches a 127.0.0.1 listener.
    // Gateway / nameserver are optional fallbacks (NAT *or* mirrored eth1 /
    // dnsTunneling). Never treat them as required, and never infer NAT-only
    // just because they appear in `ip route` / resolv.conf.
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
                Some(("code", value)) => code = parse_http_code(value),
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

/// Extract a 1xx–5xx status from curl `-w %{http_code}` output.
///
/// The old `code=$(curl … || printf 000)` form could concatenate to
/// `404000`. Leading 1xx–5xx still means the listener answered.
fn parse_http_code(raw: &str) -> Option<u32> {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).take(6).collect();
    if digits.len() >= 3 {
        if let Ok(first) = digits[..3].parse::<u32>() {
            if (100..600).contains(&first) {
                return Some(first);
            }
        }
    }
    match digits.parse::<u32>() {
        Ok(code) => Some(code),
        Err(_) => None,
    }
}

/// HTTP connectivity only. 2xx/3xx/4xx (including 404 on `/`) and 5xx all
/// prove the port is reachable. `000` is timeout / connection refused / reset.
fn is_reachable(code: u32) -> bool {
    (100..600).contains(&code)
}

fn is_safe_probe_host(host: &str) -> bool {
    host.parse::<Ipv4Addr>().is_ok() || host.eq_ignore_ascii_case("localhost")
}

/// Probe body with integer port / validated hosts baked in so a dropped `$1`
/// still curls `http://127.0.0.1:<port>/` instead of `http://127.0.0.1:`.
fn probe_script(port: u16, hosts: &[HostCandidate]) -> String {
    let fallback_hosts: Vec<&str> = hosts
        .iter()
        .map(|candidate| candidate.host.as_str())
        .filter(|host| is_safe_probe_host(host))
        .collect();
    let fallback = if fallback_hosts.is_empty() {
        "127.0.0.1".to_string()
    } else {
        fallback_hosts.join(" ")
    };
    format!(
        r#"
set +e
port="${{1:-{port}}}"
timeout="${{2:-{timeout}}}"
if [ -n "${{3:-}}" ]; then
  shift 2
else
  set -- {fallback}
fi
{body}
"#,
        port = port,
        timeout = PROBE_TIMEOUT_SECONDS,
        fallback = fallback,
        body = PROBE_SCRIPT_BODY,
    )
}

/// Find the address WSL can use to reach CC Switch's local endpoint on `port`.
pub fn resolve_gateway(distro: &str, port: u16) -> PiResult<ProxyHealth> {
    let candidates = host_candidates(distro)?;
    let mut request = WslRequest::guarded(distro, &probe_script(port, &candidates))
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
    // Candidates are already localhost-first (mirrored loopback → default
    // route → resolv). A 404 on 127.0.0.1 is enough; do not require the
    // other hosts. Mirrored+dnsTunneling still lists 172.30.x / 10.255.255.254.
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

    // A 1xx–5xx on any probed host is success even if the host string
    // did not match the candidate list (dropped argv / fallback `set --`).
    if let Some(result) = results.iter().find(|result| is_reachable(result.code)) {
        let strategy = candidates
            .iter()
            .find(|candidate| candidate.host == result.host)
            .map(|candidate| candidate.strategy)
            .unwrap_or(HostStrategy::MirroredLoopback);
        return Ok(ProxyHealth {
            reachable: true,
            endpoint: Some(format!("http://{}:{port}", result.host)),
            host: Some(result.host.clone()),
            strategy: Some(strategy),
            latency_ms: Some(result.latency_ms),
            protocol: Some(ProxyProtocol::Http),
            error: None,
        });
    }

    Ok(ProxyHealth::unreachable(PROXY_NOT_RUNNING))
}

/// Verify that Pi can actually reach the internet through `endpoint` (§50–51).
///
/// Optional env-injection fallback only; Option B tests `/health` instead.
#[allow(dead_code)]
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
/// Not used on the Option B default path. Kept so an explicit env-injection
/// fallback can reuse the same protocol-preserving mapping.
#[allow(dead_code)]
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
    fn mirrored_firewall_dns_tunneling_topology_still_lists_localhost_first() {
        // User machine: WSL 2.7.10, networkingMode=mirrored, firewall=true,
        // dnsTunneling=true. Default via 172.30.213.1 and nameserver
        // 10.255.255.254 are *not* evidence of NAT.
        let candidates = build_candidates(
            Some(Ipv4Addr::new(172, 30, 213, 1)),
            Some(Ipv4Addr::new(10, 255, 255, 254)),
        );
        assert_eq!(candidates[0].host, "127.0.0.1");
        assert_eq!(candidates[0].strategy, HostStrategy::MirroredLoopback);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.host.as_str())
                .collect::<Vec<_>>(),
            ["127.0.0.1", "172.30.213.1", "10.255.255.254"]
        );
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
        assert!(environment.contains_key("NO_PROXY"));
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
        // A 404 on `/` still proves something is listening (mirrored WSL).
        assert!(is_reachable(404));
        assert!(is_reachable(200));
        assert!(is_reachable(302));
        assert!(is_reachable(500));
        assert!(!is_reachable(0));
        assert_eq!(parse_http_code("404"), Some(404));
        assert_eq!(
            parse_http_code("404000"),
            Some(404),
            "old curl||printf 000 concatenation must not hide a 404"
        );
        assert_eq!(parse_http_code("000"), Some(0));
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

    #[test]
    fn the_local_runtime_plan_uses_loopback_and_does_not_inject_env() {
        let plan = plan(
            &crate::pi_runtime::PiRuntimeTarget::Local,
            true,
            15721,
            None,
        )
        .expect("plan");
        assert!(plan.enabled);
        assert!(plan.projected);
        assert_eq!(plan.origin.as_deref(), Some("http://127.0.0.1:15721"));
        assert!(plan.environment.is_empty());
        assert!(plan.forward_proxy.is_none());
    }

    #[test]
    fn a_stopped_proxy_is_not_projected() {
        let plan = plan(&crate::pi_runtime::PiRuntimeTarget::Local, true, 0, None).expect("plan");
        assert!(!plan.enabled);
        assert!(!plan.projected);
        assert!(plan.origin.is_none());
        assert_eq!(plan.gateway.error.as_deref(), Some(PROXY_NOT_RUNNING));
    }

    #[test]
    fn connection_refused_and_ping_errors_mean_start_the_proxy() {
        assert!(looks_like_proxy_off(
            r#"Get "http://127.0.0.1:15721/ping": connection refused"#
        ));
        assert!(looks_like_proxy_off(PROXY_NOT_RUNNING));
        assert!(looks_like_proxy_off(
            "the CC Switch local proxy is not running"
        ));
        assert!(!looks_like_proxy_off("HTTP 401 from the upstream provider"));
    }

    #[test]
    fn probe_script_embeds_port_so_dropped_argv_still_hits_localhost() {
        let candidates = build_candidates(
            Some(Ipv4Addr::new(172, 30, 213, 1)),
            Some(Ipv4Addr::new(10, 255, 255, 254)),
        );
        let script = probe_script(15721, &candidates);
        assert!(script.contains(r#"port="${1:-15721}""#) || script.contains("15721"));
        assert!(script.contains("/health"));
        assert!(script.contains("/ping"));
        assert!(script.contains("127.0.0.1"));
        assert!(script.contains("for host in"));
    }

    #[test]
    fn ui_snapshot_does_not_need_a_wsl_probe() {
        let wsl = crate::pi_runtime::PiRuntimeTarget::Wsl {
            distro: "Ubuntu-22.04".into(),
            home: "/home/tfdx8045".into(),
            agent_dir: "/home/tfdx8045/.pi/agent".into(),
        };
        let plan = plan_ui_snapshot(&wsl, true, 15721);
        assert_eq!(plan.origin.as_deref(), Some("http://127.0.0.1:15721"));
        assert_eq!(plan.gateway.host.as_deref(), Some("127.0.0.1"));
        assert_eq!(plan.gateway.strategy, Some(HostStrategy::MirroredLoopback));
        assert!(plan.gateway.reachable);
    }
}
