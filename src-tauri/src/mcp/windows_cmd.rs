//! Windows/WSL translation of MCP `stdio` server commands.
//!
//! Two target shapes are involved, and they must be exact inverses:
//!
//! * **Windows target** — npm-installed CLIs (`npx`, `npm`, `pnpm`, …) are
//!   `.cmd` batch shims, so Claude Code refuses them unless they are wrapped as
//!   `cmd /c <cli> …`. `wrap_command_for_windows` adds that wrapper.
//! * **WSL target** — the config file lives on `\\wsl.localhost\…`, but the
//!   process that reads it runs *inside* the distro, where `cmd` does not
//!   exist. The very same server must therefore be written *unwrapped*.
//!
//! The unified MCP database stores one spec per server and the wrapper is
//! applied at projection time, so a spec that was written for a Windows target
//! can be imported back into the database in its *wrapped* form (the importer
//! stores live entries verbatim). Projecting that to WSL therefore has to
//! actively undo the wrapper — merely skipping the wrap is not enough, and that
//! is exactly how `cmd /c npx …` used to end up in a WSL `~/.claude.json` /
//! `~/.codex/config.toml`, where it can never run.

use serde_json::{Map, Value};
use std::path::Path;

/// Commands that are `.cmd`/`.bat` shims on Windows and therefore need the
/// `cmd /c` wrapper there — and correspondingly need no wrapper in WSL.
#[cfg(windows)]
const WINDOWS_WRAP_COMMANDS: &[&str] = &["npx", "npm", "yarn", "pnpm", "node", "bun", "deno"];

/// `npx.cmd` / `C:\…\npx` / `NPX` all reduce to a comparable command name.
#[cfg(windows)]
fn command_stem(command: &str) -> String {
    Path::new(command)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(command)
        .to_string()
}

/// Windows platform: rewrite `npx args…` as `cmd /c npx args…`.
///
/// Fixes the "Windows requires 'cmd /c' wrapper to execute npx" warning Claude
/// Code reports in `/doctor`.
#[cfg(windows)]
pub(crate) fn wrap_command_for_windows(obj: &mut Map<String, Value>) {
    // Only stdio entries (the default) are affected.
    let server_type = obj
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("stdio");
    if server_type != "stdio" {
        return;
    }

    let Some(command) = obj.get("command").and_then(|value| value.as_str()) else {
        return;
    };

    // Already wrapped — never wrap twice.
    if command.eq_ignore_ascii_case("cmd") || command.eq_ignore_ascii_case("cmd.exe") {
        return;
    }

    let needs_wrap = WINDOWS_WRAP_COMMANDS
        .iter()
        .any(|candidate| command_stem(command).eq_ignore_ascii_case(candidate));

    if !needs_wrap {
        return;
    }

    let original_args = obj
        .get("args")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let mut new_args = vec![Value::String("/c".into()), Value::String(command.into())];
    new_args.extend(original_args);

    obj.insert("command".into(), Value::String("cmd".into()));
    obj.insert("args".into(), Value::Array(new_args));
}

/// Non-Windows platforms have no `cmd /c` shim problem.
#[cfg(not(windows))]
pub(crate) fn wrap_command_for_windows(_obj: &mut Map<String, Value>) {}

/// Inverse of [`wrap_command_for_windows`].
///
/// Only the exact shape the wrapper produces is undone — `command` is
/// `cmd`/`cmd.exe`, the first arg is `/c`, and the next arg is a bare command
/// from the same allow-list. Anything else (a genuine `cmd` invocation, a
/// different interpreter, a user-authored `cmd /c` for a tool we do not manage)
/// is left untouched, so this can never silently change a working server.
///
/// Returns `true` when a wrapper was removed.
#[cfg(windows)]
pub(crate) fn strip_windows_cmd_wrapper(obj: &mut Map<String, Value>) -> bool {
    let server_type = obj
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("stdio");
    if server_type != "stdio" {
        return false;
    }

    let Some(command) = obj.get("command").and_then(|value| value.as_str()) else {
        return false;
    };
    if !command.eq_ignore_ascii_case("cmd") && !command.eq_ignore_ascii_case("cmd.exe") {
        return false;
    }

    let Some(args) = obj.get("args").and_then(|value| value.as_array()).cloned() else {
        return false;
    };
    let Some(flag) = args.first().and_then(|value| value.as_str()) else {
        return false;
    };
    if !flag.eq_ignore_ascii_case("/c") {
        return false;
    }
    let Some(inner) = args.get(1).and_then(|value| value.as_str()) else {
        return false;
    };
    if !WINDOWS_WRAP_COMMANDS
        .iter()
        .any(|candidate| command_stem(inner).eq_ignore_ascii_case(candidate))
    {
        return false;
    }

    obj.insert("command".into(), Value::String(inner.to_string()));
    obj.insert("args".into(), Value::Array(args[2..].to_vec()));
    true
}

#[cfg(not(windows))]
pub(crate) fn strip_windows_cmd_wrapper(_obj: &mut Map<String, Value>) -> bool {
    false
}

/// Detect WSL network paths such as `\\wsl$\Ubuntu\…` or
/// `\\wsl.localhost\Ubuntu\…`.
///
/// WSL runs Linux, so config written there must not carry `cmd /c` wrappers.
/// Only direct UNC paths are detected; a mapped drive letter pointing at
/// `\\wsl$\…` cannot be recognised here.
#[cfg(windows)]
pub(crate) fn is_wsl_path(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    if let Some(Component::Prefix(prefix)) = path.components().next() {
        match prefix.kind() {
            Prefix::UNC(server, _) | Prefix::VerbatimUNC(server, _) => {
                let server = server.to_string_lossy();
                server.eq_ignore_ascii_case("wsl$") || server.eq_ignore_ascii_case("wsl.localhost")
            }
            _ => false,
        }
    } else {
        false
    }
}

#[cfg(not(windows))]
pub(crate) fn is_wsl_path(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(value: Value) -> Map<String, Value> {
        value.as_object().expect("object literal").clone()
    }

    #[test]
    fn wrap_command_wraps_npx() {
        let mut entry = obj(json!({"command": "npx", "args": ["-y", "@upstash/context7-mcp"]}));
        wrap_command_for_windows(&mut entry);

        #[cfg(windows)]
        {
            assert_eq!(entry["command"], "cmd");
            assert_eq!(
                entry["args"],
                json!(["/c", "npx", "-y", "@upstash/context7-mcp"])
            );
        }

        #[cfg(not(windows))]
        assert_eq!(entry["command"], "npx");
    }

    #[test]
    fn wrap_command_wraps_npm() {
        let mut entry = obj(json!({"command": "npm", "args": ["run", "start"]}));
        wrap_command_for_windows(&mut entry);

        #[cfg(windows)]
        {
            assert_eq!(entry["command"], "cmd");
            assert_eq!(entry["args"], json!(["/c", "npm", "run", "start"]));
        }
    }

    #[test]
    fn wrap_command_is_idempotent_on_already_wrapped() {
        let mut entry = obj(json!({"command": "cmd", "args": ["/c", "npx", "-y", "foo"]}));
        wrap_command_for_windows(&mut entry);

        assert_eq!(entry["command"], "cmd");
        assert_eq!(entry["args"], json!(["/c", "npx", "-y", "foo"]));
    }

    #[test]
    fn wrap_command_skips_http_type_and_unknown_commands() {
        let mut http = obj(json!({"type": "http", "url": "https://example.com/mcp"}));
        wrap_command_for_windows(&mut http);
        assert!(!http.contains_key("command"));

        let mut python = obj(json!({"command": "python", "args": ["server.py"]}));
        wrap_command_for_windows(&mut python);
        assert_eq!(python["command"], "python");
        assert_eq!(python["args"], json!(["server.py"]));
    }

    #[test]
    fn wrap_command_handles_missing_args_suffix_and_case() {
        let mut no_args = obj(json!({"command": "npx"}));
        wrap_command_for_windows(&mut no_args);

        #[cfg(windows)]
        assert_eq!(no_args["args"], json!(["/c", "npx"]));

        let mut with_suffix = obj(json!({"command": "npx.cmd", "args": ["-y", "foo"]}));
        wrap_command_for_windows(&mut with_suffix);

        #[cfg(windows)]
        assert_eq!(with_suffix["args"], json!(["/c", "npx.cmd", "-y", "foo"]));

        let mut upper = obj(json!({"command": "NPX", "args": ["-y", "foo"]}));
        wrap_command_for_windows(&mut upper);

        #[cfg(windows)]
        assert_eq!(upper["args"], json!(["/c", "NPX", "-y", "foo"]));
    }

    /// The regression this module exists for: a spec already carrying the
    /// Windows wrapper must come back out unwrapped for a WSL target.
    #[test]
    fn strip_command_undoes_the_wrapper() {
        let mut entry = obj(
            json!({"type": "stdio", "command": "cmd", "args": ["/c", "npx", "-y", "@upstash/context7-mcp"]}),
        );
        let stripped = strip_windows_cmd_wrapper(&mut entry);

        #[cfg(windows)]
        {
            assert!(stripped, "wrapper should be recognised and removed");
            assert_eq!(entry["command"], "npx");
            assert_eq!(entry["args"], json!(["-y", "@upstash/context7-mcp"]));
        }

        #[cfg(not(windows))]
        {
            assert!(!stripped);
            // Non-Windows builds must not rewrite the entry at all.
            assert_eq!(entry["command"], "cmd");
        }
    }

    #[test]
    fn strip_command_round_trips_with_wrap() {
        let original = json!({"type": "stdio", "command": "pnpm", "args": ["dlx", "some-mcp"]});

        #[cfg(windows)]
        {
            let mut entry = obj(original.clone());
            wrap_command_for_windows(&mut entry);
            assert_eq!(entry["command"], "cmd");

            assert!(strip_windows_cmd_wrapper(&mut entry));
            assert_eq!(Value::Object(entry), original);
        }

        #[cfg(not(windows))]
        let _ = original;
    }

    #[test]
    fn strip_command_leaves_unmanaged_and_genuine_cmd_alone() {
        // A command outside the wrapper's allow-list is not ours to unwrap.
        let mut other = obj(json!({"command": "cmd", "args": ["/c", "my-tool.cmd", "--serve"]}));
        assert!(!strip_windows_cmd_wrapper(&mut other));
        assert_eq!(other["command"], "cmd");
        assert_eq!(other["args"], json!(["/c", "my-tool.cmd", "--serve"]));

        // `cmd /k` is not the wrapper shape either.
        let mut keep = obj(json!({"command": "cmd", "args": ["/k", "npx", "foo"]}));
        assert!(!strip_windows_cmd_wrapper(&mut keep));
        assert_eq!(keep["args"], json!(["/k", "npx", "foo"]));

        // http entries never carry a wrapper.
        let mut http = obj(json!({"type": "http", "command": "cmd", "args": ["/c", "npx"]}));
        assert!(!strip_windows_cmd_wrapper(&mut http));

        // A bare `npx` needs no strip and must not gain a `cmd` prefix.
        let mut plain = obj(json!({"command": "npx", "args": ["-y", "foo"]}));
        assert!(!strip_windows_cmd_wrapper(&mut plain));
        assert_eq!(plain["command"], "npx");
    }

    #[test]
    fn is_wsl_path_recognises_both_unc_servers() {
        #[cfg(windows)]
        {
            assert!(is_wsl_path(Path::new(r"\\wsl$\Ubuntu\home\user\.claude")));
            assert!(is_wsl_path(Path::new(
                r"\\wsl.localhost\Ubuntu\home\user\.claude"
            )));
            assert!(is_wsl_path(Path::new(r"\\WSL.LOCALHOST\Ubuntu\home\user")));
            assert!(is_wsl_path(Path::new(
                r"\\wsl.localhost\Ubuntu-22.04\home\ketty\.codex"
            )));
        }

        #[cfg(not(windows))]
        assert!(!is_wsl_path(Path::new(r"\\wsl$\Ubuntu\home\user\.claude")));
    }

    #[test]
    fn is_wsl_path_rejects_other_paths() {
        assert!(!is_wsl_path(Path::new(r"C:\Users\user\.claude")));
        assert!(!is_wsl_path(Path::new(r"D:\Workspace\project")));

        #[cfg(windows)]
        {
            assert!(!is_wsl_path(Path::new(r"\\server\share\path")));
            assert!(!is_wsl_path(Path::new(r"\\localhost\c$\Users")));
            assert!(!is_wsl_path(Path::new(r"\\192.168.1.1\share")));
        }
    }
}
