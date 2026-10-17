use crate::store::AppState;
use tauri::State;

/// DSH 的 `cordis.patch.yml` 里当前存在的供应商标识。
///
/// 前端用它把卡片分到「已添加 / 可添加」两栏——DSH 是累加式应用，patch 里的
/// `providers` 是一份映射，键就是供应商标识。
#[tauri::command]
pub(crate) fn get_dsh_live_provider_ids() -> Result<Vec<String>, String> {
    crate::dsh_config::read_providers()
        .map(|providers| providers.keys().cloned().collect())
        .map_err(|error| error.to_string())
}

/// 把 DSH 配置里已有的供应商导进数据库。
///
/// 与 OpenCode / OpenClaw / Hermes 的「导入当前配置」同构：用户可能在 WSL 里
/// 直接编辑过 `cordis.patch.yml`，先把它们收进卡片再管理。
#[tauri::command]
pub(crate) fn import_dsh_providers_from_live(state: State<'_, AppState>) -> Result<usize, String> {
    crate::services::provider::import_dsh_providers_from_live(state.inner())
        .map_err(|error| error.to_string())
}

/// 把某个 DSH 供应商的指定模型设为默认。
///
/// 单独开一条命令而不是给 `switch_provider` 加参数：切换的语义是「让这家生效」，
/// 而这里是「这家里的哪个模型生效」，混在一起会让所有应用的切换签名都带上一个
/// 只有 DSH 用得到的字段。
#[tauri::command]
pub(crate) fn set_dsh_default_model(
    state: State<'_, AppState>,
    id: String,
    #[allow(non_snake_case)] modelId: String,
) -> Result<(), String> {
    crate::services::provider::set_dsh_default_model(state.inner(), &id, &modelId)
        .map_err(|error| error.to_string())
}
