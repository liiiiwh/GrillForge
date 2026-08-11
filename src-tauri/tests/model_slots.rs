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
