use crate::configuration::{ConfigurationDocuments, ConfigurationFiles};
use serde_json::{Value, json};
use std::io::{self, Read};
use std::path::PathBuf;

const MAX_HOOK_INPUT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookDecision {
    Allow,
    Deny { reason: String },
}

/// Claude Code and Claude Client share one user settings file, so the hook they
/// both run must answer for the client whose session invoked it.
pub fn session_client_id(entrypoint: Option<&str>) -> &'static str {
    match entrypoint {
        Some("claude-desktop") => "claude_desktop",
        _ => "claude_code",
    }
}

pub fn decide(
    documents: &ConfigurationDocuments,
    input: &Value,
    child: bool,
    client_id: &str,
) -> Result<HookDecision, String> {
    let tool_name = input
        .get("tool_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "Claude route hook tool_name must be a string".to_string())?;
    if !matches!(tool_name, "Workflow" | "Agent") {
        return Ok(HookDecision::Allow);
    }
    // An extension SubAgent is a leaf worker. Letting it open another level would
    // turn one invocation into an unbounded tree of runtimes.
    if child {
        return Ok(HookDecision::Deny {
            reason: "当前会话是 GrillForge 扩展 SubAgent 的子运行时，不允许再创建下一级 SubAgent。请在本会话内直接完成任务。".into(),
        });
    }

    let mounted = documents
        .agents
        .mcp_mounted_client_ids
        .iter()
        .any(|mounted| mounted == client_id);
    let has_extensions = documents
        .agents
        .agents
        .iter()
        .find(|agent| agent.id == client_id)
        .is_some_and(|agent| !agent.extension_subagent_ids.is_empty());
    if !mounted || !has_extensions {
        return Ok(HookDecision::Allow);
    }

    // The MCP server name is per client; naming the wrong one sends the Agent to a
    // tool its session does not have.
    let server = match client_id {
        "claude_desktop" => "grillforge-claude-desktop",
        _ => "grillforge-claude-code",
    };
    Ok(HookDecision::Deny {
        reason: format!(
            "当前客户端已挂载 GrillForge 扩展 SubAgent。请先调用 mcp__{server}__list_agents；有匹配项时调用 mcp__{server}__run_agent。需要并行或 Workflow 时，并发调用多个 run_agent。不要改用原生 Workflow 或 Agent；如需原生能力，请先在 GrillForge 中关闭对应扩展 SubAgent 或卸载扩展。"
        ),
    })
}

pub fn run_from_env() -> Result<(), String> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(MAX_HOOK_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read Claude route hook input: {error}"))?;
    if bytes.len() as u64 > MAX_HOOK_INPUT_BYTES {
        return Err("Claude route hook input exceeds 1 MiB".into());
    }
    let input: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid Claude route hook input: {error}"))?;
    let child = std::env::var_os("GRILLFORGE_AGENT_CHILD").is_some();
    let root = std::env::var_os("GRILLFORGE_CONFIG_ROOT")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".grillforge")))
        .ok_or_else(|| "could not resolve the GrillForge configuration directory".to_string())?;
    let documents = ConfigurationFiles::new(root)
        .read()
        .map_err(|error| format!("could not read GrillForge routing configuration: {error}"))?;
    let client_id = session_client_id(
        std::env::var("CLAUDE_CODE_ENTRYPOINT")
            .ok()
            .as_deref()
            .map(str::trim),
    );
    let output = match decide(&documents, &input, child, client_id)? {
        HookDecision::Allow => json!({}),
        HookDecision::Deny { reason } => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason
            }
        }),
    };
    println!(
        "{}",
        serde_json::to_string(&output)
            .map_err(|error| format!("could not encode Claude route hook result: {error}"))?
    );
    Ok(())
}
