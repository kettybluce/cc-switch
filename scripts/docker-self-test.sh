#!/usr/bin/env bash
# Host wrapper: build the backend self-test image, run FULL src-tauri cargo
# tests by default, and emit a Chinese markdown report to stdout + a file.
#
#   pnpm test:docker
#   bash scripts/docker-self-test.sh
#   TEST_FILTER=pi-crud bash scripts/docker-self-test.sh
#
# SCHEMA stays 18. Does not run the Tauri GUI. Does not write Windows C:.
# Does not ship Portable / MSI.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

FILTER="${TEST_FILTER:-all}"
THREADS="${CARGO_TEST_THREADS:-1}"
REPORT_DIR="${REPORT_DIR:-${ROOT}/docs/self-test-reports}"
IMAGE_NAME="${IMAGE_NAME:-cc-switch-backend-self-test:local}"
COMPOSE=(docker compose -f docker-compose.test.yml)
STAMP="$(date -u +%Y%m%d-%H%M)"
LOG_PATH="${REPORT_DIR}/docker-self-test-${STAMP}.log"
REPORT_PATH="${REPORT_DIR}/docker-self-test-${STAMP}.md"
RENDER="${ROOT}/scripts/render-docker-self-test-report.py"

mkdir -p "${REPORT_DIR}" /tmp/cc-switch-test-home 2>/dev/null || true

SHA="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
VERSION="$(python3 -c 'import json; print(json.load(open("package.json"))["version"])' 2>/dev/null || echo unknown)"
SCHEMA="$(
  python3 - <<'PY'
import re, pathlib
text = pathlib.Path("src-tauri/src/database/mod.rs").read_text(encoding="utf-8")
m = re.search(r"pub const SCHEMA_VERSION: i32 = (\d+);", text)
print(m.group(1) if m else "unknown")
PY
)"

if [[ "${SCHEMA}" != "18" ]]; then
  echo "error: SCHEMA_VERSION must stay 18 (found ${SCHEMA})" >&2
  exit 2
fi

have_docker() {
  command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1
}

COMMAND="pnpm test:docker"
if [[ "${FILTER}" != "all" ]]; then
  COMMAND="TEST_FILTER=${FILTER} pnpm test:docker"
fi

RUNNER="docker compose (Ubuntu 22.04 image, no Tauri GUI)"
IMAGE_ID="unavailable"
EXIT_CODE=0
START_TS="$(date +%s)"

run_docker() {
  echo "docker-self-test: default=full TEST_FILTER=${FILTER} SCHEMA=${SCHEMA} sha=${SHA}"
  echo "log: ${LOG_PATH}"
  set +e
  "${COMPOSE[@]}" run --build --rm \
    -e "TEST_FILTER=${FILTER}" \
    -e "CARGO_TEST_THREADS=${THREADS}" \
    backend-self-test 2>&1 | tee "${LOG_PATH}"
  EXIT_CODE="${PIPESTATUS[0]}"
  set -e
  IMAGE_ID="$(docker image inspect "${IMAGE_NAME}" --format '{{.Id}}' 2>/dev/null || echo unavailable)"
}

host_cargo() {
  # Append cargo output to the log; keep the worst non-zero status.
  set +e
  cargo test --locked --no-fail-fast --manifest-path src-tauri/Cargo.toml "$@" -- --test-threads="${THREADS}" 2>&1 | tee -a "${LOG_PATH}"
  local pipe_rc="${PIPESTATUS[0]}"
  set -e
  if [[ "${pipe_rc}" -ne 0 ]]; then
    EXIT_CODE="${pipe_rc}"
  fi
}

run_host_fallback() {
  RUNNER="host cargo test (no Docker in this VM; CC_SWITCH_SELFTEST_HOST_FALLBACK=1). Equivalent to TEST_FILTER=${FILTER} inside Dockerfile.test."
  COMMAND="CC_SWITCH_SELFTEST_HOST_FALLBACK=1 TEST_FILTER=${FILTER} bash scripts/docker-self-test.sh"
  echo "docker-self-test: Docker unavailable; host fallback SCHEMA=${SCHEMA} sha=${SHA}" >&2
  echo "log: ${LOG_PATH}"
  mkdir -p dist
  export CC_SWITCH_TEST_HOME="${CC_SWITCH_TEST_HOME:-/tmp/cc-switch-test-home}"
  mkdir -p "${CC_SWITCH_TEST_HOME}"
  unset USERPROFILE || true
  : > "${LOG_PATH}"
  EXIT_CODE=0
  case "${FILTER}" in
    all|"*"|"")
      host_cargo
      ;;
    proxy|proxy_projection_linux)
      host_cargo --test proxy_projection_linux
      ;;
    session_usage_scan|usage-scan|usage_scan)
      host_cargo --test session_usage_scan
      ;;
    pi-crud|pi_crud|pi-crud-linux)
      host_cargo --test proxy_projection_linux linux_standin_create
      host_cargo --test proxy_projection_linux linux_standin_update
      host_cargo --test proxy_projection_linux linux_standin_delete
      host_cargo --lib fetch_models_
      host_cargo --lib live_update_writes_url
      host_cargo --lib malformed_takeover_backup
      ;;
    lib-lite|lite)
      host_cargo --lib fetch_models_
      host_cargo --lib live_update_writes_url
      host_cargo --lib malformed_takeover_backup
      host_cargo --lib linux_standin_agent_dir
      ;;
    *)
      # shellcheck disable=SC2086
      host_cargo ${FILTER}
      ;;
  esac
}

if have_docker; then
  run_docker
elif [[ "${CC_SWITCH_SELFTEST_HOST_FALLBACK:-}" == "1" ]]; then
  run_host_fallback
else
  echo "error: docker compose is required. Install Docker, or set CC_SWITCH_SELFTEST_HOST_FALLBACK=1 to run the equivalent cargo test on the host and still emit a report." >&2
  exit 127
fi

END_TS="$(date +%s)"
DURATION="$((END_TS - START_TS))"

python3 "${RENDER}" \
  --log "${LOG_PATH}" \
  --out "${REPORT_PATH}" \
  --sha "${SHA}" \
  --version "${VERSION}" \
  --schema "${SCHEMA}" \
  --image "${IMAGE_NAME}" \
  --image-id "${IMAGE_ID}" \
  --command "${COMMAND}" \
  --filter "${FILTER}" \
  --runner "${RUNNER}" \
  --duration-sec "${DURATION}" \
  --exit-code "${EXIT_CODE}" \
  --reports-dir "${REPORT_DIR}" \
  --print

echo "REPORT_PATH=${REPORT_PATH}"
exit "${EXIT_CODE}"
