use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput, SubAgentInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use grillforge_lib::integration::{
    IntegrationService, IntegrationTakeover, default_claude_config_root,
};
use grillforge_lib::selector;
use serde_json::json;
use std::fs;

fn local_provider() -> ProviderInput {
    ProviderInput {
        id: "local-responses".into(),
        name: "Local Responses".into(),
        protocol: Protocol::OpenAiResponses,
        endpoint: "http://127.0.0.1:18080/v1".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        api_key_placement: ApiKeyPlacement::None,
        api_key: None,
        enabled: true,
        models_url: None,
    }
}

fn worker_model() -> ModelInput {
    ModelInput {
        id: "review-model".into(),
        name: "Review Model".into(),
        upstream_id: "upstream-review".into(),
        provider_id: "local-responses".into(),
        capabilities: vec!["review".into(), "coding".into()],
        protocol_capabilities: vec![],
    }
}

#[test]
fn enabled_claude_workers_restore_on_exit_and_reapply_for_cli() {
    let directory = tempfile::tempdir().expect("temporary home");
    let home = directory.path().join("home");
    let grillforge_root = home.join(".grillforge");
    let claude_root = default_claude_config_root(&home);
    fs::create_dir_all(&claude_root).expect("Claude configuration root");

    let original_settings = serde_json::to_string_pretty(&json!({
        "env": {
            "ANTHROPIC_BASE_URL": "https://native.example",
            "UNCHANGED": "preserved"
        },
        "permissions": { "defaultMode": "acceptEdits" }
    }))
    .expect("original settings");
    fs::write(claude_root.join("settings.json"), &original_settings).expect("original settings");

    let control = ControlPlaneService::new(&grillforge_root);
    control
        .save_provider(local_provider())
        .expect("local provider");
    control.save_model(worker_model()).expect("worker model");
    control
        .set_native_subagent_enabled(false)
        .expect("disable native worker for an exact pool");
    control
        .save_subagent(SubAgentInput {
            id: "security-review".into(),
            name: "安全审查".into(),
            model_id: "review-model".into(),
            capabilities: vec!["review".into(), "security".into()],
            enabled: true,
        })
        .expect("custom SubAgent");
    let initial_state = control.state().expect("control-plane state");

    let first_process = IntegrationService::new(&claude_root, &grillforge_root);
    let applied = first_process
        .apply(&initial_state, "http://127.0.0.1:15721")
        .expect("apply Claude Code worker pool");
    control
        .set_client_integration_enabled("claude_code", true)
        .expect("persist enabled Claude integration");
    let state = control.state().expect("enabled control-plane state");
    assert_eq!(applied.takeover, IntegrationTakeover::Active);
    assert!(grillforge_root.join("claude-code.snapshot.json").is_file());
    assert_eq!(
        applied.generated_agent_names,
        ["grillforge-worker-security-review"]
    );
    let agent_path = claude_root.join("agents/grillforge-worker-security-review.md");
    let agent_definition = fs::read_to_string(&agent_path).expect("generated Agent definition");
    assert!(agent_definition.contains("model: grillforge/review-model"));
    let active_settings =
        fs::read_to_string(claude_root.join("settings.json")).expect("active settings");
    assert!(
        active_settings.contains("\"CLAUDE_CODE_SUBAGENT_MODEL\": \"grillforge/review-model\"")
    );

    let before_restart = selector::select(&grillforge_root).expect("selector after apply");
    assert_eq!(before_restart.workers.len(), 1);
    assert_eq!(
        before_restart.workers[0].route_alias,
        "grillforge/review-model"
    );

    // Normal application exit restores Claude's live files. The durable
    // "enabled" preference is owned by the control plane, not by this recovery
    // snapshot, so startup can safely re-apply from the saved state.
    first_process
        .disable()
        .expect("restore live configuration on normal exit");
    assert!(
        selector::select(&grillforge_root)
            .expect("selector while GrillForge is stopped")
            .workers
            .is_empty()
    );
    assert!(!grillforge_root.join("claude-code.snapshot.json").exists());
    assert!(!agent_path.exists());
    assert_eq!(
        fs::read_to_string(claude_root.join("settings.json")).expect("restored settings"),
        original_settings
    );

    drop(first_process);
    let restarted_gui = IntegrationService::new(&claude_root, &grillforge_root);
    let gateway = Gateway::new(&grillforge_root).status("http://127.0.0.1:15721".into());
    let reapplied = restarted_gui
        .apply(&state, &gateway.base_url)
        .expect("background re-apply for the persistently enabled client");
    gateway
        .activate(&state)
        .expect("restore loopback gateway routes");
    assert_eq!(reapplied.takeover, IntegrationTakeover::Active);
    let after_restart = selector::select(&grillforge_root).expect("selector after restart");
    assert_eq!(after_restart.workers, before_restart.workers);
    assert_eq!(
        fs::read_to_string(&agent_path).expect("re-applied Agent definition"),
        agent_definition
    );
    assert!(
        fs::read_to_string(claude_root.join("settings.json"))
            .expect("re-applied settings")
            .contains("\"CLAUDE_CODE_SUBAGENT_MODEL\": \"grillforge/review-model\"")
    );

    restarted_gui
        .disable()
        .expect("explicit disable restores Claude configuration");
    gateway.deactivate();
    assert!(
        selector::select(&grillforge_root)
            .expect("selector after explicit disable")
            .workers
            .is_empty()
    );
    assert!(!agent_path.exists());
    assert_eq!(
        fs::read_to_string(claude_root.join("settings.json")).expect("final restored settings"),
        original_settings
    );
}
