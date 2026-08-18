use grillforge_lib::adapters::claude_desktop::macos_paths_from_home;
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::claude_desktop_integration::ClaudeDesktopIntegrationService;
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use grillforge_lib::integration::IntegrationTakeover;
use std::fs;

fn provider() -> ProviderInput {
    ProviderInput {
        id: "local".into(),
        name: "Local".into(),
        protocol: Protocol::AnthropicMessages,
        endpoint: "http://127.0.0.1:18080".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        api_key_placement: ApiKeyPlacement::None,
        api_key: None,
        enabled: true,
        models_url: None,
    }
}

fn model() -> ModelInput {
    ModelInput {
        id: "desktop-model".into(),
        name: "Desktop Model".into(),
        upstream_id: "upstream-model".into(),
        provider_id: "local".into(),
        capabilities: vec!["coding".into()],
        protocol_capabilities: vec![],
            context_window: None,
        max_output_tokens: None,
    }
}

fn configure_complete_third_party_mode(control: &ControlPlaneService) {
    control.save_provider(provider()).expect("provider");
    control.save_model(model()).expect("model");
    for slot in ["sonnet", "opus", "fable", "haiku"] {
        control
            .set_claude_desktop_model_slot(slot.into(), Some("desktop-model".into()))
            .expect("Desktop role slot");
    }
    control
        .set_model_slot("subagent_default".into(), Some("desktop-model".into()))
        .expect("Code SubAgent slot");
}

#[test]
fn desktop_apply_writes_only_desktop_profile_and_restores_it() {
    let directory = tempfile::tempdir().expect("temp directory");
    let home = directory.path().join("home");
    let grillforge = directory.path().join("grillforge");
    let claude_code_settings = home.join(".claude/settings.json");
    fs::create_dir_all(claude_code_settings.parent().expect("settings parent"))
        .expect("settings parent");
    fs::write(&claude_code_settings, br#"{"env":{"UNCHANGED":"yes"}}"#)
        .expect("Claude Code settings");

    let control = ControlPlaneService::new(&grillforge);
    configure_complete_third_party_mode(&control);
    let state = control.state().expect("state");
    let paths = macos_paths_from_home(&home);
    let profile_path = paths.profile_path.clone();
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());
    let integration = ClaudeDesktopIntegrationService::new(paths, &grillforge);

    let active = integration.apply(&state, &gateway).expect("Desktop apply");

    assert_eq!(active.takeover, IntegrationTakeover::Active);
    assert_eq!(active.configured_routes.len(), 4);
    let profile: serde_json::Value =
        serde_json::from_slice(&fs::read(&profile_path).expect("Desktop profile"))
            .expect("Desktop profile JSON");
    assert_eq!(
        profile["inferenceGatewayBaseUrl"],
        "http://127.0.0.1:15721/claude-desktop"
    );
    assert_eq!(profile["inferenceModels"].as_array().unwrap().len(), 4);
    assert_eq!(
        fs::read_to_string(&claude_code_settings).expect("Claude Code settings unchanged"),
        r#"{"env":{"UNCHANGED":"yes"}}"#
    );

    let inactive = integration.disable(&gateway).expect("Desktop disable");
    assert_eq!(inactive.takeover, IntegrationTakeover::Inactive);
    assert!(!profile_path.exists());
}

#[test]
fn desktop_conversation_routes_include_every_required_role() {
    let directory = tempfile::tempdir().expect("temp directory");
    let home = directory.path().join("home");
    let grillforge = directory.path().join("grillforge");
    let control = ControlPlaneService::new(&grillforge);
    configure_complete_third_party_mode(&control);
    let state = control.state().expect("state");
    let paths = macos_paths_from_home(&home);
    let profile_path = paths.profile_path.clone();
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    let active = ClaudeDesktopIntegrationService::new(paths, &grillforge)
        .apply(&state, &gateway)
        .expect("Desktop apply");

    assert_eq!(active.configured_routes.len(), 4);
    assert!(profile_path.exists());
}

#[test]
fn restarted_desktop_integration_resumes_automatically() {
    let directory = tempfile::tempdir().expect("temp directory");
    let home = directory.path().join("home");
    let grillforge = directory.path().join("grillforge");
    let control = ControlPlaneService::new(&grillforge);
    configure_complete_third_party_mode(&control);
    let state = control.state().expect("state");
    let paths = macos_paths_from_home(&home);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());
    ClaudeDesktopIntegrationService::new(paths.clone(), &grillforge)
        .apply(&state, &gateway)
        .expect("first process apply");

    let restarted = ClaudeDesktopIntegrationService::new(paths, &grillforge);
    let resumed = restarted
        .resume_if_applied(&state, &gateway)
        .expect("resume unchanged apply");
    let status = restarted.status().expect("resumed status");

    assert!(resumed);
    assert!(status.snapshot_present);
    assert_eq!(status.takeover, IntegrationTakeover::Active);
}

#[test]
fn changed_desktop_profile_is_reported_and_explicit_reapply_overwrites_it() {
    let directory = tempfile::tempdir().expect("temp directory");
    let home = directory.path().join("home");
    let grillforge = directory.path().join("grillforge");
    let control = ControlPlaneService::new(&grillforge);
    configure_complete_third_party_mode(&control);
    let state = control.state().expect("state");
    let paths = macos_paths_from_home(&home);
    let profile = paths.profile_path.clone();
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());
    let integration = ClaudeDesktopIntegrationService::new(paths, &grillforge);
    integration.apply(&state, &gateway).expect("first apply");
    fs::write(&profile, br#"{"changed":true}"#).expect("change profile");

    let changed = integration.status().expect("changed status");
    assert_eq!(changed.takeover, IntegrationTakeover::Drifted);
    assert_eq!(
        changed.differences,
        ["Claude-3p/configLibrary/GrillForge.json"]
    );

    let reapplied = integration
        .apply(&state, &gateway)
        .expect("explicit reapply");
    assert_eq!(reapplied.takeover, IntegrationTakeover::Active);
    assert!(reapplied.differences.is_empty());
}

#[test]
fn empty_desktop_role_mapping_fails_before_writing_any_profile() {
    let directory = tempfile::tempdir().expect("temp directory");
    let home = directory.path().join("home");
    let grillforge = directory.path().join("grillforge");
    let control = ControlPlaneService::new(&grillforge);
    let paths = macos_paths_from_home(&home);
    let profile_path = paths.profile_path.clone();
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    let status = ClaudeDesktopIntegrationService::new(paths, &grillforge)
        .apply(&control.state().expect("state"), &gateway)
        .expect("all-native Desktop mode");

    assert_eq!(status.takeover, IntegrationTakeover::Inactive);
    assert!(!profile_path.exists());
}

#[test]
fn desktop_third_party_mode_requires_all_role_and_code_subagent_slots() {
    let directory = tempfile::tempdir().expect("temp directory");
    let home = directory.path().join("home");
    let grillforge = directory.path().join("grillforge");
    let control = ControlPlaneService::new(&grillforge);
    control.save_provider(provider()).expect("provider");
    control.save_model(model()).expect("model");
    control
        .set_claude_desktop_model_slot("sonnet".into(), Some("desktop-model".into()))
        .expect("Desktop slot");
    let paths = macos_paths_from_home(&home);
    let profile_path = paths.profile_path.clone();
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    let error = ClaudeDesktopIntegrationService::new(paths, &grillforge)
        .apply(&control.state().expect("state"), &gateway)
        .expect_err("mixed 1P/3P mapping");

    assert_eq!(
        error,
        "Claude Client 不能混合 1P 与 3P 模型；请同时配置 Sonnet、Opus、Fable、Haiku 和 Code SubAgent 默认模型，或全部恢复跟随原生"
    );
    assert!(!profile_path.exists());
}

#[test]
fn desktop_third_party_mode_accepts_four_roles_and_code_subagent_model() {
    let directory = tempfile::tempdir().expect("temp directory");
    let home = directory.path().join("home");
    let grillforge = directory.path().join("grillforge");
    let control = ControlPlaneService::new(&grillforge);
    configure_complete_third_party_mode(&control);
    let paths = macos_paths_from_home(&home);
    let gateway = Gateway::new(&grillforge).status("http://127.0.0.1:15721".into());

    let active = ClaudeDesktopIntegrationService::new(paths, &grillforge)
        .apply(&control.state().expect("state"), &gateway)
        .expect("complete 3P mapping");

    assert_eq!(active.configured_routes.len(), 4);
}
