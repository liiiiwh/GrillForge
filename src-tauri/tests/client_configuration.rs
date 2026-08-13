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
fn removed_client_is_not_known_or_exposed_in_public_state() {
    let temp = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(temp.path());
    let removed = ["open", "claw"].concat();

    let state = service.state().unwrap();
    assert!(!state.client_configurations.contains_key(&removed));
    assert!(!state.client_extension_subagent_ids.contains_key(&removed));
    assert!(
        service
            .client_integration_enabled(&removed)
            .unwrap_err()
            .contains("unsupported")
    );
    assert!(
        service
            .set_client_main_model(removed, None)
            .unwrap_err()
            .contains("unsupported")
    );
}

#[test]
fn gemini_client_accepts_unified_provider_routing_at_save_boundary() {
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
    let state = service
        .set_client_main_model("gemini".into(), Some("coder".into()))
        .expect("Gemini client may route through Responses");
    assert_eq!(
        state.client_configurations["gemini"]
            .main_model_id
            .as_deref(),
        Some("coder")
    );
    assert!(
        service
            .set_client_main_model("unknown".into(), Some("coder".into()))
            .unwrap_err()
            .contains("unsupported")
    );
}

#[test]
fn kimi_code_exposes_only_default_and_available_models() {
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
    service.save_model(model("pool", "responses")).unwrap();

    service
        .set_client_main_model("kimi_code".into(), Some("primary".into()))
        .unwrap();
    let state = service
        .set_client_model_enabled("kimi_code".into(), "pool".into(), true)
        .unwrap();

    assert_eq!(
        state.client_configurations["kimi_code"]
            .main_model_id
            .as_deref(),
        Some("primary")
    );
    assert_eq!(
        state.client_configurations["kimi_code"].enabled_model_ids,
        ["pool", "primary"]
    );
    let public = serde_json::to_value(&state.client_configurations["kimi_code"]).unwrap();
    assert!(public.get("secondaryModelId").is_none());
}
