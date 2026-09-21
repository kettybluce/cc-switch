#!/usr/bin/env bash
# CI-safe syntax checks: bash -n on scripts + extracted GHA bash, then actionlint.
# Does not build Windows MSI/Portable and does not bump versions.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

ACTIONLINT_VERSION="${ACTIONLINT_VERSION:-1.7.12}"
# linux_amd64 tarball from https://github.com/rhysd/actionlint/releases/tag/v1.7.12
ACTIONLINT_SHA256="${ACTIONLINT_SHA256:-8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8}"
REQUIRE_ACTIONLINT="${REQUIRE_ACTIONLINT:-1}"

fail=0

echo "==> python3 -m py_compile packager / extractor"
python3 -m py_compile scripts/package-windows-release.py scripts/ci/extract_gha_bash.py

echo "==> bash -n scripts/**/*.sh"
while IFS= read -r -d '' f; do
  echo "  bash -n $f"
  if ! bash -n "$f"; then
    fail=1
  fi
done < <(find scripts -type f -name '*.sh' -print0 | sort -z)

echo "==> bash -n GitHub Actions bash run blocks"
if ! python3 scripts/ci/extract_gha_bash.py --check --root "$ROOT"; then
  fail=1
fi

echo "==> actionlint v${ACTIONLINT_VERSION} (checksum-pinned)"
workdir="$(mktemp -d)"
cleanup() { rm -rf "$workdir"; }
trap cleanup EXIT

archive="actionlint_${ACTIONLINT_VERSION}_linux_amd64.tar.gz"
base="https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}"
if curl -fsSL -o "${workdir}/${archive}" "${base}/${archive}"; then
  got="$(sha256sum "${workdir}/${archive}" | awk '{print $1}')"
  if [ "$got" != "$ACTIONLINT_SHA256" ]; then
    echo "actionlint checksum mismatch: expected ${ACTIONLINT_SHA256} got ${got}" >&2
    if [ "$REQUIRE_ACTIONLINT" = "1" ]; then
      fail=1
    fi
  else
    tar -xzf "${workdir}/${archive}" -C "$workdir" actionlint
    # -shellcheck= / -pyflakes= : do not require those tools on the runner; bash -n covers syntax.
    # Ignore two upstream YAML features actionlint 1.7.12 does not model yet:
    #   - actions/stale@v10 exempt-issue-created-after
    #   - concurrency.queue (sync-r2.yml). Removing it would change serialize-on-release behavior.
    if ! "${workdir}/actionlint" -color -shellcheck= -pyflakes= \
      -ignore 'exempt-issue-created-after' \
      -ignore 'unexpected key "queue" for "concurrency"'; then
      fail=1
    fi
  fi
else
  echo "Failed to download actionlint ${ACTIONLINT_VERSION}" >&2
  if [ "$REQUIRE_ACTIONLINT" = "1" ]; then
    fail=1
  fi
fi

if [ "$fail" -ne 0 ]; then
  echo "workflow/shell syntax checks failed" >&2
  exit 1
fi
echo "workflow/shell syntax checks passed"
