#!/usr/bin/env bash
# Require bilingual-zh release notes with 修复的问题 / 优化 / 自测 before publish.
set -euo pipefail

TAG="${1:?usage: compose-release-notes.sh <tag> [out]}"
OUT="${2:-release-notes.md}"
NOTES="docs/release-notes/${TAG}.md"

if [ ! -f "$NOTES" ]; then
  echo "Refusing to publish ${TAG}: missing ${NOTES} with 修复的问题 / 优化 / 自测." >&2
  exit 1
fi
for heading in '修复的问题' '优化' '自测'; do
  if ! grep -q "$heading" "$NOTES"; then
    echo "Refusing to publish ${TAG}: ${NOTES} must contain ## ${heading}." >&2
    exit 1
  fi
done
cp "$NOTES" "$OUT"
if ! grep -q '下载' "$OUT"; then
  {
    echo
    echo "## 下载"
    echo
    echo "- **macOS（推荐）**：\`CC-Switch-${TAG}-macOS.dmg\` 或 \`CC-Switch-${TAG}-macOS.zip\`"
    echo "- **Windows x64 绿色版**：\`CC-Switch-${TAG}-Windows-Portable.zip\`"
    echo "- **Windows x64 安装包**：\`CC-Switch-${TAG}-Windows.msi\`"
  } >> "$OUT"
fi
cat "$OUT"
