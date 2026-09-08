//! Thin adapter for Pi's native files.
//!
//! Pi owns account login and the active provider/model in `settings.json`.
//! CC Switch only manages explicit provider entries in `models.json`.
//!
//! Path policy (Pi Coding Agent):
//! - `piConfigDir` is Pi **home**, default `~/.pi` (same shape as `~/.claude`).
//! - Live files Pi actually reads live one layer down:
//!   `{piConfigDir}/agent/models.json` and `{piConfigDir}/agent/settings.json`.
//! - `PI_CODING_AGENT_DIR` is Pi's own env and already names the **agent**
//!   directory; it is not the home.
//! - A legacy override that already ends in `agent` is used as-is so we do
//!   not write `{home}/agent/agent/models.json`.
//! - Top-level `{piConfigDir}/models.json` is a mirror only. The agent file
//!   is canonical; writes keep both in sync so they cannot diverge.

use crate::config::get_home_dir;
use crate::error::AppError;
use crate::pi_runtime::files::PiFile;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

const MAX_PI_FILE_BYTES: u64 = 1024 * 1024;
const MISSING_MODELS_REVISION: &str = crate::pi_runtime::files::MISSING_REVISION;
static MODELS_FILE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
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

/// Pi home (`~/.pi`). This is what Settings → Pi configuration directory
/// names; live models/settings/sessions are under the `agent/` child.
pub(crate) fn get_pi_config_dir() -> Result<PathBuf, AppError> {
    Ok(pi_home_from_agent_dir(&get_pi_agent_dir()?))
}

/// Directory Pi actually reads: `{piConfigDir}/agent`.
pub(crate) fn get_pi_agent_dir() -> Result<PathBuf, AppError> {
    #[cfg(test)]
    if let Some(path) = TEST_AGENT_DIR
        .lock()
        .expect("lock Pi test directory")
        .clone()
    {
        return require_absolute(path, "Pi settings override");
    }

    if let Some(path) = crate::settings::get_pi_override_dir() {
        let home = require_absolute(path, "Pi settings override")?;
        return Ok(agent_dir_from_pi_home(&home));
    }

    if let Some(value) = std::env::var_os("PI_CODING_AGENT_DIR").filter(|value| !value.is_empty()) {
        return require_absolute(
            crate::settings::resolve_override_path(value.to_string_lossy().as_ref()),
            "PI_CODING_AGENT_DIR",
        );
    }

    Ok(get_home_dir().join(".pi").join("agent"))
}

fn require_absolute(path: PathBuf, source: &str) -> Result<PathBuf, AppError> {
    if !path.is_absolute() {
        return Err(AppError::InvalidInput(format!(
            "{source} must resolve to an absolute directory: {}",
            path.display()
        )));
    }
    Ok(path)
}

/// True when `path` is already the agent layer (`…/agent`), not Pi home.
pub(crate) fn is_pi_agent_leaf(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("agent"))
}

/// `{piHome}/agent`, unless `pi_home` is already that directory (legacy
/// overrides that pointed at the agent layer).
pub(crate) fn agent_dir_from_pi_home(pi_home: &Path) -> PathBuf {
    if is_pi_agent_leaf(pi_home) {
        pi_home.to_path_buf()
    } else {
        pi_home.join("agent")
    }
}

pub(crate) fn pi_home_from_agent_dir(agent_dir: &Path) -> PathBuf {
    if is_pi_agent_leaf(agent_dir) {
        agent_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| agent_dir.to_path_buf())
    } else {
        agent_dir.to_path_buf()
    }
}

/// Top-level `{piHome}/models.json` (or settings.json) mirror. `None` when
/// that path would be the same as the canonical agent file.
pub(crate) fn get_pi_top_level_path(file_name: &str) -> Result<Option<PathBuf>, AppError> {
    let agent_dir = get_pi_agent_dir()?;
    let agent = agent_dir.join(file_name);
    let top = pi_home_from_agent_dir(&agent_dir).join(file_name);
    if top == agent {
        Ok(None)
    } else {
        Ok(Some(top))
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
    let value = match read_json5_value(&path, "Pi settings") {
        Ok(value) => value,
        Err(AppError::Io { .. }) => return Ok(PiNativeDefaults::default()),
        Err(error) => return Err(error),
    };
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

fn lock_models_file() -> Result<MutexGuard<'static, ()>, AppError> {
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

fn read_models_document_with_revision(path: &Path) -> Result<(Value, String), AppError> {
    let read = crate::pi_runtime::files::read(PiFile::Models, MAX_PI_FILE_BYTES)?;
    let Some(bytes) = read.bytes else {
        return Ok((
            Value::Object(Map::new()),
            MISSING_MODELS_REVISION.to_string(),
        ));
    };
    let document = parse_json5_value(path, "Pi models", bytes)?;
    Ok((document, read.revision))
}

fn read_json5_value(path: &Path, label: &str) -> Result<Value, AppError> {
    let file = if label == "Pi settings" {
        PiFile::Settings
    } else {
        PiFile::Models
    };
    let read = crate::pi_runtime::files::read(file, MAX_PI_FILE_BYTES)?;
    let bytes = read
        .bytes
        .ok_or_else(|| AppError::io(path, std::io::Error::from(std::io::ErrorKind::NotFound)))?;
    parse_json5_value(path, label, bytes)
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

fn write_models_document(
    _path: &Path,
    document: &Value,
    expected_revision: &str,
) -> Result<(), AppError> {
    let mut bytes =
        serde_json::to_vec_pretty(document).map_err(|source| AppError::JsonSerialize { source })?;
    bytes.push(b'\n');
    crate::pi_runtime::files::write(PiFile::Models, &bytes, expected_revision)?;
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
    use std::fs;

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
        let error = require_absolute(PathBuf::from("relative/pi-agent"), "PI_CODING_AGENT_DIR")
            .expect_err("relative Pi directory must be rejected");
        assert!(error.to_string().contains("absolute directory"));
    }

    #[test]
    fn pi_home_override_selects_the_agent_layer() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pi_home = temp.path().join(".pi");
        assert_eq!(
            agent_dir_from_pi_home(&pi_home),
            pi_home.join("agent"),
            "canonical live dir is <piConfigDir>/agent"
        );
        assert_eq!(
            agent_dir_from_pi_home(&pi_home.join("agent")),
            pi_home.join("agent"),
            "a legacy override that already ends in agent must not be doubled"
        );
        assert_eq!(pi_home_from_agent_dir(&pi_home.join("agent")), pi_home);
        assert!(is_pi_agent_leaf(&pi_home.join("agent")));
        assert!(!is_pi_agent_leaf(&pi_home));
    }

    #[test]
    #[serial]
    fn writing_models_keeps_the_top_level_mirror_in_sync() {
        let _agent = test_support::TestAgentDir::new();
        insert_pi_provider("cc-switch-sync", &provider()).expect("insert provider");

        let agent_path = get_pi_models_path().expect("agent models path");
        let top_path = get_pi_top_level_path("models.json")
            .expect("top-level path")
            .expect("top-level mirror is distinct");
        let agent_bytes = fs::read(&agent_path).expect("read agent models");
        let top_bytes = fs::read(&top_path).expect("read top-level models");
        assert_eq!(agent_bytes, top_bytes);
        assert!(agent_path.ends_with(Path::new("agent").join("models.json")));
        assert_eq!(
            top_path.file_name().and_then(|name| name.to_str()),
            Some("models.json")
        );
        assert_ne!(agent_path, top_path);
    }

    #[test]
    #[serial]
    fn a_stale_agent_file_is_healed_from_a_newer_top_level_write() {
        let _agent = test_support::TestAgentDir::new();
        let agent_path = get_pi_models_path().expect("agent models path");
        let top_path = get_pi_top_level_path("models.json")
            .expect("top-level path")
            .expect("top-level mirror");
        fs::create_dir_all(agent_path.parent().expect("agent dir")).expect("mkdir agent");
        fs::write(&agent_path, r#"{"providers":{"stale":{}}}"#).expect("stale agent");
        // Ensure the top-level mtime wins even on coarse filesystems.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(
            &top_path,
            r#"{"providers":{"openai":{"name":"yumcode-std"}}}"#,
        )
        .expect("fresh top-level");

        let providers = read_pi_native_providers().expect("heal and read");
        assert!(
            providers.contains_key("openai"),
            "agent file must pick up the live top-level providers Pi was missing"
        );
        assert!(!providers.contains_key("stale"));
        assert_eq!(
            fs::read_to_string(&agent_path).expect("agent after heal"),
            fs::read_to_string(&top_path).expect("top-level after heal")
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
        fs::create_dir_all(path.parent().expect("agent directory"))
            .expect("create agent directory");
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
}
