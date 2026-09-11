//! Thin adapter for Pi's native files.
//!
//! CC Switch manages explicit provider entries in `models.json` and may write
//! global `defaultProvider` / `defaultModel` in `settings.json`. Pi `/login`
//! credentials in `auth.json` stay untouched.

mod proxy;

pub(crate) use proxy::*;

use crate::config::{atomic_write_private, get_home_dir};
use crate::error::AppError;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

const MAX_PI_FILE_BYTES: u64 = 1024 * 1024;
const MISSING_MODELS_REVISION: &str = "missing";
const MISSING_SETTINGS_REVISION: &str = "missing";
static MODELS_FILE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static SETTINGS_FILE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
#[cfg(test)]
static TEST_AGENT_DIR: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiNativeDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_dir: Option<String>,
}

pub(crate) fn get_pi_agent_dir() -> Result<PathBuf, AppError> {
    #[cfg(test)]
    if let Some(path) = TEST_AGENT_DIR
        .lock()
        .expect("lock Pi test directory")
        .clone()
    {
        return resolve_pi_agent_dir(Some(path), None, get_home_dir().join(".pi").join("agent"));
    }

    resolve_pi_agent_dir(
        crate::settings::get_pi_override_dir(),
        std::env::var_os("PI_CODING_AGENT_DIR"),
        default_pi_agent_dir(),
    )
}

/// Local `~/.pi/agent`, or the same WSL distro home Claude/Codex already use.
fn default_pi_agent_dir() -> PathBuf {
    inferred_wsl_pi_agent_dir(
        crate::settings::get_claude_override_dir().as_deref(),
        crate::settings::get_codex_override_dir().as_deref(),
    )
    .unwrap_or_else(|| get_home_dir().join(".pi").join("agent"))
}

/// When `pi_config_dir` is unset, follow Claude/Codex WSL UNC home to
/// `\\wsl.localhost\<distro>\home\<user>\.pi\agent` (or `root\.pi\agent`).
pub(crate) fn inferred_wsl_pi_agent_dir(
    claude_override: Option<&Path>,
    codex_override: Option<&Path>,
) -> Option<PathBuf> {
    for dir in [claude_override, codex_override].into_iter().flatten() {
        if let Some(home) = wsl_unc_home_from_app_config_dir(dir) {
            return Some(home.join(".pi").join("agent"));
        }
    }
    None
}

/// Home of a WSL UNC app dir such as `\\wsl.localhost\Ubuntu\home\user\.claude`.
pub(crate) fn wsl_unc_home_from_app_config_dir(dir: &Path) -> Option<PathBuf> {
    wsl_unc_home_from_app_config_dir_str(&dir.to_string_lossy()).map(PathBuf::from)
}

fn wsl_unc_home_from_app_config_dir_str(raw: &str) -> Option<String> {
    let normalized = raw.replace('/', r"\");
    let lower = normalized.to_ascii_lowercase();
    let prefix_len = if lower.starts_with(r"\\?\unc\") {
        r"\\?\unc\".len()
    } else if lower.starts_with(r"\\") {
        2
    } else {
        return None;
    };
    let stripped = &normalized[prefix_len..];
    let parts: Vec<&str> = stripped
        .split('\\')
        .filter(|part| !part.is_empty())
        .collect();
    let server = parts.first()?;
    if !server.eq_ignore_ascii_case("wsl$") && !server.eq_ignore_ascii_case("wsl.localhost") {
        return None;
    }
    let last = *parts.last()?;
    if !last.starts_with('.') {
        return None;
    }
    let home_parts =
        if parts.len() == 5 && parts[2].eq_ignore_ascii_case("home") && !parts[3].is_empty() {
            &parts[..4]
        } else if parts.len() == 4 && parts[2].eq_ignore_ascii_case("root") {
            &parts[..3]
        } else {
            return None;
        };

    let mut home = String::from(r"\\");
    if lower.starts_with(r"\\?\unc\") {
        home = String::from(r"\\?\UNC\");
    }
    home.push_str(home_parts.join(r"\").as_str());
    Some(home)
}

pub(crate) fn resolve_pi_agent_dir(
    settings_override: Option<PathBuf>,
    env_override: Option<std::ffi::OsString>,
    default_path: PathBuf,
) -> Result<PathBuf, AppError> {
    let (path, source) = match settings_override {
        Some(path) => (path, "Pi settings override"),
        None => match env_override {
            Some(value) if !value.is_empty() => (
                crate::settings::resolve_override_path(value.to_string_lossy().as_ref()),
                "PI_CODING_AGENT_DIR",
            ),
            _ => (default_path, "Pi default"),
        },
    };
    let path = canonicalize_pi_agent_dir(path);
    if !proxy::is_usable_pi_agent_dir(&path) {
        return Err(AppError::InvalidInput(format!(
            "{source} must resolve to an absolute directory: {}",
            path.display()
        )));
    }
    Ok(path)
}

/// If the user points at `~/.pi` (or `\\wsl.localhost\…\.pi`), use the agent dir.
/// Session JSONL and models.json live under `.pi/agent/`, not the Pi root.
fn canonicalize_pi_agent_dir(path: PathBuf) -> PathBuf {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(".pi"))
    {
        return path.join("agent");
    }
    let raw = path.to_string_lossy();
    let normalized = raw.replace('/', r"\");
    if normalized.len() >= 3 && normalized.to_ascii_lowercase().ends_with(r"\.pi") {
        path.join("agent")
    } else {
        path
    }
}

pub(crate) fn get_pi_models_path() -> Result<PathBuf, AppError> {
    Ok(get_pi_agent_dir()?.join("models.json"))
}

pub(crate) fn get_pi_settings_path() -> Result<PathBuf, AppError> {
    Ok(get_pi_agent_dir()?.join("settings.json"))
}

pub(crate) fn read_pi_native_defaults() -> Result<PiNativeDefaults, AppError> {
    let path = get_pi_settings_path()?;
    if !path.exists() {
        return Ok(PiNativeDefaults::default());
    }
    let value = read_json5_value(&path, "Pi settings")?;
    let object = value.as_object().ok_or_else(|| {
        AppError::Config(format!(
            "Pi settings root must be an object: {}",
            path.display()
        ))
    })?;
    Ok(PiNativeDefaults {
        default_provider: optional_string(object, "defaultProvider", &path)?,
        default_model: optional_string(object, "defaultModel", &path)?,
        session_dir: optional_string(object, "sessionDir", &path)?,
    })
}

/// First explicit model id on a `models.json` provider node.
pub(crate) fn first_model_id(config: &Value) -> Option<String> {
    config
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| {
            models
                .iter()
                .find_map(|model| nonempty_string(model.get("id")).map(str::to_string))
        })
}

/// Write Pi global default provider/model, preserving unknown settings fields.
pub(crate) fn write_pi_native_defaults(
    default_provider: Option<&str>,
    default_model: Option<&str>,
) -> Result<(), AppError> {
    let _guard = lock_settings_file()?;
    let path = get_pi_settings_path()?;
    let (mut document, expected_revision) = read_settings_document_with_revision(&path)?;
    let root = document.as_object_mut().ok_or_else(|| {
        AppError::Config(format!(
            "Pi settings root must be an object: {}",
            path.display()
        ))
    })?;

    match default_provider.filter(|value| !value.is_empty()) {
        Some(provider) => {
            root.insert(
                "defaultProvider".to_string(),
                Value::String(provider.to_string()),
            );
        }
        None => {
            root.remove("defaultProvider");
        }
    }
    match default_model.filter(|value| !value.is_empty()) {
        Some(model) => {
            root.insert("defaultModel".to_string(), Value::String(model.to_string()));
        }
        None => {
            root.remove("defaultModel");
        }
    }

    write_settings_document(&path, &document, &expected_revision)
}

/// Point Pi's global default at an enabled live provider node.
pub(crate) fn set_pi_default_provider(provider_key: &str) -> Result<(), AppError> {
    let config = read_pi_native_provider(provider_key)?.ok_or_else(|| {
        AppError::InvalidInput(format!(
            "Pi provider '{provider_key}' is not enabled in models.json"
        ))
    })?;
    write_pi_native_defaults(Some(provider_key), first_model_id(&config).as_deref())
}

/// After deleting/removing the current default, pick another live node or clear.
pub(crate) fn reassign_pi_default_if_current(removed_id: &str) -> Result<(), AppError> {
    let defaults = match read_pi_native_defaults() {
        Ok(defaults) => defaults,
        Err(error) => {
            log::warn!(
                "Failed to read Pi settings while reassigning default after '{removed_id}': {error}"
            );
            return Ok(());
        }
    };
    if defaults.default_provider.as_deref() != Some(removed_id) {
        return Ok(());
    }

    let remaining = read_pi_native_providers()?;
    if let Some((next_id, config)) = remaining.first() {
        write_pi_native_defaults(Some(next_id.as_str()), first_model_id(config).as_deref())
    } else {
        write_pi_native_defaults(None, None)
    }
}

fn lock_settings_file() -> Result<MutexGuard<'static, ()>, AppError> {
    SETTINGS_FILE_LOCK
        .lock()
        .map_err(|error| AppError::Config(format!("Pi settings file lock is poisoned: {error}")))
}

fn read_settings_document_with_revision(path: &Path) -> Result<(Value, String), AppError> {
    if !path.exists() {
        return Ok((
            Value::Object(Map::new()),
            MISSING_SETTINGS_REVISION.to_string(),
        ));
    }
    let bytes = read_file_limited(path, "Pi settings")?;
    let revision = revision(&bytes);
    let document = parse_json5_value(path, "Pi settings", bytes)?;
    if !document.is_object() {
        return Err(AppError::Config(format!(
            "Pi settings root must be an object: {}",
            path.display()
        )));
    }
    Ok((document, revision))
}

fn write_settings_document(
    path: &Path,
    document: &Value,
    expected_revision: &str,
) -> Result<(), AppError> {
    let mut bytes =
        serde_json::to_vec_pretty(document).map_err(|source| AppError::JsonSerialize { source })?;
    bytes.push(b'\n');
    ensure_private_models_parent(path)?;
    ensure_settings_revision(path, expected_revision)?;
    atomic_write_private(path, &bytes)
}

fn ensure_settings_revision(path: &Path, expected_revision: &str) -> Result<(), AppError> {
    let actual_revision = match fs::File::open(path) {
        Ok(_) => revision(&read_file_limited(path, "Pi settings")?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            MISSING_SETTINGS_REVISION.to_string()
        }
        Err(error) => return Err(AppError::io(path, error)),
    };
    if actual_revision == expected_revision {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "Pi settings.json changed outside CC Switch: {}",
            path.display()
        )))
    }
}

pub(crate) fn read_pi_native_providers() -> Result<IndexMap<String, Value>, AppError> {
    let _guard = lock_models_file()?;
    read_pi_native_providers_locked(&get_pi_models_path()?)
}

pub(crate) fn read_pi_native_provider(provider_key: &str) -> Result<Option<Value>, AppError> {
    let _guard = lock_models_file()?;
    let path = get_pi_models_path()?;
    let document = read_models_document(&path)?;
    Ok(providers(&document, &path)?.get(provider_key).cloned())
}

pub(crate) fn pi_provider_exists(provider_key: &str) -> Result<bool, AppError> {
    let _guard = lock_models_file()?;
    let path = get_pi_models_path()?;
    let document = read_models_document(&path)?;
    Ok(providers(&document, &path)?.contains_key(provider_key))
}

pub(crate) fn insert_pi_provider(provider_key: &str, config: &Value) -> Result<bool, AppError> {
    validate_provider_node(provider_key, config)?;
    let _guard = lock_models_file()?;
    let path = get_pi_models_path()?;
    let (mut document, expected_revision) = read_models_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;

    match providers.get(provider_key) {
        Some(current) if current == config => return Ok(false),
        Some(_) => {
            return Err(AppError::InvalidInput(format!(
                "Pi provider key '{provider_key}' already exists in models.json"
            )))
        }
        None => {}
    }

    providers.insert(provider_key.to_string(), config.clone());
    write_models_document(&path, &document, &expected_revision)?;
    Ok(true)
}

pub(crate) fn replace_pi_provider(
    provider_key: &str,
    expected: &Value,
    replacement: &Value,
) -> Result<(), AppError> {
    validate_provider_node(provider_key, replacement)?;
    let _guard = lock_models_file()?;
    let path = get_pi_models_path()?;
    let (mut document, expected_revision) = read_models_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;
    let current = providers.get(provider_key).ok_or_else(|| {
        AppError::Conflict(format!(
            "Pi provider '{provider_key}' is no longer present in models.json"
        ))
    })?;
    if current != expected {
        return Err(AppError::Conflict(format!(
            "Pi provider '{provider_key}' changed outside CC Switch"
        )));
    }
    if current == replacement {
        return Ok(());
    }
    providers.insert(provider_key.to_string(), replacement.clone());
    write_models_document(&path, &document, &expected_revision)
}

pub(crate) fn replace_pi_provider_if_present(
    provider_key: &str,
    replacement: &Value,
) -> Result<Option<Value>, AppError> {
    validate_provider_node(provider_key, replacement)?;
    let _guard = lock_models_file()?;
    let path = get_pi_models_path()?;
    let (mut document, expected_revision) = read_models_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;
    let Some(current) = providers.get(provider_key).cloned() else {
        return Ok(None);
    };
    if current == *replacement {
        return Ok(Some(current));
    }
    providers.insert(provider_key.to_string(), replacement.clone());
    write_models_document(&path, &document, &expected_revision)?;
    Ok(Some(current))
}

pub(crate) fn remove_pi_provider(provider_key: &str) -> Result<Option<Value>, AppError> {
    remove_pi_provider_inner(provider_key, None)
}

pub(crate) fn remove_pi_provider_if_matches(
    provider_key: &str,
    expected: &Value,
) -> Result<bool, AppError> {
    remove_pi_provider_inner(provider_key, Some(expected)).map(|removed| removed.is_some())
}

fn remove_pi_provider_inner(
    provider_key: &str,
    expected: Option<&Value>,
) -> Result<Option<Value>, AppError> {
    let _guard = lock_models_file()?;
    let path = get_pi_models_path()?;
    let (mut document, expected_revision) = read_models_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;
    let Some(current) = providers.get(provider_key).cloned() else {
        return Ok(None);
    };
    if expected.is_some_and(|expected| current != *expected) {
        return Err(AppError::Conflict(format!(
            "Pi provider '{provider_key}' changed outside CC Switch"
        )));
    }
    providers.remove(provider_key);
    write_models_document(&path, &document, &expected_revision)?;
    Ok(Some(current))
}

pub(crate) fn restore_pi_provider_if_missing(
    provider_key: &str,
    config: &Value,
) -> Result<(), AppError> {
    let _guard = lock_models_file()?;
    let path = get_pi_models_path()?;
    let (mut document, expected_revision) = read_models_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;
    match providers.get(provider_key) {
        Some(current) if current == config => Ok(()),
        Some(_) => Err(AppError::Conflict(format!(
            "cannot restore Pi provider '{provider_key}' because another value now owns the key"
        ))),
        None => {
            providers.insert(provider_key.to_string(), config.clone());
            write_models_document(&path, &document, &expected_revision)
        }
    }
}

/// Validate the shape CC Switch can persist as one
/// `models.json.providers.<provider_key>` node.
///
/// Provider ownership is intentionally source-based: every explicit object in
/// `models.json.providers` is manageable, including keys also built into Pi.
/// Pi's `/login` credentials live in `auth.json` and are never read here.
pub(crate) fn validate_provider_node(provider_key: &str, config: &Value) -> Result<(), AppError> {
    if provider_key.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "Pi provider key cannot be empty".to_string(),
        ));
    }
    config.as_object().ok_or_else(|| {
        AppError::InvalidInput("Pi provider configuration must be an object".to_string())
    })?;
    Ok(())
}

pub(crate) fn provider_base_url(config: &Value) -> Result<String, AppError> {
    let provider = config.as_object().ok_or_else(|| {
        AppError::InvalidInput("Pi provider configuration must be an object".to_string())
    })?;
    nonempty_string(provider.get("baseUrl"))
        .or_else(|| {
            provider
                .get("models")
                .and_then(Value::as_array)
                .and_then(|models| {
                    models
                        .iter()
                        .find_map(|model| nonempty_string(model.get("baseUrl")))
                })
        })
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidInput("Pi provider has no request URL".to_string()))
}

pub(crate) fn lock_models_file() -> Result<MutexGuard<'static, ()>, AppError> {
    MODELS_FILE_LOCK
        .lock()
        .map_err(|error| AppError::Config(format!("Pi models file lock is poisoned: {error}")))
}

fn read_pi_native_providers_locked(path: &Path) -> Result<IndexMap<String, Value>, AppError> {
    let document = read_models_document(path)?;
    let providers = providers(&document, path)?;
    Ok(providers
        .iter()
        .map(|(provider_key, config)| (provider_key.clone(), config.clone()))
        .collect())
}

fn read_models_document(path: &Path) -> Result<Value, AppError> {
    read_models_document_with_revision(path).map(|(document, _)| document)
}

pub(crate) fn read_models_document_with_revision(path: &Path) -> Result<(Value, String), AppError> {
    if !path.exists() {
        return Ok((
            Value::Object(Map::new()),
            MISSING_MODELS_REVISION.to_string(),
        ));
    }
    let bytes = read_file_limited(path, "Pi models")?;
    let revision = revision(&bytes);
    let document = parse_json5_value(path, "Pi models", bytes)?;
    Ok((document, revision))
}

fn read_json5_value(path: &Path, label: &str) -> Result<Value, AppError> {
    parse_json5_value(path, label, read_file_limited(path, label)?)
}

fn read_file_limited(path: &Path, label: &str) -> Result<Vec<u8>, AppError> {
    let file = fs::File::open(path).map_err(|error| AppError::io(path, error))?;
    let metadata = file.metadata().map_err(|error| AppError::io(path, error))?;
    if metadata.len() > MAX_PI_FILE_BYTES {
        return Err(AppError::InvalidInput(format!(
            "{label} file exceeds the 1 MiB limit: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_PI_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io(path, error))?;
    if bytes.len() as u64 > MAX_PI_FILE_BYTES {
        return Err(AppError::InvalidInput(format!(
            "{label} file exceeds the 1 MiB limit: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

fn parse_json5_value(path: &Path, label: &str, bytes: Vec<u8>) -> Result<Value, AppError> {
    let source = String::from_utf8(bytes).map_err(|error| {
        AppError::Config(format!(
            "{label} file must be UTF-8 ({}): {error}",
            path.display()
        ))
    })?;
    json5::from_str(&source).map_err(|error| {
        AppError::Config(format!(
            "{label} file is not valid JSON/JSONC ({}): {error}",
            path.display()
        ))
    })
}

fn providers<'a>(document: &'a Value, path: &Path) -> Result<&'a Map<String, Value>, AppError> {
    let root = document.as_object().ok_or_else(|| {
        AppError::Config(format!(
            "Pi models root must be an object: {}",
            path.display()
        ))
    })?;
    match root.get("providers") {
        None => Ok(empty_json_object()),
        Some(Value::Object(providers)) => Ok(providers),
        Some(_) => Err(AppError::Config(format!(
            "Pi models 'providers' must be an object: {}",
            path.display()
        ))),
    }
}

fn providers_mut<'a>(
    document: &'a mut Value,
    path: &Path,
) -> Result<&'a mut Map<String, Value>, AppError> {
    let root = document.as_object_mut().ok_or_else(|| {
        AppError::Config(format!(
            "Pi models root must be an object: {}",
            path.display()
        ))
    })?;
    let value = root
        .entry("providers".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    value.as_object_mut().ok_or_else(|| {
        AppError::Config(format!(
            "Pi models 'providers' must be an object: {}",
            path.display()
        ))
    })
}

fn empty_json_object() -> &'static Map<String, Value> {
    static EMPTY: LazyLock<Map<String, Value>> = LazyLock::new(Map::new);
    &EMPTY
}

pub(crate) fn write_models_document(
    path: &Path,
    document: &Value,
    expected_revision: &str,
) -> Result<(), AppError> {
    let mut bytes =
        serde_json::to_vec_pretty(document).map_err(|source| AppError::JsonSerialize { source })?;
    bytes.push(b'\n');
    ensure_private_models_parent(path)?;
    ensure_models_revision(path, expected_revision)?;
    atomic_write_private(path, &bytes)?;
    maybe_mirror_legacy_models_json(path, &bytes);
    Ok(())
}

/// Mirror the agent file to `~/.pi/models.json` only when that legacy file already exists.
fn maybe_mirror_legacy_models_json(agent_models_path: &Path, bytes: &[u8]) {
    let Some(legacy) = legacy_pi_root_models_path(agent_models_path) else {
        return;
    };
    if !legacy.exists() || path_eq_ignore_separators(agent_models_path, &legacy) {
        return;
    }
    if let Err(error) = atomic_write_private(&legacy, bytes) {
        log::warn!(
            "Failed to mirror Pi models.json to legacy {}: {error}",
            legacy.display()
        );
    }
}

fn legacy_pi_root_models_path(agent_models_path: &Path) -> Option<PathBuf> {
    legacy_pi_root_models_path_str(&agent_models_path.to_string_lossy()).map(PathBuf::from)
}

fn legacy_pi_root_models_path_str(raw: &str) -> Option<String> {
    let normalized = raw.replace('\\', "/");
    const SUFFIX: &str = "/agent/models.json";
    let trimmed = normalized.trim_end_matches('/');
    if trimmed.len() <= SUFFIX.len() || !trimmed.to_ascii_lowercase().ends_with(SUFFIX) {
        return None;
    }
    // Mirror next to the agent parent (`~/.pi/models.json`), including older
    // layouts that used a non-`.pi` directory name.
    let root = &trimmed[..trimmed.len() - SUFFIX.len()];
    let slash = if raw.contains('\\') && !raw.contains('/') {
        "\\"
    } else {
        "/"
    };
    let mut legacy = root.to_string();
    legacy.push_str(slash);
    legacy.push_str("models.json");
    if slash == "\\" {
        Some(legacy.replace('/', r"\"))
    } else {
        Some(legacy)
    }
}

fn path_eq_ignore_separators(left: &Path, right: &Path) -> bool {
    left.to_string_lossy().replace('\\', "/") == right.to_string_lossy().replace('\\', "/")
}

fn ensure_models_revision(path: &Path, expected_revision: &str) -> Result<(), AppError> {
    let actual_revision = match fs::File::open(path) {
        Ok(_) => revision(&read_file_limited(path, "Pi models")?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            MISSING_MODELS_REVISION.to_string()
        }
        Err(error) => return Err(AppError::io(path, error)),
    };
    if actual_revision == expected_revision {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "Pi models.json changed outside CC Switch: {}",
            path.display()
        )))
    }
}

fn revision(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn ensure_private_models_parent(path: &Path) -> Result<(), AppError> {
    let parent = path.parent().ok_or_else(|| {
        AppError::Config(format!(
            "Pi models path has no parent directory: {}",
            path.display()
        ))
    })?;
    let created = !parent.exists();
    fs::create_dir_all(parent).map_err(|source| AppError::io(parent, source))?;

    #[cfg(not(unix))]
    let _ = created;

    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|source| AppError::io(parent, source))?;
    }

    Ok(())
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    path: &Path,
) -> Result<Option<String>, AppError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(AppError::Config(format!(
            "Pi settings '{key}' must be a string: {}",
            path.display()
        ))),
    }
}

fn nonempty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    pub(crate) struct TestAgentDir {
        _dir: Option<tempfile::TempDir>,
        previous: Option<PathBuf>,
    }

    impl TestAgentDir {
        pub(crate) fn new() -> Self {
            let dir = tempfile::tempdir().expect("create Pi test directory");
            let agent_dir = dir.path().join("agent");
            Self::set(agent_dir, Some(dir))
        }

        pub(crate) fn at(agent_dir: &Path) -> Self {
            Self::set(agent_dir.to_path_buf(), None)
        }

        fn set(agent_dir: PathBuf, dir: Option<tempfile::TempDir>) -> Self {
            let previous = super::TEST_AGENT_DIR
                .lock()
                .expect("lock Pi test directory")
                .replace(agent_dir);
            Self {
                _dir: dir,
                previous,
            }
        }
    }

    impl Drop for TestAgentDir {
        fn drop(&mut self) {
            *super::TEST_AGENT_DIR
                .lock()
                .expect("lock Pi test directory") = self.previous.take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serial_test::serial;

    fn provider() -> Value {
        json!({
            "name": "Example",
            "baseUrl": "https://api.example.com/v1",
            "api": "openai-completions",
            "apiKey": "secret",
            "models": [{"id": "example-model"}]
        })
    }

    #[test]
    fn provider_node_accepts_unknown_native_fields() {
        let mut value = provider();
        value["sdkOption"] = json!({"timeout": 30});
        value["models"][0]["compat"] = json!({"supportsDeveloperRole": true});
        validate_provider_node("cc-switch-example", &value).expect("valid provider");
    }

    #[test]
    fn provider_node_ownership_depends_on_models_json_membership() {
        let mut oauth = provider();
        oauth["oauth"] = json!("anthropic");
        validate_provider_node("cc-switch-example", &oauth)
            .expect("an explicit models.json node stays manageable");
        validate_provider_node("anthropic", &json!({}))
            .expect("a built-in provider key may be explicitly configured");
        assert!(validate_provider_node("", &json!({})).is_err());
        assert!(validate_provider_node("anthropic", &json!("invalid")).is_err());
    }

    #[test]
    fn relative_agent_directory_is_rejected() {
        let error = resolve_pi_agent_dir(
            None,
            Some("relative/pi-agent".into()),
            PathBuf::from("default"),
        )
        .expect_err("relative Pi directory must be rejected");
        assert!(error.to_string().contains("absolute directory"));
    }

    #[test]
    #[serial]
    fn models_json_lives_under_agent_not_pi_root() {
        let _agent = test_support::TestAgentDir::new();
        let path = get_pi_models_path().expect("models path");
        let normalized = path.to_string_lossy().replace('\\', "/");
        assert!(
            normalized.ends_with("/agent/models.json"),
            "takeover must write ~/.pi/agent/models.json, not ~/.pi/models.json: {normalized}"
        );
        assert!(
            !normalized.ends_with("/.pi/models.json"),
            "must not write the Pi-root models.json: {normalized}"
        );
    }

    #[test]
    fn canonicalize_dot_pi_to_agent_dir() {
        assert_eq!(
            canonicalize_pi_agent_dir(PathBuf::from("/home/user/.pi")),
            PathBuf::from("/home/user/.pi/agent")
        );
        assert_eq!(
            canonicalize_pi_agent_dir(PathBuf::from("/home/user/.pi/agent")),
            PathBuf::from("/home/user/.pi/agent")
        );
        let unc_pi = PathBuf::from(r"\\wsl.localhost\Ubuntu-22.04\home\user\.pi");
        let canonical = canonicalize_pi_agent_dir(unc_pi)
            .to_string_lossy()
            .replace('/', r"\");
        assert!(
            canonical.ends_with(r"\.pi\agent"),
            "UNC .pi must resolve under agent: {canonical}"
        );
        let unc_agent = PathBuf::from(r"\\wsl.localhost\Ubuntu-22.04\home\user\.pi\agent");
        assert_eq!(canonicalize_pi_agent_dir(unc_agent.clone()), unc_agent);

        let user_pi = PathBuf::from(r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi");
        let user_canonical = canonicalize_pi_agent_dir(user_pi)
            .to_string_lossy()
            .replace('/', r"\");
        assert!(
            user_canonical.ends_with(r"home\tfdx8045\.pi\agent"),
            "user WSL .pi override must land on .pi/agent: {user_canonical}"
        );
    }

    #[test]
    fn settings_directory_precedes_the_environment() {
        let temp = tempfile::tempdir().expect("tempdir");
        let settings_dir = temp.path().join("settings-agent");
        let env_dir = temp.path().join("env-agent");

        assert_eq!(
            resolve_pi_agent_dir(
                Some(settings_dir.clone()),
                Some(env_dir.into_os_string()),
                temp.path().join("default-agent"),
            )
            .expect("resolve Pi directory"),
            settings_dir
        );
    }

    #[test]
    #[serial]
    fn duplicate_provider_key_is_validation_not_a_write_conflict() {
        let _agent = test_support::TestAgentDir::new();
        insert_pi_provider("duplicate", &provider()).expect("insert provider");
        let mut replacement = provider();
        replacement["name"] = json!("Other");

        let error = insert_pi_provider("duplicate", &replacement)
            .expect_err("duplicate provider key must be rejected");
        assert!(matches!(error, AppError::InvalidInput(_)));
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn newly_created_models_file_and_agent_directory_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let _agent = test_support::TestAgentDir::new();
        insert_pi_provider("cc-switch-private", &provider()).expect("write private models file");

        let path = get_pi_models_path().expect("models path");
        let file_mode = fs::metadata(&path)
            .expect("models metadata")
            .permissions()
            .mode()
            & 0o777;
        let directory_mode = fs::metadata(path.parent().expect("agent directory"))
            .expect("agent directory metadata")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(file_mode, 0o600);
        assert_eq!(directory_mode, 0o700);
    }

    #[test]
    #[serial]
    fn stale_models_revision_does_not_overwrite_an_external_edit() {
        let _agent = test_support::TestAgentDir::new();
        let path = get_pi_models_path().expect("models path");
        ensure_private_models_parent(&path).expect("create agent directory");
        fs::write(&path, r#"{"providers":{"external":{"models":[]}}}"#)
            .expect("write initial models");
        let (_, stale_revision) =
            read_models_document_with_revision(&path).expect("read models revision");

        let external = r#"{"providers":{"external":{"models":[]},"pi-added":{"models":[]}}}"#;
        fs::write(&path, external).expect("edit models externally");

        let replacement = json!({"providers": {"cc-switch": provider()}});
        let error = write_models_document(&path, &replacement, &stale_revision)
            .expect_err("stale write must fail");
        assert!(matches!(error, AppError::Conflict(_)));
        assert_eq!(
            fs::read_to_string(path).expect("read external models"),
            external
        );
    }

    #[test]
    fn wsl_unc_home_follows_claude_or_codex_home_style_dirs() {
        let claude = PathBuf::from(r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude");
        let inferred = inferred_wsl_pi_agent_dir(Some(&claude), None).expect("infer from Claude");
        let normalized = inferred.to_string_lossy().replace('/', r"\");
        assert!(
            normalized.ends_with(r"home\tfdx8045\.pi\agent"),
            "Claude WSL home must map to the same distro .pi/agent: {normalized}"
        );

        let codex = PathBuf::from(r"\\wsl$\Debian\root\.codex");
        let from_codex = inferred_wsl_pi_agent_dir(None, Some(&codex)).expect("infer from Codex");
        let codex_normalized = from_codex.to_string_lossy().replace('/', r"\");
        assert!(
            codex_normalized.ends_with(r"root\.pi\agent"),
            "Codex WSL root home must map to .pi/agent: {codex_normalized}"
        );

        let custom = PathBuf::from(r"\\wsl.localhost\Ubuntu\opt\claude\.claude");
        assert!(
            inferred_wsl_pi_agent_dir(Some(&custom), None).is_none(),
            "custom WSL dirs are not a UNC home"
        );
        assert!(wsl_unc_home_from_app_config_dir(Path::new("/home/user/.claude")).is_none());
    }

    #[test]
    fn pi_config_dir_still_wins_over_inferred_wsl_home() {
        let temp = tempfile::tempdir().expect("tempdir");
        let override_dir = temp.path().join("explicit-agent");
        let inferred = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\user\.pi\agent");
        assert_eq!(
            resolve_pi_agent_dir(Some(override_dir.clone()), None, inferred).expect("resolve"),
            override_dir
        );
    }

    #[test]
    #[serial]
    fn write_defaults_preserves_unknown_settings_fields() {
        let _agent = test_support::TestAgentDir::new();
        let path = get_pi_settings_path().expect("settings path");
        fs::create_dir_all(path.parent().expect("agent dir")).expect("create agent");
        fs::write(
            &path,
            r#"{"sessionDir":"/tmp/sessions","theme":"dark","defaultProvider":"old"}"#,
        )
        .expect("seed settings");

        write_pi_native_defaults(Some("cc-switch-test"), Some("model-a")).expect("write defaults");

        let written: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read settings"))
                .expect("parse settings");
        assert_eq!(written["defaultProvider"], json!("cc-switch-test"));
        assert_eq!(written["defaultModel"], json!("model-a"));
        assert_eq!(written["sessionDir"], json!("/tmp/sessions"));
        assert_eq!(written["theme"], json!("dark"));
    }

    #[test]
    #[serial]
    fn removing_the_default_reassigns_or_clears() {
        let _agent = test_support::TestAgentDir::new();
        insert_pi_provider("keep-me", &provider()).expect("insert remaining");
        insert_pi_provider("gone", &provider()).expect("insert default");
        write_pi_native_defaults(Some("gone"), Some("model-a")).expect("set default");

        remove_pi_provider("gone").expect("remove default provider");
        reassign_pi_default_if_current("gone").expect("reassign");

        let defaults = read_pi_native_defaults().expect("read defaults");
        assert_eq!(defaults.default_provider.as_deref(), Some("keep-me"));
        assert_eq!(defaults.default_model.as_deref(), Some("example-model"));

        remove_pi_provider("keep-me").expect("remove last");
        reassign_pi_default_if_current("keep-me").expect("clear");
        let cleared = read_pi_native_defaults().expect("read cleared");
        assert_eq!(cleared.default_provider, None);
        assert_eq!(cleared.default_model, None);
    }

    #[test]
    #[serial]
    fn legacy_root_models_json_is_mirrored_only_when_it_already_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pi_root = dir.path().join(".pi");
        let agent_dir = pi_root.join("agent");
        fs::create_dir_all(&agent_dir).expect("create agent");
        let _agent = test_support::TestAgentDir::at(&agent_dir);

        insert_pi_provider("only-agent", &provider()).expect("write agent file");
        assert!(!pi_root.join("models.json").exists());

        fs::write(pi_root.join("models.json"), "{}\n").expect("create legacy");
        insert_pi_provider("mirrored", &provider()).expect("write again");

        let legacy: Value = serde_json::from_str(
            &fs::read_to_string(pi_root.join("models.json")).expect("read legacy"),
        )
        .expect("parse legacy");
        assert!(legacy["providers"].get("only-agent").is_some());
        assert!(legacy["providers"].get("mirrored").is_some());
        let agent: Value = serde_json::from_str(
            &fs::read_to_string(agent_dir.join("models.json")).expect("read agent"),
        )
        .expect("parse agent");
        assert_eq!(legacy, agent);
    }

    #[test]
    fn first_model_id_reads_the_first_nonempty_models_entry() {
        assert_eq!(
            first_model_id(&json!({"models": [{"id": "a"}, {"id": "b"}]})).as_deref(),
            Some("a")
        );
        assert_eq!(first_model_id(&json!({"models": []})), None);
        assert_eq!(first_model_id(&json!({})), None);
    }
}
