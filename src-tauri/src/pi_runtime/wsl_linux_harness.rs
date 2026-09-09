//! Linux-cloud harness for the WSL remote-control path.
//!
//! Real `wsl.exe` and a user's Ubuntu-22.04 are never touched. Tests install
//! [`super::wsl::test_support::LocalBashRunner`] (the same bash double as the
//! rest of `pi_runtime`) and a fake `~/.pi/agent` tree from
//! `tests/fixtures/pi-wsl/`.
//!
//! Covered here: nested session discovery through the runner `find` (not a
//! flat glob), JSONL usage with `cost: 0` priced from `models.json` (including
//! requested ≠ served), dual `models.json` heal/project/restore, cwd `--…--`
//! encode/decode (test-only), and the default
//! [`super::mirrored_topology::USER_MIRRORED`] host matrix (mirrored +
//! firewall + dnsTunneling, not generic NAT).

#![cfg(test)]

use super::detect;
use super::files::{self, PiFile, PiFileLocation};
use super::mirrored_topology::{UserMirroredTopologyRunner, USER_MIRRORED};
use super::proxy::{self, HostStrategy};
use super::session_jsonl_maxdepth_str;
use super::sessions;
use super::test_support::TestTarget;
use super::wsl::test_support::{LocalBashRunner, RunnerGuard};
use super::wsl::{WslExecResult, WslRunner};
use super::PiRuntimeTarget;
use super::SESSION_JSONL_MAXDEPTH;
use crate::app_config::AppType;
use crate::database::Database;
use crate::provider::Provider;
use crate::services::provider::ProviderService;
use crate::services::session_usage_pi::sync_pi_usage;
use crate::session_manager::providers::pi::{
    decode_session_cwd, encode_session_cwd, scan_sessions,
};
use crate::store::AppState;
use rust_decimal::Decimal;
use serde_json::json;
use serial_test::serial;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

const DISTRO: &str = "Ubuntu-22.04";
const CWD: &str = "/home/tfdx8045/code/agent";
const CWD_GROUP: &str = "--home-tfdx8045-code-agent--";
const PARENT_SESSION: &str = "sess-parent-abc123";
const TASK_SESSION: &str = "sess-task-1";
const DEEP_SESSION: &str = "sess-deep-task";
const MAPPED_SESSION: &str = "sess-mapped-model";
const EXPECTED_JSONL: usize = 4;
const PROVIDER_ID: &str = "cc-switch-harness";
const UPSTREAM: &str = "https://api.example.com/v1";
const PROXY_ORIGIN: &str = "http://172.30.208.1:15721";

struct WslHarness {
    _wsl_home: tempfile::TempDir,
    _cc_home: tempfile::TempDir,
    _runner: RunnerGuard,
    _target: TestTarget,
    _previous_home: Option<String>,
    wsl_home: PathBuf,
    target: PiRuntimeTarget,
}

impl WslHarness {
    fn install(case: &str) -> Self {
        let wsl_home = tempfile::tempdir().expect("WSL fake $HOME");
        let cc_home = tempfile::tempdir().expect("CC Switch config home");
        let previous_home = std::env::var("CC_SWITCH_TEST_HOME").ok();
        std::env::set_var("CC_SWITCH_TEST_HOME", cc_home.path());

        let pi_home = wsl_home.path().join(".pi");
        copy_tree(&fixture_pack().join("cases").join(case), &pi_home);
        copy_tree(
            &fixture_pack().join("sessions"),
            &pi_home.join("agent").join("sessions"),
        );

        let runner = RunnerGuard::install(Arc::new(LocalBashRunner::new(
            DISTRO,
            wsl_home.path().to_path_buf(),
        )));
        let home_str = wsl_home.path().to_string_lossy().into_owned();
        let target_guard = TestTarget::wsl(DISTRO, &home_str);
        sessions::invalidate_sync_throttle();

        let target = PiRuntimeTarget::Wsl {
            distro: DISTRO.to_string(),
            home: home_str.clone(),
            agent_dir: format!("{home_str}/.pi/agent"),
        };

        Self {
            wsl_home: wsl_home.path().to_path_buf(),
            _wsl_home: wsl_home,
            _cc_home: cc_home,
            _runner: runner,
            _target: target_guard,
            _previous_home: previous_home,
            target,
        }
    }

    fn pi_home(&self) -> PathBuf {
        self.wsl_home.join(".pi")
    }

    fn agent_models(&self) -> PathBuf {
        self.pi_home().join("agent/models.json")
    }

    fn top_models(&self) -> PathBuf {
        self.pi_home().join("models.json")
    }

    fn sessions_root(&self) -> PathBuf {
        self.pi_home().join("agent/sessions")
    }
}

impl Drop for WslHarness {
    fn drop(&mut self) {
        sessions::invalidate_sync_throttle();
        match self._previous_home.take() {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }
}

fn fixture_pack() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/pi-wsl");
    root.canonicalize().unwrap_or(root)
}

fn copy_tree(src: &Path, dst: &Path) {
    assert!(
        src.is_dir(),
        "fixture {} is missing; check tests/fixtures/pi-wsl",
        src.display()
    );
    fs::create_dir_all(dst).expect("create fixture destination");
    for entry in fs::read_dir(src).expect("read fixture directory") {
        let entry = entry.expect("fixture entry");
        let dest = dst.join(entry.file_name());
        if entry.file_type().expect("fixture file type").is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest).expect("copy fixture file");
        }
    }
}

fn location_path(location: &PiFileLocation) -> PathBuf {
    match location {
        PiFileLocation::Local(path) => path.clone(),
        PiFileLocation::Wsl { path, .. } => PathBuf::from(path),
    }
}

fn harness_card() -> Provider {
    Provider {
        id: PROVIDER_ID.to_string(),
        name: "Harness OpenAI".to_string(),
        settings_config: json!({
            "name": "Harness OpenAI",
            "baseUrl": UPSTREAM,
            "apiKey": "test-key-not-secret",
            "api": "openai-completions",
            "models": [{
                "id": "gpt-4.1-mini",
                "name": "GPT 4.1 Mini",
                "cost": { "input": 0.4, "output": 1.6, "cacheRead": 0.1, "cacheWrite": 0.5 }
            }]
        }),
        website_url: None,
        category: Some("custom".to_string()),
        created_at: Some(1),
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    }
}

fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// A naïve `sessions/*.jsonl` glob never sees Pi's cwd-group layout.
fn flat_jsonl_count(root: &Path) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && entry.path().extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        })
        .count()
}

#[test]
fn cwd_encode_decode_round_trips_the_documented_pi_layout() {
    assert_eq!(encode_session_cwd(CWD), CWD_GROUP);
    assert_eq!(decode_session_cwd(CWD_GROUP), CWD);
    assert_eq!(
        encode_session_cwd("/home/tfdx8045/code/agent"),
        "--home-tfdx8045-code-agent--"
    );
    assert_ne!(
        decode_session_cwd(&encode_session_cwd("/home/my-project")),
        "/home/my-project",
        "decode_session_cwd is lossy; production cwd is the JSONL header"
    );
}

#[test]
fn probe_and_manifest_share_one_session_jsonl_maxdepth() {
    assert_eq!(
        session_jsonl_maxdepth_str!(),
        SESSION_JSONL_MAXDEPTH.to_string()
    );
    assert!(
        SESSION_JSONL_MAXDEPTH > 4,
        "old maxdepth 4 dropped cwd-group/<id>/tasks/group/*.jsonl"
    );
}

#[test]
#[serial]
fn runner_discovers_nested_cwd_group_and_task_sessions_that_a_flat_glob_misses() {
    let harness = WslHarness::install("identical");
    let sessions_root = harness.sessions_root();

    assert_eq!(
        flat_jsonl_count(&sessions_root),
        0,
        "cwd-group JSONL must not sit at the sessions root"
    );
    assert!(sessions_root.join(CWD_GROUP).is_dir());

    let outcome = sessions::sync(&harness.target).expect("mirror via runner find");
    assert!(
        outcome.errors.is_empty(),
        "session sync errors: {:?}",
        outcome.errors
    );
    assert_eq!(
        outcome.fetched, EXPECTED_JSONL,
        "parent, mapped, tasks/*.jsonl, and tasks/group/*.jsonl (depth 5 > old maxdepth 4)"
    );
    assert_eq!(outcome.total, EXPECTED_JSONL);

    let mirrored = sessions::cache_root(DISTRO)
        .join("sessions")
        .join(CWD_GROUP);
    assert!(
        mirrored.join("2026-03-14T10-32-00_abc.jsonl").is_file(),
        "parent session should be mirrored under the encoded cwd group"
    );
    assert!(
        mirrored
            .join("2026-03-14T10-32-00_abc/tasks/task-1.jsonl")
            .is_file(),
        "task session should be mirrored at depth 4"
    );
    assert!(
        mirrored
            .join("2026-03-14T10-32-00_abc/tasks/group/deep-task.jsonl")
            .is_file(),
        "depth-5 tasks/group JSONL must not be dropped by the old maxdepth 4"
    );
    assert!(
        mirrored.join("2026-03-14T11-00-00_map.jsonl").is_file(),
        "cwd-group mapped-model JSONL should be mirrored"
    );

    let discovered = scan_sessions();
    let mut ids: Vec<_> = discovered
        .iter()
        .map(|session| session.session_id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![DEEP_SESSION, MAPPED_SESSION, PARENT_SESSION, TASK_SESSION]
    );
    assert!(
        discovered
            .iter()
            .all(|session| session.project_dir.as_deref() == Some(CWD)),
        "project_dir must come from the JSONL cwd header, not decode_session_cwd"
    );
}

#[test]
#[serial]
fn probe_session_count_includes_nested_jsonl_not_just_the_sessions_root() {
    let _harness = WslHarness::install("identical");
    let probe = detect::probe(DISTRO).expect("probe fake WSL home");
    assert!(probe.has_models);
    assert!(probe.has_sessions);
    assert_eq!(
        probe.session_count, EXPECTED_JSONL as u32,
        "probe must share SESSION_JSONL_MAXDEPTH and see depth-5 JSONL"
    );
}

#[test]
#[serial]
fn jsonl_line_parse_prices_zero_embedded_cost_from_wsl_models_json() {
    let _harness = WslHarness::install("identical");
    sessions::invalidate_sync_throttle();

    let db = Database::memory().expect("memory db");
    let result = sync_pi_usage(&db).expect("import usage from mirrored sessions");
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(
        result.imported, EXPECTED_JSONL as u32,
        "parent + task + deep-task + mapped-model assistants"
    );

    let conn = db.conn.lock().expect("lock usage db");
    let parent: String = conn
        .query_row(
            "SELECT total_cost_usd FROM proxy_request_logs
             WHERE data_source = 'pi_session' AND session_id = ?1",
            rusqlite::params![PARENT_SESSION],
            |row| row.get(0),
        )
        .expect("parent cost");
    // 1_000_000 input tokens × $0.4 / million; JSONL cost.total is 0.
    assert_eq!(
        Decimal::from_str(&parent).expect("parent decimal"),
        Decimal::from_str("0.4").expect("0.4")
    );

    let task: String = conn
        .query_row(
            "SELECT total_cost_usd FROM proxy_request_logs
             WHERE data_source = 'pi_session' AND session_id = ?1",
            rusqlite::params![TASK_SESSION],
            |row| row.get(0),
        )
        .expect("task cost");
    // 100×0.4 + 20×1.6 per million = 0.000072
    assert_eq!(
        Decimal::from_str(&task).expect("task decimal"),
        Decimal::from_str("0.000072").expect("0.000072")
    );

    let mapped: (String, String, String) = conn
        .query_row(
            "SELECT model, request_model, total_cost_usd FROM proxy_request_logs
             WHERE data_source = 'pi_session' AND session_id = ?1",
            rusqlite::params![MAPPED_SESSION],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("mapped cost");
    assert_eq!(mapped.0, "gpt-4.1-mini-served");
    assert_eq!(mapped.1, "gpt-4.1-mini");
    // Served model is $8 / million input; requested gpt-4.1-mini is $0.4.
    assert_eq!(
        Decimal::from_str(&mapped.2).expect("mapped decimal"),
        Decimal::from_str("8").expect("8")
    );
}

#[test]
#[serial]
fn wsl_read_heals_identical_diverge_and_only_agent_models_mirrors() {
    {
        let harness = WslHarness::install("identical");
        let before_agent = fs::read(harness.agent_models()).expect("agent");
        let before_top = fs::read(harness.top_models()).expect("top");
        assert_eq!(before_agent, before_top);

        files::read(PiFile::Models, 1024 * 1024).expect("read identical");
        assert_eq!(
            fs::read(harness.agent_models()).expect("agent after"),
            fs::read(harness.top_models()).expect("top after")
        );
        assert!(read_text(&harness.agent_models()).contains(PROVIDER_ID));
    }

    {
        let harness = WslHarness::install("diverge");
        assert!(read_text(&harness.top_models()).contains("stale-proxy"));
        assert!(read_text(&harness.agent_models()).contains(PROVIDER_ID));

        files::read(PiFile::Models, 1024 * 1024).expect("heal diverge");
        let agent = read_text(&harness.agent_models());
        let top = read_text(&harness.top_models());
        assert!(agent.contains(PROVIDER_ID));
        assert!(!agent.contains("stale-proxy"));
        assert_eq!(
            agent, top,
            "canonical agent file must win and overwrite top"
        );
        assert!(
            !top.contains("127.0.0.1:15721"),
            "leftover proxy URL must not survive heal"
        );
    }

    {
        let harness = WslHarness::install("only-agent");
        assert!(harness.agent_models().is_file());
        assert!(!harness.top_models().exists());

        files::read(PiFile::Models, 1024 * 1024).expect("mirror only-agent");
        assert!(
            harness.top_models().is_file(),
            "missing top-level models.json should be created from the agent file"
        );
        assert_eq!(
            fs::read(harness.agent_models()).expect("agent"),
            fs::read(harness.top_models()).expect("top")
        );

        let location = files::locate(PiFile::Models).expect("locate");
        assert!(matches!(location, PiFileLocation::Wsl { .. }));
        assert!(
            !location.display().contains(r"\\wsl"),
            "WSL paths must stay on the runner, never UNC"
        );
    }
}

#[test]
#[serial]
fn project_and_restore_dual_write_wsl_agent_and_top_level_models() {
    let harness = WslHarness::install("identical");
    let db = Database::memory().expect("memory db");
    db.save_provider("pi", &harness_card())
        .expect("store upstream card");

    let projected = crate::services::pi_proxy::sync_live_providers(&db, Some(PROXY_ORIGIN))
        .expect("project through WSL write");
    assert_eq!(projected, 1);

    let agent = read_text(&harness.agent_models());
    let top = read_text(&harness.top_models());
    assert_eq!(
        agent, top,
        "project must keep the top-level mirror in lockstep"
    );
    assert!(agent.contains(&format!("{PROXY_ORIGIN}/pi/{PROVIDER_ID}/v1")));
    assert!(!agent.contains(UPSTREAM));

    let restored = crate::services::pi_proxy::restore_live_providers(&db).expect("restore");
    assert_eq!(restored, 1);
    let agent = read_text(&harness.agent_models());
    let top = read_text(&harness.top_models());
    assert_eq!(agent, top, "restore must rewrite both models.json files");
    assert!(agent.contains(UPSTREAM));
    assert!(!agent.contains("/pi/"));

    let top_loc = files::locate_top_level(PiFile::Models)
        .expect("top-level location")
        .expect("mirror is distinct from agent");
    assert_eq!(location_path(&top_loc), harness.top_models());
}

#[test]
#[serial]
fn project_from_only_agent_creates_the_top_mirror_and_restore_keeps_lockstep() {
    let harness = WslHarness::install("only-agent");
    assert!(!harness.top_models().exists());

    let db = Database::memory().expect("memory db");
    db.save_provider("pi", &harness_card())
        .expect("store upstream card");

    let projected = crate::services::pi_proxy::sync_live_providers(&db, Some(PROXY_ORIGIN))
        .expect("project from only-agent");
    assert_eq!(projected, 1);
    assert!(
        harness.top_models().is_file(),
        "project must dual-write a missing top-level models.json"
    );
    let agent = read_text(&harness.agent_models());
    let top = read_text(&harness.top_models());
    assert_eq!(agent, top);
    assert!(agent.contains(&format!("{PROXY_ORIGIN}/pi/{PROVIDER_ID}/v1")));

    let restored = crate::services::pi_proxy::restore_live_providers(&db).expect("restore");
    assert_eq!(restored, 1);
    let agent = read_text(&harness.agent_models());
    let top = read_text(&harness.top_models());
    assert_eq!(agent, top);
    assert!(agent.contains(UPSTREAM));
    assert!(!agent.contains("/pi/"));
}

#[test]
#[serial]
fn project_from_diverge_replaces_stale_top_proxy_then_restore_dual_writes() {
    let harness = WslHarness::install("diverge");
    assert!(read_text(&harness.top_models()).contains("stale-proxy"));

    let db = Database::memory().expect("memory db");
    db.save_provider("pi", &harness_card())
        .expect("store upstream card");

    let projected = crate::services::pi_proxy::sync_live_providers(&db, Some(PROXY_ORIGIN))
        .expect("project from diverge");
    assert_eq!(projected, 1);
    let agent = read_text(&harness.agent_models());
    let top = read_text(&harness.top_models());
    assert_eq!(agent, top, "stale top must be overwritten, not left behind");
    assert!(agent.contains(&format!("{PROXY_ORIGIN}/pi/{PROVIDER_ID}/v1")));
    assert!(!top.contains("stale-proxy"));
    assert!(!top.contains("127.0.0.1:15721"));

    let restored = crate::services::pi_proxy::restore_live_providers(&db).expect("restore");
    assert_eq!(restored, 1);
    let agent = read_text(&harness.agent_models());
    let top = read_text(&harness.top_models());
    assert_eq!(agent, top);
    assert!(agent.contains(UPSTREAM));
    assert!(!agent.contains("/pi/"));
}

/// Simulates the v4.1.3 production failure: `wsl.exe` runs bash `-c` but
/// drops every positional argument. Writes must still succeed via the
/// embedded `set --` fallback — never `bash -c ''`.
#[derive(Debug)]
struct DroppedArgvRunner {
    inner: LocalBashRunner,
}

impl WslRunner for DroppedArgvRunner {
    fn run(
        &self,
        request: &crate::pi_runtime::wsl::WslRequest,
    ) -> crate::pi_runtime::PiResult<WslExecResult> {
        assert!(
            crate::pi_runtime::wsl::is_usable_wsl_script(&request.script),
            "Pi models.json write must never invoke empty bash: {:?}",
            request.script
        );
        assert!(
            !request.script.trim().is_empty() && request.script.trim() != "''",
            "script collapsed to empty quotes"
        );
        let mut stripped = request.clone();
        stripped.args.clear();
        self.inner.run(&stripped)
    }

    fn list_distros(&self) -> crate::pi_runtime::PiResult<Vec<String>> {
        self.inner.list_distros()
    }
}

#[derive(Debug)]
struct CannedProxyRunner {
    inner: LocalBashRunner,
    host_payload: String,
    probe_payload: String,
}

impl WslRunner for CannedProxyRunner {
    fn run(
        &self,
        request: &crate::pi_runtime::wsl::WslRequest,
    ) -> crate::pi_runtime::PiResult<WslExecResult> {
        let script = &request.script;
        if script.contains("ip route show default") {
            return Ok(canned_stdout(&self.host_payload));
        }
        if script.contains("for host in") && script.contains("/health") {
            return Ok(canned_stdout(&self.probe_payload));
        }
        self.inner.run(request)
    }

    fn list_distros(&self) -> crate::pi_runtime::PiResult<Vec<String>> {
        self.inner.list_distros()
    }
}

fn canned_stdout(payload: &str) -> WslExecResult {
    WslExecResult {
        exit_code: Some(0),
        stdout: format!("\n{}\n{payload}", crate::pi_runtime::wsl::OUTPUT_SENTINEL).into_bytes(),
        stderr: String::new(),
    }
}

#[test]
#[serial]
fn mocked_host_probe_treats_localhost_404_as_success_and_skips_nat_hosts() {
    let home = tempfile::tempdir().expect("home");
    let inner = LocalBashRunner::new(DISTRO, home.path().to_path_buf());
    let _runner = RunnerGuard::install(Arc::new(CannedProxyRunner {
        inner,
        host_payload: "gateway=172.30.213.1\nnameserver=10.255.255.254\n".to_string(),
        // User-verified mirrored WSL: `/` returns 404; NAT IPs are refused.
        probe_payload: "host=127.0.0.1 code=404 latency=12\n".to_string(),
    }));

    let health = proxy::resolve_gateway(DISTRO, 15721).expect("resolve");
    assert!(
        health.reachable,
        "HTTP 404 from 127.0.0.1 means the listener answered"
    );
    assert_eq!(health.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(health.strategy, Some(HostStrategy::MirroredLoopback));
    assert_eq!(health.endpoint.as_deref(), Some("http://127.0.0.1:15721"));
}

#[test]
#[serial]
fn live_curl_404_on_loopback_is_a_reachable_route() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 404 fixture");
    listener
        .set_nonblocking(true)
        .expect("nonblocking 404 fixture");
    let port = listener.local_addr().expect("local addr").port();
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let mut buf = [0u8; 2048];
                    let _ = stream.read(&mut buf);
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
    });

    let home = tempfile::tempdir().expect("home");
    let _runner = RunnerGuard::install(Arc::new(LocalBashRunner::new(
        DISTRO,
        home.path().to_path_buf(),
    )));

    let health = proxy::resolve_gateway(DISTRO, port).expect("resolve via real curl");
    let expected = format!("http://127.0.0.1:{port}");
    assert!(
        health.reachable,
        "probe must treat curl 404 on 127.0.0.1 as success, got {health:?}"
    );
    assert_eq!(health.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(health.endpoint.as_deref(), Some(expected.as_str()));

    drop(server);
}

#[test]
#[serial]
fn session_refresh_does_not_depend_on_proxy_probe() {
    let harness = WslHarness::install("identical");
    let inner = LocalBashRunner::new(DISTRO, harness.wsl_home.clone());
    let _proxy = RunnerGuard::install(Arc::new(CannedProxyRunner {
        inner,
        host_payload: "gateway=172.30.213.1\nnameserver=10.255.255.254\n".to_string(),
        probe_payload: "host=127.0.0.1 code=000 latency=3005\nhost=172.30.213.1 code=000 latency=1\nhost=10.255.255.254 code=000 latency=1\n".to_string(),
    }));

    let health = proxy::resolve_gateway(DISTRO, 15721).expect("resolve");
    assert!(!health.reachable, "fixture is a failed probe");

    sessions::invalidate_sync_throttle();
    let outcome = sessions::sync(&harness.target).expect("session sync via wsl.exe");
    assert!(
        outcome.errors.is_empty(),
        "session refresh must not gate on proxy reachability: {:?}",
        outcome.errors
    );
    assert_eq!(outcome.total, EXPECTED_JSONL);
}

#[test]
#[serial]
fn mocked_host_probe_prefers_mirrored_loopback_when_it_answers() {
    let home = tempfile::tempdir().expect("home");
    let inner = LocalBashRunner::new(DISTRO, home.path().to_path_buf());
    let _runner = RunnerGuard::install(Arc::new(CannedProxyRunner {
        inner,
        host_payload: "gateway=172.30.208.1\nnameserver=10.255.255.254\n".to_string(),
        probe_payload: "host=127.0.0.1 code=200 latency=4\nhost=172.30.208.1 code=200 latency=12\n"
            .to_string(),
    }));

    let health = proxy::resolve_gateway(DISTRO, 15721).expect("resolve");
    assert!(health.reachable);
    assert_eq!(health.host.as_deref(), Some("127.0.0.1"));
    assert_eq!(health.strategy, Some(HostStrategy::MirroredLoopback));
    assert_eq!(health.endpoint.as_deref(), Some("http://127.0.0.1:15721"));
}

#[test]
#[serial]
fn mocked_host_probe_falls_back_to_nat_gateway_when_loopback_is_dead() {
    let home = tempfile::tempdir().expect("home");
    let inner = LocalBashRunner::new(DISTRO, home.path().to_path_buf());
    let _runner = RunnerGuard::install(Arc::new(CannedProxyRunner {
        inner,
        host_payload: "gateway=172.30.208.1\nnameserver=10.255.255.254\n".to_string(),
        probe_payload:
            "host=127.0.0.1 code=000 latency=3005\nhost=172.30.208.1 code=200 latency=12\n"
                .to_string(),
    }));

    let health = proxy::resolve_gateway(DISTRO, 15721).expect("resolve");
    assert!(health.reachable);
    assert_eq!(health.host.as_deref(), Some("172.30.208.1"));
    assert_eq!(health.strategy, Some(HostStrategy::DefaultGateway));
    assert_eq!(
        health.endpoint.as_deref(),
        Some("http://172.30.208.1:15721")
    );
}

#[test]
#[serial]
fn models_json_write_survives_wsl_dropping_positional_args() {
    let harness = WslHarness::install("identical");
    let _dropped = RunnerGuard::install(Arc::new(DroppedArgvRunner {
        inner: LocalBashRunner::new(DISTRO, harness.wsl_home.clone()),
    }));
    let document = br#"{"providers":{"baisheng":{"name":"BaiSheng"}}}"#;
    files::write(
        PiFile::Models,
        document,
        &files::revision(&fs::read(harness.agent_models()).expect("current agent")),
    )
    .expect("models.json write must work when WSL drops $1 (never empty bash)");
    assert_eq!(fs::read(harness.agent_models()).expect("agent"), document);
    assert_eq!(fs::read(harness.top_models()).expect("top"), document);
    assert!(
        files::ATOMIC_STAGE_SNIPPET.contains("mktemp /tmp/cc-switch-XXXXXX")
            || files::ATOMIC_STAGE_SNIPPET.contains("mktemp -p")
    );
}

#[test]
#[serial]
fn detect_treats_localhost_http_404_as_success_like_the_toast() {
    let home = tempfile::tempdir().expect("home");
    let inner = LocalBashRunner::new(DISTRO, home.path().to_path_buf());
    let _runner = RunnerGuard::install(Arc::new(CannedProxyRunner {
        inner,
        host_payload: "gateway=172.30.213.1\nnameserver=10.255.255.254\n".to_string(),
        probe_payload: "host=127.0.0.1 code=404 latency=12\n".to_string(),
    }));
    let health = proxy::resolve_gateway(DISTRO, 15721).expect("resolve");
    assert!(
        health.reachable,
        "detect-proxy toast must match probe: HTTP 404 on 127.0.0.1 is success"
    );
    assert!(!proxy::looks_like_proxy_off(
        health.error.as_deref().unwrap_or("")
    ));
    assert!(proxy::looks_like_proxy_off(
        r#"Get "http://127.0.0.1:15721/ping": connection refused"#
    ));
}

#[test]
#[serial]
fn writing_models_json_via_wsl_runner_round_trips_and_never_mktemps_at_root() {
    let harness = WslHarness::install("identical");
    let document =
        br#"{"providers":{"baisheng":{"name":"BaiSheng","baseUrl":"https://api.example.com/v1"}}}"#;
    let location = files::locate(PiFile::Models).expect("locate models");
    assert!(matches!(location, PiFileLocation::Wsl { .. }));

    files::write(
        PiFile::Models,
        document,
        &files::revision(&fs::read(harness.agent_models()).expect("current agent")),
    )
    .expect("write models.json via WslRunner");

    let agent = fs::read(harness.agent_models()).expect("agent after write");
    let top = fs::read(harness.top_models()).expect("top after write");
    assert_eq!(agent, document);
    assert_eq!(agent, top);
    assert!(
        !files::ATOMIC_STAGE_SNIPPET.contains("$dir/.cc-switch-XXXXXX"),
        "harness writes must not inherit the root mktemp template"
    );
}

#[test]
#[serial]
fn claude_and_codex_live_writes_use_wsl_runner_never_unc() {
    let harness = WslHarness::install("identical");
    let settings = json!({
        "env": {
            "ANTHROPIC_BASE_URL": PROXY_ORIGIN,
            "ANTHROPIC_AUTH_TOKEN": "PROXY_MANAGED"
        }
    });
    assert!(
        crate::wsl_cli::write_claude_settings(&settings).expect("write claude"),
        "WSL runtime must handle the Claude write"
    );
    let claude_path = harness.wsl_home.join(".claude/settings.json");
    let written = read_text(&claude_path);
    assert!(written.contains(PROXY_ORIGIN));
    assert!(!written.contains(r"\\wsl"));

    let auth = json!({ "OPENAI_API_KEY": "PROXY_MANAGED" });
    let config = r#"
model_provider = "custom"
model = "glm-5.2"

[model_providers.custom]
name = "Custom"
base_url = "http://172.30.208.1:15721/v1"
wire_api = "responses"
transport_kind = "responses_http"
"#;
    assert!(
        crate::wsl_cli::write_codex_live(Some(&auth), Some(config)).expect("write codex"),
        "WSL runtime must handle the Codex write"
    );
    let auth_path = harness.wsl_home.join(".codex/auth.json");
    let toml_path = harness.wsl_home.join(".codex/config.toml");
    assert!(read_text(&auth_path).contains("PROXY_MANAGED"));
    let toml = read_text(&toml_path);
    assert!(toml.contains("transport_kind = \"responses_http\""));
    assert!(toml.contains("172.30.208.1:15721"));
    assert!(!toml.contains(r"\\wsl"));
    assert!(!auth_path.to_string_lossy().contains(r"\\wsl"));
}

/// Full self-test on the user's mirrored+firewall+dnsTunneling matrix.
///
/// Settings snapshot must stay green without a multi-host WSL curl.
/// 「检测代理」 must treat localhost HTTP 404 as success (same as Settings).
/// Pi `models.json` write must keep the 3.0.1 stdin→stage→sha256→`mv`
/// contract when WSL drops `$1`, never `bash -c ''`, never mktemp at `/`.
#[test]
#[serial]
fn user_mirrored_firewall_dnstunnel_e2e_self_test() {
    let profile = USER_MIRRORED;
    assert_eq!(profile.networking_mode, "mirrored");
    assert!(profile.firewall && profile.dns_tunneling);
    assert_eq!(profile.distro, DISTRO);

    let harness = WslHarness::install("identical");
    let _topo = RunnerGuard::install(Arc::new(UserMirroredTopologyRunner::new(
        LocalBashRunner::new(DISTRO, harness.wsl_home.clone()),
    )));

    let snapshot = proxy::plan_ui_snapshot(&harness.target, true, profile.proxy_port);
    assert!(
        snapshot.gateway.reachable,
        "settings click must not curl WSL; snapshot is localhost-first"
    );
    assert_eq!(
        snapshot.origin.as_deref(),
        Some(profile.endpoint().as_str())
    );
    assert_eq!(snapshot.gateway.host.as_deref(), Some(profile.listen_host));
    assert_eq!(
        snapshot.gateway.strategy,
        Some(HostStrategy::MirroredLoopback)
    );

    let health = proxy::resolve_gateway(DISTRO, profile.proxy_port).expect("detect");
    assert!(
        health.reachable,
        "detect toast must match Settings: HTTP 404 on {} is success, got {health:?}",
        profile.endpoint()
    );
    assert_eq!(health.host.as_deref(), Some(profile.listen_host));
    assert_eq!(health.strategy, Some(HostStrategy::MirroredLoopback));
    assert_eq!(
        health.endpoint.as_deref(),
        Some(profile.endpoint().as_str())
    );
    assert!(!proxy::looks_like_proxy_off(
        health.error.as_deref().unwrap_or("")
    ));
    assert!(
        health.error.as_deref().unwrap_or("").is_empty(),
        "must not toast 'no route' when localhost already answered: {:?}",
        health.error
    );

    let document = br#"{"providers":{"baisheng":{"name":"BaiSheng"}}}"#;
    files::write(
        PiFile::Models,
        document,
        &files::revision(&fs::read(harness.agent_models()).expect("current agent")),
    )
    .expect("models.json write must survive dropped $1 on the user topology");
    assert_eq!(fs::read(harness.agent_models()).expect("agent"), document);
    assert_eq!(fs::read(harness.top_models()).expect("top"), document);
    assert!(
        files::ATOMIC_STAGE_SNIPPET.contains("mktemp /tmp/cc-switch-XXXXXX")
            || files::ATOMIC_STAGE_SNIPPET.contains("mktemp -p")
    );
    assert!(!files::ATOMIC_STAGE_SNIPPET.contains("$dir/.cc-switch-XXXXXX"));
}

/// Settings click / focus must never spawn `wsl.exe` / curl. A runner that
/// panics on any invocation proves `plan_ui_snapshot` is a local snapshot.
#[derive(Debug)]
struct PanicOnWslRunner;

impl WslRunner for PanicOnWslRunner {
    fn run(
        &self,
        request: &crate::pi_runtime::wsl::WslRequest,
    ) -> crate::pi_runtime::PiResult<WslExecResult> {
        panic!(
            "plan_ui_snapshot must not invoke WSL (script starts with {:?})",
            request.script.chars().take(80).collect::<String>()
        );
    }

    fn list_distros(&self) -> crate::pi_runtime::PiResult<Vec<String>> {
        panic!("plan_ui_snapshot must not list WSL distros");
    }
}

#[test]
fn plan_ui_snapshot_never_invokes_wsl_runner() {
    let _runner = RunnerGuard::install(Arc::new(PanicOnWslRunner));
    let wsl = PiRuntimeTarget::Wsl {
        distro: DISTRO.into(),
        home: "/home/tfdx8045".into(),
        agent_dir: "/home/tfdx8045/.pi/agent".into(),
    };
    let plan = proxy::plan_ui_snapshot(&wsl, true, USER_MIRRORED.proxy_port);
    assert!(plan.gateway.reachable);
    assert_eq!(plan.origin.as_deref(), Some("http://127.0.0.1:15721"));
    assert_eq!(plan.gateway.host.as_deref(), Some("127.0.0.1"));
}

/// Enable 「百胜」 is `ProviderService::switch` → `insert_pi_provider` →
/// `files::read` + `files::write`. Production v4.1.3 failed here with
/// empty bash / dropped `$1`. Must succeed on the user topology.
#[test]
#[serial]
fn enable_pi_provider_on_user_topology_survives_dropped_argv() {
    let harness = WslHarness::install("identical");
    let _topo = RunnerGuard::install(Arc::new(UserMirroredTopologyRunner::new(
        LocalBashRunner::new(DISTRO, harness.wsl_home.clone()),
    )));

    let state = AppState::new(Arc::new(Database::memory().expect("memory db")));
    let mut card = harness_card();
    card.id = "baisheng".to_string();
    card.name = "百胜".to_string();
    if let Some(object) = card.settings_config.as_object_mut() {
        object.insert("name".into(), json!("百胜"));
    }
    state
        .db
        .save_provider("pi", &card)
        .expect("store 百胜 card");

    ProviderService::switch(&state, AppType::Pi, "baisheng")
        .expect("enable 百胜 must not empty-bash when WSL drops $1");

    let agent = read_text(&harness.agent_models());
    let top = read_text(&harness.top_models());
    assert!(
        agent.contains("baisheng"),
        "enable must insert 百胜 into agent models.json: {agent}"
    );
    assert_eq!(agent, top, "top-level mirror must stay in sync");
    assert!(
        !agent.contains("/bin/bash: line 1: '': No such file or directory"),
        "must never invoke empty bash"
    );
}

#[test]
#[serial]
fn claude_codex_writes_survive_dropped_argv_on_user_topology() {
    let harness = WslHarness::install("identical");
    let _topo = RunnerGuard::install(Arc::new(UserMirroredTopologyRunner::new(
        LocalBashRunner::new(DISTRO, harness.wsl_home.clone()),
    )));

    let settings = json!({
        "env": {
            "ANTHROPIC_BASE_URL": "http://127.0.0.1:15721",
            "ANTHROPIC_AUTH_TOKEN": "PROXY_MANAGED"
        }
    });
    assert!(
        crate::wsl_cli::write_claude_settings(&settings).expect("write claude"),
        "Claude settings write must be handled by the WSL runtime"
    );
    let claude = read_text(&harness.wsl_home.join(".claude/settings.json"));
    assert!(claude.contains("127.0.0.1:15721"));
    assert!(!claude.contains(r"\\wsl"));

    let auth = json!({ "OPENAI_API_KEY": "PROXY_MANAGED" });
    let config = r#"
model_provider = "custom"
model = "glm-5.2"

[model_providers.custom]
name = "Custom"
base_url = "http://127.0.0.1:15721/v1"
wire_api = "responses"
transport_kind = "responses_http"
"#;
    assert!(
        crate::wsl_cli::write_codex_live(Some(&auth), Some(config)).expect("write codex"),
        "Codex write must be handled by the WSL runtime"
    );
    let auth_text = read_text(&harness.wsl_home.join(".codex/auth.json"));
    let toml = read_text(&harness.wsl_home.join(".codex/config.toml"));
    assert!(auth_text.contains("PROXY_MANAGED"));
    assert!(toml.contains("127.0.0.1:15721"));
    assert!(!toml.contains(r"\\wsl"));
}

#[test]
fn wsl_cli_rejects_unc_overrides_in_harness() {
    assert!(crate::wsl_cli::is_wsl_unc_str(
        r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude"
    ));
    assert!(
        crate::wsl_cli::reject_unc_override(Some(r"\\wsl$\Ubuntu-22.04\home\tfdx8045\.codex"))
            .is_none()
    );
}
