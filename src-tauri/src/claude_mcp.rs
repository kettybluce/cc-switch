use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{atomic_write, get_claude_mcp_path};
use crate::error::AppError;
use crate::mcp::windows_cmd::{is_wsl_path, strip_windows_cmd_wrapper, wrap_command_for_windows};

/// 需要在 Windows 上用 cmd /c 包装的命令 —— 迁移到 `mcp::windows_cmd`。
///
/// 包装/解包是一对互逆操作，必须放在一起维护：数据库统一存一份 spec，而
/// 投影时才决定目标形态。历史遗留的已包装条目在投影到 WSL 时必须被**解包**
/// （`strip_windows_cmd_wrapper`），只"跳过包装"是不够的 —— 那正是
/// `cmd /c npx …` 曾经被写进 WSL `~/.claude.json` 的原因。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub user_config_path: String,
    pub user_config_exists: bool,
    pub server_count: usize,
}

fn user_config_path() -> PathBuf {
    get_claude_mcp_path()
}

fn read_json_value(path: &Path) -> Result<Value, AppError> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let content = fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;
    let value: Value = serde_json::from_str(&content).map_err(|e| AppError::json(path, e))?;
    Ok(value)
}

fn write_json_value(path: &Path, value: &Value) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }
    let json =
        serde_json::to_string_pretty(value).map_err(|e| AppError::JsonSerialize { source: e })?;
    atomic_write(path, json.as_bytes())
}

pub fn get_mcp_status() -> Result<McpStatus, AppError> {
    let path = user_config_path();
    let (exists, count) = if path.exists() {
        let v = read_json_value(&path)?;
        let servers = v.get("mcpServers").and_then(|x| x.as_object());
        (true, servers.map(|m| m.len()).unwrap_or(0))
    } else {
        (false, 0)
    };

    Ok(McpStatus {
        user_config_path: path.to_string_lossy().to_string(),
        user_config_exists: exists,
        server_count: count,
    })
}

pub fn read_mcp_json() -> Result<Option<String>, AppError> {
    let path = user_config_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    Ok(Some(content))
}

/// 在 ~/.claude.json 根对象写入 hasCompletedOnboarding=true（用于跳过 Claude Code 初次安装确认）
/// 仅增量写入该字段，其他字段保持不变
pub fn set_has_completed_onboarding() -> Result<bool, AppError> {
    let path = user_config_path();
    let mut root = if path.exists() {
        read_json_value(&path)?
    } else {
        serde_json::json!({})
    };

    let obj = root
        .as_object_mut()
        .ok_or_else(|| AppError::Config("~/.claude.json 根必须是对象".into()))?;

    let already = obj
        .get("hasCompletedOnboarding")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if already {
        return Ok(false);
    }

    obj.insert("hasCompletedOnboarding".into(), Value::Bool(true));
    write_json_value(&path, &root)?;
    Ok(true)
}

/// 删除 ~/.claude.json 根对象的 hasCompletedOnboarding 字段（恢复 Claude Code 初次安装确认）
/// 仅增量删除该字段，其他字段保持不变
pub fn clear_has_completed_onboarding() -> Result<bool, AppError> {
    let path = user_config_path();
    if !path.exists() {
        return Ok(false);
    }

    let mut root = read_json_value(&path)?;
    let obj = root
        .as_object_mut()
        .ok_or_else(|| AppError::Config("~/.claude.json 根必须是对象".into()))?;

    let existed = obj.remove("hasCompletedOnboarding").is_some();
    if !existed {
        return Ok(false);
    }

    write_json_value(&path, &root)?;
    Ok(true)
}

pub fn upsert_mcp_server(id: &str, spec: Value) -> Result<bool, AppError> {
    if id.trim().is_empty() {
        return Err(AppError::InvalidInput("MCP 服务器 ID 不能为空".into()));
    }
    // 基础字段校验（尽量宽松）
    if !spec.is_object() {
        return Err(AppError::McpValidation(
            "MCP 服务器定义必须为 JSON 对象".into(),
        ));
    }
    let t_opt = spec.get("type").and_then(|x| x.as_str());
    let is_stdio = t_opt.map(|t| t == "stdio").unwrap_or(true); // 兼容缺省（按 stdio 处理）
    let is_http = t_opt.map(|t| t == "http").unwrap_or(false);
    let is_sse = t_opt.map(|t| t == "sse").unwrap_or(false);
    if !(is_stdio || is_http || is_sse) {
        return Err(AppError::McpValidation(
            "MCP 服务器 type 必须是 'stdio'、'http' 或 'sse'（或省略表示 stdio）".into(),
        ));
    }

    // stdio 类型必须有 command
    if is_stdio {
        let cmd = spec.get("command").and_then(|x| x.as_str()).unwrap_or("");
        if cmd.is_empty() {
            return Err(AppError::McpValidation(
                "stdio 类型的 MCP 服务器缺少 command 字段".into(),
            ));
        }
    }

    // http/sse 类型必须有 url
    if is_http || is_sse {
        let url = spec.get("url").and_then(|x| x.as_str()).unwrap_or("");
        if url.is_empty() {
            return Err(AppError::McpValidation(if is_http {
                "http 类型的 MCP 服务器缺少 url 字段".into()
            } else {
                "sse 类型的 MCP 服务器缺少 url 字段".into()
            }));
        }
    }

    let path = user_config_path();
    let mut root = if path.exists() {
        read_json_value(&path)?
    } else {
        serde_json::json!({})
    };

    // 确保 mcpServers 对象存在
    {
        let obj = root
            .as_object_mut()
            .ok_or_else(|| AppError::Config("mcp.json 根必须是对象".into()))?;
        if !obj.contains_key("mcpServers") {
            obj.insert("mcpServers".into(), serde_json::json!({}));
        }
    }

    let before = root.clone();
    if let Some(servers) = root.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
        servers.insert(id.to_string(), spec);
    }

    if before == root && path.exists() {
        return Ok(false);
    }

    write_json_value(&path, &root)?;
    Ok(true)
}

pub fn delete_mcp_server(id: &str) -> Result<bool, AppError> {
    if id.trim().is_empty() {
        return Err(AppError::InvalidInput("MCP 服务器 ID 不能为空".into()));
    }
    let path = user_config_path();
    if !path.exists() {
        return Ok(false);
    }
    let mut root = read_json_value(&path)?;
    let Some(servers) = root.get_mut("mcpServers").and_then(|v| v.as_object_mut()) else {
        return Ok(false);
    };
    let existed = servers.remove(id).is_some();
    if !existed {
        return Ok(false);
    }
    write_json_value(&path, &root)?;
    Ok(true)
}

pub fn validate_command_in_path(cmd: &str) -> Result<bool, AppError> {
    if cmd.trim().is_empty() {
        return Ok(false);
    }
    // 如果包含路径分隔符，直接判断是否存在可执行文件
    if cmd.contains('/') || cmd.contains('\\') {
        return Ok(Path::new(cmd).exists());
    }

    let path_var = env::var_os("PATH").unwrap_or_default();
    let paths = env::split_paths(&path_var);

    #[cfg(windows)]
    let exts: Vec<String> = env::var("PATHEXT")
        .unwrap_or(".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .map(|s| s.trim().to_uppercase())
        .collect();

    for p in paths {
        let candidate = p.join(cmd);
        if candidate.is_file() {
            return Ok(true);
        }
        #[cfg(windows)]
        {
            for ext in &exts {
                let cand = p.join(format!("{}{}", cmd, ext));
                if cand.is_file() {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// 读取 ~/.claude.json 中的 mcpServers 映射
pub fn read_mcp_servers_map() -> Result<std::collections::HashMap<String, Value>, AppError> {
    let path = user_config_path();
    if !path.exists() {
        return Ok(std::collections::HashMap::new());
    }

    let root = read_json_value(&path)?;
    let servers = root
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    Ok(servers)
}

/// 将给定的启用 MCP 服务器映射写入到用户级 ~/.claude.json 的 mcpServers 字段
/// 仅覆盖 mcpServers，其他字段保持不变
pub fn set_mcp_servers_map(
    servers: &std::collections::HashMap<String, Value>,
) -> Result<(), AppError> {
    let path = user_config_path();
    let mut root = if path.exists() {
        read_json_value(&path)?
    } else {
        serde_json::json!({})
    };

    // 构建 mcpServers 对象：移除 UI 辅助字段（enabled/source），仅保留实际 MCP 规范
    // 检测目标路径是否为 WSL，若是则必须把历史遗留的 cmd /c 包装解开
    let is_wsl_target = is_wsl_path(&path);
    if is_wsl_target {
        log::info!(
            "检测到 WSL 路径，解包 cmd /c 包装（WSL 内不存在 cmd）: {}",
            path.display()
        );
    }
    let mut out: Map<String, Value> = Map::new();
    for (id, spec) in servers.iter() {
        let mut obj = if let Some(map) = spec.as_object() {
            map.clone()
        } else {
            return Err(AppError::McpValidation(format!(
                "MCP 服务器 '{id}' 不是对象"
            )));
        };

        if let Some(server_val) = obj.remove("server") {
            let server_obj = server_val.as_object().cloned().ok_or_else(|| {
                AppError::McpValidation(format!("MCP 服务器 '{id}' server 字段不是对象"))
            })?;
            obj = server_obj;
        }

        obj.remove("enabled");
        obj.remove("source");
        obj.remove("id");
        obj.remove("name");
        obj.remove("description");
        obj.remove("tags");
        obj.remove("homepage");
        obj.remove("docs");

        // Windows 平台自动包装 npx/npm 等命令为 cmd /c 格式；WSL 目标相反 ——
        // 必须把数据库里可能已存在的包装解开，否则写过去的服务器在发行版内
        // 根本无法启动（`cmd` 不存在）。
        if is_wsl_target {
            strip_windows_cmd_wrapper(&mut obj);
        } else {
            wrap_command_for_windows(&mut obj);
        }

        out.insert(id.clone(), Value::Object(obj));
    }

    {
        let obj = root
            .as_object_mut()
            .ok_or_else(|| AppError::Config("~/.claude.json 根必须是对象".into()))?;
        obj.insert("mcpServers".into(), Value::Object(out));
    }

    write_json_value(&path, &root)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // 包装/解包与 WSL 路径判定的纯函数单测已随实现迁移到
    // `crate::mcp::windows_cmd::tests`（两者必须成对维护）。这里只保留真实
    // 文件系统上的投影契约测试。

    /// 真机契约测试：WSL 目标必须拿到**解包后**的命令。
    ///
    /// `mcp::windows_cmd` 的单测覆盖了包装/解包这一对纯函数；本测试验证的是
    /// 真实投影链路（读取实时文件 → 合并 → 落盘）在真实 `\\wsl.localhost\…`
    /// 文件系统上确实解开了历史遗留的 `cmd /c` 包装 —— 这正是
    /// `/home/<user>/.claude.json` 里出现 `command = "cmd"` 的路径。
    ///
    /// 需要 `CC_SWITCH_WSL_TEST_DIR` 指向真实 WSL2 UNC 目录，并被 `#[ignore]`
    /// 门控（与 `config::tests::atomic_write_replaces_existing_wsl_unc_file`
    /// 同一约定），用法：`cargo test -- --ignored`。
    #[cfg(windows)]
    #[test]
    #[ignore = "requires CC_SWITCH_WSL_TEST_DIR to point to a WSL2 UNC directory"]
    fn set_mcp_servers_map_unwraps_cmd_for_a_real_wsl_target() {
        let root = PathBuf::from(
            std::env::var_os("CC_SWITCH_WSL_TEST_DIR").expect("CC_SWITCH_WSL_TEST_DIR must be set"),
        );
        let unc = root.to_string_lossy();
        assert!(
            unc.starts_with(r"\\wsl.localhost\") || unc.starts_with(r"\\wsl$\"),
            "expected a WSL UNC path, got {unc}"
        );

        let previous = crate::settings::get_settings();
        crate::settings::update_settings(crate::settings::AppSettings {
            claude_config_dir: Some(root.join(".claude").to_string_lossy().to_string()),
            ..previous.clone()
        })
        .expect("set claude override dir");
        crate::settings::reload_settings().expect("reload settings");

        let path = get_claude_mcp_path();
        assert!(
            is_wsl_path(&path),
            "test target must be recognised as WSL, got {}",
            path.display()
        );
        std::fs::create_dir_all(path.parent().expect("mcp parent")).expect("create mcp parent");

        // SSOT 里存的是给 Windows 目标写出的"已包装"形态。
        let mut servers = std::collections::HashMap::new();
        servers.insert(
            "context7".to_string(),
            json!({
                "type": "stdio",
                "command": "cmd",
                "args": ["/c", "npx", "-y", "@upstash/context7-mcp"]
            }),
        );
        set_mcp_servers_map(&servers).expect("project mcp servers to WSL target");

        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read mcp file"))
                .expect("parse mcp file");
        assert_eq!(
            written["mcpServers"]["context7"]["command"], "npx",
            "WSL 目标不应收到 cmd 包装: {written}"
        );
        assert_eq!(
            written["mcpServers"]["context7"]["args"],
            json!(["-y", "@upstash/context7-mcp"])
        );

        let _ = std::fs::remove_file(&path);
        let _ = crate::settings::update_settings(previous);
        let _ = crate::settings::reload_settings();
    }

}
