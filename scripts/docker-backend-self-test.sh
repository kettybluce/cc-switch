#!/usr/bin/env bash
# Container entrypoint for the backend Docker self-test.
# Default: FULL `cargo test` for src-tauri (SCHEMA 18).
# Narrower named filters are explicit overrides only.
# Does NOT run the Tauri GUI. Isolated CC_SWITCH_TEST_HOME under /tmp.
set -euo pipefail

TEST_HOME="${CC_SWITCH_TEST_HOME:-/tmp/cc-switch-test-home}"
mkdir -p "${TEST_HOME}" /app/dist

# Named volumes are root-owned and may still contain files from an
# earlier root-run. CI cargo tests run as a regular user; chown the
# whole tree so tester can reset `.cc-switch` (otherwise Permission denied
# poisons the integration-test mutex).
if [[ "$(id -u)" -eq 0 && -z "${CC_SWITCH_DROPPED_ROOT:-}" ]]; then
  chown -R tester:tester "${TEST_HOME}" /app/dist 2>/dev/null || chmod -R a+rwx "${TEST_HOME}" /app/dist
  export CC_SWITCH_DROPPED_ROOT=1
  export HOME=/home/tester
  export USER=tester
  export LOGNAME=tester
  exec runuser -u tester --preserve-environment -- "$0" "$@"
fi

# Isolation is CC_SWITCH_TEST_HOME (see src-tauri/src/config.rs). Do not
# rewrite HOME here: cargo fingerprints HOME and would rebuild the crate.
export CC_SWITCH_TEST_HOME="${TEST_HOME}"
unset USERPROFILE || true

THREADS="${CARGO_TEST_THREADS:-1}"
FILTER="${TEST_FILTER:-all}"

cd /app

run_cargo_test() {
  cargo test --offline --locked --no-fail-fast --manifest-path src-tauri/Cargo.toml "$@" -- --test-threads="${THREADS}"
}

if [[ $# -gt 0 ]]; then
  exec "$@"
fi

echo "docker-backend-self-test: TEST_FILTER=${FILTER} CARGO_TEST_THREADS=${THREADS} SCHEMA_VERSION=18"

case "${FILTER}" in
  all|"*"|"")
    # Default: full crate (lib + integration + doc tests).
    run_cargo_test
    ;;
  proxy|proxy_projection_linux)
    run_cargo_test --test proxy_projection_linux
    ;;
  pi-crud|pi_crud|pi-crud-linux)
    # #46 Linux stand-in Pi CRUD + fetch_models / live-update lib tests.
    run_cargo_test --test proxy_projection_linux linux_standin_create
    run_cargo_test --test proxy_projection_linux linux_standin_update
    run_cargo_test --test proxy_projection_linux linux_standin_delete
    run_cargo_test --lib fetch_models_
    run_cargo_test --lib live_update_writes_url
    run_cargo_test --lib malformed_takeover_backup
    ;;
  lib-lite|lite)
    # Lightweight compose profile: fetch_models_ + key Pi CRUD lib tests.
    run_cargo_test --lib fetch_models_
    run_cargo_test --lib live_update_writes_url
    run_cargo_test --lib malformed_takeover_backup
    run_cargo_test --lib linux_standin_agent_dir
    ;;
  *)
    # cargo treats a single positional as a test-name substring filter.
    # shellcheck disable=SC2086
    run_cargo_test ${FILTER}
    ;;
esac
