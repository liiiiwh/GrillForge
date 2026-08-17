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

pub fn decide(documents: &ConfigurationDocuments, input: &Value) -> Result<HookDecision, String> {
    let tool_name = input
        .get("tool_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "Claude route hook tool_name must be a string".to_string())?;
    if !matches!(tool_name, "Workflow" | "Agent") {
        return Ok(HookDecision::Allow);
    }

    let mounted = documents
        .agents
        .mcp_mounted_client_ids
        .iter()
        .any(|client_id| client_id == "claude_code");
    let has_extensions = documents
        .agents
        .agents
        .iter()
        .find(|agent| agent.id == "claude_code")
        .is_some_and(|agent| !agent.extension_subagent_ids.is_empty());
    if !mounted || !has_extensions {
        return Ok(HookDecision::Allow);
    }

    Ok(HookDecision::Deny {
        reason: "Claude Code 已挂载 GrillForge 扩展 SubAgent。请先调用 mcp__grillforge-claude-code__list_agents；有匹配项时调用 mcp__grillforge-claude-code__run_agent。需要并行或 Workflow 时，并发调用多个 run_agent。不要改用原生 Workflow 或 Agent；如需原生能力，请先在 GrillForge 中关闭对应扩展 SubAgent 或卸载扩展。".into(),
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
    let root = std::env::var_os("GRILLFORGE_CONFIG_ROOT")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".grillforge")))
        .ok_or_else(|| "could not resolve the GrillForge configuration directory".to_string())?;
    let documents = ConfigurationFiles::new(root)
        .read()
        .map_err(|error| format!("could not read GrillForge routing configuration: {error}"))?;
    let output = match decide(&documents, &input)? {
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
