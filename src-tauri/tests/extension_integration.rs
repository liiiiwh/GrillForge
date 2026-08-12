use grillforge_lib::application::{
    ControlPlaneService, ExtensionSubAgentInput, ModelInput, ProviderInput,
};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::extension_integration::ExtensionIntegrationService;
use grillforge_lib::gateway::Gateway;
use grillforge_lib::mcp_mount::{McpClientFormat, McpMountManager, McpMountTarget};
use std::fs;

#[test]
fn binding_changes_mount_update_and_unmount_the_client_mcp_transactionally() {
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
    let restored: serde_json::Value =
        serde_json::from_slice(&fs::read(&claude_json).expect("restored")).expect("JSON");
    assert_eq!(restored, serde_json::json!({"theme":"dark"}));
    assert!(
        gateway
            .agent_broker_routes_for_client("claude_code")
            .is_err()
    );
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
        )],
    )
    .expect("mounts");
    let integration = ExtensionIntegrationService::new(mounts, &claude, Some(runtime), None);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    integration
        .set_binding(&control, &gateway, "claude_desktop", "reviewer", true)
        .expect("bind in 1p");
    let one_p: serde_json::Value =
        serde_json::from_slice(&fs::read(&desktop_config).expect("1p config")).expect("JSON");
    assert_eq!(one_p["deploymentMode"], "1p");
    assert_eq!(
        one_p["mcpServers"]["grillforge-claude-desktop"]["transport"],
        "http"
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
        )],
    )
    .expect("mounts");
    let integration = ExtensionIntegrationService::new(mounts, &claude, Some(runtime), None);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());
    integration
        .set_binding(&control, &gateway, "claude_desktop", "reviewer", true)
        .expect("bind");

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
        configured["mcpServers"]["grillforge-claude-desktop"]["transport"],
        "http"
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
            )],
        )
        .expect("mounts")
    };
    let first = ExtensionIntegrationService::new(manager(), &claude, Some(runtime.clone()), None);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());
    first
        .set_binding(&control, &gateway, "claude_desktop", "reviewer", true)
        .expect("bind before crash");

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
        live["mcpServers"]["grillforge-claude-desktop"]["transport"],
        "http"
    );
    restarted.restore_live_mounts(&gateway).expect("exit");
    let restored: serde_json::Value =
        serde_json::from_slice(&fs::read(&desktop_config).expect("restored")).expect("JSON");
    assert_eq!(restored, serde_json::json!({"deploymentMode":"3p"}));
}

#[test]
fn missing_source_agent_rolls_back_the_requested_binding() {
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

    let error = integration
        .set_binding(&control, &gateway, "claude_code", "missing", true)
        .expect_err("missing source must fail");

    assert!(error.contains("source Agent does not exist"));
    assert!(
        control
            .state()
            .expect("state")
            .client_extension_subagent_ids["claude_code"]
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

    let error = integration
        .set_binding(&control, &gateway, "pi", "reviewer", true)
        .expect_err("missing Pi extension");
    assert!(error.contains("pi-mcp-extension"));
    assert!(!pi_mcp.exists());

    fs::write(
        &pi_settings,
        r#"{"packages":["npm:pi-mcp-extension@1.5.0"]}"#,
    )
    .expect("installed extension");
    integration
        .set_binding(&control, &gateway, "pi", "reviewer", true)
        .expect("bind Pi after install");
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
