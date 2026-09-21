//! 托盘 / `ExitRequested` / SIGTERM 退出分类与 Live 恢复编排（SCHEMA 18）。
//!
//! 不升 schema。分类只看退出码与窗口可见性，不碰数据库。

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::Manager;

/// `RunEvent::ExitRequested` 的三类来源，处理方式必须区分。
///
/// 关键约束：重启请求（`code == RESTART_EXIT_CODE`）上 `prevent_exit()` 会被
/// Tauri 静默忽略（见 `ExitRequestApi::prevent_exit` 文档），事件循环必定继续
/// 退出并触发各插件的 `RunEvent::Exit` 钩子；任何与之并发的自定义清理任务都
/// 可能与插件退出钩子争用同一状态而死锁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitRequestAction {
    /// `code` 为 `None` 且当前没有可见窗口：运行时自动触发（隐藏到托盘后
    /// WebView 被回收、轻量模式销毁主窗口、关窗后无存活窗口）。
    /// 阻止退出、保持托盘后台运行；随后托盘「打开主界面」必须能恢复窗口。
    StayInTray,
    /// `code` 为 `RESTART_EXIT_CODE`：`app.restart()` / 自更新 relaunch 发起的
    /// 重启，不拦截、不做自定义清理，交还 Tauri 默认 re-exec 流程。
    DeferToTauriRestart,
    /// 用户主动退出：托盘「退出」、`app.exit(code)`、macOS Cmd+Q
    /// （`ExitRequested(None)` 且仍有可见窗口）、以及可见的次窗口。
    /// Unix `SIGTERM`/`SIGINT` 不走本分类，直接调用同一套清理序列。
    CleanupAndExit,
}

/// 当前 WebView 窗口集合的可见性快照，供退出分类与托盘恢复使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowPresence {
    pub has_main: bool,
    pub main_visible: bool,
    pub other_visible: bool,
}

impl WindowPresence {
    pub fn any_visible(self) -> bool {
        self.main_visible || self.other_visible
    }
}

/// `StayInTray` 之后必须做的托盘侧收尾，否则 `ExitRequested(None)` 回收
/// WebView 后「打开主界面」会变成空操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StayInTrayFollowup {
    /// 主窗口仍在（最小化到托盘）：下次 show_main 显示现窗。
    KeepHiddenMain,
    /// 主窗口已不在：标记轻量模式，托盘 / Dock / 单实例按重建路径恢复。
    MarkLightweightAndRecreateOnShow,
}

/// 托盘「打开主界面」在主窗口缺失时不得再依赖轻量模式标志。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayRestoreKind {
    ShowExisting,
    Recreate,
}

/// 用户退出（托盘退出 / Cmd+Q / SIGTERM）必须按此顺序执行。
/// 托盘退出与 Unix 信号共用同一函数，避免漏掉 `destroy_single_instance_lock`。
pub(crate) const USER_EXIT_CLEANUP_STEPS: &[&str] = &[
    "save_window_state",
    "restore_live_keep_state",
    "remove_tray_icon",
    "destroy_single_instance_lock",
    "process_exit_0",
];

const _: () = assert!(USER_EXIT_CLEANUP_STEPS.len() == 5);

static EXIT_CLEANUP_STARTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn classify_exit_request(
    code: Option<i32>,
    presence: WindowPresence,
) -> ExitRequestAction {
    match code {
        // 仍有可见窗口时的 `None`：macOS Cmd+Q、部分 WM 的真正退出。
        // 隐藏到托盘后主窗口仍在但 `is_visible() == false`，必须 StayInTray，
        // 否则关窗/最小化会被 salvage 的 `has_main_window` 误杀成 Live 恢复退出。
        None if presence.any_visible() => ExitRequestAction::CleanupAndExit,
        None => ExitRequestAction::StayInTray,
        Some(tauri::RESTART_EXIT_CODE) => ExitRequestAction::DeferToTauriRestart,
        Some(_) => ExitRequestAction::CleanupAndExit,
    }
}

pub(crate) fn stay_in_tray_followup(presence: WindowPresence) -> StayInTrayFollowup {
    match tray_restore_kind(presence.has_main) {
        TrayRestoreKind::ShowExisting => StayInTrayFollowup::KeepHiddenMain,
        TrayRestoreKind::Recreate => StayInTrayFollowup::MarkLightweightAndRecreateOnShow,
    }
}

pub(crate) fn tray_restore_kind(has_main_window: bool) -> TrayRestoreKind {
    if has_main_window {
        TrayRestoreKind::ShowExisting
    } else {
        TrayRestoreKind::Recreate
    }
}

pub(crate) fn window_presence_from_app(app: &tauri::AppHandle) -> WindowPresence {
    let windows = app.webview_windows();
    let main = windows.get("main");
    let has_main = main.is_some();
    // 查询失败时按可见处理：保证 Cmd+Q 仍走 Live 恢复；托盘「退出」始终是 Some(0)。
    // 最小化到托盘后 `hide()` 应返回 `Ok(false)`，不会误杀。
    let main_visible = main
        .map(|window| window.is_visible().unwrap_or(true))
        .unwrap_or(false);
    let other_visible = windows
        .iter()
        .any(|(label, window)| label.as_str() != "main" && window.is_visible().unwrap_or(true));
    WindowPresence {
        has_main,
        main_visible,
        other_visible,
    }
}

/// 托盘退出与 SIGTERM 可能同时触发清理；只允许第一路执行 Live 恢复。
pub(crate) fn begin_exit_cleanup() -> bool {
    EXIT_CLEANUP_STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

#[cfg(test)]
pub(crate) fn reset_exit_cleanup_for_tests() {
    EXIT_CLEANUP_STARTED.store(false, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::{
        begin_exit_cleanup, classify_exit_request, reset_exit_cleanup_for_tests,
        stay_in_tray_followup, tray_restore_kind, ExitRequestAction, StayInTrayFollowup,
        TrayRestoreKind, WindowPresence, USER_EXIT_CLEANUP_STEPS,
    };

    const NONE: WindowPresence = WindowPresence {
        has_main: false,
        main_visible: false,
        other_visible: false,
    };
    const HIDDEN_MAIN: WindowPresence = WindowPresence {
        has_main: true,
        main_visible: false,
        other_visible: false,
    };
    const VISIBLE_MAIN: WindowPresence = WindowPresence {
        has_main: true,
        main_visible: true,
        other_visible: false,
    };
    const OTHER_VISIBLE_ONLY: WindowPresence = WindowPresence {
        has_main: false,
        main_visible: false,
        other_visible: true,
    };
    const HIDDEN_MAIN_PLUS_VISIBLE_OTHER: WindowPresence = WindowPresence {
        has_main: true,
        main_visible: false,
        other_visible: true,
    };

    #[test]
    fn exit_requested_none_without_windows_stays_in_tray() {
        assert_eq!(
            classify_exit_request(None, NONE),
            ExitRequestAction::StayInTray
        );
    }

    #[test]
    fn exit_requested_none_with_hidden_main_stays_in_tray_for_restore() {
        // 回归：关窗最小化到托盘后主窗口仍在。salvage 只看 has_main 会
        // CleanupAndExit，把托盘会话杀掉并误走 Live 恢复。
        assert_eq!(
            classify_exit_request(None, HIDDEN_MAIN),
            ExitRequestAction::StayInTray
        );
        assert_eq!(
            stay_in_tray_followup(HIDDEN_MAIN),
            StayInTrayFollowup::KeepHiddenMain
        );
        assert_eq!(
            tray_restore_kind(HIDDEN_MAIN.has_main),
            TrayRestoreKind::ShowExisting
        );
    }

    #[test]
    fn tray_restore_after_exit_requested_none_recreates_missing_main() {
        assert_eq!(
            classify_exit_request(None, NONE),
            ExitRequestAction::StayInTray
        );
        assert_eq!(
            stay_in_tray_followup(NONE),
            StayInTrayFollowup::MarkLightweightAndRecreateOnShow
        );
        // WebView 回收后轻量标志可能仍为 false；托盘恢复不得再门控该标志。
        assert_eq!(tray_restore_kind(NONE.has_main), TrayRestoreKind::Recreate);
    }

    #[test]
    fn exit_requested_none_with_visible_main_cleans_up_like_cmd_q() {
        assert_eq!(
            classify_exit_request(None, VISIBLE_MAIN),
            ExitRequestAction::CleanupAndExit
        );
    }

    #[test]
    fn multi_window_visible_secondary_without_main_is_user_exit() {
        // 轻量模式销毁主窗口后，插件/对话框等次窗口仍可见时的 Cmd+Q。
        assert_eq!(
            classify_exit_request(None, OTHER_VISIBLE_ONLY),
            ExitRequestAction::CleanupAndExit
        );
    }

    #[test]
    fn multi_window_hidden_main_plus_visible_other_is_user_exit() {
        assert_eq!(
            classify_exit_request(None, HIDDEN_MAIN_PLUS_VISIBLE_OTHER),
            ExitRequestAction::CleanupAndExit
        );
    }

    #[test]
    fn multi_window_all_hidden_stays_in_tray() {
        assert_eq!(
            classify_exit_request(None, HIDDEN_MAIN),
            ExitRequestAction::StayInTray
        );
        assert_eq!(
            stay_in_tray_followup(HIDDEN_MAIN),
            StayInTrayFollowup::KeepHiddenMain
        );
    }

    #[test]
    fn restart_exit_code_defers_regardless_of_windows() {
        assert_eq!(
            classify_exit_request(Some(tauri::RESTART_EXIT_CODE), NONE),
            ExitRequestAction::DeferToTauriRestart
        );
        assert_eq!(
            classify_exit_request(Some(tauri::RESTART_EXIT_CODE), VISIBLE_MAIN),
            ExitRequestAction::DeferToTauriRestart
        );
    }

    #[test]
    fn user_exit_codes_run_cleanup_then_exit() {
        assert_eq!(
            classify_exit_request(Some(0), VISIBLE_MAIN),
            ExitRequestAction::CleanupAndExit
        );
        assert_eq!(
            classify_exit_request(Some(1), NONE),
            ExitRequestAction::CleanupAndExit
        );
        assert_eq!(
            classify_exit_request(Some(0), HIDDEN_MAIN),
            ExitRequestAction::CleanupAndExit
        );
    }

    #[test]
    fn tray_quit_and_sigterm_share_live_restore_cleanup_steps() {
        assert_eq!(
            USER_EXIT_CLEANUP_STEPS,
            &[
                "save_window_state",
                "restore_live_keep_state",
                "remove_tray_icon",
                "destroy_single_instance_lock",
                "process_exit_0",
            ]
        );
    }

    #[test]
    fn begin_exit_cleanup_is_once_against_sigterm_exitrequested_race() {
        reset_exit_cleanup_for_tests();
        assert!(begin_exit_cleanup());
        assert!(
            !begin_exit_cleanup(),
            "SIGTERM 与 ExitRequested(CleanupAndExit) 竞态时只应恢复一次 Live"
        );
        reset_exit_cleanup_for_tests();
        assert!(begin_exit_cleanup());
        reset_exit_cleanup_for_tests();
    }
}
