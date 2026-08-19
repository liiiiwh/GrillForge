use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use grillforge_lib::integration::{IntegrationService, IntegrationTakeover};
use std::fs;

fn provider() -> ProviderInput {
    ProviderInput {
        id: "local".into(),
        name: "Local".into(),
        protocol: Protocol::OpenAiResponses,
        endpoint: "http://127.0.0.1:8080/v1".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        api_key_placement: ApiKeyPlacement::None,
        api_key: None,
        enabled: true,
        models_url: None,
    }
}

fn model(id: &str) -> ModelInput {
    ModelInput {
        id: id.into(),
        name: id.into(),
        upstream_id: id.into(),
        provider_id: "local".into(),
        capabilities: vec!["coding".into()],
        protocol_capabilities: vec![],
        context_window: None,
        max_output_tokens: None,
    }
}

#[test]
fn native_subagent_model_is_an_explicit_model_slot() {
    let directory = tempfile::tempdir().expect("temp directory");
    let grillforge_root = directory.path().join("grillforge");
    let claude_root = directory.path().join("claude");
    let control = ControlPlaneService::new(&grillforge_root);
    control.save_provider(provider()).expect("provider");
    control.save_model(model("coder")).expect("model");
    control
        .set_model_slot("subagent_default".into(), Some("coder".into()))
        .expect("subagent slot");

    IntegrationService::new(&claude_root, &grillforge_root)
        .apply(&control.state().expect("state"), "http://127.0.0.1:15721")
        .expect("apply");

    let settings = fs::read_to_string(claude_root.join("settings.json")).expect("settings");
    assert!(settings.contains("CLAUDE_CODE_SUBAGENT_MODEL"));
    assert!(settings.contains("grillforge/coder"));
}

#[test]
fn model_slot_selection_is_applied_and_restored() {
    let directory = tempfile::tempdir().expect("temp directory");
    let grillforge_root = directory.path().join("grillforge");
    let claude_root = directory.path().join("claude");
    fs::create_dir_all(&claude_root).expect("Claude root");
    fs::write(
        claude_root.join("settings.json"),
        r#"{"env":{"ANTHROPIC_DEFAULT_SONNET_MODEL":"native-sonnet"}}"#,
    )
    .expect("settings");
    let control = ControlPlaneService::new(&grillforge_root);
    control.save_provider(provider()).expect("provider");
    control.save_model(model("reviewer")).expect("model");
    control
        .set_model_slot("sonnet".into(), Some("reviewer".into()))
        .expect("sonnet slot");

    let integration = IntegrationService::new(&claude_root, &grillforge_root);
    integration
        .apply(&control.state().expect("state"), "http://127.0.0.1:15721")
        .expect("apply");
    let active = fs::read_to_string(claude_root.join("settings.json")).expect("active settings");
    assert!(active.contains("grillforge/reviewer"));

    integration.disable().expect("disable");
    let restored =
        fs::read_to_string(claude_root.join("settings.json")).expect("restored settings");
    assert!(restored.contains("native-sonnet"));
    assert!(!restored.contains("grillforge/reviewer"));
}

#[test]
fn applying_an_empty_native_configuration_is_a_noop() {
    let directory = tempfile::tempdir().expect("temp directory");
    let grillforge_root = directory.path().join("grillforge");
    let claude_root = directory.path().join("claude");
    let mut state = ControlPlaneService::new(&grillforge_root)
        .state()
        .expect("state");
    state.agent_enabled = false;

    let status = IntegrationService::new(&claude_root, &grillforge_root)
        .apply(&state, "http://127.0.0.1:15721")
        .expect("native configuration is valid");

    assert_eq!(status.takeover, IntegrationTakeover::Inactive);
    assert!(!claude_root.join("settings.json").exists());
}

#[test]
fn native_model_choices_apply_without_starting_a_gateway_takeover() {
    let directory = tempfile::tempdir().expect("temp directory");
    let grillforge_root = directory.path().join("grillforge");
    let claude_root = directory.path().join("claude");
    let control = ControlPlaneService::new(&grillforge_root);
    control
        .set_claude_native_model("main".into(), Some("fable".into()))
        .expect("main");
    control
        .set_claude_native_model("subagent_default".into(), Some("haiku".into()))
        .expect("subagent");

    let integration = IntegrationService::new(&claude_root, &grillforge_root);
    let status = integration
        .apply(&control.state().expect("state"), "http://127.0.0.1:15721")
        .expect("apply native models");

    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(claude_root.join("settings.json")).expect("settings"))
            .expect("json");
    assert_eq!(settings["model"], "fable");
    assert_eq!(settings["env"]["CLAUDE_CODE_SUBAGENT_MODEL"], "haiku");
    assert!(settings["env"].get("ANTHROPIC_BASE_URL").is_none());
    assert_eq!(
        status.native_model_slots.get("main").map(String::as_str),
        Some("fable")
    );
}

#[test]
fn status_exposes_the_real_native_catalog_and_current_selection() {
    let directory = tempfile::tempdir().expect("temp directory");
    let grillforge_root = directory.path().join("grillforge");
    let claude_root = directory.path().join(".claude");
    let claude_state = directory.path().join(".claude.json");
    fs::create_dir_all(&claude_root).expect("Claude root");
    fs::write(
        claude_root.join("settings.json"),
        r#"{"model":"claude-opus-4-8[1m]"}"#,
    )
    .expect("settings");
    fs::write(
        &claude_state,
        r#"{"additionalModelOptionsCache":[{"value":"claude-fable-5[1m]"}]}"#,
    )
    .expect("state");

    let status = IntegrationService::new(&claude_root, &grillforge_root)
        .with_native_catalog_paths(&claude_state, None)
        .status()
        .expect("status");

    assert_eq!(
        status.native_current_model.as_deref(),
        Some("claude-opus-4-8[1m]")
    );
    assert!(status.native_models_error.is_none());
    assert!(
        status
            .native_models
            .iter()
            .any(|model| model.id == "claude-fable-5[1m]")
    );
}

#[test]
fn an_unchanged_snapshot_from_an_earlier_process_resumes_automatically() {
    let directory = tempfile::tempdir().expect("temp directory");
    let grillforge_root = directory.path().join("grillforge");
    let claude_root = directory.path().join("claude");
    let control = ControlPlaneService::new(&grillforge_root);
    control.save_provider(provider()).expect("provider");
    control.save_model(model("coder")).expect("model");
    control.set_main_model(Some("coder".into())).expect("main");
    IntegrationService::new(&claude_root, &grillforge_root)
        .apply(&control.state().expect("state"), "http://127.0.0.1:15721")
        .expect("first process apply");

    let restarted = IntegrationService::new(&claude_root, &grillforge_root);
    let gateway = Gateway::new(&grillforge_root).status("http://127.0.0.1:15721".into());
    let resumed = restarted
        .resume_if_applied(&control.state().expect("state"), &gateway)
        .expect("resume unchanged apply");
    let status = restarted.status().expect("resumed status");

    assert!(resumed);
    assert!(status.snapshot_present);
    assert_eq!(status.takeover, IntegrationTakeover::Active);
}

#[test]
fn changed_managed_key_is_reported_and_explicit_reapply_overwrites_it() {
    let directory = tempfile::tempdir().expect("temp directory");
    let grillforge_root = directory.path().join("grillforge");
    let claude_root = directory.path().join("claude");
    let control = ControlPlaneService::new(&grillforge_root);
    control.save_provider(provider()).expect("provider");
    control.save_model(model("coder")).expect("model");
    control.set_main_model(Some("coder".into())).expect("main");
    let state = control.state().expect("state");
    let integration = IntegrationService::new(&claude_root, &grillforge_root);
    integration
        .apply(&state, "http://127.0.0.1:15721")
        .expect("first apply");
    let settings_path = claude_root.join("settings.json");
    let mut settings: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings_path).expect("settings")).expect("JSON");
    settings["env"]["ANTHROPIC_BASE_URL"] = serde_json::json!("http://127.0.0.1:9999");
    fs::write(
        &settings_path,
        serde_json::to_vec_pretty(&settings).unwrap(),
    )
    .unwrap();

    let changed = integration.status().expect("changed status");
    assert_eq!(changed.takeover, IntegrationTakeover::Drifted);
    assert_eq!(changed.differences, ["ANTHROPIC_BASE_URL"]);

    let reapplied = integration
        .apply(&state, "http://127.0.0.1:15721")
        .expect("explicit reapply");
    assert_eq!(reapplied.takeover, IntegrationTakeover::Active);
    assert!(reapplied.differences.is_empty());
}
