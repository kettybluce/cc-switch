// restore-pi-cards.mjs -- 把投影（PROXY_MANAGED / 127.0.0.1）写回成真实配置
// 起因：投影后的 models.json 被同步回 DB，覆盖了卡片的真实 baseUrl / apiKey，
// 导致 pi_provider_is_forwardable 认为所有卡片都不可转发（零候选 → 503）。
import { DatabaseSync } from "node:sqlite";

const db = new DatabaseSync(process.argv[2]);

/** @type {Record<string, {baseUrl:string, api:string, apiKey:string, models:string[]}>} */
const REAL = {
  fengwind: {
    baseUrl: "https://api.fengwind.com",
    api: "anthropic-messages",
    apiKey: "REDACTED_FENGWIND_KEY",
    models: ["deepseek-v4.1-flash", "mimo-v2.6-flash"],
  },
  baisheng: {
    baseUrl: "http://internal.example/v1",
    api: "openai-completions",
    apiKey: "REDACTED_BAISHENG_KEY",
    models: ["deepseek/yumcode-lite", "openai/yumcode-std", "z-ai/yumcode-pro"],
  },
  "42": {
    baseUrl: "https://api.42x.shop/",
    api: "openai-completions",
    apiKey: "REDACTED_42X_KEY",
    models: ["deepseek-v4.1-flash", "glm-5.3-flash"],
  },
  amd: {
    baseUrl: "https://developer.amd.com.cn/radeon/api/v1",
    api: "openai-completions",
    apiKey: "REDACTED_AMD_KEY",
    models: ["MiMo-V2.6-Flash", "DeepSeek-V4.1-Flash", "Qwen3.8-Flash-Next"],
  },
  grok: {
    baseUrl: "https://windhub.cc",
    api: "openai-completions",
    apiKey: "REDACTED_GROK_KEY",
    models: ["gpt-6-luna", "qwen3.8-flash-next", "kimi-k3", "voxtral-small-latest"],
  },
  elysiver: {
    baseUrl: "https://elysia.h-e.top/v1",
    api: "openai-completions",
    apiKey: "REDACTED_ELYSIA_KEY",
    models: ["kimi-k3", "deepseek-v4.1-flash"],
  },
  cpa: {
    baseUrl: "http://REDACTED_VPS_HOST:8317/v1",
    api: "openai-completions",
    apiKey: "REDACTED_CPA_KEY",
    models: [
      "gpt-6-astra", "deepseek-v4-pro", "MiMo-V2.6-Flash", "Qwen3.8-27B",
      "glm-5.3", "glm-5.3-flash", "gpt-6-astra-cc-format",
      "claude-haiku-4-5-20251001", "kimi-k3", "grok-4.7",
      "voxtral-small-latest", "gpt-6-luna", "qwen3.8-flash-next",
    ],
  },
};

const rows = db.prepare("SELECT id, settings_config FROM providers WHERE app_type='pi'").all();
let fixed = 0;
for (const r of rows) {
  const real = REAL[r.id];
  if (!real) {
    console.log(`skip ${r.id}: 没有已知的真实配置`);
    continue;
  }
  let cfg;
  try {
    cfg = JSON.parse(r.settings_config);
  } catch {
    cfg = {};
  }
  // 只改被投影污染的字段，其余（display name / 未知字段）保留
  cfg.baseUrl = real.baseUrl;
  cfg.api = real.api;
  cfg.apiKey = real.apiKey;
  const existing = Array.isArray(cfg.models) ? cfg.models : [];
  const byId = new Map(existing.map((m) => [m && m.id, m]));
  cfg.models = real.models.map((id) => {
    const prev = byId.get(id);
    return prev ? { ...prev, id, name: prev.name || id } : { id, name: id };
  });
  db.prepare("UPDATE providers SET settings_config=? WHERE app_type='pi' AND id=?").run(
    JSON.stringify(cfg),
    r.id
  );
  fixed += 1;
  console.log(`fixed ${r.id}: ${real.baseUrl} models=${real.models.length}`);
}
console.log(`\nrestored ${fixed} / ${rows.length} cards`);
db.close();
