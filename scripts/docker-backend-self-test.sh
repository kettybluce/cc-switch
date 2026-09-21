#!/usr/bin/env bash
# Container entrypoint for the backend Docker self-test.
# Default: proxy_projection_linux + session_usage_scan + provider_profile_race
# (SCHEMA 18 Pi/Claude/Codex fixtures, isolated HOME JSONL usage scan,
# overlapping profile/provider CRUD + takeover).
# TEST_FILTER=all runs the full crate; any other value is a cargo test filter.
set -euo pipefail

TEST_HOME="${CC_SWITCH_TEST_HOME:-/tmp/cc-switch-test-home}"
mkdir -p "${TEST_HOME}" /app/dist
# Isolation is CC_SWITCH_TEST_HOME (see src-tauri/src/config.rs). Do not
# rewrite HOME here: cargo fingerprints HOME and would rebuild the crate.
export CC_SWITCH_TEST_HOME="${TEST_HOME}"
unset USERPROFILE || true

THREADS="${CARGO_TEST_THREADS:-1}"
FILTER="${TEST_FILTER:-}"

cd /app

run_cargo_test() {
  cargo test --offline --locked --manifest-path src-tauri/Cargo.toml "$@" -- --test-threads="${THREADS}"
}

if [[ $# -gt 0 ]]; then
  exec "$@"
fi

if [[ -z "${FILTER}" ]]; then
  run_cargo_test --test proxy_projection_linux --test session_usage_scan --test provider_profile_race
elif [[ "${FILTER}" == "proxy_projection_linux" ]]; then
  run_cargo_test --test proxy_projection_linux
elif [[ "${FILTER}" == "session_usage_scan" ]]; then
  run_cargo_test --test session_usage_scan
elif [[ "${FILTER}" == "provider_profile_race" ]]; then
  run_cargo_test --test provider_profile_race
elif [[ "${FILTER}" == "all" || "${FILTER}" == "*" ]]; then
  run_cargo_test
else
  # cargo treats a single positional as a test-name substring filter.
  # shellcheck disable=SC2086
  run_cargo_test ${FILTER}
fi
