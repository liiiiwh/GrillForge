use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};

fn provider(id: &str, protocol: Protocol, placement: ApiKeyPlacement) -> ProviderInput {
    ProviderInput {
        id: id.into(),
        name: id.into(),
        protocol,
        endpoint: "https://api.example.com/v1".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        api_key_placement: placement,
        api_key: Some("secret".into()),
        enabled: true,
        models_url: None,
    }
}

fn model(id: &str, provider_id: &str) -> ModelInput {
    ModelInput {
        id: id.into(),
        name: id.into(),
        upstream_id: id.into(),
        provider_id: provider_id.into(),
        capabilities: vec!["coding".into()],
        protocol_capabilities: vec![],
    }
}

#[test]
fn generic_clients_keep_independent_main_and_model_pool_selections() {
    let temp = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(temp.path());
    service
        .save_provider(provider(
            "responses",
            Protocol::OpenAiResponses,
            ApiKeyPlacement::Bearer,
        ))
        .unwrap();
    service.save_model(model("coder", "responses")).unwrap();
    service.save_model(model("reviewer", "responses")).unwrap();

    service
        .set_client_main_model("opencode".into(), Some("coder".into()))
        .unwrap();
    let state = service
        .set_client_model_enabled("opencode".into(), "reviewer".into(), true)
        .unwrap();
    assert_eq!(
        state.client_configurations["opencode"]
            .main_model_id
            .as_deref(),
        Some("coder")
    );
    assert_eq!(
        state.client_configurations["opencode"].enabled_model_ids,
        ["coder", "reviewer"]
    );
    assert_eq!(state.client_configurations["openclaw"].main_model_id, None);
    service
        .set_client_main_model("hermes".into(), Some("reviewer".into()))
        .unwrap();
    let state = service
        .set_client_model_enabled("hermes".into(), "coder".into(), true)
        .unwrap();
    assert_eq!(
        state.client_configurations["hermes"].enabled_model_ids,
        ["coder", "reviewer"]
    );
    assert!(
        service
            .set_client_model_enabled("opencode".into(), "coder".into(), false)
            .unwrap_err()
            .contains("main model")
    );
}

#[test]
fn client_selection_rejects_incompatible_provider_protocols_at_save_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(temp.path());
    service
        .save_provider(provider(
            "responses",
            Protocol::OpenAiResponses,
            ApiKeyPlacement::Bearer,
        ))
        .unwrap();
    service.save_model(model("coder", "responses")).unwrap();
    assert!(
        service
            .set_client_main_model("gemini".into(), Some("coder".into()))
            .unwrap_err()
            .contains("incompatible")
    );
    assert!(
        service
            .set_client_main_model("unknown".into(), Some("coder".into()))
            .unwrap_err()
            .contains("unsupported")
    );
}

#[test]
fn kimi_code_keeps_primary_secondary_and_available_models_as_distinct_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(temp.path());
    service
        .save_provider(provider(
            "responses",
            Protocol::OpenAiResponses,
            ApiKeyPlacement::Bearer,
        ))
        .unwrap();
    service.save_model(model("primary", "responses")).unwrap();
    service.save_model(model("secondary", "responses")).unwrap();

    service
        .set_client_main_model("kimi_code".into(), Some("primary".into()))
        .unwrap();
    let state = service
        .set_client_secondary_model("kimi_code".into(), Some("secondary".into()))
        .unwrap();

    assert_eq!(
        state.client_configurations["kimi_code"]
            .main_model_id
            .as_deref(),
        Some("primary")
    );
    assert_eq!(
        state.client_configurations["kimi_code"]
            .secondary_model_id
            .as_deref(),
        Some("secondary")
    );
    assert_eq!(
        state.client_configurations["kimi_code"].enabled_model_ids,
        ["primary", "secondary"]
    );
}
