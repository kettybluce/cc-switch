// build-pi-models.mjs
// Rebuild ~/.pi/agent/models.json from the CC Switch DB Pi provider catalog.
// Run with the takeover temporarily OFF so the app does not fight us.
import { DatabaseSync } from "node:sqlite";
import fs from "node:fs";
import path from "node:path";

const dbPath = process.argv[2];
const outPath = process.argv[3];
const mode = process.argv[4] || "real"; // real | proxy

const db = new DatabaseSync(dbPath, { readOnly: true });
const rows = db
  .prepare(
    "SELECT id, name, settings_config FROM providers WHERE app_type='pi' ORDER BY rowid"
  )
  .all();

const providers = {};
for (const r of rows) {
  let cfg;
  try {
    cfg = JSON.parse(r.settings_config);
  } catch {
    console.error("skip unparsable:", r.id);
    continue;
  }
  if (!cfg || typeof cfg !== "object") continue;

  // 跳过没有可用模型的卡片：Pi 会把它当成一个"存在但没模型"的 provider，
  // 既污染列表，也会在切换时报 provider 未配置模型。
  const modelCount = Array.isArray(cfg.models) ? cfg.models.filter(Boolean).length : 0;
  if (modelCount === 0) {
    console.error(`skip ${r.id}: no models`);
    continue;
  }

  // Preserve the native Pi schema; only guarantee the display name.
  const node = { ...cfg };
  if (!node.name) node.name = r.name;
  if (!node.api) node.api = "openai-completions";

  // Normalize developer-role compat for strict OpenAI-compatible gateways.
  if (node.api === "openai-completions") {
    node.compat = { supportsDeveloperRole: false, ...(node.compat || {}) };
    if (Array.isArray(node.models)) {
      node.models = node.models.map((m) =>
        m && typeof m === "object"
          ? { ...m, compat: { supportsDeveloperRole: false, ...(m.compat || {}) } }
          : m
      );
    }
  }

  if (mode === "proxy") {
    node.baseUrl =
      node.api === "openai-completions" || node.api === "openai-responses"
        ? "http://127.0.0.1:15721/v1"
        : "http://127.0.0.1:15721";
    node.apiKey = "PROXY_MANAGED";
    node.headers = { ...(node.headers || {}), "x-cc-switch-app": "pi" };
    if (Array.isArray(node.models)) {
      node.models = node.models.map((m) =>
        m && typeof m === "object" && "baseUrl" in m
          ? { ...m, baseUrl: node.baseUrl }
          : m
      );
    }
  }

  providers[r.id] = node;
}

const doc = { providers };
fs.writeFileSync(outPath, JSON.stringify(doc, null, 2) + "\n", "utf8");

console.log("wrote", outPath, "mode=" + mode);
console.log("providers:", Object.keys(providers).length);
for (const [id, v] of Object.entries(providers)) {
  console.log(
    `  ${id.padEnd(10)} api=${String(v.api).padEnd(20)} baseUrl=${v.baseUrl} models=${(v.models || []).length}`
  );
}
