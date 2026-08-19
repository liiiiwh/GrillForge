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
        id: "desktop-sonnet".into(),
        name: "Desktop Sonnet".into(),
        upstream_id: "desktop-sonnet".into(),
        provider_id: "local".into(),
        capabilities: vec!["coding".into()],
        protocol_capabilities: vec![],
        context_window: None,
        max_output_tokens: None,
    }
}

#[test]
fn claude_desktop_slot_is_independent_from_claude_code() {
    let root = tempfile::tempdir().expect("config root");
    let service = ControlPlaneService::new(root.path());
    service.save_provider(provider()).expect("provider");
    service.save_model(model()).expect("model");

    let state = service
        .set_claude_desktop_model_slot("sonnet".into(), Some("desktop-sonnet".into()))
        .expect("desktop slot");

    assert!(state.model_slots.is_empty());
    assert_eq!(
        state
            .claude_desktop_model_slots
            .get("sonnet")
            .map(String::as_str),
        Some("desktop-sonnet")
    );
    assert!(service.delete_model("desktop-sonnet").is_err());
}

#[test]
fn invalid_desktop_slot_preserves_the_previous_valid_state() {
    let root = tempfile::tempdir().expect("config root");
    let service = ControlPlaneService::new(root.path());

    let error = service
        .set_claude_desktop_model_slot("main".into(), None)
        .expect_err("unsupported desktop slot");

    assert_eq!(error, "unsupported Claude Client model slot: main");
    assert!(
        service
            .state()
            .expect("state")
            .claude_desktop_model_slots
            .is_empty()
    );
}
