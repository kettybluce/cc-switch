// drop-saviour-model.mjs -- remove the duplicate gpt-6-astra from the saviour Pi card
import { DatabaseSync } from "node:sqlite";

const db = new DatabaseSync(process.argv[2]);

const row = db
  .prepare("SELECT settings_config FROM providers WHERE app_type='pi' AND id='saviour'")
  .get();
if (!row) {
  console.log("saviour card not found");
  process.exit(0);
}

const cfg = JSON.parse(row.settings_config);
const before = (cfg.models || []).map((m) => m.id || m.name);

// 保留 Key / baseUrl（随时可恢复），只清掉重复的模型列表
cfg.models = [];

db.prepare("UPDATE providers SET settings_config=? WHERE app_type='pi' AND id='saviour'").run(
  JSON.stringify(cfg)
);

const after = JSON.parse(
  db.prepare("SELECT settings_config FROM providers WHERE app_type='pi' AND id='saviour'").get()
    .settings_config
).models;

console.log("saviour models before:", JSON.stringify(before));
console.log("saviour models after :", JSON.stringify(after));
console.log("apiKey still kept    :", cfg.apiKey ? "yes" : "no");
db.close();
