//! Runtime-agnostic access to Pi's native configuration files.
//!
//! `pi_config` owns the JSON semantics (which provider node to touch, which
//! fields to preserve, how conflicts are reported). This module owns *where*
//! those bytes live and how they are replaced safely.
//!
//! Every write is:
//!
//! 1. guarded by the revision the caller read,
//! 2. staged in a sibling temporary file,
//! 3. verified against the payload digest,
//! 4. moved into place atomically,
//! 5. read back and verified again.
//!
//! Steps 3 and 5 matter more for WSL than locally: bytes cross a pipe into
//! another kernel, and a truncated transfer must never be mistaken for a
//! successful save.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::{PiResult, PiRuntimeError};
use super::wsl::{self, WslRequest};
use super::PiRuntimeTarget;

/// Revision reported for a file that does not exist yet.
pub const MISSING_REVISION: &str = "missing";

const READ_SCRIPT: &str = r#"
set -u
if [ ! -e "$1" ]; then printf 'missing\n'; exit 0; fi
if [ ! -f "$1" ]; then printf 'notfile\n'; exit 0; fi
size=$(wc -c < "$1")
if [ "$size" -gt "$2" ]; then printf 'toolarge %s\n' "$size"; exit 0; fi
printf 'ok %s\n' "$(sha256sum < "$1" | cut -d' ' -f1)"
cat -- "$1"
"#;

/// Stage the payload under `$TMPDIR` or `/tmp`. Never `mktemp` at `/` — an
/// empty `$1` used to expand `"$dir/.cc-switch-XXXXXX"` into
/// `/.cc-switch-XXXXXX` (Codex WSL write on Ubuntu-22.04).
macro_rules! atomic_stage_snippet {
    () => {
        r#"
stage="${TMPDIR:-/tmp}"
case "$stage" in
  /*) ;;
  *) stage=/tmp ;;
esac
if [ ! -d "$stage" ]; then
  stage=/tmp
fi
mkdir -p -- "$stage" || { printf 'mkdir-failed\n' >&2; exit 1; }
tmp=$(mktemp -p "$stage" cc-switch-XXXXXX 2>/dev/null) \
  || tmp=$(mktemp /tmp/cc-switch-XXXXXX) \
  || { printf 'mktemp-failed\n' >&2; exit 1; }
"#
    };
}

pub const ATOMIC_STAGE_SNIPPET: &str = atomic_stage_snippet!();
pub(crate) use atomic_stage_snippet;

/// `$1` target, `$2` expected revision, `$3` digest of the incoming payload.
const WRITE_SCRIPT: &str = concat!(
    r#"
set -u
target="${1:-}"; expected="${2:-}"; payload="${3:-}"
if [ -z "$target" ] || [ "$target" = "/" ]; then
  printf 'empty-target\n' >&2
  exit 1
fi
dir=$(dirname -- "$target")
mkdir -p -- "$dir" || { printf 'mkdir-failed\n' >&2; exit 1; }
chmod 700 -- "$dir" 2>/dev/null || true
if [ -f "$target" ]; then
  actual=$(sha256sum < "$target" | cut -d' ' -f1)
elif [ -e "$target" ]; then
  printf 'not-a-file\n' >&2; exit 1
else
  actual=missing
fi
if [ "$actual" != "$expected" ]; then printf 'revision-mismatch\n'; exit 0; fi
umask 077
"#,
    atomic_stage_snippet!(),
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

/// `$1` target, `$2` digest of the incoming payload. No revision check: the
/// top-level `~/.pi/models.json` mirror is overwritten to match the agent file.
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
    atomic_stage_snippet!(),
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

/// Pi's native configuration files that CC Switch reads or writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiFile {
    Models,
    Settings,
}

impl PiFile {
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Models => "models.json",
            Self::Settings => "settings.json",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Models => "Pi models",
            Self::Settings => "Pi settings",
        }
    }
}

/// Where a Pi file lives on the resolved runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiFileLocation {
    Local(PathBuf),
    Wsl { distro: String, path: String },
}

impl PiFileLocation {
    /// Human-readable location for error messages and logs.
    pub fn display(&self) -> String {
        match self {
            Self::Local(path) => path.display().to_string(),
            Self::Wsl { distro, path } => format!("wsl:{distro}:{path}"),
        }
    }
}

/// Result of reading a Pi file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiFileRead {
    /// `None` when the file does not exist.
    pub bytes: Option<Vec<u8>>,
    /// Digest of the bytes on disk, or [`MISSING_REVISION`].
    pub revision: String,
    pub location: PiFileLocation,
}

/// SHA-256 digest used for optimistic concurrency on Pi's native files.
pub fn revision(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Resolve where `file` lives on the active runtime (canonical agent layer).
pub fn locate(file: PiFile) -> PiResult<PiFileLocation> {
    locate_on(&super::target(), file)
}

pub fn locate_on(target: &PiRuntimeTarget, file: PiFile) -> PiResult<PiFileLocation> {
    match target {
        PiRuntimeTarget::Local => Ok(PiFileLocation::Local(
            crate::pi_config::get_pi_agent_dir()
                .map_err(|error| PiRuntimeError::config_not_found(error.to_string()))?
                .join(file.file_name()),
        )),
        PiRuntimeTarget::Wsl {
            distro, agent_dir, ..
        } => locate_wsl_under(distro, agent_dir, file.file_name()),
    }
}

/// Top-level `{piHome}/models.json` (or settings.json) mirror, if it is a
/// distinct path from the canonical agent file.
pub fn locate_top_level(file: PiFile) -> PiResult<Option<PiFileLocation>> {
    locate_top_level_on(&super::target(), file)
}

fn locate_top_level_on(target: &PiRuntimeTarget, file: PiFile) -> PiResult<Option<PiFileLocation>> {
    match target {
        PiRuntimeTarget::Local => {
            let Some(path) = crate::pi_config::get_pi_top_level_path(file.file_name())
                .map_err(|error| PiRuntimeError::config_not_found(error.to_string()))?
            else {
                return Ok(None);
            };
            Ok(Some(PiFileLocation::Local(path)))
        }
        PiRuntimeTarget::Wsl {
            distro, agent_dir, ..
        } => {
            let Some(home) = agent_dir
                .trim_end_matches('/')
                .strip_suffix("/agent")
                .filter(|home| !home.is_empty())
            else {
                return Ok(None);
            };
            Ok(Some(locate_wsl_under(distro, home, file.file_name())?))
        }
    }
}

fn locate_wsl_under(distro: &str, dir: &str, file_name: &str) -> PiResult<PiFileLocation> {
    let path = format!("{dir}/{file_name}");
    if !wsl::is_valid_linux_path(&path) {
        return Err(PiRuntimeError::invalid_input(format!(
            "unusable Pi {file_name} path in WSL '{distro}'"
        )));
    }
    Ok(PiFileLocation::Wsl {
        distro: distro.to_string(),
        path,
    })
}

/// Read `file` from the active runtime, rejecting anything over `max_bytes`.
pub fn read(file: PiFile, max_bytes: u64) -> PiResult<PiFileRead> {
    heal_agent_from_top_level(file, max_bytes);
    read_at(&locate(file)?, file, max_bytes)
}

fn read_at(location: &PiFileLocation, file: PiFile, max_bytes: u64) -> PiResult<PiFileRead> {
    match location {
        PiFileLocation::Local(path) => read_local(file, path, max_bytes, location.clone()),
        PiFileLocation::Wsl { distro, path } => {
            read_wsl(file, distro, path, max_bytes, location.clone())
        }
    }
}

/// Replace `file` with `bytes`, but only if it still matches
/// `expected_revision`.
pub fn write(file: PiFile, bytes: &[u8], expected_revision: &str) -> PiResult<()> {
    let location = locate(file)?;
    match &location {
        PiFileLocation::Local(path) => write_local(file, path, bytes, expected_revision)?,
        PiFileLocation::Wsl { distro, path } => {
            write_wsl(file, distro, path, bytes, expected_revision)?
        }
    }
    sync_top_level_mirror(file, bytes);
    Ok(())
}

/// Re-copy the canonical agent file onto `{piHome}/models.json` (or
/// settings.json) even when the agent bytes did not change. Restore/project
/// paths use this so a leftover top-level file cannot keep proxy URLs.
pub fn sync_canonical_to_mirror(file: PiFile) {
    let Ok(location) = locate(file) else {
        return;
    };
    let Ok(read) = read_at(&location, file, 1024 * 1024) else {
        return;
    };
    let Some(bytes) = read.bytes.as_deref() else {
        return;
    };
    sync_top_level_mirror(file, bytes);
}

/// Best-effort copy of the canonical agent file to `{piHome}/models.json`
/// (or settings.json). A mirror miss must not fail the agent write Pi reads.
fn sync_top_level_mirror(file: PiFile, bytes: &[u8]) {
    let Ok(Some(location)) = locate_top_level(file) else {
        return;
    };
    if let Err(error) = overwrite_at(&location, file, bytes) {
        log::warn!(
            "[PiConfig] could not sync top-level {} mirror ({}): {error}",
            file.file_name(),
            location.display()
        );
    }
}

fn overwrite_at(location: &PiFileLocation, file: PiFile, bytes: &[u8]) -> PiResult<()> {
    match location {
        PiFileLocation::Local(path) => {
            ensure_private_parent(path)?;
            crate::config::atomic_write_private(path, bytes)
                .map_err(|error| PiRuntimeError::command_failed(error.to_string()))?;
            Ok(())
        }
        PiFileLocation::Wsl { distro, path } => overwrite_wsl(file, distro, path, bytes),
    }
}

/// Keep `{piHome}/agent/{file}` and `{piHome}/{file}` from diverging.
///
/// The agent file is canonical (what Pi reads). Top-level is a mirror:
/// - missing agent + existing top → one-time migration into the agent file
/// - empty `models.json` providers + populated top → same migration
/// - otherwise the agent file wins and the top-level copy is overwritten
///
/// Never clobber a populated agent file from a newer top-level write: that
/// reintroduced proxy `baseUrl`s after takeover restore.
fn heal_agent_from_top_level(file: PiFile, max_bytes: u64) {
    if !matches!(file, PiFile::Models | PiFile::Settings) {
        return;
    }
    let Ok(agent_loc) = locate(file) else {
        return;
    };
    let Ok(Some(top_loc)) = locate_top_level(file) else {
        return;
    };
    if top_loc == agent_loc {
        return;
    }
    let Ok(agent) = read_at(&agent_loc, file, max_bytes) else {
        return;
    };
    let Ok(top) = read_at(&top_loc, file, max_bytes) else {
        return;
    };
    match (&agent.bytes, &top.bytes) {
        (None, None) => {}
        (Some(bytes), None) => {
            let _ = overwrite_at(&top_loc, file, bytes);
        }
        (None, Some(bytes)) => {
            if let Err(error) = write_at(&agent_loc, file, bytes, MISSING_REVISION) {
                log::warn!(
                    "[PiConfig] could not restore {} from top-level mirror: {error}",
                    file.file_name()
                );
            }
        }
        (Some(agent_bytes), Some(top_bytes)) if agent_bytes == top_bytes => {}
        (Some(agent_bytes), Some(top_bytes)) => {
            let migrate_empty_models = matches!(file, PiFile::Models)
                && models_document_has_no_providers(agent_bytes)
                && !models_document_has_no_providers(top_bytes);
            if migrate_empty_models {
                if let Err(error) = write_at(&agent_loc, file, top_bytes, &agent.revision) {
                    log::warn!(
                        "[PiConfig] could not migrate {} from top-level mirror: {error}",
                        file.file_name()
                    );
                }
            } else {
                let _ = overwrite_at(&top_loc, file, agent_bytes);
            }
        }
    }
}

fn models_document_has_no_providers(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    match value.get("providers") {
        None => true,
        Some(serde_json::Value::Object(providers)) => providers.is_empty(),
        Some(_) => false,
    }
}

fn write_at(
    location: &PiFileLocation,
    file: PiFile,
    bytes: &[u8],
    expected_revision: &str,
) -> PiResult<()> {
    match location {
        PiFileLocation::Local(path) => write_local(file, path, bytes, expected_revision),
        PiFileLocation::Wsl { distro, path } => {
            write_wsl(file, distro, path, bytes, expected_revision)
        }
    }
}

fn read_local(
    file: PiFile,
    path: &Path,
    max_bytes: u64,
    location: PiFileLocation,
) -> PiResult<PiFileRead> {
    if !path.exists() {
        return Ok(PiFileRead {
            bytes: None,
            revision: MISSING_REVISION.to_string(),
            location,
        });
    }
    let metadata = fs::metadata(path).map_err(|error| {
        PiRuntimeError::config_not_found(format!(
            "cannot stat {} ({}): {error}",
            file.label(),
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(PiRuntimeError::config_not_found(format!(
            "{} is not a file: {}",
            file.label(),
            path.display()
        )));
    }
    if metadata.len() > max_bytes {
        return Err(PiRuntimeError::invalid_input(format!(
            "{} file exceeds the {max_bytes}-byte limit: {}",
            file.label(),
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|error| {
        PiRuntimeError::config_not_found(format!(
            "cannot read {} ({}): {error}",
            file.label(),
            path.display()
        ))
    })?;
    Ok(PiFileRead {
        revision: revision(&bytes),
        bytes: Some(bytes),
        location,
    })
}

fn read_wsl(
    file: PiFile,
    distro: &str,
    path: &str,
    max_bytes: u64,
    location: PiFileLocation,
) -> PiResult<PiFileRead> {
    let output = wsl::run(
        &WslRequest::guarded(distro, READ_SCRIPT)
            .arg(path)
            .arg(max_bytes.to_string()),
    )?;
    output.require_success(&format!("reading {}", file.label()))?;

    let payload = output.payload();
    let newline = payload
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or_else(|| {
            PiRuntimeError::command_failed(format!(
                "reading {} returned no status line",
                file.label()
            ))
        })?;
    let status = String::from_utf8_lossy(&payload[..newline])
        .trim()
        .to_string();
    let body = &payload[newline + 1..];

    let mut fields = status.split_whitespace();
    match fields.next() {
        Some("missing") => Ok(PiFileRead {
            bytes: None,
            revision: MISSING_REVISION.to_string(),
            location,
        }),
        Some("notfile") => Err(PiRuntimeError::config_not_found(format!(
            "{} is not a file: {}",
            file.label(),
            location.display()
        ))),
        Some("toolarge") => Err(PiRuntimeError::invalid_input(format!(
            "{} file exceeds the {max_bytes}-byte limit: {}",
            file.label(),
            location.display()
        ))),
        Some("ok") => {
            let expected = fields.next().unwrap_or_default().to_string();
            let actual = revision(body);
            if expected != actual {
                // The digest is computed inside WSL before the bytes cross the
                // pipe, so a mismatch means the transfer was truncated.
                return Err(PiRuntimeError::command_failed(format!(
                    "{} was truncated in transit from WSL '{distro}'",
                    file.label()
                )));
            }
            Ok(PiFileRead {
                bytes: Some(body.to_vec()),
                revision: actual,
                location,
            })
        }
        _ => Err(PiRuntimeError::command_failed(format!(
            "reading {} returned an unexpected status: {status}",
            file.label()
        ))),
    }
}

fn write_local(file: PiFile, path: &Path, bytes: &[u8], expected_revision: &str) -> PiResult<()> {
    ensure_private_parent(path)?;

    let actual_revision = match fs::read(path) {
        Ok(current) => revision(&current),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => MISSING_REVISION.to_string(),
        Err(error) => {
            return Err(PiRuntimeError::config_not_found(format!(
                "cannot read {} before writing ({}): {error}",
                file.label(),
                path.display()
            )))
        }
    };
    if actual_revision != expected_revision {
        return Err(PiRuntimeError::config_conflict(format!(
            "Pi {} changed outside CC Switch: {}",
            file.file_name(),
            path.display()
        )));
    }

    crate::config::atomic_write_private(path, bytes)
        .map_err(|error| PiRuntimeError::command_failed(error.to_string()))?;

    let written = fs::read(path).map_err(|error| {
        PiRuntimeError::command_failed(format!(
            "cannot verify {} after writing ({}): {error}",
            file.label(),
            path.display()
        ))
    })?;
    if written != bytes {
        return Err(PiRuntimeError::command_failed(format!(
            "{} did not match the intended contents after writing: {}",
            file.label(),
            path.display()
        )));
    }
    Ok(())
}

fn write_wsl(
    file: PiFile,
    distro: &str,
    path: &str,
    bytes: &[u8],
    expected_revision: &str,
) -> PiResult<()> {
    let payload_revision = revision(bytes);
    if !wsl::is_valid_linux_path(path) {
        return Err(PiRuntimeError::invalid_input(format!(
            "unusable Pi write path in WSL '{distro}'"
        )));
    }
    let script =
        wsl::with_dropped_arg_fallback(WRITE_SCRIPT, &[path, expected_revision, &payload_revision]);
    let output = wsl::run(
        &WslRequest::guarded(distro, &script)
            .arg(path)
            .arg(expected_revision)
            .arg(&payload_revision)
            .stdin(bytes.to_vec())
            .timeout(wsl::DEFAULT_TIMEOUT),
    )?;

    if !output.succeeded() {
        let detail = wsl::first_line(&output.stderr).unwrap_or("no stderr output");
        return Err(PiRuntimeError::command_failed(format!(
            "writing Pi {} to WSL '{distro}' failed: {detail}",
            file.file_name()
        )));
    }

    match wsl::first_line(&output.payload_lossy()) {
        Some("ok") => {
            log::info!(
                "[PiRuntime] wrote Pi {} to WSL '{distro}' ({} bytes)",
                file.file_name(),
                bytes.len()
            );
            Ok(())
        }
        Some("revision-mismatch") => Err(PiRuntimeError::config_conflict(format!(
            "Pi {} changed outside CC Switch in WSL '{distro}'",
            file.file_name()
        ))),
        other => Err(PiRuntimeError::command_failed(format!(
            "writing Pi {} to WSL '{distro}' returned an unexpected status: {}",
            file.file_name(),
            other.unwrap_or("no output")
        ))),
    }
}

fn overwrite_wsl(file: PiFile, distro: &str, path: &str, bytes: &[u8]) -> PiResult<()> {
    let payload_revision = revision(bytes);
    if !wsl::is_valid_linux_path(path) {
        return Err(PiRuntimeError::invalid_input(format!(
            "unusable Pi overwrite path in WSL '{distro}'"
        )));
    }
    let script = wsl::with_dropped_arg_fallback(OVERWRITE_SCRIPT, &[path, &payload_revision]);
    let output = wsl::run(
        &WslRequest::guarded(distro, &script)
            .arg(path)
            .arg(&payload_revision)
            .stdin(bytes.to_vec())
            .timeout(wsl::DEFAULT_TIMEOUT),
    )?;
    if !output.succeeded() {
        let detail = wsl::first_line(&output.stderr).unwrap_or("no stderr output");
        return Err(PiRuntimeError::command_failed(format!(
            "mirroring Pi {} in WSL '{distro}' failed: {detail}",
            file.file_name()
        )));
    }
    match wsl::first_line(&output.payload_lossy()) {
        Some("ok") => Ok(()),
        other => Err(PiRuntimeError::command_failed(format!(
            "mirroring Pi {} in WSL '{distro}' returned an unexpected status: {}",
            file.file_name(),
            other.unwrap_or("no output")
        ))),
    }
}

/// Create the agent directory if needed, keeping it owner-only.
fn ensure_private_parent(path: &Path) -> PiResult<()> {
    let parent = path.parent().ok_or_else(|| {
        PiRuntimeError::invalid_input(format!(
            "Pi config path has no parent directory: {}",
            path.display()
        ))
    })?;
    let created = !parent.exists();
    fs::create_dir_all(parent).map_err(|error| {
        PiRuntimeError::command_failed(format!("cannot create {}: {error}", parent.display()))
    })?;

    #[cfg(not(unix))]
    let _ = created;

    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|error| {
            PiRuntimeError::command_failed(format!("cannot restrict {}: {error}", parent.display()))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_runtime::test_support::TestTarget;
    use crate::pi_runtime::wsl::test_support::{LocalBashRunner, RunnerGuard};
    use serial_test::serial;
    use std::sync::Arc;

    const LIMIT: u64 = 1024 * 1024;

    struct WslFixture {
        _home: tempfile::TempDir,
        _runner: RunnerGuard,
        _target: TestTarget,
        agent_dir: PathBuf,
    }

    /// A WSL runtime whose "Linux" filesystem is a temp directory reached
    /// through a real `bash`, so the scripts above are genuinely executed.
    fn wsl_fixture() -> WslFixture {
        let home = tempfile::tempdir().expect("tempdir");
        let home_path = home.path().to_string_lossy().into_owned();
        let agent_dir = home.path().join(".pi/agent");
        let runner = RunnerGuard::install(Arc::new(LocalBashRunner::new(
            "Ubuntu-22.04",
            home.path().to_path_buf(),
        )));
        let target = TestTarget::wsl("Ubuntu-22.04", &home_path);
        WslFixture {
            _home: home,
            _runner: runner,
            _target: target,
            agent_dir,
        }
    }

    #[test]
    #[serial]
    fn a_missing_wsl_models_file_reads_as_missing() {
        let _fixture = wsl_fixture();

        let read = read(PiFile::Models, LIMIT).expect("read models");
        assert!(read.bytes.is_none());
        assert_eq!(read.revision, MISSING_REVISION);
    }

    #[test]
    #[serial]
    fn writing_creates_the_agent_directory_and_round_trips_bytes() {
        let fixture = wsl_fixture();
        let document = b"{\n  \"providers\": {\n    \"cc-switch\": {}\n  }\n}\n";

        write(PiFile::Models, document, MISSING_REVISION).expect("write models");

        let read_back = read(PiFile::Models, LIMIT).expect("read models");
        assert_eq!(read_back.bytes.as_deref(), Some(document.as_slice()));
        assert_eq!(read_back.revision, revision(document));
        assert!(fixture.agent_dir.join("models.json").is_file());
    }

    #[test]
    #[serial]
    fn writing_with_a_stale_revision_leaves_the_file_alone() {
        let fixture = wsl_fixture();
        let original = b"{\"providers\":{\"external\":{}}}\n";
        write(PiFile::Models, original, MISSING_REVISION).expect("seed models");

        // Pi (or the user) edits the file behind our back.
        let external = b"{\"providers\":{\"external\":{},\"pi-added\":{}}}\n";
        fs::write(fixture.agent_dir.join("models.json"), external).expect("external edit");

        let error = write(PiFile::Models, b"{}\n", &revision(original))
            .expect_err("a stale write must be refused");

        assert_eq!(
            error.code,
            crate::pi_runtime::error::PiRuntimeErrorCode::PiConfigConflict
        );
        assert_eq!(
            fs::read(fixture.agent_dir.join("models.json")).expect("read models"),
            external
        );
    }

    #[test]
    #[serial]
    fn a_failed_write_leaves_no_temporary_files_behind() {
        let fixture = wsl_fixture();
        write(PiFile::Models, b"{}\n", MISSING_REVISION).expect("seed models");

        let _ = write(PiFile::Models, b"{\"a\":1}\n", "0000")
            .expect_err("a stale write must be refused");

        let leftovers: Vec<_> = fs::read_dir(&fixture.agent_dir)
            .expect("read agent dir")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".cc-switch")
            })
            .collect();
        assert!(leftovers.is_empty(), "temporary files were left behind");
    }

    #[test]
    #[serial]
    fn oversized_wsl_files_are_refused_instead_of_streamed() {
        let fixture = wsl_fixture();
        fs::create_dir_all(&fixture.agent_dir).expect("create agent dir");
        fs::write(fixture.agent_dir.join("models.json"), vec![b'x'; 4096])
            .expect("write oversized models");

        let error = read(PiFile::Models, 1024).expect_err("expected a size failure");
        assert!(error.to_string().contains("exceeds"));
    }

    #[test]
    #[serial]
    fn utf8_content_survives_the_pipe_intact() {
        let _fixture = wsl_fixture();
        let document = "{\"providers\":{\"示例\":{\"name\":\"日本語 provider\"}}}\n".as_bytes();

        write(PiFile::Models, document, MISSING_REVISION).expect("write models");

        let read_back = read(PiFile::Models, LIMIT).expect("read models");
        assert_eq!(read_back.bytes.as_deref(), Some(document));
    }

    #[test]
    #[serial]
    fn settings_and_models_resolve_to_distinct_files() {
        let _fixture = wsl_fixture();

        write(
            PiFile::Settings,
            b"{\"defaultModel\":\"x\"}\n",
            MISSING_REVISION,
        )
        .expect("write settings");

        assert!(read(PiFile::Models, LIMIT)
            .expect("read models")
            .bytes
            .is_none());
        assert!(read(PiFile::Settings, LIMIT)
            .expect("read settings")
            .bytes
            .is_some());
    }

    #[test]
    #[serial]
    fn wsl_locations_are_described_without_a_unc_path_users_can_paste() {
        let _fixture = wsl_fixture();
        let location = locate(PiFile::Models).expect("locate models");
        assert!(matches!(location, PiFileLocation::Wsl { .. }));
        assert!(location.display().contains("Ubuntu-22.04"));
        assert!(
            !location.display().contains(r"\\wsl"),
            "error text must not suggest a pasteable UNC path"
        );
    }

    #[test]
    #[serial]
    fn the_local_runtime_reads_and_writes_the_filesystem_directly() {
        let _agent = crate::pi_config::test_support::TestAgentDir::new();
        let _target = TestTarget::install(PiRuntimeTarget::Local);

        assert!(read(PiFile::Models, LIMIT)
            .expect("read models")
            .bytes
            .is_none());
        write(PiFile::Models, b"{\"providers\":{}}\n", MISSING_REVISION).expect("write models");

        let read_back = read(PiFile::Models, LIMIT).expect("read models");
        assert_eq!(
            read_back.bytes.as_deref(),
            Some(b"{\"providers\":{}}\n".as_slice())
        );
    }

    #[test]
    #[serial]
    fn the_local_runtime_also_enforces_the_revision_guard() {
        let _agent = crate::pi_config::test_support::TestAgentDir::new();
        let _target = TestTarget::install(PiRuntimeTarget::Local);
        write(PiFile::Models, b"{}\n", MISSING_REVISION).expect("seed models");

        let error =
            write(PiFile::Models, b"{\"a\":1}\n", MISSING_REVISION).expect_err("stale write");
        assert_eq!(
            error.code,
            crate::pi_runtime::error::PiRuntimeErrorCode::PiConfigConflict
        );
    }

    #[test]
    #[serial]
    fn local_writes_sync_the_top_level_models_mirror() {
        let _agent = crate::pi_config::test_support::TestAgentDir::new();
        let _target = TestTarget::install(PiRuntimeTarget::Local);
        let document = b"{\"providers\":{\"openai\":{}}}\n";
        write(PiFile::Models, document, MISSING_REVISION).expect("write models");

        let agent = crate::pi_config::get_pi_models_path().expect("agent path");
        let top = crate::pi_config::get_pi_top_level_path("models.json")
            .expect("top-level path")
            .expect("mirror");
        assert_eq!(fs::read(&agent).expect("agent"), document);
        assert_eq!(fs::read(&top).expect("top-level"), document);
        assert_ne!(agent, top);
    }

    #[test]
    #[serial]
    fn local_writes_sync_the_top_level_settings_mirror() {
        let _agent = crate::pi_config::test_support::TestAgentDir::new();
        let _target = TestTarget::install(PiRuntimeTarget::Local);
        let document = b"{\"defaultModel\":\"opus\"}\n";
        write(PiFile::Settings, document, MISSING_REVISION).expect("write settings");

        let agent = crate::pi_config::get_pi_settings_path().expect("agent settings");
        let top = crate::pi_config::get_pi_top_level_path("settings.json")
            .expect("top-level path")
            .expect("mirror");
        assert_eq!(fs::read(&agent).expect("agent"), document);
        assert_eq!(fs::read(&top).expect("top-level"), document);
    }

    #[test]
    #[serial]
    fn a_missing_agent_file_is_restored_from_the_top_level_mirror() {
        let _agent = crate::pi_config::test_support::TestAgentDir::new();
        let _target = TestTarget::install(PiRuntimeTarget::Local);
        let agent = crate::pi_config::get_pi_models_path().expect("agent path");
        let top = crate::pi_config::get_pi_top_level_path("models.json")
            .expect("top-level path")
            .expect("mirror");
        fs::create_dir_all(top.parent().expect("pi home")).expect("mkdir home");
        fs::write(&top, r#"{"providers":{"openai":{"name":"from-top"}}}"#).expect("top only");

        let read_back = read(PiFile::Models, LIMIT).expect("migrate");
        let live = String::from_utf8(read_back.bytes.expect("agent restored")).expect("utf8");
        assert!(live.contains("from-top"));
        assert_eq!(
            fs::read_to_string(&agent).expect("agent"),
            fs::read_to_string(&top).expect("top")
        );
    }

    #[test]
    #[serial]
    fn a_canonical_agent_file_overwrites_a_diverged_top_level_mirror() {
        let _agent = crate::pi_config::test_support::TestAgentDir::new();
        let _target = TestTarget::install(PiRuntimeTarget::Local);
        let agent = crate::pi_config::get_pi_models_path().expect("agent path");
        let top = crate::pi_config::get_pi_top_level_path("models.json")
            .expect("top-level path")
            .expect("mirror");
        fs::create_dir_all(agent.parent().expect("agent dir")).expect("mkdir agent");
        fs::write(&agent, r#"{"providers":{"canonical":{"name":"agent"}}}"#).expect("agent");
        fs::write(
            &top,
            r#"{"providers":{"stale":{"baseUrl":"http://127.0.0.1:15721/pi/stale"}}}"#,
        )
        .expect("diverged top");

        read(PiFile::Models, LIMIT).expect("heal");
        let agent_text = fs::read_to_string(&agent).expect("agent");
        let top_text = fs::read_to_string(&top).expect("top");
        assert!(agent_text.contains("canonical"));
        assert!(!agent_text.contains("stale"));
        assert_eq!(agent_text, top_text);
    }

    #[test]
    fn atomic_write_scripts_stage_under_tmp_never_at_root() {
        for script in [WRITE_SCRIPT, OVERWRITE_SCRIPT] {
            assert!(
                script.contains("mktemp -p") || script.contains("mktemp /tmp/cc-switch-XXXXXX"),
                "atomic write must mktemp under /tmp"
            );
            assert!(
                !script.contains("$dir/.cc-switch-XXXXXX"),
                "empty $dir must not expand to /.cc-switch-XXXXXX"
            );
            assert!(script.contains("empty-target"));
        }
        assert!(ATOMIC_STAGE_SNIPPET.contains("/tmp"));
        // v3.0.1 write contract: stdin → stage → sha256 → mv. Do not replace
        // this with a Windows-side copy or a non-atomic write.
        for script in [WRITE_SCRIPT, OVERWRITE_SCRIPT] {
            assert!(script.contains("cat > \"$tmp\""));
            assert!(script.contains("sha256sum < \"$tmp\""));
            assert!(script.contains("mv -f -- \"$tmp\" \"$target\""));
        }
    }

    #[test]
    #[serial]
    fn empty_wsl_target_fails_without_mktemp_at_root() {
        let _fixture = wsl_fixture();
        let output = wsl::run(
            &WslRequest::guarded("Ubuntu-22.04", WRITE_SCRIPT)
                .stdin(b"{}\n".to_vec())
                .timeout(wsl::DEFAULT_TIMEOUT),
        )
        .expect("run write script without $1");
        assert!(!output.succeeded(), "empty target must fail");
        let detail = format!("{}{}", output.stderr, output.payload_lossy());
        assert!(
            detail.contains("empty-target"),
            "expected empty-target, got {detail}"
        );
        assert!(
            !detail.contains("/.cc-switch-XXXXXX"),
            "must not mktemp at /: {detail}"
        );
    }

    #[test]
    #[serial]
    fn an_empty_agent_models_file_is_migrated_from_a_populated_top_level() {
        let _agent = crate::pi_config::test_support::TestAgentDir::new();
        let _target = TestTarget::install(PiRuntimeTarget::Local);
        let agent = crate::pi_config::get_pi_models_path().expect("agent path");
        let top = crate::pi_config::get_pi_top_level_path("models.json")
            .expect("top-level path")
            .expect("mirror");
        fs::create_dir_all(agent.parent().expect("agent dir")).expect("mkdir agent");
        fs::write(&agent, r#"{"providers":{}}"#).expect("empty agent");
        fs::write(&top, r#"{"providers":{"openai":{"name":"yumcode-std"}}}"#).expect("top");

        let providers = crate::pi_config::read_pi_native_providers().expect("migrate");
        assert!(providers.contains_key("openai"));
        assert_eq!(
            fs::read_to_string(&agent).expect("agent"),
            fs::read_to_string(&top).expect("top")
        );
    }
}
