//! DSH（DeepSeek Harness）配置适配层。
//!
//! DSH 的配置是一棵**按层叠加**的树，后者覆盖前者：
//!
//! ```text
//! 空根
//!  └ dsh.profile.bundles 中各组合包的 patch
//!    └ profile 自身的 cordis.patch.yml       ($DSH_HOME/profiles/<name>/cordis.patch.yml)
//!      └ home 级 patch                        ($DSH_HOME/cordis.patch.yml)  ← 本模块写入这里
//!        └ --patch 命令行覆盖层
//! ```
//!
//! 本模块只负责 home 级 patch（`$DSH_HOME/cordis.patch.yml`），因为它的优先级最高，
//! 且不必改动 profile 目录。
//!
//! # 关键约束：providers 是整体替换
//!
//! 实测（2026-10，隔离沙箱）：profile 层写 `user-real`、home 层写 `ccs-new`，
//! 合并结果**只剩 `ccs-new`** —— `providers` 遵循**整体替换**语义，不是深合并。
//!
//! 因此写入时必须「读全量 → 内存合并 → 整体写回」，绝不能只写本应用管理的那一项，
//! 否则会把用户原有的供应商全部弄丢。
//!
//! # 保真要求
//!
//! patch 是一个 YAML 数组，里面可能放着 CC Switch 完全不认识的条目与字段。
//! 读写必须保持它们原样，故这里一律用 `serde_yaml::Value` 做无类型往返，
//! 不做结构体反序列化。

use serde_yaml::Value;
use std::path::{Path, PathBuf};

use crate::error::AppError;

/// patch 文件里代表模型供应商列表的条目 id。
pub(crate) const ID_LLM_PI_AI: &str = "llm-pi-ai";
/// patch 文件里代表默认模型选择的条目 id。
pub(crate) const ID_AGENT_DEFAULT_MODEL: &str = "agent-default-model";

const NAME_LLM_PI_AI: &str = "@deepseek-ai/dsh-llm-pi-ai";
const NAME_AGENT_DEFAULT_MODEL: &str = "@deepseek-ai/dsh-agent-default-model";

// ============================================================================
// 路径
// ============================================================================

/// `$DSH_HOME/cordis.patch.yml` 的文件名。
pub(crate) const HOME_PATCH_FILE_NAME: &str = "cordis.patch.yml";

/// 由给定的 DSH home 目录推出 home 级 patch 路径。
pub(crate) fn home_patch_path(dsh_home: &Path) -> PathBuf {
    dsh_home.join(HOME_PATCH_FILE_NAME)
}

/// DSH home 目录的环境变量名；未设置时回落到 `~/.dsh`。
pub(crate) const DSH_HOME_ENV: &str = "DSH_HOME";
/// 默认 home 目录名。
pub(crate) const DSH_HOME_DIR_NAME: &str = ".dsh";

/// 解析 DSH home 目录。
///
/// 优先级（与其它应用保持一致的直觉）：
/// 1. 设置里用户显式配置的目录（`dshConfigDir`，可指向 WSL 侧）；
/// 2. 环境变量 `DSH_HOME`（与 DSH 自身语义一致）；
/// 3. `~/.dsh`。
pub(crate) fn resolve_dsh_home() -> Result<PathBuf, AppError> {
    if let Some(path) = crate::settings::get_dsh_override_dir() {
        return Ok(path);
    }
    match std::env::var(DSH_HOME_ENV) {
        Ok(value) if !value.trim().is_empty() => Ok(PathBuf::from(value.trim())),
        _ => {
            let home = crate::config::get_home_dir();
            Ok(home.join(DSH_HOME_DIR_NAME))
        }
    }
}

/// 默认的 home 级 patch 完整路径。
pub(crate) fn default_home_patch_path() -> Result<PathBuf, AppError> {
    Ok(home_patch_path(&resolve_dsh_home()?))
}

// ============================================================================
// 读取
// ============================================================================

/// 读取 patch 文件。文件不存在时返回空数组（等价于「无覆盖」）。
///
/// 空文件同样视为空数组 —— DSH 侧对空文件的处理未定义，这里取最保守的解释。
pub(crate) fn read_patch(path: &Path) -> Result<Vec<Value>, AppError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| AppError::Config(format!("读取 DSH 配置失败 {}: {e}", path.display())))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    parse_patch(&text)
}

/// 解析 patch 文本。根节点必须是数组。
pub(crate) fn parse_patch(text: &str) -> Result<Vec<Value>, AppError> {
    let value: Value = serde_yaml::from_str(text)
        .map_err(|e| AppError::Config(format!("解析 DSH 配置失败: {e}")))?;
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Sequence(items) => Ok(items),
        other => Err(AppError::Config(format!(
            "DSH 配置根节点应为数组，实际是 {}",
            yaml_kind(&other)
        ))),
    }
}

fn yaml_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Sequence(_) => "sequence",
        Value::Mapping(_) => "mapping",
        Value::Tagged(_) => "tagged",
    }
}

// ============================================================================
// 条目定位
// ============================================================================

/// 在 patch 数组中按 `id` 找条目下标。
pub(crate) fn find_entry_index(entries: &[Value], id: &str) -> Option<usize> {
    entries.iter().position(|entry| {
        entry
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|value| value == id)
    })
}

// ============================================================================
// providers 读写
// ============================================================================

/// 取 `llm-pi-ai.config.providers` 的值。
pub(crate) fn providers_of(entries: &[Value]) -> Option<&Value> {
    let index = find_entry_index(entries, ID_LLM_PI_AI)?;
    entries[index].get("config")?.get("providers")
}

/// 取单个 provider 的 baseURL。
pub(crate) fn provider_base_url(provider: &Value) -> Option<&str> {
    provider.get("baseURL").and_then(Value::as_str)
}

/// 取单个 provider 的 api 类型。
pub(crate) fn provider_api(provider: &Value) -> Option<&str> {
    provider.get("api").and_then(Value::as_str)
}

/// 从 JSON 形式的供应商配置里取 baseURL。
///
/// 数据库卡片里的 `settings_config` 是 JSON，而 DSH 配置文件里的同一份数据
/// 是 YAML。两个 helper 做同样的事，只是接受不同的值类型。
pub(crate) fn provider_base_url_json(config: &serde_json::Value) -> Option<String> {
    config
        .get("baseURL")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// [`validate_provider_node`] 的 JSON 版本，供供应商服务在写库前校验卡片。
pub(crate) fn validate_provider_json(
    provider_key: &str,
    config: &serde_json::Value,
) -> Result<(), AppError> {
    if provider_key.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "DSH provider key cannot be empty".to_string(),
        ));
    }
    if !config.is_object() {
        return Err(AppError::InvalidInput(
            "DSH provider configuration must be an object".to_string(),
        ));
    }
    Ok(())
}

/// 校验一张 DSH 供应商卡片能否写进 patch。
///
/// 只做「能否安全写入」的最低要求：key 非空、配置是对象。
/// 不做字段完整性校验——用户的 DSH 配置可能刻意省略某些字段，
/// 由 DSH 自己决定默认值。
pub(crate) fn validate_provider_node(provider_key: &str, config: &Value) -> Result<(), AppError> {
    if provider_key.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "DSH provider key cannot be empty".to_string(),
        ));
    }
    match &config {
        Value::Mapping(_) => Ok(()),
        other => Err(AppError::InvalidInput(format!(
            "DSH provider configuration must be a mapping, got {}",
            yaml_kind(other)
        ))),
    }
}

/// 把 `serde_json::Value` 转成 `serde_yaml::Value`。
///
/// 供应商卡片在数据库里是 JSON，而 patch 文件是 YAML；两边都只是结构化的
/// 无类型数据，经 serde 互转即可，不需要手写映射。
pub(crate) fn json_to_yaml(value: &serde_json::Value) -> Result<Value, AppError> {
    serde_yaml::to_value(value)
        .map_err(|e| AppError::Config(format!("将供应商配置转换为 YAML 失败: {e}")))
}

// ============================================================================
// 写入（读全量 → 合并 → 整体写回）
// ============================================================================

/// 合并结果：写回后应包含的完整 providers 映射。
///
/// 这是本模块的核心不变量：**输出一定包含输入的每一项**，只在之上增改，
/// 从不删除用户已有的 provider（除非显式用 [`remove_provider`]）。
#[derive(Debug, Default, Clone)]
pub(crate) struct ProvidersPatch {
    pub upserts: Vec<(String, Value)>,
    pub removals: Vec<String>,
}

impl ProvidersPatch {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn upsert(mut self, key: impl Into<String>, provider: Value) -> Self {
        self.upserts.push((key.into(), provider));
        self
    }

    pub(crate) fn remove(mut self, key: impl Into<String>) -> Self {
        self.removals.push(key.into());
        self
    }
}

/// 把 [`ProvidersPatch`] 应用到 patch 数组上。
///
/// 已有的 providers **原样保留**，只在其上增改；条目不存在时创建。
pub(crate) fn apply_providers_patch(
    entries: &mut Vec<Value>,
    patch: &ProvidersPatch,
) -> Result<(), AppError> {
    if patch.upserts.is_empty() && patch.removals.is_empty() {
        return Ok(());
    }

    let index = match find_entry_index(entries, ID_LLM_PI_AI) {
        Some(index) => index,
        None => {
            // 没有该条目时补一个骨架，避免把整个 patch 写坏
            let mut entry = Value::Mapping(Default::default());
            entry["id"] = Value::String(ID_LLM_PI_AI.to_string());
            entry["name"] = Value::String(NAME_LLM_PI_AI.to_string());
            entries.push(entry);
            entries.len() - 1
        }
    };

    let entry = &mut entries[index];
    if !entry.is_mapping() {
        return Err(AppError::Config(
            "DSH 配置中 llm-pi-ai 条目不是映射，拒绝改写".to_string(),
        ));
    }
    if entry.get("config").is_none() {
        entry["config"] = Value::Mapping(Default::default());
    }
    let config = &mut entry["config"];
    if !config.is_mapping() {
        return Err(AppError::Config(
            "DSH 配置中 llm-pi-ai.config 不是映射，拒绝改写".to_string(),
        ));
    }
    if config.get("providers").is_none() {
        config["providers"] = Value::Mapping(Default::default());
    }
    let providers = &mut config["providers"];
    if !providers.is_mapping() {
        return Err(AppError::Config(
            "DSH 配置中 providers 不是映射，拒绝改写".to_string(),
        ));
    }

    for key in &patch.removals {
        providers.as_mapping_mut()
            .expect("checked is_mapping above")
            .remove(Value::String(key.clone()));
    }
    for (key, value) in &patch.upserts {
        providers
            .as_mapping_mut()
            .expect("checked is_mapping above")
            .insert(Value::String(key.clone()), value.clone());
    }
    Ok(())
}

// ============================================================================
// 默认模型
// ============================================================================

/// 读当前默认模型：(provider, model)。
pub(crate) fn default_model_of(entries: &[Value]) -> Option<(String, String)> {
    let index = find_entry_index(entries, ID_AGENT_DEFAULT_MODEL)?;
    let config = entries[index].get("config")?;
    let provider = config.get("provider")?.as_str()?.to_string();
    let model = config.get("model")?.as_str()?.to_string();
    Some((provider, model))
}

/// 设置默认模型；条目不存在时创建。
pub(crate) fn set_default_model(
    entries: &mut Vec<Value>,
    provider: &str,
    model: &str,
) -> Result<(), AppError> {
    let index = match find_entry_index(entries, ID_AGENT_DEFAULT_MODEL) {
        Some(index) => index,
        None => {
            let mut entry = Value::Mapping(Default::default());
            entry["id"] = Value::String(ID_AGENT_DEFAULT_MODEL.to_string());
            entry["name"] = Value::String(NAME_AGENT_DEFAULT_MODEL.to_string());
            entries.push(entry);
            entries.len() - 1
        }
    };

    let entry = &mut entries[index];
    if !entry.is_mapping() {
        return Err(AppError::Config(
            "DSH 配置中 agent-default-model 条目不是映射，拒绝改写".to_string(),
        ));
    }
    if entry.get("config").is_none() {
        entry["config"] = Value::Mapping(Default::default());
    }
    let config = &mut entry["config"];
    if !config.is_mapping() {
        return Err(AppError::Config(
            "DSH 配置中 agent-default-model.config 不是映射，拒绝改写".to_string(),
        ));
    }
    config["provider"] = Value::String(provider.to_string());
    config["model"] = Value::String(model.to_string());
    Ok(())
}

// ============================================================================
// 序列化与原子写入
// ============================================================================

/// 序列化为 YAML 文本。空数组写成 `[]`，与 DSH 自身行为一致。
pub(crate) fn serialize_patch(entries: &[Value]) -> Result<String, AppError> {
    serde_yaml::to_string(entries)
        .map_err(|e| AppError::Config(format!("序列化 DSH 配置失败: {e}")))
}

/// 原子写入：先写同目录临时文件，再改名覆盖。
///
/// 直接原地写在写到一半崩溃时会留下损坏的配置，故一律走临时文件。
pub(crate) fn write_patch_atomic(path: &Path, entries: &[Value]) -> Result<(), AppError> {
    let text = if entries.is_empty() {
        "[]\n".to_string()
    } else {
        serialize_patch(entries)?
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AppError::Config(format!("创建 DSH 配置目录失败 {}: {e}", parent.display()))
        })?;
    }

    let tmp = path.with_extension("yml.tmp");
    std::fs::write(&tmp, text.as_bytes())
        .map_err(|e| AppError::Config(format!("写入 DSH 临时配置失败 {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| AppError::Config(format!("替换 DSH 配置失败 {}: {e}", path.display())))?;
    Ok(())
}

// ============================================================================
// 高层业务操作（读 → 改 → 整体写回）
// ============================================================================

/// 文件锁。多个命令并发写同一份 patch 时会互相覆盖，故串行化。
static PATCH_FILE_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

fn lock_patch_file() -> Result<std::sync::MutexGuard<'static, ()>, AppError> {
    PATCH_FILE_LOCK
        .lock()
        .map_err(|e| AppError::Config(format!("DSH 配置锁已损坏: {e}")))
}

/// 以 JSON 形式读取当前全部供应商（数据库卡片用的形态）。
pub(crate) fn read_providers() -> Result<serde_json::Map<String, serde_json::Value>, AppError> {
    let path = default_home_patch_path()?;
    let entries = read_patch(&path)?;
    let mut out = serde_json::Map::new();
    if let Some(providers) = providers_of(&entries) {
        if let Some(map) = providers.as_mapping() {
            for (key, value) in map {
                let Some(name) = key.as_str() else { continue };
                let json = serde_json::to_value(value).map_err(|e| {
                    AppError::Config(format!("转换 DSH 供应商 '{name}' 失败: {e}"))
                })?;
                out.insert(name.to_string(), json);
            }
        }
    }
    Ok(out)
}

/// 读当前默认模型。
pub(crate) fn read_default_model() -> Result<Option<(String, String)>, AppError> {
    let path = default_home_patch_path()?;
    let entries = read_patch(&path)?;
    Ok(default_model_of(&entries))
}

/// 新增或更新一个供应商，并整体写回。
///
/// 这是「读全量 → 合并 → 整体写回」的唯一入口。任何调用方都不该自己拼
/// patch 文本，否则会重演「providers 被整体替换」的事故。
pub(crate) fn upsert_provider(
    key: &str,
    config: &serde_json::Value,
) -> Result<(), AppError> {
    validate_provider_json(key, config)?;
    let _guard = lock_patch_file()?;
    let path = default_home_patch_path()?;
    upsert_provider_at(&path, key, config)
}

/// [`upsert_provider`] 的可测版本：显式指定 patch 路径，不读设置。
pub(crate) fn upsert_provider_at(
    path: &Path,
    key: &str,
    config: &serde_json::Value,
) -> Result<(), AppError> {
    validate_provider_json(key, config)?;
    let mut entries = read_patch(path)?;
    let yaml = json_to_yaml(config)?;
    apply_providers_patch(&mut entries, &ProvidersPatch::new().upsert(key, yaml))?;
    write_patch_atomic(path, &entries)
}

/// 删除一个供应商。不存在时视为成功（幂等）。
pub(crate) fn remove_provider(key: &str) -> Result<(), AppError> {
    let _guard = lock_patch_file()?;
    let path = default_home_patch_path()?;
    remove_provider_at(&path, key)
}

/// [`remove_provider`] 的可测版本。
pub(crate) fn remove_provider_at(path: &Path, key: &str) -> Result<(), AppError> {
    let mut entries = read_patch(path)?;
    if providers_of(&entries)
        .and_then(Value::as_mapping)
        .is_none_or(|map| !map.contains_key(Value::String(key.to_string())))
    {
        return Ok(());
    }
    apply_providers_patch(&mut entries, &ProvidersPatch::new().remove(key))?;
    write_patch_atomic(path, &entries)
}

/// 设置默认模型，并整体写回。
pub(crate) fn write_default_model(provider: &str, model: &str) -> Result<(), AppError> {
    let _guard = lock_patch_file()?;
    let path = default_home_patch_path()?;
    write_default_model_at(&path, provider, model)
}

/// [`write_default_model`] 的可测版本。
pub(crate) fn write_default_model_at(
    path: &Path,
    provider: &str,
    model: &str,
) -> Result<(), AppError> {
    let mut entries = read_patch(path)?;
    set_default_model(&mut entries, provider, model)?;
    write_patch_atomic(path, &entries)
}

/// 判断某个供应商 key 是否已存在于 patch 中。
pub(crate) fn provider_exists(key: &str) -> Result<bool, AppError> {
    let path = default_home_patch_path()?;
    provider_exists_at(&path, key)
}

/// [`provider_exists`] 的可测版本。
pub(crate) fn provider_exists_at(path: &Path, key: &str) -> Result<bool, AppError> {
    let entries = read_patch(path)?;
    Ok(providers_of(&entries)
        .and_then(Value::as_mapping)
        .is_some_and(|map| map.contains_key(Value::String(key.to_string()))))
}

/// [`read_providers`] 的可测版本。
pub(crate) fn read_providers_at(
    path: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>, AppError> {
    let entries = read_patch(path)?;
    let mut out = serde_json::Map::new();
    if let Some(providers) = providers_of(&entries) {
        if let Some(map) = providers.as_mapping() {
            for (key, value) in map {
                let Some(name) = key.as_str() else { continue };
                let json = serde_json::to_value(value)
                    .map_err(|e| AppError::Config(format!("转换 DSH 供应商 '{name}' 失败: {e}")))?;
                out.insert(name.to_string(), json);
            }
        }
    }
    Ok(out)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的 patch 文本。同时供 `sample_patch()` 与「写到磁盘」的用例使用，
    /// 保证只有一份数据源。
    const SAMPLE_PATCH_TEXT: &str = r#"
- id: ui-chat
  name: "@deepseek-ai/dsh-client-ui-chat"
  config:
    unknownField: keep-me
- id: llm-pi-ai
  name: "@deepseek-ai/dsh-llm-pi-ai"
  config:
    providers:
      user-real:
        displayName: UserReal
        apiKeyEnv: K_USER
        api: openai-completions
        baseURL: http://user.example/v1
        models:
          - id: user-model
        unknownProviderField: keep-me-too
- id: agent-default-model
  name: "@deepseek-ai/dsh-agent-default-model"
  config:
    provider: user-real
    model: user-model
"#;

    fn sample_patch_text() -> &'static str {
        SAMPLE_PATCH_TEXT
    }

    fn sample_patch() -> Vec<Value> {
        parse_patch(sample_patch_text())
            .expect("sample parses")
    }

    #[test]
    fn parses_array_root_and_rejects_mapping_root() {
        assert_eq!(parse_patch("[]").unwrap().len(), 0);
        assert_eq!(parse_patch("").unwrap().len(), 0);
        assert!(parse_patch("- id: a\n").unwrap().len() == 1);
        let err = parse_patch("id: a\n").unwrap_err().to_string();
        assert!(err.contains("应为数组"), "得到: {err}");
    }

    #[test]
    fn reads_providers_and_default_model() {
        let entries = sample_patch();
        let providers = providers_of(&entries).expect("providers present");
        assert!(providers.get("user-real").is_some());
        assert_eq!(
            default_model_of(&entries),
            Some(("user-real".to_string(), "user-model".to_string()))
        );
        let provider = &providers["user-real"];
        assert_eq!(provider_base_url(provider), Some("http://user.example/v1"));
        assert_eq!(provider_api(provider), Some("openai-completions"));
    }

    #[test]
    fn apply_providers_patch_never_drops_existing_providers() {
        // 核心不变量：新增一个 provider 不能把 user-real 弄丢。
        // 这正是实测中「整体替换」陷阱的防线。
        let mut entries = sample_patch();
        let patch = ProvidersPatch::new().upsert(
            "ccs-new",
            serde_yaml::from_str(
                r#"
displayName: CCSNew
apiKeyEnv: K_CCS
api: openai-completions
baseURL: http://ccs.example/v1
models:
  - id: ccs-model
"#,
            )
            .unwrap(),
        );
        apply_providers_patch(&mut entries, &patch).expect("apply ok");

        let providers = providers_of(&entries).unwrap();
        assert!(providers.get("user-real").is_some(), "原有 provider 被丢弃");
        assert!(providers.get("ccs-new").is_some(), "新 provider 未写入");
    }

    #[test]
    fn apply_providers_patch_preserves_unknown_fields() {
        let mut entries = sample_patch();
        apply_providers_patch(
            &mut entries,
            &ProvidersPatch::new().upsert(
                "x",
                serde_yaml::from_str("baseURL: http://x/v1\n").unwrap(),
            ),
        )
        .unwrap();

        let providers = providers_of(&entries).unwrap();
        assert_eq!(
            providers["user-real"]["unknownProviderField"].as_str(),
            Some("keep-me-too"),
            "未知字段被丢弃"
        );
        // 与 providers 无关的其它条目也必须原样保留
        let ui = &entries[find_entry_index(&entries, "ui-chat").unwrap()];
        assert_eq!(ui["config"]["unknownField"].as_str(), Some("keep-me"));
    }

    #[test]
    fn apply_providers_patch_updates_in_place_and_removes_explicitly() {
        let mut entries = sample_patch();
        apply_providers_patch(
            &mut entries,
            &ProvidersPatch::new()
                .upsert(
                    "user-real",
                    serde_yaml::from_str("baseURL: http://new.example/v1\n").unwrap(),
                )
                .remove("nope"),
        )
        .unwrap();
        let providers = providers_of(&entries).unwrap();
        assert_eq!(
            providers["user-real"]["baseURL"].as_str(),
            Some("http://new.example/v1")
        );
    }

    #[test]
    fn creates_missing_llm_entry() {
        let mut entries: Vec<Value> = Vec::new();
        apply_providers_patch(
            &mut entries,
            &ProvidersPatch::new().upsert(
                "p",
                serde_yaml::from_str("baseURL: http://p/v1\n").unwrap(),
            ),
        )
        .unwrap();
        assert_eq!(find_entry_index(&entries, ID_LLM_PI_AI), Some(0));
        assert_eq!(
            entries[0]["name"].as_str(),
            Some(NAME_LLM_PI_AI),
            "补建的条目应带上 name"
        );
        assert!(providers_of(&entries).unwrap().get("p").is_some());
    }

    #[test]
    fn set_default_model_updates_existing_and_creates_missing() {
        let mut entries = sample_patch();
        set_default_model(&mut entries, "ccs-new", "ccs-model").unwrap();
        assert_eq!(
            default_model_of(&entries),
            Some(("ccs-new".to_string(), "ccs-model".to_string()))
        );

        let mut empty: Vec<Value> = Vec::new();
        set_default_model(&mut empty, "a", "b").unwrap();
        assert_eq!(
            default_model_of(&empty),
            Some(("a".to_string(), "b".to_string()))
        );
        assert_eq!(empty[0]["name"].as_str(), Some(NAME_AGENT_DEFAULT_MODEL));
    }

    #[test]
    fn refuses_to_rewrite_non_mapping_nodes() {
        let mut entries = parse_patch("- id: llm-pi-ai\n  config: oops\n").unwrap();
        let err = apply_providers_patch(
            &mut entries,
            &ProvidersPatch::new().upsert("x", Value::Null),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("不是映射"), "得到: {err}");
    }

    #[test]
    fn round_trips_through_yaml_text() {
        let entries = sample_patch();
        let text = serialize_patch(&entries).unwrap();
        let again = parse_patch(&text).unwrap();
        assert_eq!(again.len(), entries.len());
        assert_eq!(default_model_of(&again), default_model_of(&entries));
        // 未知字段在文本往返后仍在
        let providers = providers_of(&again).unwrap();
        assert_eq!(
            providers["user-real"]["unknownProviderField"].as_str(),
            Some("keep-me-too")
        );
    }

    #[test]
    fn writes_atomically_and_round_trips_on_disk() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("dsh_cfg_test_{}_{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        let path = home_patch_path(&dir);
        let mut entries = sample_patch();
        apply_providers_patch(
            &mut entries,
            &ProvidersPatch::new().upsert(
                "disk",
                serde_yaml::from_str("baseURL: http://disk/v1\n").unwrap(),
            ),
        )
        .unwrap();

        write_patch_atomic(&path, &entries).expect("write ok");
        let read_back = read_patch(&path).expect("read ok");
        assert!(providers_of(&read_back).unwrap().get("disk").is_some());
        assert!(
            !path.with_extension("yml.tmp").exists(),
            "临时文件未清理"
        );

        // 空数组写成 [] 且能读回
        write_patch_atomic(&path, &[]).expect("write empty ok");
        assert_eq!(read_patch(&path).unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_patch_treats_missing_and_blank_as_empty() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let missing = std::env::temp_dir().join("dsh_cfg_test_missing_xyz/cordis.patch.yml");
        assert_eq!(read_patch(&missing).unwrap().len(), 0);

        let dir =
            std::env::temp_dir().join(format!("dsh_cfg_blank_{}_{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        let blank = home_patch_path(&dir);
        std::fs::write(&blank, "   \n\n").unwrap();
        assert_eq!(read_patch(&blank).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ========================================================================
    // 高层业务操作（走文件的完整链路）
    // ========================================================================

    /// 建一个临时目录并放入给定的 patch 内容，返回 (目录, patch 路径)。
    ///
    /// 目录名带原子自增序号——不能用 `line!()`：同一行上的多个用例会拿到
    /// 同一个名字，而 Rust 测试默认并行执行，会互相踩。
    fn temp_patch(content: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("dsh_biz_{}_{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = home_patch_path(&dir);
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    fn provider_json(base_url: &str) -> serde_json::Value {
        serde_json::json!({
            "displayName": "CCS Card",
            "apiKeyEnv": "K_CCS",
            "api": "openai-completions",
            "baseURL": base_url,
            "models": [{ "id": "model-a" }]
        })
    }

    #[test]
    fn upsert_provider_preserves_existing_providers_on_disk() {
        // 端到端验证「读全量 → 合并 → 整体写回」：磁盘上原有的 provider
        // 必须在写入后仍然存在。这是防「整体替换」事故的最后一道关卡。
        let (_dir, path) = temp_patch(&sample_patch_text());
        upsert_provider_at(&path, "ccs-new", &provider_json("http://ccs/v1")).unwrap();

        let providers = read_providers_at(&path).unwrap();
        assert!(providers.contains_key("user-real"), "原有 provider 丢失");
        assert!(providers.contains_key("ccs-new"), "新 provider 未写入");
        assert_eq!(
            providers["ccs-new"]["baseURL"].as_str(),
            Some("http://ccs/v1")
        );
        assert_eq!(
            providers["user-real"]["apiKeyEnv"].as_str(),
            Some("K_USER"),
            "原有 provider 的字段被改动"
        );
    }

    #[test]
    fn upsert_provider_updates_in_place_without_duplicating() {
        let (_dir, path) = temp_patch(&sample_patch_text());
        upsert_provider_at(&path, "user-real", &provider_json("http://updated/v1")).unwrap();

        let providers = read_providers_at(&path).unwrap();
        assert_eq!(providers.len(), 1, "不应产生重复条目");
        assert_eq!(
            providers["user-real"]["baseURL"].as_str(),
            Some("http://updated/v1")
        );
    }

    #[test]
    fn upsert_provider_rejects_empty_key_and_non_object() {
        let (_dir, path) = temp_patch(&sample_patch_text());
        assert!(upsert_provider_at(&path, "  ", &provider_json("http://x/v1")).is_err());
        assert!(
            upsert_provider_at(&path, "k", &serde_json::json!("not an object")).is_err()
        );
        // 校验失败不该留下半成品
        assert_eq!(read_providers_at(&path).unwrap().len(), 1);
    }

    #[test]
    fn remove_provider_removes_only_the_target() {
        let (_dir, path) = temp_patch(&sample_patch_text());
        upsert_provider_at(&path, "ccs-new", &provider_json("http://ccs/v1")).unwrap();

        remove_provider_at(&path, "user-real").unwrap();
        let providers = read_providers_at(&path).unwrap();
        assert!(!providers.contains_key("user-real"), "目标未删除");
        assert!(providers.contains_key("ccs-new"), "误删了别的 provider");
    }

    #[test]
    fn remove_provider_is_idempotent() {
        let (_dir, path) = temp_patch(&sample_patch_text());
        // 删不存在的 key 应成功且不改动文件内容
        remove_provider_at(&path, "nope").unwrap();
        assert_eq!(read_providers_at(&path).unwrap().len(), 1);

        // 删两次也只成功一次
        remove_provider_at(&path, "user-real").unwrap();
        remove_provider_at(&path, "user-real").unwrap();
        assert!(read_providers_at(&path).unwrap().is_empty());
    }

    #[test]
    fn write_default_model_persists_across_reads() {
        let (_dir, path) = temp_patch(&sample_patch_text());
        write_default_model_at(&path, "user-real", "model-x").unwrap();

        let entries = read_patch(&path).unwrap();
        assert_eq!(
            default_model_of(&entries),
            Some(("user-real".to_string(), "model-x".to_string()))
        );
    }

    #[test]
    fn provider_exists_reflects_disk_state() {
        let (_dir, path) = temp_patch(&sample_patch_text());
        assert!(provider_exists_at(&path, "user-real").unwrap());
        assert!(!provider_exists_at(&path, "ccs-new").unwrap());

        upsert_provider_at(&path, "ccs-new", &provider_json("http://ccs/v1")).unwrap();
        assert!(provider_exists_at(&path, "ccs-new").unwrap());
    }

    #[test]
    fn upsert_creates_patch_when_file_absent() {
        // 全新环境：patch 文件不存在，第一次写入应能创建并带上正确的条目 id
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("dsh_fresh_{}_{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = home_patch_path(&dir);

        upsert_provider_at(&path, "first", &provider_json("http://first/v1")).unwrap();
        let entries = read_patch(&path).unwrap();
        assert_eq!(find_entry_index(&entries, ID_LLM_PI_AI), Some(0));
        assert_eq!(entries[0]["name"].as_str(), Some(NAME_LLM_PI_AI));
        assert!(provider_exists_at(&path, "first").unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn full_lifecycle_matches_how_cc_switch_uses_it() {
        // 模拟 CC Switch 的典型用法序列，确认各函数能协同工作
        let (_dir, path) = temp_patch(&sample_patch_text());

        // 1. 新增卡片
        upsert_provider_at(&path, "cpa", &provider_json("http://cpa/v1")).unwrap();
        // 2. 编辑卡片
        upsert_provider_at(&path, "cpa", &provider_json("http://cpa-new/v1")).unwrap();
        // 3. 启用并设为默认
        write_default_model_at(&path, "cpa", "model-a").unwrap();

        let providers = read_providers_at(&path).unwrap();
        assert_eq!(providers.len(), 2);
        assert_eq!(providers["cpa"]["baseURL"].as_str(), Some("http://cpa-new/v1"));
        assert_eq!(
            default_model_of(&read_patch(&path).unwrap()),
            Some(("cpa".to_string(), "model-a".to_string()))
        );

        // 4. 彻底删除
        remove_provider_at(&path, "cpa").unwrap();
        assert!(!provider_exists_at(&path, "cpa").unwrap());
        assert!(provider_exists_at(&path, "user-real").unwrap());
        // 默认模型条目仍在（DSH 侧会忽略不存在的 provider，这里不代为清理）
        assert!(default_model_of(&read_patch(&path).unwrap()).is_some());
    }
}
