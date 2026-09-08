//! Mirroring WSL Pi session files into a local cache.
//!
//! Pi writes JSONL session logs inside the distribution. Reading them one
//! `wsl.exe`+`cat` at a time is what made WSL session browsing unusable: each
//! call pays login-shell and VM-entry cost, so a few hundred sessions turn
//! into minutes of work and a busy WSL CPU.
//!
//! Instead the runtime does two things per refresh:
//!
//! 1. one `find` call that returns a manifest (`size`, `mtime`, relative path),
//! 2. one batched transfer of only the files whose manifest entry changed.
//!
//! Once mirrored, the existing local session scanner, parser and usage
//! importer run unchanged — the cache is a transport detail, not a second
//! session format.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::error::{PiResult, PiRuntimeError};
use super::session_jsonl_maxdepth_str;
use super::wsl::{self, WslRequest};
use super::PiRuntimeTarget;

/// `$1` sessions root.
///
/// GNU `find -printf` is preferred. If that predicate is missing (BSD find),
/// fall back to portable `find -print` + `stat` rather than swallowing stderr
/// into a silent empty manifest (`fetched = 0`). A listing tool that cannot
/// run at all fails the script so [`read_manifest`] surfaces a hard error.
const MANIFEST_SCRIPT: &str = concat!(
    r#"
set -u
root="$1"
if [ ! -d "$root" ]; then printf 'missing\n'; exit 0; fi

# Probe GNU -printf on the sessions root itself. Do not hide later listing
# errors: an empty payload after a failed -printf looks like "no sessions".
if find "$root" -maxdepth 0 -printf '%P' >/dev/null 2>&1; then
  printf 'ok\n'
  find "$root" -maxdepth "#,
    session_jsonl_maxdepth_str!(),
    r#" -type f -name '*.jsonl' -printf '%s\t%T@\t%P\n'
  exit $?
fi

printf 'ok\n'
tmp=$(mktemp) || { printf 'mktemp-failed\n' >&2; exit 1; }
trap 'rm -f -- "$tmp"' EXIT
if ! find "$root" -maxdepth "#,
    session_jsonl_maxdepth_str!(),
    r#" -type f -name '*.jsonl' -print >"$tmp"; then
  printf 'find-failed\n' >&2
  exit 1
fi
while IFS= read -r file; do
  rel="${file#"$root"/}"
  size=$(wc -c < "$file" | tr -d ' \t\r')
  if mtime=$(stat -c '%Y' "$file" 2>/dev/null); then
    :
  elif mtime=$(stat -f '%m' "$file" 2>/dev/null); then
    :
  else
    printf 'stat-failed %s\n' "$rel" >&2
    exit 1
  fi
  printf '%s\t%s\t%s\n' "$size" "$mtime" "$rel"
done <"$tmp"
"#
);

/// `$1` sessions root; relative paths arrive on stdin, one per line.
///
/// Emits a framed stream so one call can carry many files:
/// `FILE <bytes> <relative path>\n<payload>` repeated, then `END`.
/// The whole stream is gzipped because JSONL compresses roughly tenfold and
/// the `wsl.exe` pipe is the bottleneck.
const FETCH_SCRIPT: &str = r#"
set -u
root="$1"
{
  while IFS= read -r rel; do
    file="$root/$rel"
    [ -f "$file" ] || continue
    printf 'FILE %s %s\n' "$(wc -c < "$file")" "$rel"
    cat -- "$file"
  done
  printf 'END\n'
} | gzip -c
"#;

/// `$1` sessions root, `$2` relative path of the session to delete.
const DELETE_SCRIPT: &str = r#"
set -u
root="$1"; rel="$2"
file="$root/$rel"
if [ ! -f "$file" ]; then printf 'missing\n'; exit 0; fi
rm -f -- "$file" || { printf 'delete-failed\n' >&2; exit 1; }
printf 'ok\n'
"#;

/// A refresh transfers at most this many bytes per `wsl.exe` call so a large
/// backlog is spread over several bounded round trips instead of one
/// unbounded allocation.
const MAX_BATCH_BYTES: u64 = 24 * 1024 * 1024;
/// Companion cap for many small files.
const MAX_BATCH_FILES: usize = 250;
/// Individual sessions larger than this are left in WSL; the local scanner
/// already refuses to parse them.
const MAX_SESSION_BYTES: u64 = 128 * 1024 * 1024;
/// Upper bound on tracked sessions. Beyond this the most recently modified
/// ones win, which is also the order the session list shows.
const MAX_TRACKED_SESSIONS: usize = 5_000;

/// One session file as WSL reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionManifestEntry {
    pub rel_path: String,
    pub size: u64,
    pub mtime_ms: i64,
}

/// Cache bookkeeping, persisted next to the mirrored files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheManifest {
    #[serde(default)]
    distro: String,
    #[serde(default)]
    sessions_root: String,
    #[serde(default)]
    entries: Vec<SessionManifestEntry>,
}

/// What a refresh did, for logging and the UI's "last sync" line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSyncOutcome {
    pub fetched: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub total: usize,
    pub bytes: u64,
    pub truncated: bool,
    pub errors: Vec<String>,
}

/// Session browsing, message loading and usage import all resolve the session
/// root, so an unthrottled mirror would run several times per refresh and keep
/// the WSL VM busy. One refresh per interval is enough to notice a session that
/// Pi is actively appending to (design document §39, §74).
const MIN_SYNC_INTERVAL: Duration = Duration::from_secs(5);

static LAST_SYNC: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

/// Mirror WSL sessions unless a refresh already ran within the throttle
/// window. Failures are logged rather than propagated: a stale mirror still
/// renders, whereas a hard error would empty the session list.
pub fn sync_if_stale(target: &PiRuntimeTarget) {
    if !target.is_wsl() {
        return;
    }
    {
        let mut last = LAST_SYNC.lock().expect("lock the Pi session sync clock");
        if last.is_some_and(|instant| instant.elapsed() < MIN_SYNC_INTERVAL) {
            return;
        }
        *last = Some(Instant::now());
    }
    if let Err(error) = sync(target) {
        log::warn!("[PiSession] mirroring WSL sessions failed: {error}");
    }
}

/// Force the next [`sync_if_stale`] to do real work, for an explicit refresh.
pub fn invalidate_sync_throttle() {
    *LAST_SYNC.lock().expect("lock the Pi session sync clock") = None;
}

/// Command that resumes a mirrored session inside WSL.
///
/// Proxy variables are deliberately absent: a proxy URL can carry
/// credentials, and a resume command is displayed, copied and written to shell
/// history (design document §41).
pub fn wsl_resume_command(distro: &str, linux_path: &str) -> String {
    format!(
        "wsl.exe -d {distro} -- bash -lic {}",
        shell_single_quote(&format!("pi --session {}", shell_single_quote(linux_path)))
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Root of the local mirror for `distro`.
pub fn cache_root(distro: &str) -> PathBuf {
    crate::config::get_app_config_dir()
        .join("pi-wsl-sessions")
        .join(distro)
}

/// Directory the local session scanner should treat as Pi's sessions root.
///
/// `None` for the local runtime, which reads Pi's real directory directly.
pub fn local_sessions_root(target: &PiRuntimeTarget) -> Option<PathBuf> {
    target
        .distro()
        .map(|distro| cache_root(distro).join("sessions"))
}

/// Translate a mirrored file back to its path inside WSL, so resume commands
/// and deletions act on the real session rather than the copy.
pub fn wsl_source_path(target: &PiRuntimeTarget, cached: &Path) -> Option<String> {
    let distro = target.distro()?;
    let sessions_root = target.sessions_path()?;
    let relative = cached
        .strip_prefix(cache_root(distro).join("sessions"))
        .ok()?;
    let relative = relative.to_str()?.replace('\\', "/");
    is_safe_relative_path(&relative).then(|| format!("{sessions_root}/{relative}"))
}

/// Mirror the WSL session directory into the local cache.
pub fn sync(target: &PiRuntimeTarget) -> PiResult<SessionSyncOutcome> {
    let (distro, sessions_root) = match target {
        PiRuntimeTarget::Local => return Ok(SessionSyncOutcome::default()),
        PiRuntimeTarget::Wsl { distro, .. } => (
            distro.clone(),
            target
                .sessions_path()
                .expect("a WSL target always has a sessions path"),
        ),
    };

    let flags = super::settings().flags;
    if !flags.session {
        log::debug!("[PiSession] mirroring is disabled by pi.session.enabled");
        return Ok(SessionSyncOutcome::default());
    }

    let cache = cache_root(&distro).join("sessions");
    fs::create_dir_all(&cache).map_err(|error| {
        PiRuntimeError::session_read(format!(
            "cannot create the Pi session cache {}: {error}",
            cache.display()
        ))
    })?;

    let remote = read_manifest(&distro, &sessions_root)?;
    let (remote, truncated) = cap_manifest(remote);

    let previous = load_cache_manifest(&distro, &sessions_root, &cache);
    let plan = plan_sync(&previous, &remote, &cache, flags.session_incremental);

    let mut outcome = SessionSyncOutcome {
        unchanged: remote.len().saturating_sub(plan.fetch.len()),
        total: remote.len(),
        truncated,
        ..Default::default()
    };

    for rel_path in &plan.remove {
        let path = cache.join(rel_path);
        match fs::remove_file(&path) {
            Ok(()) => outcome.removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => outcome
                .errors
                .push(format!("cannot remove {}: {error}", path.display())),
        }
    }

    for batch in batches(&plan.fetch) {
        match fetch_batch(&distro, &sessions_root, batch, &cache) {
            Ok((fetched, bytes)) => {
                outcome.fetched += fetched;
                outcome.bytes += bytes;
            }
            Err(error) => outcome.errors.push(error.to_string()),
        }
    }

    store_cache_manifest(
        &cache,
        &CacheManifest {
            distro: distro.clone(),
            sessions_root: sessions_root.clone(),
            entries: remote,
        },
    );

    log::info!(
        "[PiSession] mirrored WSL '{distro}': {} fetched, {} unchanged, {} removed, {} bytes",
        outcome.fetched,
        outcome.unchanged,
        outcome.removed,
        outcome.bytes
    );
    Ok(outcome)
}

/// Delete a session inside WSL and drop its mirrored copy.
pub fn delete(target: &PiRuntimeTarget, cached: &Path) -> PiResult<bool> {
    let PiRuntimeTarget::Wsl { distro, .. } = target else {
        return Err(PiRuntimeError::invalid_input(
            "WSL session deletion requires the WSL runtime".to_string(),
        ));
    };
    let sessions_root = target
        .sessions_path()
        .expect("a WSL target always has a sessions path");
    let relative = cached
        .strip_prefix(cache_root(distro).join("sessions"))
        .map_err(|_| {
            PiRuntimeError::invalid_input(
                "session path is outside the Pi session cache".to_string(),
            )
        })?
        .to_string_lossy()
        .replace('\\', "/");
    if !is_safe_relative_path(&relative) {
        return Err(PiRuntimeError::invalid_input(format!(
            "unsafe Pi session path: {relative}"
        )));
    }

    let output = wsl::run(
        &WslRequest::guarded(distro, DELETE_SCRIPT)
            .arg(&sessions_root)
            .arg(&relative),
    )?;
    output.require_success("deleting a Pi session")?;

    let deleted = wsl::first_line(&output.payload_lossy()) == Some("ok");
    if deleted {
        let _ = fs::remove_file(cached);
    }
    Ok(deleted)
}

fn read_manifest(distro: &str, sessions_root: &str) -> PiResult<Vec<SessionManifestEntry>> {
    let output = wsl::run(
        &WslRequest::guarded(distro, MANIFEST_SCRIPT)
            .arg(sessions_root)
            .timeout(wsl::DEFAULT_TIMEOUT),
    )?;
    output.require_success("listing Pi sessions")?;

    let payload = output.payload_lossy();
    let mut lines = payload.lines();
    match lines.next().map(str::trim) {
        Some("ok") => {}
        // Pi has not created the directory yet; an empty manifest is correct.
        Some("missing") => return Ok(Vec::new()),
        other => {
            return Err(PiRuntimeError::session_read(format!(
                "listing Pi sessions returned an unexpected status: {}",
                other.unwrap_or("no output")
            )))
        }
    }
    Ok(parse_manifest(lines))
}

/// Parse `find -printf '%s\t%T@\t%P\n'` output.
fn parse_manifest<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<SessionManifestEntry> {
    let mut entries = Vec::new();
    for line in lines {
        let mut fields = line.trim_end_matches('\r').splitn(3, '\t');
        let Some(size) = fields.next().and_then(|value| value.parse::<u64>().ok()) else {
            continue;
        };
        let Some(mtime_ms) = fields.next().and_then(parse_epoch_millis) else {
            continue;
        };
        let Some(rel_path) = fields.next() else {
            continue;
        };
        if !is_safe_relative_path(rel_path) || size > MAX_SESSION_BYTES {
            log::debug!("[PiSession] skipping unusable session entry '{rel_path}'");
            continue;
        }
        entries.push(SessionManifestEntry {
            rel_path: rel_path.to_string(),
            size,
            mtime_ms,
        });
    }
    entries
}

/// `find -printf '%T@'` prints seconds with a fractional part.
fn parse_epoch_millis(value: &str) -> Option<i64> {
    let seconds = value.trim().parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some((seconds * 1000.0) as i64)
}

fn cap_manifest(mut entries: Vec<SessionManifestEntry>) -> (Vec<SessionManifestEntry>, bool) {
    if entries.len() <= MAX_TRACKED_SESSIONS {
        entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        return (entries, false);
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.mtime_ms));
    entries.truncate(MAX_TRACKED_SESSIONS);
    entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    (entries, true)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SyncPlan {
    fetch: Vec<SessionManifestEntry>,
    remove: Vec<String>,
}

/// Decide which files to transfer and which stale copies to drop.
fn plan_sync(
    previous: &[SessionManifestEntry],
    remote: &[SessionManifestEntry],
    cache: &Path,
    incremental: bool,
) -> SyncPlan {
    let known: HashMap<&str, &SessionManifestEntry> = previous
        .iter()
        .map(|entry| (entry.rel_path.as_str(), entry))
        .collect();

    let mut plan = SyncPlan::default();
    for entry in remote {
        let unchanged = incremental
            && known.get(entry.rel_path.as_str()).is_some_and(|cached| {
                cached.size == entry.size && cached.mtime_ms == entry.mtime_ms
            })
            // A manifest entry means nothing if the mirrored file went away.
            && cache.join(&entry.rel_path).is_file();
        if !unchanged {
            plan.fetch.push(entry.clone());
        }
    }

    let live: HashMap<&str, ()> = remote
        .iter()
        .map(|entry| (entry.rel_path.as_str(), ()))
        .collect();
    for entry in previous {
        if !live.contains_key(entry.rel_path.as_str()) {
            plan.remove.push(entry.rel_path.clone());
        }
    }
    plan
}

fn batches(entries: &[SessionManifestEntry]) -> Vec<&[SessionManifestEntry]> {
    let mut batches = Vec::new();
    let mut start = 0usize;
    let mut bytes = 0u64;

    for (index, entry) in entries.iter().enumerate() {
        let would_be = bytes + entry.size;
        let count = index - start;
        if count > 0 && (would_be > MAX_BATCH_BYTES || count >= MAX_BATCH_FILES) {
            batches.push(&entries[start..index]);
            start = index;
            bytes = 0;
        }
        bytes += entry.size;
    }
    if start < entries.len() {
        batches.push(&entries[start..]);
    }
    batches
}

fn fetch_batch(
    distro: &str,
    sessions_root: &str,
    batch: &[SessionManifestEntry],
    cache: &Path,
) -> PiResult<(usize, u64)> {
    let request_list = batch
        .iter()
        .map(|entry| entry.rel_path.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let output = wsl::run(
        &WslRequest::guarded(distro, FETCH_SCRIPT)
            .arg(sessions_root)
            .stdin(format!("{request_list}\n").into_bytes())
            .timeout(wsl::TRANSFER_TIMEOUT),
    )?;
    output.require_success("transferring Pi sessions")?;

    let mut decoder = flate2::read::GzDecoder::new(output.payload());
    let mut fetched = 0usize;
    let mut bytes = 0u64;
    for (rel_path, contents) in parse_frames(&mut decoder)? {
        let destination = cache.join(&rel_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                PiRuntimeError::session_read(format!("cannot create {}: {error}", parent.display()))
            })?;
        }
        fs::write(&destination, &contents).map_err(|error| {
            PiRuntimeError::session_read(format!("cannot write {}: {error}", destination.display()))
        })?;
        fetched += 1;
        bytes += contents.len() as u64;
    }
    Ok((fetched, bytes))
}

/// Read the `FILE <bytes> <path>` framing produced by [`FETCH_SCRIPT`].
fn parse_frames<R: Read>(reader: &mut R) -> PiResult<Vec<(String, Vec<u8>)>> {
    let mut stream = Vec::new();
    reader.read_to_end(&mut stream).map_err(|error| {
        PiRuntimeError::session_read(format!("Pi session transfer was unreadable: {error}"))
    })?;

    let mut files = Vec::new();
    let mut cursor = 0usize;
    loop {
        let Some(newline) = stream[cursor..].iter().position(|byte| *byte == b'\n') else {
            return Err(PiRuntimeError::session_read(
                "Pi session transfer ended without a terminator".to_string(),
            ));
        };
        let header = String::from_utf8_lossy(&stream[cursor..cursor + newline])
            .trim_end_matches('\r')
            .to_string();
        cursor += newline + 1;

        if header == "END" {
            return Ok(files);
        }
        let Some(rest) = header.strip_prefix("FILE ") else {
            return Err(PiRuntimeError::session_read(format!(
                "unexpected frame in the Pi session transfer: {header}"
            )));
        };
        let Some((size, rel_path)) = rest.split_once(' ') else {
            return Err(PiRuntimeError::session_read(format!(
                "malformed frame header in the Pi session transfer: {header}"
            )));
        };
        let size: usize = size.parse().map_err(|_| {
            PiRuntimeError::session_read(format!(
                "malformed frame length in the Pi session transfer: {header}"
            ))
        })?;
        if !is_safe_relative_path(rel_path) {
            return Err(PiRuntimeError::session_read(format!(
                "unsafe path in the Pi session transfer: {rel_path}"
            )));
        }
        if cursor + size > stream.len() {
            return Err(PiRuntimeError::session_read(format!(
                "Pi session transfer was truncated inside {rel_path}"
            )));
        }
        files.push((rel_path.to_string(), stream[cursor..cursor + size].to_vec()));
        cursor += size;
    }
}

/// Relative paths from WSL become local filesystem paths, so they must not be
/// able to escape the cache directory.
fn is_safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && path.ends_with(".jsonl")
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && !path.contains('\0')
        && !path.contains(':')
        && !path
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == "." || part == "..")
        && !path.chars().any(char::is_control)
}

fn manifest_path(cache: &Path) -> PathBuf {
    cache.join(".cc-switch-manifest.json")
}

fn load_cache_manifest(
    distro: &str,
    sessions_root: &str,
    cache: &Path,
) -> Vec<SessionManifestEntry> {
    let Ok(bytes) = fs::read(manifest_path(cache)) else {
        return Vec::new();
    };
    let Ok(manifest) = serde_json::from_slice::<CacheManifest>(&bytes) else {
        return Vec::new();
    };
    // A different distribution or a moved sessionDir invalidates the mirror.
    if manifest.distro != distro || manifest.sessions_root != sessions_root {
        log::info!("[PiSession] Pi session root changed; re-mirroring from scratch");
        return Vec::new();
    }
    manifest.entries
}

fn store_cache_manifest(cache: &Path, manifest: &CacheManifest) {
    match serde_json::to_vec_pretty(manifest) {
        Ok(bytes) => {
            if let Err(error) = fs::write(manifest_path(cache), bytes) {
                log::warn!("[PiSession] cannot persist the session manifest: {error}");
            }
        }
        Err(error) => log::warn!("[PiSession] cannot serialize the session manifest: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_runtime::wsl::test_support::{LocalBashRunner, RunnerGuard};
    use serial_test::serial;
    use std::sync::Arc;

    fn entry(rel_path: &str, size: u64, mtime_ms: i64) -> SessionManifestEntry {
        SessionManifestEntry {
            rel_path: rel_path.to_string(),
            size,
            mtime_ms,
        }
    }

    #[test]
    fn manifest_and_probe_scripts_share_the_session_jsonl_maxdepth() {
        let needle = format!("-maxdepth {}", super::super::SESSION_JSONL_MAXDEPTH);
        assert_eq!(
            MANIFEST_SCRIPT.matches(&needle).count(),
            2,
            "GNU -printf and portable find must both use SESSION_JSONL_MAXDEPTH"
        );
        assert!(
            !MANIFEST_SCRIPT.contains("-printf '%s\\t%T@\\t%P\\n' 2>/dev/null"),
            "the GNU listing must not swallow -printf stderr into fetched=0"
        );
    }

    #[test]
    fn manifest_lines_are_parsed_into_entries() {
        let entries = parse_manifest(
            [
                "1024\t1757300000.1234567890\tproject-a/abc.jsonl",
                "2048\t1757300100.0000000000\tproject-b/def.jsonl",
            ]
            .into_iter(),
        );

        assert_eq!(
            entries,
            vec![
                entry("project-a/abc.jsonl", 1024, 1757300000123),
                entry("project-b/def.jsonl", 2048, 1757300100000),
            ]
        );
    }

    #[test]
    fn manifest_entries_that_could_escape_the_cache_are_dropped() {
        let entries = parse_manifest(
            [
                "10\t1.0\t../../../etc/passwd.jsonl",
                "10\t1.0\t/absolute/path.jsonl",
                "10\t1.0\tproject/../escape.jsonl",
                "10\t1.0\tnot-a-session.txt",
                "10\t1.0\tok.jsonl",
            ]
            .into_iter(),
        );

        assert_eq!(
            entries
                .iter()
                .map(|e| e.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["ok.jsonl"]
        );
    }

    #[test]
    fn unchanged_files_are_not_refetched() {
        let cache = tempfile::tempdir().expect("tempdir");
        std::fs::write(cache.path().join("a.jsonl"), "{}").expect("write cached session");

        let previous = vec![entry("a.jsonl", 10, 100), entry("gone.jsonl", 10, 100)];
        let remote = vec![entry("a.jsonl", 10, 100), entry("b.jsonl", 20, 200)];

        let plan = plan_sync(&previous, &remote, cache.path(), true);

        assert_eq!(
            plan.fetch
                .iter()
                .map(|e| e.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["b.jsonl"]
        );
        assert_eq!(plan.remove, vec!["gone.jsonl"]);
    }

    #[test]
    fn a_growing_session_is_refetched() {
        let cache = tempfile::tempdir().expect("tempdir");
        std::fs::write(cache.path().join("a.jsonl"), "{}").expect("write cached session");

        let previous = vec![entry("a.jsonl", 10, 100)];
        // Pi appended to the running session.
        let remote = vec![entry("a.jsonl", 40, 180)];

        let plan = plan_sync(&previous, &remote, cache.path(), true);
        assert_eq!(plan.fetch.len(), 1);
    }

    #[test]
    fn a_missing_local_copy_is_refetched_even_when_the_manifest_matches() {
        let cache = tempfile::tempdir().expect("tempdir");
        let previous = vec![entry("a.jsonl", 10, 100)];
        let remote = vec![entry("a.jsonl", 10, 100)];

        let plan = plan_sync(&previous, &remote, cache.path(), true);
        assert_eq!(plan.fetch.len(), 1);
    }

    #[test]
    fn disabling_incremental_mode_refetches_everything() {
        let cache = tempfile::tempdir().expect("tempdir");
        std::fs::write(cache.path().join("a.jsonl"), "{}").expect("write cached session");

        let previous = vec![entry("a.jsonl", 10, 100)];
        let remote = vec![entry("a.jsonl", 10, 100)];

        let plan = plan_sync(&previous, &remote, cache.path(), false);
        assert_eq!(plan.fetch.len(), 1);
    }

    #[test]
    fn batches_stay_under_the_byte_and_file_caps() {
        let big = vec![
            entry("a.jsonl", MAX_BATCH_BYTES - 1, 1),
            entry("b.jsonl", MAX_BATCH_BYTES - 1, 2),
        ];
        assert_eq!(batches(&big).len(), 2);

        let many: Vec<_> = (0..MAX_BATCH_FILES * 2 + 1)
            .map(|index| entry(&format!("{index}.jsonl"), 1, index as i64))
            .collect();
        assert_eq!(batches(&many).len(), 3);

        // A single oversized file still gets its own batch rather than being
        // dropped.
        let single = vec![entry("huge.jsonl", MAX_BATCH_BYTES * 4, 1)];
        assert_eq!(batches(&single).len(), 1);
        assert!(batches(&[]).is_empty());
    }

    #[test]
    fn very_large_session_sets_keep_the_most_recent_entries() {
        let entries: Vec<_> = (0..MAX_TRACKED_SESSIONS + 10)
            .map(|index| entry(&format!("{index:05}.jsonl"), 1, index as i64))
            .collect();

        let (capped, truncated) = cap_manifest(entries);

        assert!(truncated);
        assert_eq!(capped.len(), MAX_TRACKED_SESSIONS);
        assert!(capped.iter().all(|entry| entry.mtime_ms >= 10));
    }

    #[test]
    fn frames_are_parsed_back_into_files() {
        // Payloads are byte-exact: a session's own trailing newline is part of
        // its frame length, and no separator is added between frames.
        let stream = b"FILE 6 a.jsonl\nhello\nFILE 3 dir/b.jsonl\nbyeEND\n";
        let mut reader = &stream[..];

        let files = parse_frames(&mut reader).expect("parse frames");
        assert_eq!(files[0], ("a.jsonl".to_string(), b"hello\n".to_vec()));
        assert_eq!(files[1], ("dir/b.jsonl".to_string(), b"bye".to_vec()));
    }

    #[test]
    fn a_truncated_transfer_is_an_error_not_a_partial_import() {
        let mut reader = &b"FILE 100 a.jsonl\nshort"[..];
        let error = parse_frames(&mut reader).expect_err("expected truncation to fail");
        assert!(error.to_string().contains("truncated"));

        let mut missing_end = &b"FILE 5 a.jsonl\nhello"[..];
        assert!(parse_frames(&mut missing_end).is_err());
    }

    #[test]
    fn frames_cannot_write_outside_the_cache() {
        let mut reader = &b"FILE 1 ../escape.jsonl\nx\nEND\n"[..];
        let error = parse_frames(&mut reader).expect_err("expected an unsafe path to fail");
        assert!(error.to_string().contains("unsafe path"));
    }

    #[test]
    #[serial]
    fn a_full_refresh_mirrors_wsl_sessions_and_then_goes_incremental() {
        let home = tempfile::tempdir().expect("tempdir");
        let config = tempfile::tempdir().expect("tempdir");
        let _home_guard = TestHome::install(config.path());
        let sessions = home.path().join(".pi/agent/sessions/project-a");
        fs::create_dir_all(&sessions).expect("create sessions");
        fs::write(sessions.join("one.jsonl"), "{\"type\":\"session\"}\n").expect("write session");
        fs::write(sessions.join("two.jsonl"), "{\"type\":\"session\"}\n").expect("write session");

        let _runner = RunnerGuard::install(Arc::new(LocalBashRunner::new(
            "Ubuntu-22.04",
            home.path().to_path_buf(),
        )));
        let target = PiRuntimeTarget::Wsl {
            distro: "Ubuntu-22.04".to_string(),
            home: home.path().to_string_lossy().into_owned(),
            agent_dir: format!("{}/.pi/agent", home.path().to_string_lossy()),
        };

        let first = sync(&target).expect("first sync");
        assert_eq!(first.fetched, 2);
        assert_eq!(first.total, 2);
        assert!(first.errors.is_empty(), "{:?}", first.errors);

        let mirrored = cache_root("Ubuntu-22.04").join("sessions/project-a/one.jsonl");
        assert_eq!(
            fs::read_to_string(&mirrored).expect("read mirrored session"),
            "{\"type\":\"session\"}\n"
        );

        // Nothing changed in WSL, so the second refresh transfers nothing.
        let second = sync(&target).expect("second sync");
        assert_eq!(second.fetched, 0);
        assert_eq!(second.unchanged, 2);

        // A deleted session disappears from the mirror.
        fs::remove_file(sessions.join("two.jsonl")).expect("remove session");
        let third = sync(&target).expect("third sync");
        assert_eq!(third.removed, 1);
        assert_eq!(third.total, 1);
        assert!(!cache_root("Ubuntu-22.04")
            .join("sessions/project-a/two.jsonl")
            .exists());
    }

    #[cfg(unix)]
    fn write_path_stub(dir: &Path, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(dir).expect("stub bin");
        let path = dir.join(name);
        fs::write(&path, body).expect("write stub");
        let mut perms = fs::metadata(&path).expect("stub meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod stub");
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn a_non_gnu_find_uses_the_portable_listing_instead_of_fetched_zero() {
        let home = tempfile::tempdir().expect("tempdir");
        let config = tempfile::tempdir().expect("tempdir");
        let stubs = tempfile::tempdir().expect("stub bin");
        let _home_guard = TestHome::install(config.path());
        let sessions = home.path().join(".pi/agent/sessions/project-a");
        fs::create_dir_all(&sessions).expect("create sessions");
        fs::write(sessions.join("one.jsonl"), "{\"type\":\"session\"}\n").expect("write session");

        let real_find = which_find();
        write_path_stub(
            stubs.path(),
            "find",
            &format!(
                "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = \"-printf\" ]; then\n    echo 'find: unknown predicate -printf' >&2\n    exit 1\n  fi\ndone\nexec {real_find} \"$@\"\n"
            ),
        );

        let _runner = RunnerGuard::install(Arc::new(
            LocalBashRunner::new("Ubuntu-22.04", home.path().to_path_buf())
                .with_path_prepend(stubs.path().to_path_buf()),
        ));
        let target = PiRuntimeTarget::Wsl {
            distro: "Ubuntu-22.04".to_string(),
            home: home.path().to_string_lossy().into_owned(),
            agent_dir: format!("{}/.pi/agent", home.path().to_string_lossy()),
        };

        let outcome = sync(&target).expect("portable find listing");
        assert!(
            outcome.errors.is_empty(),
            "portable listing errors: {:?}",
            outcome.errors
        );
        assert_eq!(
            outcome.fetched, 1,
            "BSD find must not collapse to fetched=0"
        );
        assert!(cache_root("Ubuntu-22.04")
            .join("sessions/project-a/one.jsonl")
            .is_file());
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn an_unusable_find_is_a_hard_error_not_a_silent_empty_manifest() {
        let home = tempfile::tempdir().expect("tempdir");
        let config = tempfile::tempdir().expect("tempdir");
        let stubs = tempfile::tempdir().expect("stub bin");
        let _home_guard = TestHome::install(config.path());
        let sessions = home.path().join(".pi/agent/sessions/project-a");
        fs::create_dir_all(&sessions).expect("create sessions");
        fs::write(sessions.join("one.jsonl"), "{}\n").expect("write session");

        write_path_stub(
            stubs.path(),
            "find",
            "#!/bin/sh\necho 'find: broken listing tool' >&2\nexit 1\n",
        );

        let _runner = RunnerGuard::install(Arc::new(
            LocalBashRunner::new("Ubuntu-22.04", home.path().to_path_buf())
                .with_path_prepend(stubs.path().to_path_buf()),
        ));
        let target = PiRuntimeTarget::Wsl {
            distro: "Ubuntu-22.04".to_string(),
            home: home.path().to_string_lossy().into_owned(),
            agent_dir: format!("{}/.pi/agent", home.path().to_string_lossy()),
        };

        let error = sync(&target).expect_err("unusable find must fail the sync");
        let message = error.to_string();
        assert!(
            message.contains("listing Pi sessions") || message.contains("find"),
            "expected a clear listing failure, got: {message}"
        );
    }

    #[cfg(unix)]
    fn which_find() -> String {
        ["/usr/bin/find", "/bin/find"]
            .iter()
            .copied()
            .find(|path| Path::new(path).is_file())
            .unwrap_or("/usr/bin/find")
            .to_string()
    }

    #[test]
    #[serial]
    fn wsl_source_paths_round_trip_through_the_cache() {
        let config = tempfile::tempdir().expect("tempdir");
        let _home_guard = TestHome::install(config.path());
        let target = PiRuntimeTarget::Wsl {
            distro: "Ubuntu-22.04".to_string(),
            home: "/home/me".to_string(),
            agent_dir: "/home/me/.pi/agent".to_string(),
        };

        let cached = cache_root("Ubuntu-22.04").join("sessions/project-a/abc.jsonl");
        assert_eq!(
            wsl_source_path(&target, &cached).as_deref(),
            Some("/home/me/.pi/agent/sessions/project-a/abc.jsonl")
        );
        assert_eq!(
            wsl_source_path(&target, Path::new("/somewhere/else.jsonl")),
            None
        );
    }

    /// Point `get_app_config_dir` at a temp directory so the cache does not
    /// touch a real profile.
    struct TestHome {
        previous: Option<String>,
    }

    impl TestHome {
        fn install(path: &Path) -> Self {
            let previous = std::env::var("CC_SWITCH_TEST_HOME").ok();
            std::env::set_var("CC_SWITCH_TEST_HOME", path);
            Self { previous }
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }
}
