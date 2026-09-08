#!/usr/bin/env bash
# Cloud Agent bootstrap for CC Switch.
#
# CC Switch is a Tauri 2 desktop app: a React + TypeScript frontend (Vite,
# pnpm) and a Rust backend. This script prepares a checked-out working copy so
# `pnpm dev` / `pnpm tauri dev`, `pnpm build`, the frontend tests, and the
# `cargo` toolchain all work.
#
# It is intentionally idempotent: it can run repeatedly and against cached or
# prebuilt state (e.g. an environment build / snapshot that already contains the
# system libraries and toolchains) without doing redundant work.
set -euo pipefail

# Run apt via sudo only when we are not already root.
SUDO=""
if [ "$(id -u)" -ne 0 ]; then
  SUDO="sudo"
fi

# 1) System libraries required to compile and run the Tauri app on Linux
#    (WebKitGTK webview, GTK, libsoup, appindicator tray, OpenSSL, and the
#    build/link tooling). Skipped quickly when they are already installed.
if ! pkg-config --exists webkit2gtk-4.1 gtk+-3.0 libsoup-3.0 2>/dev/null; then
  export DEBIAN_FRONTEND=noninteractive
  $SUDO apt-get update
  $SUDO apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libssl-dev \
    libgtk-3-dev \
    librsvg2-dev \
    libayatana-appindicator3-dev \
    libwebkit2gtk-4.1-dev \
    libsoup-3.0-dev \
    curl \
    wget \
    file \
    patchelf \
    xdg-utils
fi

# 2) Frontend dependencies, using the pnpm version pinned in package.json's
#    "packageManager" field (installed through Corepack, matching CI).
export COREPACK_ENABLE_DOWNLOAD_PROMPT=0
corepack enable
corepack install
pnpm install --frozen-lockfile

echo "CC Switch cloud-agent environment ready."
