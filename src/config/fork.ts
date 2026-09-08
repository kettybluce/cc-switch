/**
 * Fork-only switches for kettybluce/cc-switch.
 *
 * Keep these as named constants (not scattered literals) so a later sync from
 * upstream main can cherry-pick around them or flip them back.
 */

/**
 * Do not check or install CC Switch app updates.
 *
 * Upstream updater endpoints (`dl.ccswitch.io` / farion1231 `latest.json`)
 * would otherwise prompt this fork to "upgrade" onto an unrelated build.
 */
export const FORK_DISABLE_APP_UPDATER = true;
