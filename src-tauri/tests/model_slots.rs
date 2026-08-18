use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};

fn provider() -> ProviderInput {
    ProviderInput {
        id: "local".into(),
        name: "Local".into(),
        protocol: Protocol::AnthropicMessages,
        endpoint: "http://127.0.0.1:8080".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        api_key_placement: ApiKeyPlacement::None,
        api_key: None,
        enabled: true,
        models_url: None,
    }
}

fn model() -> ModelInput {
    ModelInput {
        id: "reviewer".into(),
        name: "Reviewer".into(),
        upstream_id: "reviewer".into(),
        provider_id: "local".into(),
        capabilities: vec!["review".into()],
        protocol_capabilities: vec![],
            context_window: None,
        max_output_tokens: None,
    }
}

#[test]
fn claude_code_slots_are_persisted_and_exposed() {
    let root = tempfile::tempdir().expect("config root");
    let service = ControlPlaneService::new(root.path());
    service.save_provider(provider()).expect("provider");
    service.save_model(model()).expect("model");

    let state = service
        .set_model_slot("sonnet".into(), Some("reviewer".into()))
        .expect("slot");

    assert_eq!(
        state.model_slots.get("sonnet").map(String::as_str),
        Some("reviewer")
    );
    assert!(service.delete_model("reviewer").is_err());
}

#[test]
fn unsupported_or_unknown_slot_selection_fails_without_mutation() {
    let root = tempfile::tempdir().expect("config root");
    let service = ControlPlaneService::new(root.path());

    assert!(
        service
            .set_model_slot("unknown".into(), None)
            .expect_err("unsupported slot")
            .contains("unsupported Claude Code model slot")
    );
    assert!(
        service
            .set_model_slot("sonnet".into(), Some("missing".into()))
            .expect_err("unknown model")
            .contains("unknown model slot selection")
    );
    assert!(service.state().expect("state").model_slots.is_empty());
}

#[test]
fn managed_slot_rejects_a_model_without_a_verified_route() {
    let root = tempfile::tempdir().expect("config root");
    let service = ControlPlaneService::new(root.path());
    service.save_provider(provider()).expect("provider");
    service.save_model(model()).expect("model");
    service
        .update_provider(provider())
        .expect("editing a provider invalidates its protocol probes");

    let error = service
        .set_model_slot("sonnet".into(), Some("reviewer".into()))
        .expect_err("an untested model must not be assigned to a slot");

    assert_eq!(
        error,
        "Claude Code model slot sonnet model reviewer has not been protocol-tested; synchronize provider local models first"
    );
    assert!(
        service
            .state()
            .expect("unchanged state")
            .model_slots
            .is_empty()
    );
}

#[test]
fn native_claude_models_are_persisted_without_a_provider() {
    let root = tempfile::tempdir().expect("config root");
    let service = ControlPlaneService::new(root.path());

    let state = service
        .set_claude_native_model("main".into(), Some("fable".into()))
        .expect("native main");
    assert_eq!(state.main_model_id, None);
    assert_eq!(
        state
            .claude_native_model_slots
            .get("main")
            .map(String::as_str),
        Some("fable")
    );
    assert!(state.providers.is_empty());

    let state = service
        .set_claude_native_model("subagent_default".into(), Some("haiku".into()))
        .expect("native subagent");
    assert_eq!(
        state
            .claude_native_model_slots
            .get("subagent_default")
            .map(String::as_str),
        Some("haiku")
    );
}

#[test]
fn versioned_native_claude_models_are_persisted_exactly() {
    let root = tempfile::tempdir().expect("config root");
    let service = ControlPlaneService::new(root.path());

    let state = service
        .set_claude_native_model("main".into(), Some("claude-opus-4-8[1m]".into()))
        .expect("versioned native main");
    assert_eq!(
        state
            .claude_native_model_slots
            .get("main")
            .map(String::as_str),
        Some("claude-opus-4-8[1m]")
    );

    let state = service
        .set_claude_native_model("subagent_default".into(), Some("claude-sonnet-5".into()))
        .expect("versioned native subagent");
    assert_eq!(
        state
            .claude_native_model_slots
            .get("subagent_default")
            .map(String::as_str),
        Some("claude-sonnet-5")
    );
}

#[test]
fn unsupported_native_claude_model_fails_without_mutation() {
    let root = tempfile::tempdir().expect("config root");
    let service = ControlPlaneService::new(root.path());

    assert!(
        service
            .set_claude_native_model("main".into(), Some("made-up".into()))
            .expect_err("unsupported native model")
            .contains("unsupported Claude Code native model")
    );
    assert!(
        service
            .state()
            .expect("state")
            .claude_native_model_slots
            .is_empty()
    );
}
