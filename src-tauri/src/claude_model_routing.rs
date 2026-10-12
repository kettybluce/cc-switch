//! Claude Code 跨供应商模型目录路由
//!
//! 维护 clientModel → (providerId, upstreamModel) 目录，投影到 Claude
//! `settings.json` 的 `modelPicker`，并在本地代理上按请求 `model` 选中目标供应商。
//!
//! 存储：settings KV `claude_model_routing`（无 SCHEMA 变更）。
//! clientModel 默认格式：`{providerId}/{upstreamModel}`（碰撞安全、稳定）。

use crate::proxy::model_mapper::strip_one_m_suffix_for_upstream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const SETTINGS_KEY: &str = "claude_model_routing";

/// 一条目录条目：Claude Code `/model` 可见的 clientModel，映射到真实上游。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeModelRoutingEntry {
    /// Claude Code 客户端看到的 model 字符串（发往代理 body.model）
    pub client_model: String,
    /// 目标 Claude 供应商 id
    pub provider_id: String,
    /// 发往上游的真实 model id
    pub upstream_model: String,
    /// modelPicker 显示名
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// modelPicker 描述
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// 全局 Claude 跨供应商模型路由配置（settings KV）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeModelRoutingConfig {
    /// 总开关；默认关闭，关闭时行为与 v3.20.10 完全一致
    #[serde(default)]
    pub enabled: bool,
    /// 是否用自定义 options 替换内置 sonnet/opus/…；默认 false（保留内置）
    #[serde(default)]
    pub replace_built_in_options: bool,
    /// 是否同时写 `availableModels`（辅）；默认 false（一期仅 modelPicker）
    #[serde(default)]
    pub write_available_models: bool,
    /// 预留：gateway discovery（一期不实现 Anthropic `/v1/models`）
    #[serde(default)]
    pub enable_gateway_discovery: bool,
    /// 目录条目
    #[serde(default)]
    pub entries: Vec<ClaudeModelRoutingEntry>,
}

/// 解析命中结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedClaudeModelRoute {
    pub provider_id: String,
    pub upstream_model: String,
    pub client_model: String,
}

/// 默认 clientModel：`{providerId}/{upstreamModel}`。
///
/// 选用 providerId 而非 display name：id 稳定、全局唯一，避免改名或重名碰撞。
pub fn default_client_model(provider_id: &str, upstream_model: &str) -> String {
    let pid = provider_id.trim();
    let model = upstream_model.trim();
    if pid.is_empty() || model.is_empty() {
        return String::new();
    }
    format!("{pid}/{model}")
}

/// 规范化用于匹配的 model：trim + 剥离 Claude Code 的 `[1M]` 本地能力标记。
pub fn normalize_request_model(model: &str) -> String {
    strip_one_m_suffix_for_upstream(model.trim())
        .trim()
        .to_string()
}

/// 在目录中查找 request_model；功能关闭或空 model 时返回 None。
pub fn resolve_claude_model_route(
    config: &ClaudeModelRoutingConfig,
    request_model: &str,
) -> Option<ResolvedClaudeModelRoute> {
    if !config.enabled {
        return None;
    }
    let needle = normalize_request_model(request_model);
    if needle.is_empty() || needle == "unknown" {
        return None;
    }

    config.entries.iter().find_map(|entry| {
        let client = normalize_request_model(&entry.client_model);
        if client.is_empty() || !client.eq_ignore_ascii_case(&needle) {
            return None;
        }
        let provider_id = entry.provider_id.trim();
        let upstream = entry.upstream_model.trim();
        if provider_id.is_empty() || upstream.is_empty() {
            return None;
        }
        Some(ResolvedClaudeModelRoute {
            provider_id: provider_id.to_string(),
            upstream_model: upstream.to_string(),
            client_model: entry.client_model.trim().to_string(),
        })
    })
}

/// 将目录投影到 Claude settings.json 根对象的 `modelPicker`（+ 可选 `availableModels`）。
///
/// - `enabled == false` 或条目为空：清除本功能写入的字段
/// - `enabled == true`：写入 `modelPicker.options`，**不**默认 `replaceBuiltInOptions`
pub fn project_or_clear_model_picker(config_json: &mut Value, routing: &ClaudeModelRoutingConfig) {
    if !config_json.is_object() {
        *config_json = json!({});
    }
    let root = config_json
        .as_object_mut()
        .expect("claude settings root must be object");

    if !routing.enabled || routing.entries.is_empty() {
        root.remove("modelPicker");
        // 仅在我们可能写过时清理；用户自有 availableModels 会在接管备份中保留，
        // 关闭接管后由 proxy_live_backup 还原。接管期间这些键由本功能托管。
        root.remove("availableModels");
        return;
    }

    let options: Vec<Value> = routing
        .entries
        .iter()
        .filter(|e| !e.client_model.trim().is_empty())
        .map(|e| {
            let label = if e.label.trim().is_empty() {
                e.client_model.trim().to_string()
            } else {
                e.label.trim().to_string()
            };
            let mut opt = json!({
                "value": e.client_model.trim(),
                "label": label,
            });
            if !e.description.trim().is_empty() {
                opt["description"] = json!(e.description.trim());
            }
            opt
        })
        .collect();

    if options.is_empty() {
        root.remove("modelPicker");
        root.remove("availableModels");
        return;
    }

    root.insert(
        "modelPicker".to_string(),
        json!({
            "options": options,
            "replaceBuiltInOptions": routing.replace_built_in_options,
        }),
    );

    if routing.write_available_models {
        let mut models: Vec<Value> = routing
            .entries
            .iter()
            .filter(|e| !e.client_model.trim().is_empty())
            .map(|e| json!(e.client_model.trim()))
            .collect();
        for alias in ["sonnet", "opus", "haiku", "fable", "default"] {
            if !models.iter().any(|m| m.as_str() == Some(alias)) {
                models.push(json!(alias));
            }
        }
        root.insert("availableModels".to_string(), Value::Array(models));
    } else {
        root.remove("availableModels");
    }
}

/// 目录命中后把 body.model 改写为 upstreamModel（跳过角色折叠映射）。
pub fn rewrite_body_model_to_upstream(mut body: Value, upstream_model: &str) -> Value {
    let trimmed = upstream_model.trim();
    if !trimmed.is_empty() {
        body["model"] = json!(trimmed);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_config() -> ClaudeModelRoutingConfig {
        ClaudeModelRoutingConfig {
            enabled: true,
            replace_built_in_options: false,
            write_available_models: false,
            enable_gateway_discovery: false,
            entries: vec![
                ClaudeModelRoutingEntry {
                    client_model: "prov_deepseek/deepseek-chat".to_string(),
                    provider_id: "prov_deepseek".to_string(),
                    upstream_model: "deepseek-chat".to_string(),
                    label: "DeepSeek Chat".to_string(),
                    description: "via DeepSeek".to_string(),
                },
                ClaudeModelRoutingEntry {
                    client_model: "other/claude-sonnet-4".to_string(),
                    provider_id: "other".to_string(),
                    upstream_model: "claude-sonnet-4".to_string(),
                    label: String::new(),
                    description: String::new(),
                },
            ],
        }
    }

    #[test]
    fn default_client_model_uses_provider_id() {
        assert_eq!(default_client_model("prov_a", "gpt-4o"), "prov_a/gpt-4o");
        assert_eq!(default_client_model("  ", "x"), "");
    }

    #[test]
    fn resolve_exact_hit() {
        let cfg = sample_config();
        let hit = resolve_claude_model_route(&cfg, "prov_deepseek/deepseek-chat").unwrap();
        assert_eq!(hit.provider_id, "prov_deepseek");
        assert_eq!(hit.upstream_model, "deepseek-chat");
    }

    #[test]
    fn resolve_strips_one_m_marker() {
        let cfg = sample_config();
        let hit = resolve_claude_model_route(&cfg, "prov_deepseek/deepseek-chat[1m]").unwrap();
        assert_eq!(hit.upstream_model, "deepseek-chat");
    }

    #[test]
    fn resolve_case_insensitive_client_model() {
        let cfg = sample_config();
        let hit = resolve_claude_model_route(&cfg, "PROV_DEEPSEEK/DeepSeek-Chat").unwrap();
        assert_eq!(hit.provider_id, "prov_deepseek");
    }

    #[test]
    fn resolve_miss_and_disabled() {
        let mut cfg = sample_config();
        assert!(resolve_claude_model_route(&cfg, "sonnet").is_none());
        assert!(resolve_claude_model_route(&cfg, "unknown").is_none());
        cfg.enabled = false;
        assert!(resolve_claude_model_route(&cfg, "prov_deepseek/deepseek-chat").is_none());
    }

    #[test]
    fn project_model_picker_writes_options_keeps_builtins() {
        let cfg = sample_config();
        let mut live = json!({"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:15721"}});
        project_or_clear_model_picker(&mut live, &cfg);
        let picker = live.get("modelPicker").unwrap();
        assert_eq!(picker["replaceBuiltInOptions"], json!(false));
        let options = picker["options"].as_array().unwrap();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0]["value"], "prov_deepseek/deepseek-chat");
        assert_eq!(options[0]["label"], "DeepSeek Chat");
        assert_eq!(options[0]["description"], "via DeepSeek");
        assert_eq!(options[1]["label"], "other/claude-sonnet-4"); // empty label → clientModel
        assert!(live.get("availableModels").is_none());
    }

    #[test]
    fn project_clears_when_disabled() {
        let mut live = json!({
            "modelPicker": {"options": [{"value": "x"}]},
            "availableModels": ["x"],
            "env": {}
        });
        project_or_clear_model_picker(&mut live, &ClaudeModelRoutingConfig::default());
        assert!(live.get("modelPicker").is_none());
        assert!(live.get("availableModels").is_none());
        assert!(live.get("env").is_some());
    }

    #[test]
    fn project_available_models_when_enabled() {
        let mut cfg = sample_config();
        cfg.write_available_models = true;
        let mut live = json!({});
        project_or_clear_model_picker(&mut live, &cfg);
        let models = live["availableModels"].as_array().unwrap();
        assert!(models
            .iter()
            .any(|m| m.as_str() == Some("prov_deepseek/deepseek-chat")));
        assert!(models.iter().any(|m| m.as_str() == Some("sonnet")));
    }

    #[test]
    fn rewrite_body_model() {
        let body = json!({"model": "prov_a/x", "messages": []});
        let out = rewrite_body_model_to_upstream(body, "deepseek-chat");
        assert_eq!(out["model"], "deepseek-chat");
    }

    #[test]
    fn settings_kv_round_trip_shape() {
        let cfg = sample_config();
        let json_str = serde_json::to_string(&cfg).unwrap();
        let back: ClaudeModelRoutingConfig = serde_json::from_str(&json_str).unwrap();
        assert_eq!(back, cfg);
        // camelCase keys
        let v: Value = serde_json::from_str(&json_str).unwrap();
        assert!(v.get("replaceBuiltInOptions").is_some());
        assert!(v.get("clientModel").is_none());
        assert!(v["entries"][0].get("clientModel").is_some());
    }

    #[test]
    fn default_is_disabled() {
        let cfg = ClaudeModelRoutingConfig::default();
        assert!(!cfg.enabled);
        assert!(!cfg.replace_built_in_options);
        assert!(cfg.entries.is_empty());
    }
}
