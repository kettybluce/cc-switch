//! DSH（DeepSeek Harness）供应商服务。
//!
//! 与 Pi 的服务结构对应，但有几处刻意的差异：
//!
//! - **没有「投影/还原」概念**：阶段一不做本地代理接管，patch 里存的就是真实
//!   上游地址，因此不需要 SSOT 投影守卫。
//! - **写入必须整体回写**：DSH 的 `providers` 是整体替换语义（实测），底层
//!   `dsh_config::upsert_provider` 负责「读全量 → 合并 → 整体写回」，
//!   本层只做业务编排，不自己拼 patch。
//! - **删除 = 从 patch 移除**：DSH 里没有「启用/禁用」的二态，一个 provider
//!   要么在 patch 里（可用），要么不在。故 `delete` 与 `remove` 都映射到
//!   「从 patch 移除」，区别只在于是否保留数据库卡片。

use super::{ProviderService, SwitchResult};
use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};
use crate::store::AppState;
use indexmap::IndexMap;
use serde_json::Value;

const DSH_APP: &str = "dsh";

/// 把 patch 里的供应商映射成 CC Switch 的卡片结构。
///
/// 缺失的 `id` / `name` 用 key 补上——patch 里的 key 本身就是唯一标识。
fn to_provider(key: &str, config: &Value) -> Provider {
    let display = config
        .get("displayName")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(key);
    Provider {
        id: key.to_string(),
        name: display.to_string(),
        settings_config: config.clone(),
        website_url: None,
        category: Some("custom".to_string()),
        created_at: None,
        sort_index: None,
        notes: None,
        meta: Some(ProviderMeta::default()),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    }
}

/// 列出 DSH 的全部供应商。
///
/// 数据来源是 patch 文件本身，而不是数据库——patch 是 DSH 唯一读取的地方，
/// 以它为准可以避免「库里有一张卡但 DSH 看不见」的错位。
pub(super) fn list(state: &AppState) -> Result<IndexMap<String, Provider>, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(DSH_APP));

    let raw = match crate::dsh_config::read_providers() {
        Ok(value) => value,
        Err(error) => {
            // 读不到时回落到数据库，至少让用户看到已有卡片而不是空列表
            log::warn!("读取 DSH 配置失败，回落到数据库: {error}");
            return state.db.get_all_providers(DSH_APP);
        }
    };

    let mut out = IndexMap::new();
    for (key, config) in raw {
        out.insert(key.clone(), to_provider(&key, &config));
    }
    Ok(out)
}

/// 从 live（patch）导入供应商到数据库，返回导入/更新条数。
pub(super) fn import_from_live(state: &AppState) -> Result<usize, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(DSH_APP));
    let raw = crate::dsh_config::read_providers()?;
    let mut count = 0usize;
    for (key, config) in raw {
        let provider = to_provider(&key, &config);
        state.db.save_provider(DSH_APP, &provider)?;
        count += 1;
    }
    Ok(count)
}

/// 新增供应商。`add_to_live` 为真时同步写进 patch。
pub(super) fn add(
    state: &AppState,
    provider: Provider,
    add_to_live: bool,
) -> Result<bool, AppError> {
    let app_type = AppType::Dsh;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));

    ProviderService::validate_provider_settings(&app_type, &provider)?;
    ProviderService::normalize_usage_script_credential_overrides(&app_type, &mut {
        provider.clone()
    });

    if state
        .db
        .get_provider_by_id(&provider.id, app_type.as_str())?
        .is_some()
    {
        return Err(AppError::InvalidInput(format!(
            "DSH provider '{}' already exists",
            provider.id
        )));
    }
    if crate::dsh_config::provider_exists(&provider.id)? {
        return Err(AppError::InvalidInput(format!(
            "DSH provider key '{}' already exists in cordis.patch.yml",
            provider.id
        )));
    }

    if add_to_live {
        crate::dsh_config::upsert_provider(&provider.id, &provider.settings_config)?;
    }

    if let Err(error) = state.db.save_provider(app_type.as_str(), &provider) {
        // 数据库失败时把刚写进 patch 的项撤掉，避免留下孤儿
        if add_to_live {
            if let Err(rollback) = crate::dsh_config::remove_provider(&provider.id) {
                return Err(AppError::Config(format!(
                    "保存 DSH 供应商失败: {error}; 配置回滚失败: {rollback}"
                )));
            }
        }
        return Err(error);
    }
    Ok(true)
}

/// 更新供应商。DSH 的 provider key 不可改名——它是 patch 里的映射键。
pub(super) fn update(
    state: &AppState,
    original_id: Option<&str>,
    provider: Provider,
) -> Result<bool, AppError> {
    let app_type = AppType::Dsh;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));

    let original_id = original_id.unwrap_or(&provider.id).to_string();
    if original_id != provider.id {
        return Err(AppError::InvalidInput(
            "DSH provider keys cannot be renamed".to_string(),
        ));
    }

    state
        .db
        .get_provider_by_id(&original_id, app_type.as_str())?
        .ok_or_else(|| {
            AppError::InvalidInput(format!("DSH provider '{original_id}' not found"))
        })?;

    ProviderService::validate_provider_settings(&app_type, &provider)?;

    // 已在 patch 里的才跟着改；纯数据库卡片不因编辑而凭空出现在配置里
    let existed = crate::dsh_config::provider_exists(&original_id)?;
    if existed {
        crate::dsh_config::upsert_provider(&original_id, &provider.settings_config)?;
    }

    if let Err(error) = state.db.save_provider(app_type.as_str(), &provider) {
        return Err(error);
    }
    Ok(true)
}

/// 彻底删除：从 patch 移除并删掉数据库卡片。
pub(super) fn delete(state: &AppState, id: &str) -> Result<(), AppError> {
    let app_type = AppType::Dsh;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));

    if state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .is_none()
    {
        return Ok(());
    }

    crate::dsh_config::remove_provider(id)?;
    state.db.delete_provider(app_type.as_str(), id)?;
    Ok(())
}

/// 从配置移除，但保留数据库卡片。
pub(super) fn remove(state: &AppState, id: &str) -> Result<(), AppError> {
    let app_type = AppType::Dsh;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));

    state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .ok_or_else(|| AppError::InvalidInput(format!("DSH provider '{id}' not found")))?;

    crate::dsh_config::remove_provider(id)?;
    Ok(())
}

/// 启用（写入 patch）并把该供应商设为默认模型。
///
/// DSH 没有「多供应商同时启用」的概念之外的选择问题——写进 patch 就可用，
/// 但真正生效的是 `agent-default-model` 指向的那一个。故这里一并切默认。
pub(super) fn enable(state: &AppState, id: &str) -> Result<SwitchResult, AppError> {
    let app_type = AppType::Dsh;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));

    let provider = state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .ok_or_else(|| AppError::InvalidInput(format!("DSH provider '{id}' not found")))?;

    ProviderService::validate_provider_settings(&app_type, &provider)?;
    crate::dsh_config::upsert_provider(id, &provider.settings_config)?;

    // 取第一个模型作为默认；没有模型时只写 provider，让 DSH 自己决定
    let first_model = provider
        .settings_config
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| models.iter().find_map(|m| m.get("id")))
        .and_then(Value::as_str);

    if let Some(model) = first_model {
        crate::dsh_config::write_default_model(id, model)?;
    } else if let Some(model) = provider
        .settings_config
        .get("defaultModel")
        .and_then(Value::as_str)
    {
        crate::dsh_config::write_default_model(id, model)?;
    }

    Ok(SwitchResult {
        warnings: Vec::new(),
    })
}
