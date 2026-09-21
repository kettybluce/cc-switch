#!/usr/bin/env bash
# Fabricate tiny Tauri Windows outputs and run the real packager.
# Proves MSI/Portable asset names + zip layout without a version bump or GHA windows-2022.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TAG="${1:-v0.0.0-dryrun}"
STAGE="$(mktemp -d)"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

WORKDIR="${STAGE}/work"
mkdir -p "${WORKDIR}/src-tauri/target/release/bundle/msi"
mkdir -p "${WORKDIR}/scripts"
cp "${ROOT}/scripts/package-windows-release.py" "${WORKDIR}/scripts/"
# Dummy payloads are small on purpose; --min-bytes 16 still rejects empty files.
printf 'dummy-msi-xxxxxxxxxxxxxxxxxxxxxxxx' \
  > "${WORKDIR}/src-tauri/target/release/bundle/msi/CC-Switch_0.0.0_x64_en-US.msi"
printf 'dummy-exe-xxxxxxxxxxxxxxxxxxxxxxxx' \
  > "${WORKDIR}/src-tauri/target/release/cc-switch.exe"

(
  cd "$WORKDIR"
  python3 scripts/package-windows-release.py --version "$TAG" --min-bytes 16
)

bash "${ROOT}/scripts/ci/assert-windows-release-assets.sh" "$TAG" "${WORKDIR}/release-assets" 16

echo "OK Windows Portable packaging dry-run for ${TAG}"
echo "  ${WORKDIR}/release-assets/CC-Switch-${TAG}-Windows.msi"
echo "  ${WORKDIR}/release-assets/CC-Switch-${TAG}-Windows-Portable.zip"
