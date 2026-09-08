//! Injection-safe `wsl.exe` execution.
//!
//! Core Pi runtime features must never reach into `\\wsl.localhost`: UNC access
//! to a WSL9p share is slow and fails outright while the distribution is
//! stopped. Everything goes through `wsl.exe` instead.
//!
//! # Injection safety
//!
//! Scripts are always compile-time constants owned by CC Switch. User-supplied
//! values (distro names, paths, session IDs) are never concatenated into a
//! shell string; they are passed as positional parameters and read back as
//! `"$1"`, `"$2"`, … inside the script:
//!
//! ```text
//! wsl.exe -d <distro> -- bash -lc '<our script>' cc-switch <arg1> <arg2>
//! ```
//!
//! `sh -c script name args...` assigns `name` to `$0`, so the first user value
//! lands in `$1`.

use std::fmt;
use std::io::Write;
use std::process::Command;
use std::sync::Arc;
#[cfg(test)]
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::error::{PiResult, PiRuntimeError};

/// Login shell startup is required so version managers (nvm, asdf, mise) put
/// `pi` on `PATH`, which makes every call pay shell-init cost. Keep the default
/// generous enough for a cold distribution start.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
/// Distribution enumeration and reachability probes must stay snappy because
/// they run on UI paths.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(6);
/// Batched session transfers move real payloads and need more headroom.
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);

const SHELL_ARG0: &str = "cc-switch";

/// Marks where our own output starts.
///
/// A login shell runs the user's `~/.profile` and `~/.bashrc`, and those
/// routinely print banners, `fortune`, version-manager notices or `neofetch`.
/// Without a marker that noise would be prepended to a `models.json` read.
pub const OUTPUT_SENTINEL: &str = "__CC_SWITCH_PI__";

/// Wrap a script body so its output can be separated from shell-startup noise.
pub fn guarded_script(body: &str) -> String {
    format!("printf '\\n%s\\n' '{OUTPUT_SENTINEL}'\n{body}")
}

#[derive(Debug, Clone)]
pub struct WslRequest {
    pub distro: String,
    pub script: String,
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Duration,
    /// Start a login shell. Required to find version-manager-installed `pi`,
    /// and avoided elsewhere because profile startup dominates latency.
    pub login: bool,
}

impl WslRequest {
    pub fn new(distro: impl Into<String>, script: impl Into<String>) -> Self {
        Self {
            distro: distro.into(),
            script: script.into(),
            args: Vec::new(),
            stdin: None,
            timeout: DEFAULT_TIMEOUT,
            login: false,
        }
    }

    /// A request whose output is fenced by [`OUTPUT_SENTINEL`].
    pub fn guarded(distro: impl Into<String>, body: &str) -> Self {
        Self::new(distro, guarded_script(body))
    }

    pub fn login(mut self, login: bool) -> Self {
        self.login = login;
        self
    }

    pub fn arg(mut self, value: impl Into<String>) -> Self {
        self.args.push(value.into());
        self
    }

    pub fn stdin(mut self, payload: Vec<u8>) -> Self {
        self.stdin = Some(payload);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[derive(Debug, Clone)]
pub struct WslExecResult {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl WslExecResult {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// Script stdout is produced by Linux tools, so it is UTF-8.
    ///
    /// Production code reads [`WslExecResult::payload`] instead; this exists
    /// so tests can assert on what a login shell printed before our output.
    #[cfg(test)]
    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Stdout with any shell-startup noise before [`OUTPUT_SENTINEL`] removed.
    ///
    /// Falls back to the whole of stdout when the script was not guarded. The
    /// *first* match is used because guarded payloads can be binary (gzip),
    /// and only CC Switch's own prologue ever emits the marker.
    pub fn payload(&self) -> &[u8] {
        let marker = OUTPUT_SENTINEL.as_bytes();
        let Some(start) = self
            .stdout
            .windows(marker.len())
            .position(|window| window == marker)
        else {
            return &self.stdout;
        };
        let after_marker = start + marker.len();
        match self.stdout[after_marker..].first() {
            Some(b'\n') => &self.stdout[after_marker + 1..],
            Some(b'\r') if self.stdout.get(after_marker + 1) == Some(&b'\n') => {
                &self.stdout[after_marker + 2..]
            }
            _ => &self.stdout[after_marker..],
        }
    }

    pub fn payload_lossy(&self) -> String {
        String::from_utf8_lossy(self.payload()).into_owned()
    }

    /// Fail unless the script exited 0, attributing the error to `context`.
    pub fn require_success(&self, context: &str) -> PiResult<()> {
        if self.succeeded() {
            return Ok(());
        }
        let code = self
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string());
        let detail = first_line(&self.stderr).unwrap_or("no stderr output");
        Err(PiRuntimeError::command_failed(format!(
            "{context} failed in WSL (exit {code}): {detail}"
        )))
    }
}

/// Executes a script inside a WSL distribution.
///
/// Behind a trait so file, session and proxy logic can be exercised against a
/// local `bash` in tests instead of requiring a Windows host with WSL.
pub trait WslRunner: Send + Sync + fmt::Debug {
    fn run(&self, request: &WslRequest) -> PiResult<WslExecResult>;

    /// Enumerate installed distributions (`wsl.exe -l -q`).
    fn list_distros(&self) -> PiResult<Vec<String>>;
}

/// Real implementation, spawning `wsl.exe`.
#[derive(Debug, Default, Clone, Copy)]
pub struct WslExeRunner;

impl WslRunner for WslExeRunner {
    fn run(&self, request: &WslRequest) -> PiResult<WslExecResult> {
        let argv = build_wsl_argv(request)?;
        run_wsl_exe(&argv, request)
    }

    fn list_distros(&self) -> PiResult<Vec<String>> {
        let request = WslRequest {
            distro: String::new(),
            script: String::new(),
            args: Vec::new(),
            stdin: None,
            timeout: PROBE_TIMEOUT,
            login: false,
        };
        let argv = ["-l".to_string(), "-q".to_string()];
        let output = run_wsl_exe(&argv, &request)?;
        output.require_success("wsl -l -q")?;
        Ok(parse_distro_list(&decode_console_output(&output.stdout)))
    }
}

/// Parse the `wsl -l -q` listing into distribution names.
///
/// The listing is UTF-16LE with CRLF endings, and a stopped default
/// distribution can still carry a `(Default)` suffix in some locales.
pub fn parse_distro_list(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let name = line
            .trim()
            .trim_end_matches('\r')
            .split(" (")
            .next()
            .unwrap_or_default()
            .trim();
        if name.is_empty() || !is_valid_distro_name(name) || names.iter().any(|seen| seen == name) {
            continue;
        }
        names.push(name.to_string());
    }
    names
}

/// Test-only substitute for `wsl.exe`, so release builds never pay for the
/// lookup and cannot have the runner swapped at runtime.
#[cfg(test)]
static RUNNER_OVERRIDE: LazyLock<Mutex<Option<Arc<dyn WslRunner>>>> =
    LazyLock::new(|| Mutex::new(None));

/// Return the process-wide runner, honouring a test override.
pub fn runner() -> Arc<dyn WslRunner> {
    #[cfg(test)]
    if let Some(runner) = RUNNER_OVERRIDE
        .lock()
        .expect("lock WSL runner override")
        .clone()
    {
        return runner;
    }
    Arc::new(WslExeRunner)
}

/// Install a runner for the current process. Returns the previous override so
/// callers can restore it.
#[cfg(test)]
pub fn set_runner_override(runner: Option<Arc<dyn WslRunner>>) -> Option<Arc<dyn WslRunner>> {
    let mut guard = RUNNER_OVERRIDE.lock().expect("lock WSL runner override");
    std::mem::replace(&mut *guard, runner)
}

/// Run `request` through the process-wide runner.
pub fn run(request: &WslRequest) -> PiResult<WslExecResult> {
    let started = Instant::now();
    let result = runner().run(request);
    let elapsed = started.elapsed().as_millis();
    match &result {
        Ok(output) => log::debug!(
            "[WslExecutor] distro={} args={} exit={:?} in {elapsed}ms",
            request.distro,
            request.args.len(),
            output.exit_code
        ),
        Err(error) => log::warn!("[WslExecutor] distro={} {error}", request.distro),
    }
    result
}

/// WSL distribution names are validated before they can reach a command line.
///
/// Deliberately narrower than what WSL itself accepts: the registered names
/// shipped by Microsoft Store distributions only use these characters, and a
/// name that fails this check is far more likely to be an injection attempt
/// than a real distribution.
pub fn is_valid_distro_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        // A leading hyphen would sit where `wsl.exe` expects the value of
        // `-d` and could be read as a flag instead.
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Absolute Linux paths are the only paths the runtime will act on.
pub fn is_valid_linux_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.is_empty()
        && path.len() <= 4096
        && !path.contains('\0')
        && !path.contains('\n')
        && !path.chars().any(char::is_control)
}

/// Build the `wsl.exe` argument vector for a script invocation.
pub fn build_wsl_argv(request: &WslRequest) -> PiResult<Vec<String>> {
    if !is_valid_distro_name(&request.distro) {
        return Err(PiRuntimeError::invalid_input(format!(
            "invalid WSL distribution name: '{}'",
            request.distro
        )));
    }
    if request.script.is_empty() {
        return Err(PiRuntimeError::invalid_input(
            "WSL script cannot be empty".to_string(),
        ));
    }
    for arg in &request.args {
        if arg.contains('\0') {
            return Err(PiRuntimeError::invalid_input(
                "WSL argument cannot contain a NUL byte".to_string(),
            ));
        }
    }

    let mut argv = vec![
        "-d".to_string(),
        request.distro.clone(),
        "--".to_string(),
        "bash".to_string(),
        if request.login { "-lc" } else { "-c" }.to_string(),
        request.script.clone(),
        SHELL_ARG0.to_string(),
    ];
    argv.extend(request.args.iter().cloned());
    Ok(argv)
}

#[cfg(target_os = "windows")]
fn run_wsl_exe(argv: &[String], request: &WslRequest) -> PiResult<WslExecResult> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut command = Command::new("wsl.exe");
    command
        .args(argv)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(if request.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    spawn_and_wait(command, request)
}

#[cfg(not(target_os = "windows"))]
fn run_wsl_exe(_argv: &[String], _request: &WslRequest) -> PiResult<WslExecResult> {
    Err(PiRuntimeError::wsl_not_found(
        "the WSL runtime is only available on Windows",
    ))
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn spawn_and_wait(mut command: Command, request: &WslRequest) -> PiResult<WslExecResult> {
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            PiRuntimeError::wsl_not_found("wsl.exe was not found on PATH")
        } else {
            PiRuntimeError::command_failed(format!("failed to start wsl.exe: {error}"))
        }
    })?;

    if let Some(payload) = request.stdin.clone() {
        if let Some(mut pipe) = child.stdin.take() {
            // A closed pipe means the script exited early; the exit code below
            // carries the real diagnosis, so a write failure is not fatal here.
            let _ = pipe.write_all(&payload);
            let _ = pipe.flush();
        }
    }
    drop(child.stdin.take());

    let stdout_reader = child.stdout.take().map(spawn_reader);
    let stderr_reader = child.stderr.take().map(spawn_reader);

    let deadline = Instant::now() + request.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                return Err(PiRuntimeError::command_failed(format!(
                    "failed to wait for wsl.exe: {error}"
                )))
            }
        }
    };

    let stdout = stdout_reader.map(join_reader).unwrap_or_default();
    let stderr_bytes = stderr_reader.map(join_reader).unwrap_or_default();

    let Some(status) = status else {
        return Err(PiRuntimeError::command_failed(format!(
            "wsl.exe timed out after {}s",
            request.timeout.as_secs()
        )));
    };

    Ok(WslExecResult {
        exit_code: status.code(),
        stdout,
        stderr: decode_console_output(&stderr_bytes),
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn spawn_reader<R: std::io::Read + Send + 'static>(
    mut pipe: R,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = std::io::Read::read_to_end(&mut pipe, &mut buffer);
        buffer
    })
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn join_reader(handle: std::thread::JoinHandle<Vec<u8>>) -> Vec<u8> {
    handle.join().unwrap_or_default()
}

/// Decode output that may come from `wsl.exe` itself rather than from Linux.
///
/// `wsl.exe` writes its own diagnostics ("There is no distribution with the
/// supplied name.") and `wsl -l -q` writes its listing as UTF-16LE, while
/// anything produced inside the distribution is UTF-8.
pub fn decode_console_output(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16le(rest);
    }
    if looks_like_utf16le(bytes) {
        return decode_utf16le(bytes);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn looks_like_utf16le(bytes: &[u8]) -> bool {
    if bytes.len() < 4 || bytes.len() % 2 != 0 {
        return false;
    }
    // ASCII text encoded as UTF-16LE puts a zero in every high byte.
    let high_bytes = bytes.len() / 2;
    let zero_high_bytes = bytes
        .chunks_exact(2)
        .filter(|pair| pair[1] == 0 && pair[0] != 0)
        .count();
    zero_high_bytes * 2 >= high_bytes
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

pub fn first_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

/// Test double that runs scripts through the local `bash`, which lets the
/// runtime's shell scripts be verified on Linux CI instead of being asserted
/// only as strings.
#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::process::Stdio;

    #[derive(Debug)]
    pub struct LocalBashRunner {
        pub known_distros: HashSet<String>,
        pub home: PathBuf,
    }

    impl LocalBashRunner {
        pub fn new(distro: &str, home: PathBuf) -> Self {
            Self {
                known_distros: HashSet::from([distro.to_string()]),
                home,
            }
        }
    }

    /// `bash -c` is on PATH and actually runs.
    pub fn bash_available() -> bool {
        Command::new("bash")
            .args(["-c", "printf ok"])
            .output()
            .is_ok_and(|output| output.status.success() && output.stdout.starts_with(b"ok"))
    }

    /// Host temp paths look like Linux paths (`/tmp/...`), so
    /// [`super::is_valid_linux_path`] accepts a WSL `agent_dir` built from a
    /// tempfile. False on native Windows (`C:\Users\...`).
    pub fn posix_temp_home_available() -> bool {
        bash_available() && super::is_valid_linux_path(&std::env::temp_dir().to_string_lossy())
    }

    /// Production `MANIFEST_SCRIPT` uses GNU `find -printf`. macOS ships BSD
    /// find; skip those tests there instead of failing CI.
    pub fn gnu_find_emulation_available() -> bool {
        if !posix_temp_home_available() {
            return false;
        }
        Command::new("bash")
            .args([
                "-c",
                "find . -maxdepth 0 -printf '%s\\t%T@\\t%P\\n' >/dev/null",
            ])
            .current_dir(std::env::temp_dir())
            .status()
            .is_ok_and(|status| status.success())
    }

    impl WslRunner for LocalBashRunner {
        fn run(&self, request: &WslRequest) -> PiResult<WslExecResult> {
            // Validate exactly like the real runner so tests exercise the same
            // argument checks.
            let argv = build_wsl_argv(request)?;
            if !self.known_distros.contains(&request.distro) {
                return Ok(WslExecResult {
                    exit_code: Some(1),
                    stdout: Vec::new(),
                    stderr: "There is no distribution with the supplied name.".to_string(),
                });
            }

            // Drop the `wsl.exe`-specific prefix (`-d <distro> --`).
            let shell_argv = &argv[3..];
            let mut command = Command::new(&shell_argv[0]);
            command
                .args(&shell_argv[1..])
                .env("HOME", &self.home)
                .env_remove("PI_CODING_AGENT_DIR")
                .stdin(if request.stdin.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            spawn_and_wait(command, request)
        }

        fn list_distros(&self) -> PiResult<Vec<String>> {
            let mut names: Vec<String> = self.known_distros.iter().cloned().collect();
            names.sort();
            Ok(names)
        }
    }

    pub struct RunnerGuard {
        previous: Option<Arc<dyn WslRunner>>,
    }

    impl RunnerGuard {
        pub fn install(runner: Arc<dyn WslRunner>) -> Self {
            Self {
                previous: set_runner_override(Some(runner)),
            }
        }
    }

    impl Drop for RunnerGuard {
        fn drop(&mut self) {
            set_runner_override(self.previous.take());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{
        bash_available, gnu_find_emulation_available, LocalBashRunner, RunnerGuard,
    };
    use super::*;
    use serial_test::serial;

    #[test]
    fn linux_ci_keeps_the_local_bash_wsl_emulator() {
        let available = gnu_find_emulation_available();
        if cfg!(target_os = "linux") {
            assert!(
                available,
                "Linux CI must keep running LocalBashRunner against GNU find"
            );
        }
    }

    #[test]
    fn argv_passes_user_values_as_positional_parameters() {
        let argv = build_wsl_argv(
            &WslRequest::new("Ubuntu-22.04", "cat -- \"$1\"").arg("/home/me/.pi/agent/models.json"),
        )
        .expect("build argv");

        assert_eq!(
            argv,
            vec![
                "-d",
                "Ubuntu-22.04",
                "--",
                "bash",
                "-c",
                "cat -- \"$1\"",
                "cc-switch",
                "/home/me/.pi/agent/models.json",
            ]
        );
    }

    #[test]
    fn only_discovery_pays_for_a_login_shell() {
        let argv = build_wsl_argv(&WslRequest::new("Ubuntu-22.04", "true").login(true))
            .expect("build argv");
        assert_eq!(argv[4], "-lc");
    }

    #[test]
    fn argv_rejects_distro_names_that_could_smuggle_arguments() {
        for distro in [
            "Ubuntu; rm -rf /",
            "Ubuntu 22.04",
            "--exec",
            "",
            "Ubuntu\n-d",
        ] {
            assert!(
                build_wsl_argv(&WslRequest::new(distro, "true")).is_err(),
                "expected '{distro}' to be rejected"
            );
        }
    }

    #[test]
    #[serial]
    fn shell_startup_noise_is_stripped_from_the_payload() {
        if !bash_available() {
            return;
        }
        let runner = tempfile::tempdir().expect("tempdir");
        let _guard = RunnerGuard::install(std::sync::Arc::new(LocalBashRunner::new(
            "Ubuntu-22.04",
            runner.path().to_path_buf(),
        )));

        // Stands in for a `.bashrc` that greets every shell.
        let result = run(&WslRequest::new(
            "Ubuntu-22.04",
            format!(
                "echo 'Welcome to Ubuntu 22.04.3 LTS'\n{}",
                guarded_script("printf '%s' '{\"providers\":{}}'")
            ),
        ))
        .expect("run script");

        assert!(result.stdout_lossy().contains("Welcome to Ubuntu"));
        assert_eq!(result.payload_lossy(), "{\"providers\":{}}");
    }

    #[test]
    fn unguarded_output_is_returned_whole() {
        let result = WslExecResult {
            exit_code: Some(0),
            stdout: b"plain output".to_vec(),
            stderr: String::new(),
        };
        assert_eq!(result.payload_lossy(), "plain output");
    }

    #[test]
    #[serial]
    fn shell_metacharacters_in_arguments_stay_inert() {
        if !bash_available() {
            return;
        }
        let runner = tempfile::tempdir().expect("tempdir");
        let _guard = RunnerGuard::install(std::sync::Arc::new(LocalBashRunner::new(
            "Ubuntu-22.04",
            runner.path().to_path_buf(),
        )));

        // If the value were concatenated into the script, the subshell would
        // run and stdout would contain "pwned".
        let result = run(&WslRequest::new("Ubuntu-22.04", "printf '%s' \"$1\"")
            .arg("$(echo pwned); `echo pwned`; 'quoted'"))
        .expect("run script");

        assert!(result.succeeded());
        assert_eq!(
            result.stdout_lossy(),
            "$(echo pwned); `echo pwned`; 'quoted'"
        );
    }

    #[test]
    #[serial]
    fn unknown_distro_reports_the_wsl_diagnostic() {
        if !bash_available() {
            return;
        }
        let runner = tempfile::tempdir().expect("tempdir");
        let _guard = RunnerGuard::install(std::sync::Arc::new(LocalBashRunner::new(
            "Ubuntu-22.04",
            runner.path().to_path_buf(),
        )));

        let result = run(&WslRequest::new("Debian", "true")).expect("run script");
        assert!(!result.succeeded());
        assert!(result
            .require_success("probe")
            .expect_err("must fail")
            .to_string()
            .contains("no distribution"));
    }

    #[test]
    #[serial]
    fn stdin_payloads_reach_the_script() {
        if !bash_available() {
            return;
        }
        let runner = tempfile::tempdir().expect("tempdir");
        let _guard = RunnerGuard::install(std::sync::Arc::new(LocalBashRunner::new(
            "Ubuntu-22.04",
            runner.path().to_path_buf(),
        )));

        let result = run(&WslRequest::new("Ubuntu-22.04", "cat").stdin(b"payload".to_vec()))
            .expect("run script");

        assert_eq!(result.stdout_lossy(), "payload");
    }

    #[test]
    #[serial]
    fn timeouts_kill_the_child_instead_of_hanging() {
        if !bash_available() {
            return;
        }
        let runner = tempfile::tempdir().expect("tempdir");
        let _guard = RunnerGuard::install(std::sync::Arc::new(LocalBashRunner::new(
            "Ubuntu-22.04",
            runner.path().to_path_buf(),
        )));

        let error =
            run(&WslRequest::new("Ubuntu-22.04", "sleep 30").timeout(Duration::from_millis(300)))
                .expect_err("expected a timeout");

        assert!(error.to_string().contains("timed out"));
    }

    #[test]
    fn utf16le_console_output_is_decoded() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "Ubuntu-22.04\r\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_console_output(&bytes), "Ubuntu-22.04\r\n");

        let without_bom: Vec<u8> = "Ubuntu\r\nDebian\r\n"
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        assert_eq!(decode_console_output(&without_bom), "Ubuntu\r\nDebian\r\n");
    }

    #[test]
    fn utf8_console_output_is_left_alone() {
        assert_eq!(
            decode_console_output("Ubuntu 中文\n".as_bytes()),
            "Ubuntu 中文\n"
        );
        assert_eq!(decode_console_output(b""), "");
    }

    #[test]
    fn distro_listing_drops_blank_lines_and_default_markers() {
        let listing = "Ubuntu-22.04\r\ndocker-desktop\r\n\r\nUbuntu-22.04\r\nDebian (Default)\r\n";
        assert_eq!(
            parse_distro_list(listing),
            vec!["Ubuntu-22.04", "docker-desktop", "Debian"]
        );
    }

    #[test]
    fn linux_paths_must_be_absolute_and_control_free() {
        assert!(is_valid_linux_path("/home/me/.pi/agent"));
        assert!(!is_valid_linux_path("relative/path"));
        assert!(!is_valid_linux_path("/home/me\npwned"));
        assert!(!is_valid_linux_path(""));
    }
}
