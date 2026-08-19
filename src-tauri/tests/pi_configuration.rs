use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use tempfile::tempdir;

#[test]
fn pi_default_and_enabled_models_are_independent_from_claude() {
    let temp = tempdir().unwrap();
    let service = ControlPlaneService::new(temp.path());
    service
        .save_provider(ProviderInput {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            protocol: Protocol::OpenAiChatCompletions,
            endpoint: "https://api.deepseek.com".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("test-key".into()),
            enabled: true,
            models_url: None,
        })
        .unwrap();
    for id in ["deepseek-chat", "deepseek-reasoner"] {
        service
            .save_model(ModelInput {
                id: id.into(),
                name: id.into(),
                upstream_id: id.into(),
                provider_id: "deepseek".into(),
                capabilities: vec!["coding".into()],
                protocol_capabilities: Vec::new(),
                context_window: None,
                max_output_tokens: None,
            })
            .unwrap();
    }

    service
        .set_pi_model_enabled("deepseek-chat".into(), true)
        .unwrap();
    service
        .set_pi_model_enabled("deepseek-reasoner".into(), true)
        .unwrap();
    let state = service
        .set_pi_main_model(Some("deepseek-reasoner".into()))
        .unwrap();

    assert_eq!(state.pi_main_model_id.as_deref(), Some("deepseek-reasoner"));
    assert_eq!(
        state.pi_enabled_model_ids,
        vec!["deepseek-chat", "deepseek-reasoner"]
    );
    assert!(
        !state.pi_enabled,
        "saving Pi choices does not apply the client"
    );
    assert!(state.model_slots.is_empty());
}
