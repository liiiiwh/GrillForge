use axum::{Json, Router, routing::post};
use grillforge_lib::adapters::codex::{CodexAdapter, CodexPaths};
use grillforge_lib::application::{
    ControlPlaneService, ExtensionSubAgentInput, ModelInput, ProviderInput,
};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use serde_json::{Value, json};
use tokio::net::TcpListener;

fn provider() -> ProviderInput {
    ProviderInput {
        id: "local".into(),
        name: "Local".into(),
        protocol: Protocol::OpenAiChatCompletions,
        endpoint: "http://127.0.0.1:8080/v1".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        api_key_placement: ApiKeyPlacement::Bearer,
        api_key: Some("local-secret".into()),
        enabled: true,
        models_url: None,
    }
}

fn model() -> ModelInput {
    ModelInput {
        id: "local-coder".into(),
        name: "Local Coder".into(),
        upstream_id: "coder".into(),
        provider_id: "local".into(),
        capabilities: vec!["coding".into()],
        protocol_capabilities: vec![],
        context_window: None,
        max_output_tokens: None,
    }
}

#[test]
fn public_state_never_contains_provider_secrets() {
    let directory = tempfile::tempdir().expect("temp directory");
    let service = ControlPlaneService::new(directory.path());

    let state = service.save_provider(provider()).expect("valid provider");
    let json = serde_json::to_string(&state).expect("public JSON");

    assert!(state.providers[0].credential_set);
    assert!(!json.contains("local-secret"));
    assert!(!json.contains("\"apiKey\":"));
}

#[test]
fn creating_the_same_provider_slug_twice_is_rejected() {
    let directory = tempfile::tempdir().expect("temp directory");
    let service = ControlPlaneService::new(directory.path());
    service.save_provider(provider()).expect("first provider");

    let error = service
        .save_provider(provider())
        .expect_err("duplicate create must fail");

    assert_eq!(error, "duplicate provider id: local");
}

#[test]
fn deleting_a_provider_cascades_its_unreferenced_models() {
    let directory = tempfile::tempdir().expect("temp directory");
    let service = ControlPlaneService::new(directory.path());
    service.save_provider(provider()).expect("provider");
    service.save_model(model()).expect("model");

    let state = service.delete_provider("local").expect("provider deletion");

    assert!(state.providers.is_empty());
    assert!(state.models.is_empty());
}

#[test]
fn deleting_a_provider_refuses_models_selected_by_a_client() {
    let directory = tempfile::tempdir().expect("temp directory");
    let service = ControlPlaneService::new(directory.path());
    service.save_provider(provider()).expect("provider");
    service.save_model(model()).expect("model");
    service
        .set_main_model(Some("local-coder".into()))
        .expect("selection");

    let error = service
        .delete_provider("local")
        .expect_err("selected model must block deletion");
    let state = service.state().expect("unchanged state");

    assert!(error.contains("local-coder"));
    assert!(error.contains("claude_code"));
    assert_eq!(state.providers.len(), 1);
    assert_eq!(state.models.len(), 1);
}

#[test]
fn deleting_a_provider_refuses_models_selected_by_an_extension_subagent() {
    let directory = tempfile::tempdir().expect("temp directory");
    let service = ControlPlaneService::new(directory.path());
    service.save_provider(provider()).expect("provider");
    service.save_model(model()).expect("model");
    service
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: Some("local-coder".into()),
            capabilities: vec![],
        })
        .expect("extension SubAgent");

    let error = service
        .delete_provider("local")
        .expect_err("extension model must block deletion");
    let state = service.state().expect("unchanged state");

    assert!(error.contains("local-coder"));
    assert!(error.contains("reviewer"));
    assert_eq!(state.providers.len(), 1);
    assert_eq!(state.models.len(), 1);
}

#[test]
fn explicit_provider_update_can_rotate_a_key_without_exposing_it() {
    let directory = tempfile::tempdir().expect("temp directory");
    let service = ControlPlaneService::new(directory.path());
    service.save_provider(provider()).expect("provider");
    let mut changed = provider();
    changed.name = "Updated Local".into();
    changed.api_key = Some("rotated-secret".into());

    let state = service.update_provider(changed).expect("provider update");

    assert_eq!(state.providers[0].name, "Updated Local");
    let serialized = serde_json::to_string(&state).expect("public state");
    assert!(!serialized.contains("rotated-secret"));
}

#[test]
fn codex_accepts_chat_provider_through_the_explicit_local_route() {
    let directory = tempfile::tempdir().expect("temp directory");
    let service = ControlPlaneService::new(directory.path());
    service.save_provider(provider()).expect("Chat provider");
    service.save_model(model()).expect("Chat model");

    let state = service
        .set_codex_main_model(Some("local-coder".into()))
        .expect("Codex Chat route");

    assert_eq!(state.codex_main_model_id.as_deref(), Some("local-coder"));
}

#[test]
fn codex_accepts_anthropic_provider_through_the_explicit_responses_bridge() {
    let directory = tempfile::tempdir().expect("temp directory");
    let service = ControlPlaneService::new(directory.path());
    let mut anthropic = provider();
    anthropic.protocol = Protocol::AnthropicMessages;
    anthropic.api_key_placement = ApiKeyPlacement::XApiKey;
    service
        .save_provider(anthropic)
        .expect("Anthropic provider");
    service.save_model(model()).expect("Anthropic model");

    let state = service
        .set_codex_main_model(Some("local-coder".into()))
        .expect("Codex Anthropic bridge route");

    assert_eq!(state.codex_main_model_id.as_deref(), Some("local-coder"));
}

#[test]
fn codex_can_keep_the_current_real_main_model_while_configuring_subagents() {
    let directory = tempfile::tempdir().expect("temp directory");
    let config = directory.path().join("home/.codex/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        "model = \"gpt-5.6-sol\"\nmodel_provider = \"openai\"\n",
    )
    .unwrap();
    let service = ControlPlaneService::new(directory.path().join("grillforge"));
    service
        .set_codex_native_default_subagent_model(Some("gpt-5.4-mini".into()))
        .unwrap();
    let adapter = CodexAdapter::new(
        CodexPaths::new(config.clone()),
        directory.path().join("grillforge"),
    );
    let current = adapter.configured_model().unwrap();

    let request = service
        .codex_request("http://127.0.0.1:15721", "token", current.as_ref())
        .unwrap();
    adapter.apply(request).unwrap();

    let written = std::fs::read_to_string(config)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert_eq!(written["model"].as_str(), Some("gpt-5.6-sol"));
    assert_eq!(written["model_provider"].as_str(), Some("openai"));
    assert_eq!(
        written["agents"]["default_subagent_model"].as_str(),
        Some("gpt-5.4-mini")
    );
}

#[test]
fn claude_and_pi_accept_gemini_native_registry_models() {
    let directory = tempfile::tempdir().expect("temp directory");
    let service = ControlPlaneService::new(directory.path());
    service
        .save_provider(ProviderInput {
            id: "gemini".into(),
            name: "Gemini".into(),
            protocol: Protocol::GeminiNative,
            endpoint: "https://generativelanguage.googleapis.com".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::XApiKey,
            api_key: Some("test-key".into()),
            enabled: true,
            models_url: None,
        })
        .expect("Gemini provider");
    service
        .save_model(ModelInput {
            id: "gemini-pro".into(),
            name: "Gemini Pro".into(),
            upstream_id: "gemini-2.5-pro".into(),
            provider_id: "gemini".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
            context_window: None,
            max_output_tokens: None,
        })
        .expect("Gemini model");

    service
        .set_main_model(Some("gemini-pro".into()))
        .expect("Claude main selection");
    let state = service
        .set_pi_main_model(Some("gemini-pro".into()))
        .expect("Pi main selection");

    assert_eq!(state.main_model_id.as_deref(), Some("gemini-pro"));
    assert_eq!(state.pi_main_model_id.as_deref(), Some("gemini-pro"));
}

#[test]
fn client_integration_state_is_persisted_separately_from_model_selection() {
    let directory = tempfile::tempdir().expect("temp directory");
    let service = ControlPlaneService::new(directory.path());
    service.save_provider(provider()).expect("provider");
    service.save_model(model()).expect("model");
    service
        .set_client_main_model("opencode".into(), Some("local-coder".into()))
        .expect("saved selection");

    service
        .set_client_integration_enabled("opencode", false)
        .expect("disabled integration");
    assert!(!service.client_integration_enabled("opencode").unwrap());
    assert!(
        service
            .client_has_managed_configuration("opencode")
            .unwrap()
    );

    service
        .set_client_integration_enabled("opencode", true)
        .expect("enabled integration");
    assert!(service.client_integration_enabled("opencode").unwrap());
}

#[test]
fn extension_bindings_do_not_turn_on_the_client_model_configuration() {
    let directory = tempfile::tempdir().expect("temp directory");
    let service = ControlPlaneService::new(directory.path());
    service
        .save_extension_subagent(ExtensionSubAgentInput {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            source_client_id: "claude_code".into(),
            source_agent_id: "reviewer".into(),
            model_id: None,
            capabilities: vec![],
        })
        .expect("extension");
    service
        .set_client_extension_subagent_enabled("opencode", "reviewer", true)
        .expect("binding");

    assert!(!service.client_integration_enabled("opencode").unwrap());
    assert!(
        !service
            .client_has_managed_configuration("opencode")
            .unwrap()
    );
    assert_eq!(
        service.state().unwrap().client_extension_subagent_ids["opencode"],
        ["reviewer"]
    );
}

#[tokio::test]
async fn explicit_model_connection_test_returns_the_resolved_route() {
    let gateway = Router::new().route(
        "/v1/messages",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["model"], "grillforge/local-coder");
            Json(json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "OK"}],
                "model": "coder",
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {"input_tokens": 1, "output_tokens": 1}
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, gateway).await.expect("serve gateway") });

    let directory = tempfile::tempdir().expect("temp directory");
    let service = ControlPlaneService::new(directory.path());
    service.save_provider(provider()).expect("provider");
    service.save_model(model()).expect("model");

    let connected = service
        .test_model_connection(&format!("http://{address}"), "local-coder")
        .await
        .expect("connected model");

    assert_eq!(connected.model_id, "local-coder");
    assert_eq!(connected.provider_id, "local");
    assert_eq!(connected.upstream_id, "coder");
}
