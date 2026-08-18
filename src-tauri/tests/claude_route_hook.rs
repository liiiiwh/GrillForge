use grillforge_lib::claude_route_hook::{HookDecision, decide};
use grillforge_lib::configuration::{
    AgentRecord, AgentsDocument, ConfigurationDocuments, ConfigurationFiles,
    ExtensionSubAgentRecord,
};
use std::io::Write;
use std::process::{Command, Stdio};

fn documents(bound: bool, mounted: bool) -> ConfigurationDocuments {
    let mut documents = ConfigurationDocuments::default();
    let version = documents.agents.version;
    documents.agents = AgentsDocument {
        version,
        agents: vec![AgentRecord {
            id: "claude_code".into(),
            adapter: "claude_code".into(),
            enabled: false,
            main: grillforge_lib::configuration::MainRecord::Native,
            model_slots: Default::default(),
            native_model_slots: Default::default(),
            model_pool: vec![],
            codex_agent_models: vec![],
            extension_subagent_ids: bound.then(|| vec!["reviewer".into()]).unwrap_or_default(),
        }],
        extension_subagents: vec![ExtensionSubAgentRecord {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec![],
        }],
        mcp_mounted_client_ids: mounted
            .then(|| vec!["claude_code".into()])
            .unwrap_or_default(),
    };
    documents
}

#[test]
fn mounted_extensions_deny_native_workflow_and_agent_tools() {
    for tool_name in ["Workflow", "Agent"] {
        let input = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": tool_name,
            "tool_input": {"description": "Inspect the repository"}
        });
        let decision = decide(&documents(true, true), &input).expect("decision");
        let HookDecision::Deny { reason } = decision else {
            panic!("native agent tool must be denied");
        };
        assert!(reason.contains("grillforge-claude-code"));
        assert!(reason.contains("list_agents"));
        assert!(reason.contains("run_agent"));
        assert!(reason.contains("并行"));
    }
}

#[test]
fn unrelated_tools_and_clients_without_live_extensions_are_allowed() {
    let bash = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "pwd"}
    });
    assert_eq!(
        decide(&documents(true, true), &bash).unwrap(),
        HookDecision::Allow
    );

    let workflow = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Workflow",
        "tool_input": {"description": "Inspect"}
    });
    assert_eq!(
        decide(&documents(false, true), &workflow).unwrap(),
        HookDecision::Allow
    );
    assert_eq!(
        decide(&documents(true, false), &workflow).unwrap(),
        HookDecision::Allow
    );
}

#[test]
fn malformed_hook_payload_fails_fast() {
    let error = decide(&documents(true, true), &serde_json::json!({"tool_name": 3}))
        .expect_err("malformed hook payload");
    assert_eq!(error, "Claude route hook tool_name must be a string");
}

#[test]
fn installed_hook_command_returns_the_official_pre_tool_use_deny_shape() {
    let root = tempfile::tempdir().expect("root");
    let documents = documents(true, true);
    ConfigurationFiles::new(root.path())
        .save(&documents.config, &documents.models, &documents.agents)
        .expect("configuration");
    let mut child = Command::new(env!("CARGO_BIN_EXE_grillforge"))
        .arg("claude-route-hook")
        .env("GRILLFORGE_CONFIG_ROOT", root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hook process");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            serde_json::json!({
                "hook_event_name":"PreToolUse",
                "tool_name":"Workflow",
                "tool_input": {"description":"parallel review"}
            })
            .to_string()
            .as_bytes(),
        )
        .expect("hook input");
    let output = child.wait_with_output().expect("hook result");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).expect("hook JSON");
    assert_eq!(
        response["hookSpecificOutput"]["hookEventName"],
        "PreToolUse"
    );
    assert_eq!(response["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        response["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("list_agents")
    );
}

#[test]
fn a_grillforge_launched_child_runtime_keeps_its_native_agent_tools() {
    let root = tempfile::tempdir().expect("root");
    let documents = documents(true, true);
    ConfigurationFiles::new(root.path())
        .save(&documents.config, &documents.models, &documents.agents)
        .expect("configuration");
    let mut child = Command::new(env!("CARGO_BIN_EXE_grillforge"))
        .arg("claude-route-hook")
        .env("GRILLFORGE_CONFIG_ROOT", root.path())
        .env("GRILLFORGE_AGENT_CHILD", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hook process");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            serde_json::json!({
                "hook_event_name":"PreToolUse",
                "tool_name":"Agent",
                "tool_input": {"description":"delegate"}
            })
            .to_string()
            .as_bytes(),
        )
        .expect("hook input");
    let output = child.wait_with_output().expect("hook result");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).expect("hook JSON");
    assert_eq!(response, serde_json::json!({}));
}
