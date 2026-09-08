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

/// `$1` target, `$2` expected revision, `$3` digest of the incoming payload.
const WRITE_SCRIPT: &str = r#"
set -u
target="$1"; expected="$2"; payload="$3"
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
tmp=$(mktemp -- "$dir/.cc-switch-XXXXXX") || { printf 'mktemp-failed\n' >&2; exit 1; }
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
"#;

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
            Self::Wsl { distro, path } => format!("\\\\wsl\\{distro}{path}"),
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

impl PiFileRead {
    pub fn exists(&self) -> bool {
        self.bytes.is_some()
    }
}

/// SHA-256 digest used for optimistic concurrency on Pi's native files.
pub fn revision(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Resolve where `file` lives on the active runtime.
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
        } => {
            let path = format!("{agent_dir}/{}", file.file_name());
            if !wsl::is_valid_linux_path(&path) {
                return Err(PiRuntimeError::invalid_input(format!(
                    "unusable Pi {} path in WSL '{distro}'",
                    file.file_name()
                )));
            }
            Ok(PiFileLocation::Wsl {
                distro: distro.clone(),
                path,
            })
        }
    }
}

/// Read `file` from the active runtime, rejecting anything over `max_bytes`.
pub fn read(file: PiFile, max_bytes: u64) -> PiResult<PiFileRead> {
    let location = locate(file)?;
    match &location {
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
    let status = String::from_utf8_lossy(&payload[..newline]).trim().to_string();
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

fn write_local(
    file: PiFile,
    path: &Path,
    bytes: &[u8],
    expected_revision: &str,
) -> PiResult<()> {
    ensure_private_parent(path)?;

    let actual_revision = match fs::read(path) {
        Ok(current) => revision(&current),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            MISSING_REVISION.to_string()
        }
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
    let output = wsl::run(
        &WslRequest::guarded(distro, WRITE_SCRIPT)
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
        PiRuntimeError::command_failed(format!(
            "cannot create {}: {error}",
            parent.display()
        ))
    })?;

    #[cfg(not(unix))]
    let _ = created;

    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|error| {
            PiRuntimeError::command_failed(format!(
                "cannot restrict {}: {error}",
                parent.display()
            ))
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
        assert!(!read.exists());
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

        assert_eq!(error.code, crate::pi_runtime::error::PiRuntimeErrorCode::PiConfigConflict);
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
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".cc-switch"))
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

        write(PiFile::Settings, b"{\"defaultModel\":\"x\"}\n", MISSING_REVISION)
            .expect("write settings");

        assert!(!read(PiFile::Models, LIMIT).expect("read models").exists());
        assert!(read(PiFile::Settings, LIMIT).expect("read settings").exists());
    }

    #[test]
    #[serial]
    fn wsl_locations_are_described_without_a_unc_path_users_can_paste() {
        let _fixture = wsl_fixture();
        let location = locate(PiFile::Models).expect("locate models");
        assert!(matches!(location, PiFileLocation::Wsl { .. }));
        assert!(location.display().contains("Ubuntu-22.04"));
    }

    #[test]
    #[serial]
    fn the_local_runtime_reads_and_writes_the_filesystem_directly() {
        let _agent = crate::pi_config::test_support::TestAgentDir::new();
        let _target = TestTarget::install(PiRuntimeTarget::Local);

        assert!(!read(PiFile::Models, LIMIT).expect("read models").exists());
        write(PiFile::Models, b"{\"providers\":{}}\n", MISSING_REVISION)
            .expect("write models");

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
        assert_eq!(error.code, crate::pi_runtime::error::PiRuntimeErrorCode::PiConfigConflict);
    }
}
