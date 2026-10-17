//! L3 集成测试：DSH 配置读写的真实链路验证。
//!
//! 这些测试**通过 `DSH_HOME` 环境变量指向真实目录**，验证
//! 「命令层调用的那套函数 → 真实文件 → DSH 能解析」的完整链路，
//! 而不只是内存里的逻辑。
//!
//! 默认 `#[ignore]`，需显式运行：
//! ```bash
//! DSH_HOME=/tmp/l3-dsh cargo test --lib l3_dsh -- --ignored --test-threads=1
//! ```
//!
//! 之所以默认忽略：它依赖外部环境（`DSH_HOME` 指向可写目录），
//! 不适合混在常规 `cargo test` 里。

#[cfg(test)]
mod tests {
    use crate::dsh_config;

    /// 取出测试用的 home；未设置 DSH_HOME 时跳过。
    fn home_or_skip() -> Option<std::path::PathBuf> {
        let raw = std::env::var("DSH_HOME").ok()?;
        if raw.trim().is_empty() {
            return None;
        }
        Some(std::path::PathBuf::from(raw.trim()))
    }

    fn patch_path(home: &std::path::Path) -> std::path::PathBuf {
        dsh_config::home_patch_path(home)
    }

    /// 写入一份含 2 个 provider + 1 个非 provider 条目的初始配置。
    fn seed(home: &std::path::Path) {
        let path = patch_path(home);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(
            &path,
            r#"- id: ui-chat
  name: "@deepseek-ai/dsh-client-ui-chat"
  config:
    transcriptView: verbose
- id: llm-pi-ai
  name: "@deepseek-ai/dsh-llm-pi-ai"
  config:
    providers:
      existing-a:
        displayName: Existing A
        apiKeyEnv: K_A
        api: openai-completions
        baseURL: http://a.example/v1
        models:
          - id: model-a
        customField: keep-me
      existing-b:
        displayName: Existing B
        apiKeyEnv: K_B
        api: openai-responses
        baseURL: http://b.example/v1
        models:
          - id: model-b
- id: agent-default-model
  name: "@deepseek-ai/dsh-agent-default-model"
  config:
    provider: existing-a
    model: model-a
"#,
        )
        .expect("seed");
    }

    fn provider_json(base_url: &str) -> serde_json::Value {
        serde_json::json!({
            "displayName": "New Card",
            "apiKeyEnv": "K_NEW",
            "api": "openai-completions",
            "baseURL": base_url,
            "models": [{ "id": "new-model" }]
        })
    }

    /// L3-B2：新增第 3 个 provider，原有 2 个必须都还在。
    /// 这是「providers 整体替换」事故的防线，也是本次测试最重要的一条。
    #[test]
    #[ignore = "requires DSH_HOME pointing at a writable dir"]
    fn l3_b2_adding_provider_keeps_existing_ones() {
        let Some(home) = home_or_skip() else {
            eprintln!("skip: DSH_HOME not set");
            return;
        };
        seed(&home);

        dsh_config::upsert_provider("new-card", &provider_json("http://new/v1"))
            .expect("upsert");

        let providers = dsh_config::read_providers().expect("read");
        assert_eq!(providers.len(), 3, "应为 3 个 provider，实际 {:?}", providers.keys());
        assert!(providers.contains_key("existing-a"), "原有 existing-a 丢失");
        assert!(providers.contains_key("existing-b"), "原有 existing-b 丢失");
        assert!(providers.contains_key("new-card"), "新 provider 未写入");
    }

    /// L3-B6 / L3-B7：未知字段与非 provider 条目必须原样保留。
    #[test]
    #[ignore = "requires DSH_HOME pointing at a writable dir"]
    fn l3_b6_b7_unknown_fields_and_entries_survive() {
        let Some(home) = home_or_skip() else {
            eprintln!("skip: DSH_HOME not set");
            return;
        };
        seed(&home);

        dsh_config::upsert_provider("new-card", &provider_json("http://new/v1"))
            .expect("upsert");

        let path = patch_path(&home);
        let entries = dsh_config::read_patch(&path).expect("read patch");

        // 非 provider 条目 ui-chat 仍在，且其 config 未被改动
        let ui = dsh_config::find_entry_index(&entries, "ui-chat").expect("ui-chat 丢失");
        assert_eq!(
            entries[ui]["config"]["transcriptView"].as_str(),
            Some("verbose"),
            "ui-chat 的配置被改动"
        );

        // 原有 provider 的自定义字段仍在
        let providers = dsh_config::providers_of(&entries).expect("providers");
        assert_eq!(
            providers["existing-a"]["customField"].as_str(),
            Some("keep-me"),
            "customField 被丢弃"
        );
    }

    /// L3-B4：切换默认模型后，agent-default-model 必须更新。
    #[test]
    #[ignore = "requires DSH_HOME pointing at a writable dir"]
    fn l3_b4_default_model_switch_persists() {
        let Some(home) = home_or_skip() else {
            eprintln!("skip: DSH_HOME not set");
            return;
        };
        seed(&home);

        dsh_config::write_default_model("existing-b", "model-b").expect("write default");

        let got = dsh_config::read_default_model().expect("read default");
        assert_eq!(
            got,
            Some(("existing-b".to_string(), "model-b".to_string()))
        );
    }

    /// L3-B5：删除一个 provider，其它必须保留。
    #[test]
    #[ignore = "requires DSH_HOME pointing at a writable dir"]
    fn l3_b5_removing_provider_keeps_others() {
        let Some(home) = home_or_skip() else {
            eprintln!("skip: DSH_HOME not set");
            return;
        };
        seed(&home);

        dsh_config::remove_provider("existing-a").expect("remove");

        let providers = dsh_config::read_providers().expect("read");
        assert!(!providers.contains_key("existing-a"), "目标未删除");
        assert!(providers.contains_key("existing-b"), "误删了其它 provider");
    }

    /// L3-B3：编辑卡片只改目标字段，不动别的。
    #[test]
    #[ignore = "requires DSH_HOME pointing at a writable dir"]
    fn l3_b3_editing_only_touches_target() {
        let Some(home) = home_or_skip() else {
            eprintln!("skip: DSH_HOME not set");
            return;
        };
        seed(&home);

        // 只换 baseURL，其余字段沿用
        let updated = serde_json::json!({
            "displayName": "Existing A",
            "apiKeyEnv": "K_A",
            "api": "openai-completions",
            "baseURL": "http://a-changed/v1",
            "models": [{ "id": "model-a" }]
        });
        dsh_config::upsert_provider("existing-a", &updated).expect("upsert");

        let providers = dsh_config::read_providers().expect("read");
        assert_eq!(
            providers["existing-a"]["baseURL"].as_str(),
            Some("http://a-changed/v1")
        );
        assert_eq!(
            providers["existing-b"]["baseURL"].as_str(),
            Some("http://b.example/v1"),
            "误改了其它 provider"
        );
    }

    /// L3-B1：patch 文件不存在时，首次写入能创建且结构正确。
    #[test]
    #[ignore = "requires DSH_HOME pointing at a writable dir"]
    fn l3_b1_creates_patch_from_scratch() {
        let Some(home) = home_or_skip() else {
            eprintln!("skip: DSH_HOME not set");
            return;
        };
        let path = patch_path(&home);
        let _ = std::fs::remove_file(&path);

        dsh_config::upsert_provider("first", &provider_json("http://first/v1"))
            .expect("upsert");

        let entries = dsh_config::read_patch(&path).expect("read");
        assert_eq!(
            dsh_config::find_entry_index(&entries, "llm-pi-ai"),
            Some(0),
            "应创建 llm-pi-ai 条目"
        );
        assert!(dsh_config::provider_exists("first").expect("exists"));
    }
}
