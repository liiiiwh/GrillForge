use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use grillforge_lib::integration::{IntegrationService, IntegrationTakeover};
use std::fs;
use std::process::Command;

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
    }
}

#[test]
fn apply_and_disable_restore_claude_code_and_install_selector_skill() {
    let directory = tempfile::tempdir().expect("temp directory");
    let grillforge_root = directory.path().join("grillforge");
    let claude_root = directory.path().join("claude");
    fs::create_dir_all(&claude_root).expect("Claude root");
    fs::write(
        claude_root.join("settings.json"),
        r#"{"env":{"UNCHANGED":"yes"}}"#,
    )
    .expect("settings");

    let control = ControlPlaneService::new(&grillforge_root);
    control.save_provider(provider()).expect("provider");
    control.save_model(model("coder-a")).expect("model a");
    control.save_model(model("coder-b")).expect("model b");
    control
        .set_worker("coder-a".into(), true)
        .expect("worker a");
    control
        .set_worker("coder-b".into(), true)
        .expect("worker b");
    control.set_worker_mode(true).expect("worker mode");

    let integration = IntegrationService::new(&claude_root, &grillforge_root);
    let status = integration
        .apply(&control.state().expect("state"), "http://127.0.0.1:15721")
        .expect("apply");

    assert_eq!(status.takeover, IntegrationTakeover::Active);
    assert_eq!(status.generated_agent_names.len(), 3);
    assert!(
        claude_root
            .join("skills/grillforge-model-selector/SKILL.md")
            .is_file()
    );
    let selector = Command::new("python3")
        .arg(claude_root.join("skills/grillforge-model-selector/scripts/select_models.py"))
        .args(["--config-dir"])
        .arg(&grillforge_root)
        .env("GRILLFORGE_BIN", env!("CARGO_BIN_EXE_grillforge"))
        .output()
        .expect("installed selector Skill");
    assert!(
        selector.status.success(),
        "selector failed: {}",
        String::from_utf8_lossy(&selector.stderr)
    );
    let selected: serde_json::Value =
        serde_json::from_slice(&selector.stdout).expect("selector JSON");
    assert_eq!(selected["workers"].as_array().expect("workers").len(), 3);
    assert_eq!(
        selected["workers"][0]["agentName"],
        "grillforge-worker-claude-native"
    );
    assert_eq!(
        selected["workers"][1]["agentName"],
        "grillforge-worker-coder-a"
    );
    let client_official = Command::new("python3")
        .arg(claude_root.join("skills/grillforge-model-selector/scripts/select_models.py"))
        .args(["--config-dir"])
        .arg(&grillforge_root)
        .env("GRILLFORGE_BIN", env!("CARGO_BIN_EXE_grillforge"))
        .env("CLAUDE_CODE_ENTRYPOINT", "claude-desktop")
        .output()
        .expect("Claude Client selector");
    assert!(!client_official.status.success());
    assert!(client_official.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&client_official.stderr)
            .contains("Claude Client Code 正在使用官方路由")
    );
    let settings = fs::read_to_string(claude_root.join("settings.json")).expect("settings");
    assert!(settings.contains("http://127.0.0.1:15721"));
    assert!(settings.contains("UNCHANGED"));
    assert!(!settings.contains("CLAUDE_CODE_SUBAGENT_MODEL"));

    let restored = integration.disable().expect("disable");
    assert_eq!(restored.takeover, IntegrationTakeover::Inactive);
    let disabled_selector = Command::new("python3")
        .arg(claude_root.join("skills/grillforge-model-selector/scripts/select_models.py"))
        .args(["--config-dir"])
        .arg(&grillforge_root)
        .env("GRILLFORGE_BIN", env!("CARGO_BIN_EXE_grillforge"))
        .output()
        .expect("disabled selector");
    assert!(disabled_selector.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&disabled_selector.stdout)
            .expect("disabled selector JSON")["workers"],
        serde_json::json!([])
    );
    assert_eq!(
        fs::read_to_string(claude_root.join("settings.json")).expect("restored settings"),
        "{\n  \"env\": {\n    \"UNCHANGED\": \"yes\"\n  }\n}"
    );
}

#[test]
fn one_external_worker_without_native_fallback_becomes_the_default_and_stays_selectable() {
    let directory = tempfile::tempdir().expect("temp directory");
    let grillforge_root = directory.path().join("grillforge");
    let claude_root = directory.path().join("claude");
    let control = ControlPlaneService::new(&grillforge_root);
    control.save_provider(provider()).expect("provider");
    control.save_model(model("coder")).expect("model");
    control.set_worker("coder".into(), true).expect("worker");
    control.set_worker_mode(true).expect("worker mode");
    control
        .set_native_subagent_enabled(false)
        .expect("disable native fallback");

    IntegrationService::new(&claude_root, &grillforge_root)
        .apply(&control.state().expect("state"), "http://127.0.0.1:15721")
        .expect("apply");

    let settings = fs::read_to_string(claude_root.join("settings.json")).expect("settings");
    assert!(settings.contains("\"CLAUDE_CODE_SUBAGENT_MODEL\": \"grillforge/coder\""));
    assert!(settings.contains("GRILLFORGE_BIN"));
    let agent = fs::read_to_string(claude_root.join("agents/grillforge-worker-coder.md"))
        .expect("generated Agent");
    assert!(agent.contains("model: grillforge/coder"));
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
