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
//! encode/decode (test-only), and mocked proxy host candidates.

#![cfg(test)]

use super::detect;
use super::files::{self, PiFile, PiFileLocation};
use super::proxy::{self, HostStrategy};
use super::session_jsonl_maxdepth_str;
use super::sessions;
use super::test_support::TestTarget;
use super::wsl::test_support::{LocalBashRunner, RunnerGuard};
use super::wsl::{WslExecResult, WslRunner};
use super::PiRuntimeTarget;
use super::SESSION_JSONL_MAXDEPTH;
use crate::database::Database;
use crate::provider::Provider;
use crate::services::session_usage_pi::sync_pi_usage;
use crate::session_manager::providers::pi::{
    decode_session_cwd, encode_session_cwd, scan_sessions,
};
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

#[test]
fn wsl_cli_rejects_unc_overrides_in_harness() {
    assert!(crate::wsl_cli::is_wsl_unc_str(
        r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude"
    ));
    assert!(crate::wsl_cli::reject_unc_override(Some(
        r"\\wsl$\Ubuntu-22.04\home\tfdx8045\.codex"
    ))
    .is_none());
}
