# OPTIONAL. Does not access a user's Windows PC, machineId, or real Ubuntu-22.04.
#
# The Pi WSL remote-control path is exercised on Linux with LocalBashRunner +
# tests/fixtures/pi-wsl (see docs/dev-pi-wsl-test-harness-zh.md). This script
# is only a convenience wrapper if you already have bash/cargo on Windows.
#
# Prefer Linux:
#   cargo test --manifest-path src-tauri/Cargo.toml --lib wsl_linux_harness
#   pnpm test:unit tests/pi-wsl-harness.test.ts

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

Write-Host "optional-run-pi-wsl-harness: Linux-cloud harness via cargo (no real WSL distro)."
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo is not on PATH. Run the Linux commands in docs/dev-pi-wsl-test-harness-zh.md instead."
}

Set-Location $root
cargo test --manifest-path src-tauri/Cargo.toml --lib wsl_linux_harness -- --nocapture
