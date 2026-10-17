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
/// 合并两个来源，缺一不可：
///
/// - **patch** 是 DSH 真正读取的地方，以它为准才不会出现「库里有一张卡但 DSH 看不见」；
/// - **数据库**装的是「已移除但保留卡片」的那些——`remove` 只摘 patch、`add` 也可能
///   只存库不写 patch。只读 patch 会让这些卡在前端彻底消失，用户既删不掉也加不回来。
///
/// patch 里有的以 patch 为准（它可能被用户手改过），只有库的卡片按库里的样子补上。
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

    let mut saved = state.db.get_all_providers(DSH_APP)?;
    let mut out = IndexMap::new();

    // patch 里的顺序即 DSH 侧的顺序，保持它
    for (key, config) in &raw {
        let mut provider = match saved.get(key) {
            // 卡片身份（名字、图标、分类、备注）以数据库为准，配置以 patch 为准
            Some(row) => {
                let mut row = row.clone();
                row.settings_config = config.clone();
                row
            }
            None => to_provider(key, config),
        };
        // 前端据此判断「已在配置里」——与 OpenCode / OpenClaw / MCode 同一字段
        ProviderService::set_provider_live_config_managed(&mut provider, true);
        out.insert(key.clone(), provider);
    }

    // 只剩库的卡片：保留可见、可删，但明确标成「不在配置里」，
    // 前端会把它放进「可添加」分区而不是当成生效中的供应商。
    for (key, mut provider) in saved.drain(..) {
        if !out.contains_key(&key) {
            ProviderService::set_provider_live_config_managed(&mut provider, false);
            out.insert(key, provider);
        }
    }

    Ok(out)
}

/// 把某家供应商的指定模型设为 DSH 默认。
///
/// 先确认 provider 存在于 patch：`agent-default-model` 指向一个不存在的
/// route 会让 DSH 启动时找不到模型。卡片存在时再校验模型确实在它的
/// `models` 里（卡片没写 `models` 则不拦，DSH 会用自己的目录）。
pub(super) fn set_default_model(
    state: &AppState,
    id: &str,
    model: &str,
) -> Result<(), AppError> {
    let app_type = AppType::Dsh;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));

    if !crate::dsh_config::provider_exists(id)? {
        return Err(AppError::InvalidInput(format!(
            "DSH provider '{id}' is not in cordis.patch.yml"
        )));
    }

    // 卡片可能已被删，但 patch 里还在（手改过的配置）。此时以 patch 为准，不拦。
    if let Some(provider) = state.db.get_provider_by_id(id, app_type.as_str())? {
        let known: Vec<&str> = provider
            .settings_config
            .get("models")
            .and_then(Value::as_array)
            .map(|models| {
                models
                    .iter()
                    .filter_map(|m| m.get("id").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        // 卡片里没写 models 时不拦：DSH 会用它自带的目录，用户可能挑的是目录里的模型。
        // 卡片写了 models 就必须命中，否则是笔误。
        if !known.is_empty() && !known.contains(&model) {
            return Err(AppError::InvalidInput(format!(
                "Model '{model}' is not in DSH provider '{id}' models list"
            )));
        }
    }

    crate::dsh_config::write_default_model(id, model)
}

/// 从 live（patch）导入供应商到数据库，返回导入/更新条数。
pub(super) fn import_from_live(state: &AppState) -> Result<usize, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(DSH_APP));
    let raw = crate::dsh_config::read_providers()?;
    let mut count = 0usize;
    for (key, config) in raw {
        let mut provider = to_provider(&key, &config);
        ProviderService::set_provider_live_config_managed(&mut provider, true);
        state.db.save_provider(DSH_APP, &provider)?;
        count += 1;
    }
    Ok(count)
}

/// 新增供应商。`add_to_live` 为真时同步写进 patch。
pub(super) fn add(
    state: &AppState,
    mut provider: Provider,
    add_to_live: bool,
) -> Result<bool, AppError> {
    let app_type = AppType::Dsh;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));

    ProviderService::validate_provider_settings(&app_type, &provider)?;
    // 这里必须改 `provider` 本身。早先写成 `&mut { provider.clone() }`，
    // 归一化作用在一份临时克隆上，等于什么都没做。
    ProviderService::normalize_usage_script_credential_overrides(&app_type, &mut provider);

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

    // 注意：这里**不**改默认模型。新增只是让这家变得可用；真正切过去是
    // `enable`（用户点「添加」或「设为默认」）的事。早先在这里顺手写了
    // `models[0]`，会把人正在用的 DSH 默认模型悄悄换掉。
    Ok(true)
}

/// 卡片里第一个模型的 id。DSH 侧的 `models` 是数组，顺序即优先级。
fn first_model_of(provider: &Provider) -> String {
    provider
        .settings_config
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| models.iter().find_map(|m| m.get("id")))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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

        // 编辑可能把默认模型从卡片里删掉。此时 agent-default-model 会指向一个
        // 不存在的模型，DSH 启动即报错。改回卡片里的第一个模型；卡片不再有任何
        // 模型且原来指向的模型已失效时，把 provider/model 一起清掉太激进——
        // 留给用户去 DSH 侧重选，这里只在不产生悬空引用时补第一个模型。
        let current_default = crate::dsh_config::read_default_model().unwrap_or(None);
        let first = first_model_of(&provider);
        let needs_repick = current_default
            .as_ref()
            .map(|(provider_id, model)| {
                if provider_id != &original_id {
                    // 默认在别家，与这次编辑无关
                    return false;
                }
                let models = provider
                    .settings_config
                    .get("models")
                    .and_then(Value::as_array);
                match models {
                    // 卡片没写 models：DSH 会查自己的目录，不在这里下结论
                    None => false,
                    Some(list) => !list
                        .iter()
                        .any(|m| m.get("id").and_then(Value::as_str) == Some(model.as_str())),
                }
            })
            .unwrap_or(false);
        if needs_repick && !first.is_empty() {
            crate::dsh_config::write_default_model(&original_id, &first)?;
        }
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
/// DSH 里「写进 patch」和「生效」是两件事：写进 `providers` 只代表这条 route 可用，
/// 真正决定用哪家的是 `agent-default-model`。所以这里必须把 default 也切过来，
/// 否则卡片显示已添加、DSH 实际还在用上一家。
///
/// 模型的选择顺序：
/// 1. `agent-default-model` 已经指向这家、且该模型仍在这家的 `models` 里时，
///    **保留原来的模型**——反复点同一个供应商不该把用户挑过的模型重置成第一个；
/// 2. 否则用卡片里的 `defaultModel`；
/// 3. 再否则用 `models[0]`。
///
/// 三者都没有时才只写 provider。此时 DSH 侧仍是旧的默认，调用方会拿到一条 warning，
/// 免得用户以为切换成功了。
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

    let models: Vec<String> = provider
        .settings_config
        .get("models")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let current_default = crate::dsh_config::read_default_model().unwrap_or(None);
    let keep_existing = current_default
        .as_ref()
        .filter(|(current_provider, model)| {
            current_provider == id
                // 卡片没写 models 时无从判断，沿用原默认更安全
                && (models.is_empty() || models.iter().any(|known| known == model))
        })
        .map(|(_, model)| model.clone());

    let model = keep_existing
        .or_else(|| {
            provider
                .settings_config
                .get("defaultModel")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| models.first().cloned());

    match model {
        Some(model) => {
            crate::dsh_config::write_default_model(id, &model)?;
            Ok(SwitchResult {
                warnings: Vec::new(),
            })
        }
        None => Ok(SwitchResult {
            warnings: vec![
                "DSH 未记录默认模型：这张卡片没有 models，也没有已有默认可沿用。\
                 请在 DSH 侧选择一个模型，或在卡片里补上 models。"
                    .to_string(),
            ],
        }),
    }
}
