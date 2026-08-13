use grillforge_lib::application::{
    ControlPlaneService, ExtensionSubAgentInput, ModelInput, ProviderInput,
};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::extension_integration::ExtensionIntegrationService;
use grillforge_lib::gateway::Gateway;
use grillforge_lib::mcp_mount::{McpClientFormat, McpMountManager, McpMountTarget};
use std::fs;

#[test]
fn binding_and_mcp_lifecycle_are_independent_and_empty_routes_stay_mounted() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(claude.join("agents")).expect("agents");
    fs::write(
        claude.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nReview.\n",
    )
    .expect("agent");
    let runtime = root.path().join("claude");
    fs::write(&runtime, "runtime").expect("runtime");
    let claude_json = root.path().join(".claude.json");
    fs::write(&claude_json, r#"{"theme":"dark"}"#).expect("config");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_provider(ProviderInput {
            id: "local".into(),
            name: "Local".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: "http://127.0.0.1:18080/v1".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    control
        .save_model(ModelInput {
            id: "coder".into(),
            name: "Coder".into(),
            upstream_id: "coder".into(),
            provider_id: "local".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
        })
        .expect("model");
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: Some("coder".into()),
            capabilities: vec!["review".into()],
        })
        .expect("extension");
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_code",
            &claude_json,
            McpClientFormat::ClaudeJson,
        )],
    )
    .expect("mounts");
    let integration = ExtensionIntegrationService::new(mounts, &claude, Some(runtime), None);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    integration
        .set_binding(&control, &gateway, "claude_code", "reviewer", true)
        .expect("bind");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(&claude_json).expect("config"))
            .expect("JSON"),
        serde_json::json!({"theme":"dark"})
    );
    integration
        .mount_client(&control, &gateway, "claude_code")
        .expect("mount");
    let active: serde_json::Value =
        serde_json::from_slice(&fs::read(&claude_json).expect("active")).expect("JSON");
    assert_eq!(
        active["mcpServers"]["grillforge-claude-code"]["url"],
        "http://127.0.0.1:15721/mcp/claude_code"
    );
    assert_eq!(
        gateway
            .agent_broker_routes_for_client("claude_code")
            .expect("routes")[0]
            .extension_id,
        "reviewer"
    );

    integration
        .set_binding(&control, &gateway, "claude_code", "reviewer", false)
        .expect("unbind");
    let still_mounted: serde_json::Value =
        serde_json::from_slice(&fs::read(&claude_json).expect("mounted")).expect("JSON");
    assert!(still_mounted["mcpServers"]["grillforge-claude-code"].is_object());
    assert!(
        gateway
            .agent_broker_routes_for_client("claude_code")
            .expect("empty routes")
            .is_empty()
    );
    assert!(
        integration
            .client_status(&control.state().expect("state"), "claude_code")
            .expect("status")
            .mounted
    );

    integration
        .unmount_client(&control, &gateway, "claude_code")
        .expect("unmount");
    let restored: serde_json::Value =
        serde_json::from_slice(&fs::read(&claude_json).expect("restored")).expect("JSON");
    assert_eq!(restored, serde_json::json!({"theme":"dark"}));
}

#[test]
fn mcp_status_reports_when_the_client_configuration_changed() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(&claude).expect("claude");
    let config = root.path().join(".claude.json");
    let control = ControlPlaneService::new(&grillforge);
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_code",
            &config,
            McpClientFormat::ClaudeJson,
        )],
    )
    .expect("mounts");
    let integration = ExtensionIntegrationService::new(mounts, &claude, None, None);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());
    integration
        .mount_client(&control, &gateway, "claude_code")
        .expect("mount empty MCP");

    fs::write(
        &config,
        r#"{"mcpServers":{"grillforge-claude-code":{"url":"http://127.0.0.1:9/changed"}}}"#,
    )
    .expect("change client config");
    let status = integration
        .client_status(&control.state().expect("state"), "claude_code")
        .expect("status");

    assert!(status.desired_mounted);
    assert!(!status.mounted);
    assert!(status.configuration_changed);
}

#[test]
fn failed_unmount_restores_the_saved_mount_preference() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(&claude).expect("claude");
    let control = ControlPlaneService::new(&grillforge);
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_code",
            root.path().join(".claude.json"),
            McpClientFormat::ClaudeJson,
        )],
    )
    .expect("mounts");
    let integration = ExtensionIntegrationService::new(mounts, &claude, None, None);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());
    integration
        .mount_client(&control, &gateway, "claude_code")
        .expect("mount empty MCP");
    fs::write(
        grillforge.join("mcp-snapshots/mcp-claude_code.json"),
        b"not-json",
    )
    .expect("corrupt snapshot");

    let error = integration
        .unmount_client(&control, &gateway, "claude_code")
        .expect_err("invalid snapshot must fail closed");

    assert!(error.contains("invalid MCP mount snapshot"));
    assert!(
        control
            .state()
            .expect("rolled back preference")
            .mcp_mounted_client_ids
            .contains(&"claude_code".to_string())
    );
}

#[test]
fn codex_source_binding_activates_the_codex_runtime_without_requiring_claude_agents() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(&claude).expect("claude root");
    let codex = root.path().join(".codex");
    fs::create_dir_all(codex.join("agents")).expect("codex agents");
    fs::write(
        codex.join("agents/reviewer.toml"),
        "name = \"reviewer\"\ndescription = \"Reviews code\"\ndeveloper_instructions = \"Review.\"\n",
    )
    .expect("agent");
    let codex_runtime = root.path().join("codex");
    fs::write(&codex_runtime, "runtime").expect("runtime");
    let claude_json = root.path().join(".claude.json");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "codex-reviewer".into(),
            name: "Codex Reviewer".into(),
            source_client_id: "codex".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec!["review".into()],
        })
        .expect("extension");
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_code",
            &claude_json,
            McpClientFormat::ClaudeJson,
        )],
    )
    .expect("mounts");
    let integration = ExtensionIntegrationService::new(mounts, &claude, None, None)
        .with_codex(&codex, Some(codex_runtime));
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    integration
        .set_binding(&control, &gateway, "claude_code", "codex-reviewer", true)
        .expect("bind Codex source");
    integration
        .mount_client(&control, &gateway, "claude_code")
        .expect("mount");

    let routes = gateway
        .agent_broker_routes_for_client("claude_code")
        .expect("routes");
    assert_eq!(routes[0].source_client_id, "codex");
    assert_eq!(routes[0].source_agent_id, "reviewer");
}

#[test]
fn gemini_source_binding_accepts_builtin_and_custom_agents() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    let gemini = root.path().join(".gemini");
    fs::create_dir_all(&claude).unwrap();
    fs::create_dir_all(gemini.join("agents")).unwrap();
    fs::write(
        gemini.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nReview.\n",
    )
    .unwrap();
    let runtime = root.path().join("gemini");
    fs::write(&runtime, "runtime").unwrap();
    let control = ControlPlaneService::new(&grillforge);
    for (id, source_agent_id) in [
        ("gemini-investigator", "codebase_investigator"),
        ("gemini-reviewer", "reviewer"),
    ] {
        control
            .save_extension_subagent(ExtensionSubAgentInput {
                id: id.into(),
                name: id.into(),
                source_client_id: "gemini".into(),
                source_agent_id: source_agent_id.into(),
                model_id: None,
                capabilities: vec![],
            })
            .unwrap();
    }
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_code",
            root.path().join(".claude.json"),
            McpClientFormat::ClaudeJson,
        )],
    )
    .unwrap();
    let integration = ExtensionIntegrationService::new(mounts, claude, None, None)
        .with_gemini(gemini, Some(runtime));
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    for id in ["gemini-investigator", "gemini-reviewer"] {
        integration
            .set_binding(&control, &gateway, "claude_code", id, true)
            .unwrap();
    }
    integration
        .mount_client(&control, &gateway, "claude_code")
        .unwrap();

    let routes = gateway
        .agent_broker_routes_for_client("claude_code")
        .unwrap();
    assert_eq!(
        routes
            .iter()
            .map(|route| route.source_agent_id.as_str())
            .collect::<Vec<_>>(),
        ["codebase_investigator", "reviewer"]
    );
}

#[test]
fn pi_source_binding_activates_the_pi_runtime_from_a_real_agent_definition() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(&claude).unwrap();
    let pi = root.path().join(".pi/agent");
    fs::create_dir_all(pi.join("agents")).unwrap();
    fs::write(
        pi.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nReview.\n",
    )
    .unwrap();
    let pi_runtime = root.path().join("pi");
    fs::write(&pi_runtime, "runtime").unwrap();
    let config = root.path().join(".claude.json");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "pi-reviewer".into(),
            name: "Pi Reviewer".into(),
            source_client_id: "pi".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec!["review".into()],
        })
        .unwrap();
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_code",
            &config,
            McpClientFormat::ClaudeJson,
        )],
    )
    .unwrap();
    let integration = ExtensionIntegrationService::new(mounts, &claude, None, None)
        .with_pi(&pi, Some(pi_runtime));
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    integration
        .set_binding(&control, &gateway, "claude_code", "pi-reviewer", true)
        .unwrap();
    integration
        .mount_client(&control, &gateway, "claude_code")
        .unwrap();

    let routes = gateway
        .agent_broker_routes_for_client("claude_code")
        .unwrap();
    assert_eq!(routes[0].source_client_id, "pi");
    assert_eq!(routes[0].source_agent_id, "reviewer");
}

#[test]
fn opencode_source_binding_activates_the_installed_runtime_and_named_agent() {
    let root = tempfile::tempdir().unwrap();
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(&claude).unwrap();
    let opencode = root.path().join(".config/opencode");
    fs::create_dir_all(opencode.join("agents")).unwrap();
    fs::write(
        opencode.join("agents/reviewer.md"),
        "---\ndescription: Reviews code\nmode: subagent\n---\nReview.\n",
    )
    .unwrap();
    let runtime = root.path().join("opencode");
    fs::write(&runtime, "runtime").unwrap();
    let config = root.path().join(".claude.json");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "opencode-reviewer".into(),
            name: "OpenCode Reviewer".into(),
            source_client_id: "opencode".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec!["review".into()],
        })
        .unwrap();
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_code",
            config,
            McpClientFormat::ClaudeJson,
        )],
    )
    .unwrap();
    let integration = ExtensionIntegrationService::new(mounts, claude, None, None)
        .with_opencode(opencode, Some(runtime));
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    integration
        .set_binding(&control, &gateway, "claude_code", "opencode-reviewer", true)
        .unwrap();
    integration
        .mount_client(&control, &gateway, "claude_code")
        .unwrap();

    let routes = gateway
        .agent_broker_routes_for_client("claude_code")
        .unwrap();
    assert_eq!(routes[0].source_client_id, "opencode");
    assert_eq!(routes[0].source_agent_id, "reviewer");
}

#[test]
fn kimi_source_binding_activates_an_exactly_selectable_builtin_agent() {
    let root = tempfile::tempdir().unwrap();
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    let kimi = root.path().join(".kimi-code");
    fs::create_dir_all(&claude).unwrap();
    let runtime = root.path().join("kimi");
    fs::write(&runtime, "runtime").unwrap();
    let config = root.path().join(".claude.json");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "kimi-explore".into(),
            name: "Kimi Explore".into(),
            source_client_id: "kimi_code".into(),
            source_agent_id: "explore".into(),
            model_id: None,
            capabilities: vec![],
        })
        .unwrap();
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_code",
            config,
            McpClientFormat::ClaudeJson,
        )],
    )
    .unwrap();
    let integration =
        ExtensionIntegrationService::new(mounts, claude, None, None).with_kimi(kimi, Some(runtime));
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    integration
        .set_binding(&control, &gateway, "claude_code", "kimi-explore", true)
        .unwrap();
    integration
        .mount_client(&control, &gateway, "claude_code")
        .unwrap();

    let routes = gateway
        .agent_broker_routes_for_client("claude_code")
        .unwrap();
    assert_eq!(routes[0].source_client_id, "kimi_code");
    assert_eq!(routes[0].source_agent_id, "explore");
}

#[test]
fn kimi_project_agent_can_be_bound_before_the_execution_working_directory_is_known() {
    let root = tempfile::tempdir().unwrap();
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    let kimi = root.path().join(".kimi-code");
    fs::create_dir_all(&claude).unwrap();
    fs::create_dir_all(&kimi).unwrap();
    let runtime = root.path().join("kimi");
    fs::write(&runtime, "runtime").unwrap();
    let config = root.path().join(".claude.json");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "kimi-project-reviewer".into(),
            name: "Kimi Project Reviewer".into(),
            source_client_id: "kimi_code".into(),
            source_agent_id: "project-reviewer".into(),
            model_id: None,
            capabilities: vec![],
        })
        .unwrap();
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_code",
            config,
            McpClientFormat::ClaudeJson,
        )],
    )
    .unwrap();
    let integration =
        ExtensionIntegrationService::new(mounts, claude, None, None).with_kimi(kimi, Some(runtime));
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    integration
        .set_binding(
            &control,
            &gateway,
            "claude_code",
            "kimi-project-reviewer",
            true,
        )
        .unwrap();
    integration
        .mount_client(&control, &gateway, "claude_code")
        .unwrap();

    let routes = gateway
        .agent_broker_routes_for_client("claude_code")
        .unwrap();
    assert_eq!(routes[0].source_agent_id, "project-reviewer");
}

#[test]
fn pi_project_agent_can_be_bound_before_the_execution_working_directory_is_known() {
    let root = tempfile::tempdir().unwrap();
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    let pi = root.path().join(".pi/agent");
    fs::create_dir_all(&claude).unwrap();
    fs::create_dir_all(&pi).unwrap();
    let runtime = root.path().join("pi");
    fs::write(&runtime, "runtime").unwrap();
    let config = root.path().join(".claude.json");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "project-reviewer".into(),
            name: "Project Reviewer".into(),
            source_client_id: "pi".into(),
            source_agent_id: "project_reviewer".into(),
            model_id: None,
            capabilities: vec![],
        })
        .unwrap();
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_code",
            config,
            McpClientFormat::ClaudeJson,
        )],
    )
    .unwrap();
    let integration =
        ExtensionIntegrationService::new(mounts, claude, None, None).with_pi(pi, Some(runtime));
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    integration
        .set_binding(&control, &gateway, "claude_code", "project-reviewer", true)
        .unwrap();
    integration
        .mount_client(&control, &gateway, "claude_code")
        .unwrap();
}

#[test]
fn editing_a_bound_extension_updates_the_live_broker_route_immediately() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(claude.join("agents")).expect("agents");
    fs::write(
        claude.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nReview.\n",
    )
    .expect("agent");
    let runtime = root.path().join("claude");
    fs::write(&runtime, "runtime").expect("runtime");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_provider(ProviderInput {
            id: "local".into(),
            name: "Local".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: "http://127.0.0.1:18080/v1".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    for id in ["first", "second"] {
        control
            .save_model(ModelInput {
                id: id.into(),
                name: id.into(),
                upstream_id: id.into(),
                provider_id: "local".into(),
                capabilities: vec![],
                protocol_capabilities: vec![],
            })
            .expect("model");
    }
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: Some("first".into()),
            capabilities: vec![],
        })
        .expect("extension");
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_code",
            root.path().join(".claude.json"),
            McpClientFormat::ClaudeJson,
        )],
    )
    .expect("mounts");
    let integration = ExtensionIntegrationService::new(mounts, &claude, Some(runtime), None);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());
    integration
        .set_binding(&control, &gateway, "claude_code", "reviewer", true)
        .expect("bind");
    integration
        .mount_client(&control, &gateway, "claude_code")
        .expect("mount");
    assert_eq!(
        gateway
            .agent_broker_routes_for_client("claude_code")
            .expect("first route")[0]
            .model_id
            .as_deref(),
        Some("first")
    );

    integration
        .update_extension(
            &control,
            &gateway,
            ExtensionSubAgentInput {
                id: "reviewer".into(),
                name: "Reviewer".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "reviewer".into(),
                model_id: Some("second".into()),
                capabilities: vec![],
            },
        )
        .expect("update");
    assert_eq!(
        gateway
            .agent_broker_routes_for_client("claude_code")
            .expect("second route")[0]
            .model_id
            .as_deref(),
        Some("second")
    );
}

#[test]
fn claude_client_mcp_is_independent_from_one_p_and_three_p_inference_modes() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(claude.join("agents")).expect("agents");
    fs::write(
        claude.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nReview.\n",
    )
    .expect("agent");
    let runtime = root.path().join("claude");
    fs::write(&runtime, "runtime").expect("runtime");
    let desktop_config = root.path().join("claude_desktop_config.json");
    fs::write(
        &desktop_config,
        r#"{"deploymentMode":"1p","mcpServers":{"keep":{"command":"keep"}}}"#,
    )
    .expect("config");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec!["review".into()],
        })
        .expect("extension");
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_desktop",
            &desktop_config,
            McpClientFormat::ClaudeDesktopJson,
        )
        .with_stdio_command("/Applications/GrillForge.app/Contents/MacOS/grillforge")],
    )
    .expect("mounts");
    let integration = ExtensionIntegrationService::new(mounts, &claude, Some(runtime), None);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    integration
        .set_binding(&control, &gateway, "claude_desktop", "reviewer", true)
        .expect("bind in 1p");
    integration
        .mount_client(&control, &gateway, "claude_desktop")
        .expect("mount");
    let one_p: serde_json::Value =
        serde_json::from_slice(&fs::read(&desktop_config).expect("1p config")).expect("JSON");
    assert_eq!(one_p["deploymentMode"], "1p");
    assert_eq!(
        one_p["mcpServers"]["grillforge-claude-desktop"]["args"],
        serde_json::json!(["mcp-stdio"])
    );

    let mut three_p = one_p;
    three_p["deploymentMode"] = "3p".into();
    fs::write(
        &desktop_config,
        serde_json::to_vec_pretty(&three_p).unwrap(),
    )
    .expect("3p config");
    integration
        .reconcile_client(&control.state().expect("state"), &gateway, "claude_desktop")
        .expect("remain mounted in 3p");
    let active_three_p: serde_json::Value =
        serde_json::from_slice(&fs::read(&desktop_config).expect("3p mounted")).expect("JSON");
    assert_eq!(active_three_p["deploymentMode"], "3p");

    integration
        .set_binding(&control, &gateway, "claude_desktop", "reviewer", false)
        .expect("unbind");
    let empty_routes = gateway
        .agent_broker_routes_for_client("claude_desktop")
        .expect("mounted empty broker");
    assert!(empty_routes.is_empty());
    integration
        .unmount_client(&control, &gateway, "claude_desktop")
        .expect("manual unmount");
    let restored: serde_json::Value =
        serde_json::from_slice(&fs::read(&desktop_config).expect("restored")).expect("JSON");
    assert_eq!(restored["deploymentMode"], "3p");
    assert_eq!(restored["mcpServers"]["keep"]["command"], "keep");
    assert!(
        restored["mcpServers"]
            .get("grillforge-claude-desktop")
            .is_none()
    );
}

#[test]
fn suspending_a_client_preserves_mcp_while_the_lower_model_layer_changes() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(claude.join("agents")).expect("agents");
    fs::write(
        claude.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nReview.\n",
    )
    .expect("agent");
    let runtime = root.path().join("claude");
    fs::write(&runtime, "runtime").expect("runtime");
    let desktop_config = root.path().join("claude_desktop_config.json");
    fs::write(&desktop_config, r#"{"deploymentMode":"1p"}"#).expect("config");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec![],
        })
        .expect("extension");
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_desktop",
            &desktop_config,
            McpClientFormat::ClaudeDesktopJson,
        )
        .with_stdio_command("/Applications/GrillForge.app/Contents/MacOS/grillforge")],
    )
    .expect("mounts");
    let integration = ExtensionIntegrationService::new(mounts, &claude, Some(runtime), None);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());
    integration
        .set_binding(&control, &gateway, "claude_desktop", "reviewer", true)
        .expect("bind");
    integration
        .mount_client(&control, &gateway, "claude_desktop")
        .expect("mount");

    integration
        .with_suspended_client(
            &control.state().expect("state"),
            &gateway,
            "claude_desktop",
            || {
                fs::write(&desktop_config, r#"{"deploymentMode":"3p"}"#)
                    .map_err(|error| error.to_string())
            },
        )
        .expect("lower layer update");

    let configured: serde_json::Value =
        serde_json::from_slice(&fs::read(&desktop_config).expect("config")).expect("JSON");
    assert_eq!(configured["deploymentMode"], "3p");
    assert_eq!(
        configured["mcpServers"]["grillforge-claude-desktop"]["args"],
        serde_json::json!(["mcp-stdio"])
    );
    assert_eq!(
        gateway
            .agent_broker_routes_for_client("claude_desktop")
            .expect("routes")[0]
            .extension_id,
        "reviewer"
    );
}

#[test]
fn crash_restart_removes_the_old_mcp_layer_before_restoring_models_then_remounts_it() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(claude.join("agents")).expect("agents");
    fs::write(
        claude.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nReview.\n",
    )
    .expect("agent");
    let runtime = root.path().join("claude");
    fs::write(&runtime, "runtime").expect("runtime");
    let desktop_config = root.path().join("claude_desktop_config.json");
    fs::write(&desktop_config, r#"{"deploymentMode":"1p"}"#).expect("config");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec![],
        })
        .expect("extension");
    let manager = || {
        McpMountManager::new(
            grillforge.join("mcp-snapshots"),
            [McpMountTarget::new(
                "claude_desktop",
                &desktop_config,
                McpClientFormat::ClaudeDesktopJson,
            )
            .with_stdio_command("/Applications/GrillForge.app/Contents/MacOS/grillforge")],
        )
        .expect("mounts")
    };
    let first = ExtensionIntegrationService::new(manager(), &claude, Some(runtime.clone()), None);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());
    first
        .set_binding(&control, &gateway, "claude_desktop", "reviewer", true)
        .expect("bind before crash");
    first
        .mount_client(&control, &gateway, "claude_desktop")
        .expect("mount before crash");

    let restarted = ExtensionIntegrationService::new(manager(), &claude, Some(runtime), None);
    restarted
        .restore_clients_then_reconcile(&control, &gateway, || {
            let lower: serde_json::Value = serde_json::from_slice(
                &fs::read(&desktop_config).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            assert!(lower.get("mcpServers").is_none());
            fs::write(&desktop_config, r#"{"deploymentMode":"3p"}"#)
                .map_err(|error| error.to_string())
        })
        .expect("restart layers");

    let live: serde_json::Value =
        serde_json::from_slice(&fs::read(&desktop_config).expect("live")).expect("JSON");
    assert_eq!(live["deploymentMode"], "3p");
    assert_eq!(
        live["mcpServers"]["grillforge-claude-desktop"]["args"],
        serde_json::json!(["mcp-stdio"])
    );
    restarted.restore_live_mounts(&gateway).expect("exit");
    let restored: serde_json::Value =
        serde_json::from_slice(&fs::read(&desktop_config).expect("restored")).expect("JSON");
    assert_eq!(restored, serde_json::json!({"deploymentMode":"3p"}));
    assert!(
        control
            .state()
            .expect("persisted preference")
            .mcp_mounted_client_ids
            .contains(&"claude_desktop".to_string())
    );
}

#[test]
fn missing_source_agent_rolls_back_only_the_mcp_mount_preference() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(&claude).expect("claude");
    let runtime = root.path().join("claude");
    fs::write(&runtime, "runtime").expect("runtime");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "missing".into(),
            name: "Missing".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "missing".into(),
            model_id: None,
            capabilities: vec![],
        })
        .expect("extension");
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "claude_code",
            root.path().join(".claude.json"),
            McpClientFormat::ClaudeJson,
        )],
    )
    .expect("mounts");
    let integration = ExtensionIntegrationService::new(mounts, &claude, Some(runtime), None);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    integration
        .set_binding(&control, &gateway, "claude_code", "missing", true)
        .expect("binding is independent from mounting");
    let error = integration
        .mount_client(&control, &gateway, "claude_code")
        .expect_err("missing source must fail while mounting");

    assert!(error.contains("source Agent does not exist"));
    assert!(
        control
            .state()
            .expect("state")
            .mcp_mounted_client_ids
            .is_empty()
    );
}

#[test]
fn pi_requires_the_installed_extension_then_mounts_without_restarting_grillforge() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(claude.join("agents")).expect("agents");
    fs::write(
        claude.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nReview.\n",
    )
    .expect("agent");
    let runtime = root.path().join("claude");
    fs::write(&runtime, "runtime").expect("runtime");
    let pi_settings = root.path().join(".pi/agent/settings.json");
    fs::create_dir_all(pi_settings.parent().unwrap()).expect("pi root");
    fs::write(&pi_settings, r#"{"packages":[]}"#).expect("settings");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec!["review".into()],
        })
        .expect("extension");
    let pi_mcp = root.path().join(".pi/agent/mcp.json");
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [McpMountTarget::new(
            "pi",
            &pi_mcp,
            McpClientFormat::PiExtensionJson,
        )],
    )
    .expect("mounts");
    let integration =
        ExtensionIntegrationService::new(mounts, &claude, Some(runtime), Some(pi_settings.clone()));
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    integration
        .set_binding(&control, &gateway, "pi", "reviewer", true)
        .expect("bind Pi");
    let error = integration
        .mount_client(&control, &gateway, "pi")
        .expect_err("missing Pi extension");
    assert!(error.contains("pi-mcp-extension"));
    assert!(!pi_mcp.exists());

    fs::write(
        &pi_settings,
        r#"{"packages":["npm:pi-mcp-extension@1.5.0"]}"#,
    )
    .expect("installed extension");
    integration
        .mount_client(&control, &gateway, "pi")
        .expect("mount Pi after install");
    let mounted: serde_json::Value =
        serde_json::from_slice(&fs::read(pi_mcp).expect("mcp config")).expect("JSON");
    assert_eq!(
        mounted["mcpServers"]["grillforge-pi"]["transport"],
        "streamable-http"
    );
    assert_eq!(
        gateway
            .agent_broker_routes_for_client("pi")
            .expect("Pi routes")[0]
            .extension_id,
        "reviewer"
    );
}

#[test]
fn failed_multi_client_update_restores_the_record_and_every_changed_mount() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(claude.join("agents")).expect("agents");
    fs::write(
        claude.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nReview.\n",
    )
    .expect("agent");
    let runtime = root.path().join("claude");
    fs::write(&runtime, "runtime").expect("runtime");
    let pi_settings = root.path().join(".pi/agent/settings.json");
    fs::create_dir_all(pi_settings.parent().unwrap()).expect("pi root");
    fs::write(&pi_settings, r#"{"packages":["npm:pi-mcp-extension"]}"#).expect("Pi extension");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_provider(ProviderInput {
            id: "local".into(),
            name: "Local".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: "http://127.0.0.1:18080/v1".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    for id in ["first", "second"] {
        control
            .save_model(ModelInput {
                id: id.into(),
                name: id.into(),
                upstream_id: id.into(),
                provider_id: "local".into(),
                capabilities: vec![],
                protocol_capabilities: vec![],
            })
            .expect("model");
    }
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: Some("first".into()),
            capabilities: vec![],
        })
        .expect("extension");
    let claude_json = root.path().join(".claude.json");
    let pi_mcp = root.path().join(".pi/agent/mcp.json");
    let mounts = McpMountManager::new(
        grillforge.join("mcp-snapshots"),
        [
            McpMountTarget::new("claude_code", &claude_json, McpClientFormat::ClaudeJson),
            McpMountTarget::new("pi", &pi_mcp, McpClientFormat::PiExtensionJson),
        ],
    )
    .expect("mounts");
    let integration =
        ExtensionIntegrationService::new(mounts, &claude, Some(runtime), Some(pi_settings.clone()));
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());
    integration
        .set_binding(&control, &gateway, "claude_code", "reviewer", true)
        .expect("bind Claude Code");
    integration
        .mount_client(&control, &gateway, "claude_code")
        .expect("mount Claude Code");
    integration
        .set_binding(&control, &gateway, "pi", "reviewer", true)
        .expect("bind Pi");
    integration
        .mount_client(&control, &gateway, "pi")
        .expect("mount Pi");
    let claude_mount = fs::read(&claude_json).expect("Claude mount");
    let pi_mount = fs::read(&pi_mcp).expect("Pi mount");

    fs::write(&pi_settings, r#"{"packages":[]}"#).expect("remove Pi extension");
    let error = integration
        .update_extension(
            &control,
            &gateway,
            ExtensionSubAgentInput {
                id: "reviewer".into(),
                name: "Reviewer".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "reviewer".into(),
                model_id: Some("second".into()),
                capabilities: vec![],
            },
        )
        .expect_err("Pi reconcile must fail");

    assert_eq!(
        error,
        "Pi needs pi-mcp-extension before it can use extension SubAgents"
    );
    assert_eq!(
        control
            .state()
            .expect("rolled back state")
            .extension_subagents[0]
            .model_id
            .as_deref(),
        Some("first")
    );
    assert_eq!(
        fs::read(&claude_json).expect("Claude restored"),
        claude_mount
    );
    assert_eq!(fs::read(&pi_mcp).expect("Pi preserved"), pi_mount);
    for client_id in ["claude_code", "pi"] {
        assert_eq!(
            gateway
                .agent_broker_routes_for_client(client_id)
                .expect("old route")[0]
                .model_id
                .as_deref(),
            Some("first")
        );
    }
}

#[test]
fn startup_reconcile_remounts_extensions_after_a_model_restore_failure() {
    let root = tempfile::tempdir().expect("root");
    let grillforge = root.path().join(".grillforge");
    let claude = root.path().join(".claude");
    fs::create_dir_all(claude.join("agents")).expect("agents");
    fs::write(
        claude.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nReview.\n",
    )
    .expect("agent");
    let runtime = root.path().join("claude");
    fs::write(&runtime, "runtime").expect("runtime");
    let config = root.path().join(".claude.json");
    fs::write(&config, r#"{"modelLayer":"old"}"#).expect("config");
    let control = ControlPlaneService::new(&grillforge);
    control
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec![],
        })
        .expect("extension");
    let manager = || {
        McpMountManager::new(
            grillforge.join("mcp-snapshots"),
            [McpMountTarget::new(
                "claude_code",
                &config,
                McpClientFormat::ClaudeJson,
            )],
        )
        .expect("mounts")
    };
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());
    let first = ExtensionIntegrationService::new(manager(), &claude, Some(runtime.clone()), None);
    first
        .set_binding(&control, &gateway, "claude_code", "reviewer", true)
        .expect("initial binding");
    first
        .mount_client(&control, &gateway, "claude_code")
        .expect("initial mount");

    let restarted = ExtensionIntegrationService::new(manager(), &claude, Some(runtime), None);
    let error = restarted
        .restore_clients_then_reconcile(&control, &gateway, || {
            let lower: serde_json::Value =
                serde_json::from_slice(&fs::read(&config).map_err(|error| error.to_string())?)
                    .map_err(|error| error.to_string())?;
            assert!(lower.get("mcpServers").is_none());
            fs::write(&config, r#"{"modelLayer":"new"}"#).map_err(|error| error.to_string())?;
            Err::<(), _>("model restore failed".to_string())
        })
        .expect_err("model restore failure is preserved");

    assert_eq!(error, "model restore failed");
    let live: serde_json::Value =
        serde_json::from_slice(&fs::read(&config).expect("remounted config")).expect("JSON");
    assert_eq!(live["modelLayer"], "new");
    assert!(live["mcpServers"].get("grillforge-claude-code").is_some());
}
