use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use crate::error::AppError;

/// 获取用户主目录，带回退和日志
///
/// ## Windows 注意事项
///
/// - `dirs::home_dir()` 在 Windows 上使用 `SHGetKnownFolderPath(FOLDERID_Profile)`，
///   返回的是真实用户目录（类似 `C:\\Users\\Alice`），与 v3.10.2 行为一致。
/// - 不要直接使用 `HOME` 环境变量：它可能由 Git/Cygwin/MSYS 等第三方工具注入，
///   且不一定等于用户目录，可能导致 `.cc-switch/cc-switch.db` 路径变化，从而“看起来像数据丢失”。
///
/// ## 测试隔离
///
/// 为了让 Windows CI/本地测试能稳定隔离真实用户数据，可通过 `CC_SWITCH_TEST_HOME`
/// 显式覆盖 home dir（仅用于测试/调试场景）。
pub fn get_home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("CC_SWITCH_TEST_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    dirs::home_dir().unwrap_or_else(|| {
        log::warn!("无法获取用户主目录，回退到当前目录");
        PathBuf::from(".")
    })
}

/// 获取 Claude Code 配置目录路径
pub fn get_claude_config_dir() -> PathBuf {
    if let Some(custom) = crate::settings::get_claude_override_dir() {
        return custom;
    }

    get_home_dir().join(".claude")
}

/// 默认 Claude MCP 配置文件路径 (~/.claude.json)
pub fn get_default_claude_mcp_path() -> PathBuf {
    get_home_dir().join(".claude.json")
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }

    normalized
}

fn comparable_path_key(path: &Path) -> String {
    let mut key = normalize_path_lexically(path).to_string_lossy().to_string();

    #[cfg(windows)]
    {
        key = key.replace('\\', "/");
    }

    while key.len() > 1 && key.ends_with('/') {
        key.pop();
    }

    #[cfg(windows)]
    {
        key.make_ascii_lowercase();
    }

    key
}

fn path_eq_lexical(left: &Path, right: &Path) -> bool {
    comparable_path_key(left) == comparable_path_key(right)
}

/// Returns true when `path` is lexically contained within `base`.
///
/// Both paths are normalized lexically (without hitting the filesystem), so
/// this works for non-existent paths. It is **not** a symlink defense: a
/// symlink inside `base` can still lead a resolved path outside it. Callers
/// that go on to open the file must canonicalize the existing path and
/// re-verify containment (see `resolve_cc_switch_catalog_path`).
/// On Windows the comparison is case-insensitive.
pub(crate) fn path_is_within(base: &Path, path: &Path) -> bool {
    let base_key = comparable_path_key(base);
    let path_key = comparable_path_key(path);

    if path_key == base_key {
        return true;
    }

    let prefix = format!("{base_key}/");
    path_key.starts_with(&prefix)
}

#[cfg(windows)]
fn derive_wsl_default_mcp_path(dir: &Path) -> Option<PathBuf> {
    use std::path::Prefix;

    let normalized = normalize_path_lexically(dir);
    let mut components = normalized.components();
    let prefix = match components.next()? {
        Component::Prefix(prefix) => prefix,
        _ => return None,
    };

    let server = match prefix.kind() {
        Prefix::UNC(server, _) | Prefix::VerbatimUNC(server, _) => server.to_string_lossy(),
        _ => return None,
    };

    if !server.eq_ignore_ascii_case("wsl$") && !server.eq_ignore_ascii_case("wsl.localhost") {
        return None;
    }

    let mut parts = Vec::new();
    for component in components {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            Component::ParentDir | Component::Prefix(_) => return None,
        }
    }

    let is_wsl_home_default =
        parts.len() == 3 && parts[0] == "home" && !parts[1].is_empty() && parts[2] == ".claude";
    let is_wsl_root_default = parts.len() == 2 && parts[0] == "root" && parts[1] == ".claude";

    if is_wsl_home_default || is_wsl_root_default {
        return normalized
            .parent()
            .map(|parent| parent.join(".claude.json"));
    }

    None
}

/// Derive the WSL-side home directory from a WSL UNC path inside a user's
/// home: `\\wsl$\<distro>\home\<user>\...` -> `\\wsl$\<distro>\home\<user>`,
/// and `\\wsl.localhost\<distro>\root\...` -> `\\wsl.localhost\<distro>\root`.
/// Returns None for non-WSL paths and for WSL paths outside a home directory
/// (e.g. `\\wsl$\<distro>\etc`), where no home can be derived safely.
#[cfg(windows)]
pub(crate) fn derive_wsl_home_dir(dir: &Path) -> Option<PathBuf> {
    use std::path::Prefix;

    let normalized = normalize_path_lexically(dir);
    let mut components = normalized.components();
    let prefix = match components.next()? {
        Component::Prefix(prefix) => prefix,
        _ => return None,
    };

    let server = match prefix.kind() {
        Prefix::UNC(server, _) | Prefix::VerbatimUNC(server, _) => server.to_string_lossy(),
        _ => return None,
    };

    if !server.eq_ignore_ascii_case("wsl$") && !server.eq_ignore_ascii_case("wsl.localhost") {
        return None;
    }

    let mut parts = Vec::new();
    for component in components {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            Component::ParentDir | Component::Prefix(_) => return None,
        }
    }

    let home_len = match parts.as_slice() {
        [home, user, ..] if home == "home" && !user.is_empty() => 2,
        [root, ..] if root == "root" => 1,
        _ => return None,
    };

    // Rebuild prefix + root + the first `home_len` components.
    let mut home_dir = PathBuf::new();
    let mut normal_seen = 0usize;
    for component in normalized.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => home_dir.push(component.as_os_str()),
            Component::Normal(_) if normal_seen < home_len => {
                home_dir.push(component.as_os_str());
                normal_seen += 1;
            }
            _ => break,
        }
    }
    Some(home_dir)
}

fn default_mcp_path_for_config_dir(dir: &Path) -> Option<PathBuf> {
    let default_config_dir = get_home_dir().join(".claude");
    if path_eq_lexical(dir, &default_config_dir) {
        return Some(get_default_claude_mcp_path());
    }

    #[cfg(windows)]
    {
        if let Some(path) = derive_wsl_default_mcp_path(dir) {
            return Some(path);
        }
    }

    None
}

fn derive_mcp_path_from_override(dir: &Path) -> PathBuf {
    dir.join(".claude.json")
}

/// 获取 Claude MCP 配置文件路径
pub fn get_claude_mcp_path() -> PathBuf {
    if let Some(custom_dir) = crate::settings::get_claude_override_dir() {
        if let Some(path) = default_mcp_path_for_config_dir(&custom_dir) {
            return path;
        }
        return derive_mcp_path_from_override(&custom_dir);
    }
    get_default_claude_mcp_path()
}

/// 获取 Claude Code 主配置文件路径
pub fn get_claude_settings_path() -> PathBuf {
    let dir = get_claude_config_dir();
    let settings = dir.join("settings.json");
    if settings.exists() {
        return settings;
    }
    // 兼容旧版命名：若存在旧文件则继续使用
    let legacy = dir.join("claude.json");
    if legacy.exists() {
        return legacy;
    }
    // 默认新建：回落到标准文件名 settings.json（不再生成 claude.json）
    settings
}

/// 获取应用配置目录路径 (~/.cc-switch)
pub fn get_app_config_dir() -> PathBuf {
    if let Some(custom) = crate::app_store::get_app_config_dir_override() {
        return custom;
    }

    let default_dir = get_home_dir().join(".cc-switch");

    // 兼容 v3.10.3：当用户环境存在 `HOME` 且与真实用户目录不同，
    // v3.10.3 可能在 `HOME/.cc-switch/` 下创建/使用了数据库。
    // 这里仅在“默认位置没有数据库”时回退到旧位置，避免再次出现“供应商消失”问题，
    // 同时也避免新安装因为 `HOME` 被设置而写入非预期路径。
    #[cfg(windows)]
    {
        let default_db = default_dir.join("cc-switch.db");
        if !default_db.exists() {
            if let Ok(home_env) = std::env::var("HOME") {
                let trimmed = home_env.trim();
                if !trimmed.is_empty() {
                    let legacy_dir = PathBuf::from(trimmed).join(".cc-switch");
                    if legacy_dir.join("cc-switch.db").exists() {
                        log::info!(
                            "Detected v3.10.3 legacy database at {}, using it instead of {}",
                            legacy_dir.display(),
                            default_dir.display()
                        );
                        return legacy_dir;
                    }
                }
            }
        }
    }

    default_dir
}

/// 获取应用配置文件路径
pub fn get_app_config_path() -> PathBuf {
    get_app_config_dir().join("config.json")
}

/// 清理供应商名称，确保文件名安全
#[allow(dead_code)]
pub fn sanitize_provider_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

/// 获取供应商配置文件路径
#[allow(dead_code)]
pub fn get_provider_config_path(provider_id: &str, provider_name: Option<&str>) -> PathBuf {
    let base_name = provider_name
        .map(sanitize_provider_name)
        .unwrap_or_else(|| sanitize_provider_name(provider_id));

    get_claude_config_dir().join(format!("settings-{base_name}.json"))
}

/// 读取 JSON 配置文件
pub fn read_json_file<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T, AppError> {
    if !path.exists() {
        return Err(AppError::Config(format!("文件不存在: {}", path.display())));
    }

    let content = fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;

    serde_json::from_str(&content).map_err(|e| AppError::json(path, e))
}

/// 递归排序 JSON 对象的键（按字母顺序），确保序列化输出是确定性的
fn sort_json_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted_map = Map::new();
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            for key in keys {
                sorted_map.insert(key.clone(), sort_json_keys(&map[key]));
            }
            Value::Object(sorted_map)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_json_keys).collect()),
        other => other.clone(),
    }
}

/// 写入 JSON 配置文件并返回实际写入的字节。
pub fn write_json_file_with_contents<T: Serialize>(
    path: &Path,
    data: &T,
) -> Result<Vec<u8>, AppError> {
    // 确保目录存在
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let contents = sorted_json_bytes(data)?;
    atomic_write(path, &contents)?;
    Ok(contents)
}

/// 排序并序列化 JSON（键按字母排序，确保确定性输出）。
fn sorted_json_bytes<T: Serialize>(data: &T) -> Result<Vec<u8>, AppError> {
    let value = serde_json::to_value(data).map_err(|e| AppError::JsonSerialize { source: e })?;
    let sorted_value = sort_json_keys(&value);
    let json = serde_json::to_string_pretty(&sorted_value)
        .map_err(|e| AppError::JsonSerialize { source: e })?;
    Ok(json.into_bytes())
}

/// 写入 JSON 配置文件（键按字母排序，确保确定性输出）
pub fn write_json_file<T: Serialize>(path: &Path, data: &T) -> Result<(), AppError> {
    write_json_file_with_contents(path, data).map(|_| ())
}

/// 同 [`write_json_file`]，用于含凭据的 live 文件（Codex `auth.json`、Claude Code
/// `settings.json`）：Unix 下新文件和替换文件都是 0600。普通写入新建文件时按 umask
/// 落成 0644，Key 就对同机其他用户可读。
pub fn write_json_file_private<T: Serialize>(path: &Path, data: &T) -> Result<(), AppError> {
    atomic_write_private(path, &sorted_json_bytes(data)?)
}

/// 原子写入文本文件（用于 TOML/纯文本）
pub fn write_text_file(path: &Path, data: &str) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }
    atomic_write(path, data.as_bytes())
}

/// 同 [`write_text_file`]，用于含凭据的 live 文件（Codex / Grok Build 的
/// `config.toml`，第三方 Key 就写在里面）：Unix 下 0600。
pub fn write_text_file_private(path: &Path, data: &str) -> Result<(), AppError> {
    atomic_write_private(path, data.as_bytes())
}

/// 原子写入：写入临时文件后 rename 替换，避免半写状态
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), AppError> {
    atomic_write_with_unix_mode(path, data, None)
}

/// 原子写入包含凭据的文件。Unix 上新文件和替换文件始终使用 0600。
pub fn atomic_write_private(path: &Path, data: &[u8]) -> Result<(), AppError> {
    atomic_write_with_unix_mode(path, data, Some(0o600))
}

/// 校验 WSL 发行版名是否可安全地作为 `wsl.exe -d` 实参。
///
/// 与 `commands::misc::is_valid_wsl_distro_name` 同口径；这里单独留一份是为了
/// 让底层写入器不依赖 commands 层。
#[cfg(windows)]
fn wsl_distro_name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// 从 WSL UNC 路径取出「发行版名 + Linux 绝对路径」。
///
/// `\\wsl.localhost\Ubuntu-22.04\home\ketty\.codex\auth.json`
/// 得到 `("Ubuntu-22.04", "/home/ketty/.codex/auth.json")`。
///
/// UNC 的 share 段就是发行版名（`wsl_unc_path_to_linux` 只取后半段，所以那里
/// 用不了）。非 WSL 路径、发行版名非法、或含 `.` / `..` 的路径一律返回 `None`——
/// 宁可漏补权限，也不对解析不清的路径执行 `chmod`。
#[cfg(windows)]
fn wsl_unc_target(path: &Path) -> Option<(String, String)> {
    use std::path::Prefix;

    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else {
        return None;
    };
    let distro = match prefix.kind() {
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            let server_name = server.to_string_lossy();
            if !(server_name.eq_ignore_ascii_case("wsl$")
                || server_name.eq_ignore_ascii_case("wsl.localhost"))
            {
                return None;
            }
            share.to_string_lossy().to_string()
        }
        _ => return None,
    };
    if !wsl_distro_name_is_safe(&distro) {
        return None;
    }

    let mut linux = String::new();
    for component in components {
        match component {
            Component::RootDir => {}
            Component::Normal(part) => {
                linux.push('/');
                linux.push_str(&part.to_string_lossy());
            }
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    (!linux.is_empty()).then_some((distro, linux))
}

/// Windows 主机写 WSL 文件后，跨 `wsl.exe` 在 Linux 侧补一次权限位。
///
/// 为什么必须补：本函数的 `set_permissions` 在 `#[cfg(unix)]` 分支里，Windows
/// 构建根本没有这段代码；而 `ReplaceFileW` 也不搬运 POSIX 权限位。于是
/// `~/.codex/auth.json`（明文 Key 就在里面）、`~/.claude/settings.json` 这类
/// 从 Windows 写进 distro 的 live 文件会按 distro 的 umask 落成 0644，同机其他
/// 用户可读。Windows 侧没有 API 能设置 WSL 文件的 POSIX 位，只能借 Linux 侧
/// 的 `chmod`。
///
/// 失败只记警告、不返回错误：文件已经写好了，为补权限把一次成功的配置写入判成
/// 失败会让调用方回滚或报错，代价更大；而且个别 distro 可能没有可用的 chmod。
#[cfg(windows)]
fn apply_mode_through_wsl(path: &Path, mode: u32) {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    /// CREATE_NO_WINDOW：避免每次写入凭据都闪一个控制台窗口。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let Some((distro, linux_path)) = wsl_unc_target(path) else {
        return;
    };

    let octal = format!("{mode:o}");
    let result = Command::new("wsl.exe")
        .arg("-d")
        .arg(&distro)
        .arg("--")
        .arg("chmod")
        .arg(&octal)
        .arg(&linux_path)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();

    match result {
        Ok(output) if output.status.success() => {}
        Ok(output) => log::warn!(
            "[WSL:{distro}] chmod {octal} {linux_path} 失败，凭据文件权限未收紧：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(err) => log::warn!("[WSL:{distro}] 无法执行 chmod 收紧 {linux_path} 权限：{err}"),
    }
}

fn atomic_write_with_unix_mode(
    path: &Path,
    data: &[u8],
    unix_mode: Option<u32>,
) -> Result<(), AppError> {
    #[cfg(not(unix))]
    let _ = unix_mode;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("无效的路径".to_string()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Config("无效的文件名".to_string()))?
        .to_string_lossy()
        .to_string();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (tmp, mut file) = (|| -> Result<(PathBuf, fs::File), AppError> {
        let mut last_collision = None;
        for _ in 0..16 {
            let counter = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let candidate = parent.join(format!(
                "{file_name}.tmp.{}.{ts}.{counter}",
                std::process::id()
            ));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            if let Some(mode) = unix_mode {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(mode);
            }
            match options.open(&candidate) {
                Ok(file) => return Ok((candidate, file)),
                Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_collision = Some((candidate, source));
                }
                Err(source) => return Err(AppError::io(&candidate, source)),
            }
        }

        let (candidate, source) = last_collision.expect("temporary filename loop must run");
        Err(AppError::io(&candidate, source))
    })()?;

    if let Err(source) = file.write_all(data).and_then(|_| file.flush()) {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return Err(AppError::io(&tmp, source));
    }
    drop(file);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(mode) = unix_mode {
            if let Err(source) = fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)) {
                let _ = fs::remove_file(&tmp);
                return Err(AppError::io(&tmp, source));
            }
        } else if let Ok(meta) = fs::metadata(path) {
            let perm = meta.permissions().mode();
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(perm));
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{
            Foundation::ERROR_NOT_SUPPORTED, Storage::FileSystem::ReplaceFileW,
        };

        let replaced: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let replacement: Vec<u16> = tmp
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut completed = false;
        let mut last_error = None;

        for _ in 0..3 {
            // SAFETY: both path buffers are NUL-terminated UTF-16 and remain alive for the
            // duration of the call. Backup, exclusion, and reserved pointers are intentionally null.
            let replaced_ok = unsafe {
                ReplaceFileW(
                    replaced.as_ptr(),
                    replacement.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            if replaced_ok != 0 {
                completed = true;
                break;
            }

            let replace_error = std::io::Error::last_os_error();
            // WSL UNC paths reject ReplaceFileW with ERROR_NOT_SUPPORTED (50).
            // std::fs::rename uses a different replace-existing API on Windows.
            let replace_not_supported =
                replace_error.raw_os_error() == Some(ERROR_NOT_SUPPORTED as i32);
            if replace_error.kind() != std::io::ErrorKind::NotFound && !replace_not_supported {
                last_error = Some(replace_error);
                break;
            }

            match fs::rename(&tmp, path) {
                Ok(()) => {
                    completed = true;
                    break;
                }
                Err(source)
                    if matches!(
                        source.kind(),
                        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                    ) =>
                {
                    last_error = Some(source);
                }
                Err(source) => {
                    last_error = Some(source);
                    break;
                }
            }
        }

        if !completed {
            let source = last_error.unwrap_or_else(std::io::Error::last_os_error);
            let _ = fs::remove_file(&tmp);
            return Err(AppError::IoContext {
                context: format!("原子替换失败: {} -> {}", tmp.display(), path.display()),
                source,
            });
        }
    }

    #[cfg(not(windows))]
    {
        if let Err(source) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(AppError::IoContext {
                context: format!("原子替换失败: {} -> {}", tmp.display(), path.display()),
                source,
            });
        }
    }

    // WSL 目标：Windows 侧设不了 POSIX 位，跨 wsl.exe 补一次（见函数文档）。
    #[cfg(windows)]
    if let Some(mode) = unix_mode {
        apply_mode_through_wsl(path, mode);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_atomic_write_replaces_existing_file(dir: &Path) {
        let path = dir.join("atomic-write-contract.json");
        std::fs::write(&path, b"old contents").unwrap();

        atomic_write(&path, b"new contents").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new contents");
        let tmp_prefix = "atomic-write-contract.json.tmp.";
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(tmp_prefix))
            .map(|entry| entry.path())
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files remain: {leftovers:?}"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_atomic_write_replaces_existing_file(dir.path());
    }

    /// 含凭据的 live 文件必须与普通写入产出完全相同的字节；差别只在权限位。
    /// 这里在 Windows 上也能跑，权限断言另由 `#[cfg(unix)]` 覆盖。
    #[test]
    fn private_writers_match_plain_writers_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let data = serde_json::json!({ "z": 1, "env": { "ANTHROPIC_AUTH_TOKEN": "secret" } });

        let plain_json = dir.path().join("plain.json");
        let private_json = dir.path().join("private.json");
        write_json_file(&plain_json, &data).unwrap();
        write_json_file_private(&private_json, &data).unwrap();
        assert_eq!(
            std::fs::read(&plain_json).unwrap(),
            std::fs::read(&private_json).unwrap()
        );

        let plain_toml = dir.path().join("plain.toml");
        let private_toml = dir.path().join("private.toml");
        write_text_file(&plain_toml, "api_key = \"secret\"\n").unwrap();
        write_text_file_private(&private_toml, "api_key = \"secret\"\n").unwrap();
        assert_eq!(
            std::fs::read(&plain_toml).unwrap(),
            std::fs::read(&private_toml).unwrap()
        );

        // 覆盖已有文件时同样走替换路径，不能因为私有写入而改变内容。
        write_text_file_private(&private_toml, "api_key = \"rotated\"\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&private_toml).unwrap(),
            "api_key = \"rotated\"\n"
        );
    }

    /// 持有凭据的 live 文件不得按 umask 落成 0644。
    #[cfg(unix)]
    #[test]
    fn private_writers_create_owner_only_files() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let json_path = dir.path().join("settings.json");
        let text_path = dir.path().join("config.toml");

        write_json_file_private(&json_path, &serde_json::json!({ "k": "v" })).unwrap();
        write_text_file_private(&text_path, "api_key = \"v\"\n").unwrap();

        for path in [&json_path, &text_path] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{} must be owner-only, got {mode:o}", path.display());
        }

        // 替换已有文件后权限位必须保持。
        write_json_file_private(&json_path, &serde_json::json!({ "k": "v2" })).unwrap();
        let mode = std::fs::metadata(&json_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "replaced file must stay owner-only");
    }

    #[cfg(windows)]
    #[test]
    fn atomic_write_preserves_destination_when_windows_replace_fails() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, b"old contents").unwrap();
        let held_file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();

        let result = atomic_write(&path, b"new contents");

        assert!(result.is_err());
        drop(held_file);
        assert_eq!(std::fs::read(&path).unwrap(), b"old contents");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires CC_SWITCH_WSL_TEST_DIR to point to a WSL2 UNC directory"]
    fn atomic_write_replaces_existing_wsl_unc_file() {
        let root = PathBuf::from(
            std::env::var_os("CC_SWITCH_WSL_TEST_DIR").expect("CC_SWITCH_WSL_TEST_DIR must be set"),
        );
        let home = get_home_dir();
        let temp = std::env::temp_dir();
        for (name, path) in [
            ("test root", root.as_path()),
            ("test home", home.as_path()),
            ("temporary directory", temp.as_path()),
        ] {
            let unc = path.to_string_lossy();
            assert!(
                unc.starts_with(r"\\wsl.localhost\") || unc.starts_with(r"\\wsl$\"),
                "expected {name} to be a WSL UNC path, got {unc}"
            );
            assert!(
                path.starts_with(&root),
                "expected {name} to be under {}, got {unc}",
                root.display()
            );
        }

        let dir = tempfile::Builder::new()
            .prefix("atomic-write-contract-")
            .tempdir_in(&root)
            .unwrap();
        assert_atomic_write_replaces_existing_file(dir.path());
    }

    /// UNC → (发行版, Linux 路径) 的解析。chmod 的目标就靠它，
    /// 所以非 WSL、发行版名可疑、含 `.`/`..` 的路径必须一律拒绝。
    #[cfg(windows)]
    #[test]
    fn wsl_unc_target_extracts_distro_and_linux_path() {
        let cases = [
            (
                r"\\wsl.localhost\Ubuntu-22.04\home\ketty\.codex\auth.json",
                Some(("Ubuntu-22.04", "/home/ketty/.codex/auth.json")),
            ),
            (
                r"\\wsl$\Ubuntu\root\.codex\config.toml",
                Some(("Ubuntu", "/root/.codex/config.toml")),
            ),
            (
                r"\\?\UNC\wsl.localhost\Ubuntu\home\u\f.json",
                Some(("Ubuntu", "/home/u/f.json")),
            ),
            // 非 WSL 主机：普通 UNC 共享不能当 distro 处理
            (r"\\fileserver\share\auth.json", None),
            // 普通盘符路径
            (r"C:\Users\me\.codex\auth.json", None),
            // 只有发行版、没有文件
            (r"\\wsl.localhost\Ubuntu", None),
            // 路径穿越必须拒绝：不能让 chmod 落到解析不清的目标上
            (r"\\wsl.localhost\Ubuntu\home\u\..\..\etc\shadow", None),
            // `.` 不是穿越：`Path::components` 已经把它折叠掉，剩下的就是同一目录
            (
                r"\\wsl.localhost\Ubuntu\home\u\.\f.json",
                Some(("Ubuntu", "/home/u/f.json")),
            ),
        ];

        for (input, expected) in cases {
            let actual = wsl_unc_target(Path::new(input));
            let actual = actual.as_ref().map(|(d, p)| (d.as_str(), p.as_str()));
            assert_eq!(actual, expected, "input: {input}");
        }
    }

    /// 真机契约：从 Windows 往 WSL 写凭据文件后，Linux 侧的权限位必须是 0600。
    ///
    /// 这条测试针对的正是 Windows 构建的盲区——`set_permissions` 在
    /// `#[cfg(unix)]` 里，`ReplaceFileW` 也不搬 POSIX 位，所以只能靠跨
    /// `wsl.exe` 的 chmod。断言读的是 Linux 侧真实 `stat`，不是 Windows 的视图。
    #[cfg(windows)]
    #[test]
    #[ignore = "requires CC_SWITCH_WSL_TEST_DIR to point to a WSL2 UNC directory"]
    fn private_write_into_wsl_yields_owner_only_file() {
        use std::process::Command;

        let root = PathBuf::from(
            std::env::var_os("CC_SWITCH_WSL_TEST_DIR").expect("CC_SWITCH_WSL_TEST_DIR must be set"),
        );
        let dir = tempfile::Builder::new()
            .prefix("wsl-private-mode-")
            .tempdir_in(&root)
            .unwrap();
        let path = dir.path().join("auth.json");
        write_json_file_private(&path, &serde_json::json!({ "OPENAI_API_KEY": "sk-test" }))
            .unwrap();

        let (distro, linux_path) =
            wsl_unc_target(&path).expect("temp file under a WSL UNC dir must parse");
        let output = Command::new("wsl.exe")
            .args(["-d", &distro, "--", "stat", "-c", "%a", &linux_path])
            .output()
            .expect("wsl.exe must be runnable");
        assert!(
            output.status.success(),
            "stat failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mode = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(mode, "600", "WSL 侧凭据文件必须是 owner-only: {linux_path}");
    }

    #[test]
    fn derive_mcp_path_from_override_uses_config_dir_for_custom_path() {
        let override_dir = PathBuf::from("/tmp/profile/.claude");
        let derived = derive_mcp_path_from_override(&override_dir);
        assert_eq!(derived, PathBuf::from("/tmp/profile/.claude/.claude.json"));
    }

    #[test]
    fn derive_mcp_path_from_override_uses_config_dir_for_non_hidden_folder() {
        let override_dir = PathBuf::from("/data/claude-config");
        let derived = derive_mcp_path_from_override(&override_dir);
        assert_eq!(derived, PathBuf::from("/data/claude-config/.claude.json"));
    }

    #[test]
    fn derive_mcp_path_from_override_supports_relative_rootless_dir() {
        let override_dir = PathBuf::from("claude");
        let derived = derive_mcp_path_from_override(&override_dir);
        assert_eq!(derived, PathBuf::from("claude/.claude.json"));
    }

    #[test]
    fn derive_mcp_path_from_root_like_dir_uses_root_file() {
        let override_dir = PathBuf::from("/");
        let derived = derive_mcp_path_from_override(&override_dir);
        assert_eq!(derived, PathBuf::from("/.claude.json"));
    }

    #[test]
    fn derive_mcp_path_from_override_preserves_leading_parent_dirs() {
        let override_dir = PathBuf::from("../../profiles/work/.claude");
        let derived = derive_mcp_path_from_override(&override_dir);
        assert_eq!(derived, override_dir.join(".claude.json"));
    }

    #[cfg(windows)]
    #[test]
    fn wsl_unc_home_default_uses_split_mcp_path() {
        let override_dir = PathBuf::from(r"\\wsl$\Ubuntu\home\travis\.claude");
        let derived = default_mcp_path_for_config_dir(&override_dir)
            .expect("WSL home default should use split MCP path");
        assert_eq!(
            derived,
            PathBuf::from(r"\\wsl$\Ubuntu\home\travis\.claude.json")
        );
    }

    #[cfg(windows)]
    #[test]
    fn wsl_unc_root_default_uses_split_mcp_path() {
        let override_dir = PathBuf::from(r"\\wsl.localhost\Ubuntu\root\.claude");
        let derived = default_mcp_path_for_config_dir(&override_dir)
            .expect("WSL root default should use split MCP path");
        assert_eq!(
            derived,
            PathBuf::from(r"\\wsl.localhost\Ubuntu\root\.claude.json")
        );
    }

    #[cfg(windows)]
    #[test]
    fn derive_wsl_home_dir_from_opencode_config_dir() {
        let dir = PathBuf::from(r"\\wsl.localhost\Ubuntu-26.04\home\travis\.config\opencode");
        let home = derive_wsl_home_dir(&dir).expect("WSL home should be derived");
        assert_eq!(
            home,
            PathBuf::from(r"\\wsl.localhost\Ubuntu-26.04\home\travis")
        );
    }

    #[cfg(windows)]
    #[test]
    fn derive_wsl_home_dir_supports_wsl_dollar_and_root() {
        let dir = PathBuf::from(r"\\wsl$\Ubuntu\root\.config\opencode");
        let home = derive_wsl_home_dir(&dir).expect("WSL root home should be derived");
        assert_eq!(home, PathBuf::from(r"\\wsl$\Ubuntu\root"));
    }

    #[cfg(windows)]
    #[test]
    fn derive_wsl_home_dir_rejects_non_home_and_non_wsl_paths() {
        assert_eq!(
            derive_wsl_home_dir(&PathBuf::from(r"\\wsl$\Ubuntu\etc\opencode")),
            None
        );
        assert_eq!(
            derive_wsl_home_dir(&PathBuf::from(r"C:\Users\travis\.config\opencode")),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn wsl_unc_custom_dir_uses_nested_mcp_path() {
        let override_dir = PathBuf::from(r"\\wsl$\Ubuntu\opt\claude\.claude");
        assert!(default_mcp_path_for_config_dir(&override_dir).is_none());
        assert_eq!(
            derive_mcp_path_from_override(&override_dir),
            PathBuf::from(r"\\wsl$\Ubuntu\opt\claude\.claude\.claude.json")
        );
    }

    #[test]
    fn sort_json_keys_sorts_top_level_object() {
        let input = serde_json::json!({
            "z": 1,
            "a": 2,
            "m": 3,
        });
        let sorted = sort_json_keys(&input);
        let serialized = serde_json::to_string(&sorted).unwrap();
        assert_eq!(serialized, r#"{"a":2,"m":3,"z":1}"#);
    }

    #[test]
    fn sort_json_keys_recurses_into_nested_objects() {
        let input = serde_json::json!({
            "outer_b": {"z": 1, "a": 2},
            "outer_a": {"y": 3, "b": 4},
        });
        let sorted = sort_json_keys(&input);
        let serialized = serde_json::to_string(&sorted).unwrap();
        assert_eq!(
            serialized,
            r#"{"outer_a":{"b":4,"y":3},"outer_b":{"a":2,"z":1}}"#
        );
    }

    #[test]
    fn sort_json_keys_preserves_array_order() {
        let input = serde_json::json!([3, 1, 2]);
        let sorted = sort_json_keys(&input);
        let serialized = serde_json::to_string(&sorted).unwrap();
        assert_eq!(serialized, "[3,1,2]");
    }

    #[test]
    fn sort_json_keys_sorts_objects_inside_arrays_but_keeps_array_order() {
        let input = serde_json::json!([
            {"z": 1, "a": 2},
            {"y": 3, "b": 4},
        ]);
        let sorted = sort_json_keys(&input);
        let serialized = serde_json::to_string(&sorted).unwrap();
        assert_eq!(serialized, r#"[{"a":2,"z":1},{"b":4,"y":3}]"#);
    }

    #[test]
    fn sort_json_keys_passes_through_primitives() {
        let cases = vec![
            serde_json::json!("hello"),
            serde_json::json!(42),
            serde_json::json!(3.5),
            serde_json::json!(true),
            serde_json::json!(null),
        ];
        for value in cases {
            let sorted = sort_json_keys(&value);
            assert_eq!(sorted, value);
        }
    }

    #[test]
    fn sort_json_keys_handles_empty_collections() {
        let empty_obj = serde_json::json!({});
        assert_eq!(
            serde_json::to_string(&sort_json_keys(&empty_obj)).unwrap(),
            "{}"
        );

        let empty_arr = serde_json::json!([]);
        assert_eq!(
            serde_json::to_string(&sort_json_keys(&empty_arr)).unwrap(),
            "[]"
        );
    }

    #[test]
    fn sort_json_keys_produces_identical_output_for_different_insertion_orders() {
        // 核心保证：同一逻辑配置无论键的插入顺序如何，写出的字节序列必须一致。
        let mut a = Map::new();
        a.insert("env".to_string(), serde_json::json!({"PATH": "/usr/bin"}));
        a.insert("model".to_string(), serde_json::json!("claude-sonnet-4-5"));
        a.insert("permissions".to_string(), serde_json::json!({"allow": []}));

        let mut b = Map::new();
        b.insert("permissions".to_string(), serde_json::json!({"allow": []}));
        b.insert("model".to_string(), serde_json::json!("claude-sonnet-4-5"));
        b.insert("env".to_string(), serde_json::json!({"PATH": "/usr/bin"}));

        let sorted_a = sort_json_keys(&Value::Object(a));
        let sorted_b = sort_json_keys(&Value::Object(b));

        assert_eq!(
            serde_json::to_string(&sorted_a).unwrap(),
            serde_json::to_string(&sorted_b).unwrap(),
        );
    }
}

/// 复制文件
pub fn copy_file(from: &Path, to: &Path) -> Result<(), AppError> {
    fs::copy(from, to).map_err(|e| AppError::IoContext {
        context: format!("复制文件失败 ({} -> {})", from.display(), to.display()),
        source: e,
    })?;
    Ok(())
}

/// 删除文件
pub fn delete_file(path: &Path) -> Result<(), AppError> {
    if path.exists() {
        fs::remove_file(path).map_err(|e| AppError::io(path, e))?;
    }
    Ok(())
}

/// 检查 Claude Code 配置状态
#[derive(Serialize, Deserialize)]
pub struct ConfigStatus {
    pub exists: bool,
    pub path: String,
}

/// 获取 Claude Code 配置状态
pub fn get_claude_config_status() -> ConfigStatus {
    let path = get_claude_settings_path();
    ConfigStatus {
        exists: path.exists(),
        path: path.to_string_lossy().to_string(),
    }
}
