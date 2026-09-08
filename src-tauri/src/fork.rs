//! Fork-only switches for kettybluce/cc-switch.
//!
//! Keep these as named constants so a later sync from upstream main can
//! cherry-pick around them or flip them back.

/// Do not check or install CC Switch app updates against upstream endpoints.
pub const DISABLE_APP_UPDATER: bool = true;

pub fn updater_disabled_message() -> &'static str {
    "App updates are disabled in this fork"
}
