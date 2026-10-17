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
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_patch() -> Vec<Value> {
        parse_patch(
            r#"
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
"#,
        )
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
        let dir = std::env::temp_dir().join(format!("dsh_cfg_test_{}", std::process::id()));
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
        let missing = std::env::temp_dir().join("dsh_cfg_test_missing_xyz/cordis.patch.yml");
        assert_eq!(read_patch(&missing).unwrap().len(), 0);

        let dir = std::env::temp_dir().join(format!("dsh_cfg_blank_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let blank = home_patch_path(&dir);
        std::fs::write(&blank, "   \n\n").unwrap();
        assert_eq!(read_patch(&blank).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
