use crate::pi_runtime::proxy::{PiProxyPlan, ProxyHealth};
use crate::pi_runtime::sessions::SessionSyncOutcome;
use crate::pi_runtime::{PiRuntimeKind, PiRuntimeSettings, PiRuntimeStatus, WslPiProbe};
use crate::provider::UsageScript;
use crate::services::pi_state::{PiCurrentState, PiStateService};
use crate::services::ProviderService;
use crate::session_manager::providers::pi::PiSessionDiscovery;
use crate::store::AppState;
use tauri::State;

#[tauri::command]
pub(crate) fn get_pi_current_state(state: State<'_, AppState>) -> Result<PiCurrentState, String> {
    PiStateService::current(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn update_pi_provider_usage_script(
    state: State<'_, AppState>,
    id: String,
    #[allow(non_snake_case)] usageScript: UsageScript,
) -> Result<bool, String> {
    ProviderService::update_pi_usage_script(state.inner(), &id, usageScript)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn get_pi_session_discovery() -> PiSessionDiscovery {
    crate::session_manager::providers::pi::session_discovery()
}

// ==================== Pi runtime (local / WSL) ====================

/// Current runtime, its Pi installation and whether WSL is usable here.
#[tauri::command]
pub(crate) fn get_pi_runtime_status() -> PiRuntimeStatus {
    crate::pi_runtime::status()
}

/// Probe every WSL distribution for a Pi installation.
///
/// Distributions without Pi are still returned so the picker can explain why
/// they cannot be selected, rather than hiding them and looking broken.
#[tauri::command]
pub(crate) fn list_pi_wsl_distros() -> Result<Vec<WslPiProbe>, String> {
    crate::pi_runtime::detect::probe_all().map_err(|error| error.to_string())
}

/// Switch the runtime Pi's providers and sessions are read from.
#[tauri::command]
pub(crate) fn set_pi_runtime(mut runtime: PiRuntimeSettings) -> Result<PiRuntimeStatus, String> {
    if runtime.kind == PiRuntimeKind::Wsl {
        // Switching to WSL without naming a distribution picks the one that
        // already has Pi configured, so the common single-distro setup needs
        // no choice at all.
        if runtime.distro.is_none() {
            let probes =
                crate::pi_runtime::detect::probe_all().map_err(|error| error.to_string())?;
            runtime.distro = crate::pi_runtime::detect::preferred_distro(&probes)
                .map(|probe| probe.distro.clone());
        }
        let Some(distro) = runtime.distro.clone() else {
            return Err(
                "No WSL distribution with Pi installed was found. Install Pi in WSL, or select a distribution explicitly."
                    .to_string(),
            );
        };
        // Validate before persisting so an unreachable selection cannot leave
        // the Pi pages pointed at a runtime that does not answer.
        crate::pi_runtime::detect::probe(&distro).map_err(|error| error.to_string())?;
    }

    crate::settings::set_pi_runtime_settings(runtime).map_err(|error| error.to_string())?;
    crate::pi_runtime::sessions::invalidate_sync_throttle();
    Ok(crate::pi_runtime::status())
}

/// Mirror WSL Pi sessions now, bypassing the refresh throttle.
#[tauri::command]
pub(crate) fn sync_pi_wsl_sessions() -> Result<SessionSyncOutcome, String> {
    let target = crate::pi_runtime::target();
    crate::pi_runtime::sessions::invalidate_sync_throttle();
    crate::pi_runtime::sessions::sync(&target).map_err(|error| error.to_string())
}

/// Proxy plan for the Pi process: how WSL reaches CC Switch, which upstream
/// proxy Pi should use, and the environment that would be applied.
#[tauri::command]
pub(crate) async fn get_pi_proxy_plan(state: State<'_, AppState>) -> Result<PiProxyPlan, String> {
    let target = crate::pi_runtime::target();
    let flags = crate::pi_runtime::settings().flags;
    let port = state
        .proxy_service
        .get_status()
        .await
        .map(|status| status.port)
        .unwrap_or_default();
    let configured = state
        .db
        .get_global_proxy_url()
        .map_err(|error| error.to_string())?;

    crate::pi_runtime::proxy::plan(&target, flags.wsl_proxy, port, configured.as_deref())
        .map_err(|error| error.to_string())
}

/// Verify that Pi can actually reach the internet through the planned proxy.
#[tauri::command]
pub(crate) async fn test_pi_proxy(state: State<'_, AppState>) -> Result<ProxyHealth, String> {
    let target = crate::pi_runtime::target();
    let configured = state
        .db
        .get_global_proxy_url()
        .map_err(|error| error.to_string())?;

    crate::pi_runtime::proxy::verify(&target, configured.as_deref())
        .map_err(|error| error.to_string())
}
