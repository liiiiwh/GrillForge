use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::model::{NativeProtocol, ProtocolCapability};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use grillforge_lib::model_discovery::DiscoveredModel;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[tokio::test]
async fn provider_models_can_be_discovered_and_atomically_imported() {
    let upstream = Router::new().route(
        "/v1/models",
        get(|| async {
            Json(json!({
                "object": "list",
                "data": [
                    {"id": "vendor/coder-pro", "owned_by": "vendor"},
                    {"id": "vendor/coder-fast", "owned_by": "vendor"}
                ]
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, upstream).await.expect("upstream") });

    let root = tempfile::tempdir().expect("configuration root");
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "vendor".into(),
            name: "Vendor".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: format!("http://{address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .expect("provider");

    let discovered = service
        .discover_provider_models("vendor")
        .await
        .expect("model discovery");
    assert_eq!(
        discovered
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["vendor/coder-fast", "vendor/coder-pro"]
    );

    let state = service
        .import_provider_models("vendor", discovered)
        .expect("atomic import");
    assert_eq!(state.models.len(), 2);
    assert_eq!(state.models[0].id, "vendor-coder-fast");
    assert_eq!(state.models[0].upstream_id, "vendor/coder-fast");
    assert_eq!(state.models[1].id, "vendor-coder-pro");
}

#[tokio::test]
async fn model_sync_preserves_only_explicit_upstream_native_protocol_metadata() {
    let upstream = Router::new().route(
        "/v1/models",
        get(|| async {
            Json(json!({
                "data": [
                    {"id": "deepseek-v4-flash", "supported_protocols": ["openai_responses"]},
                    {"id": "deepseek-v4-pro", "supported_protocols": ["openai_chat"]},
                    {"id": "future-model"}
                ]
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, upstream).await.expect("upstream") });
    let root = tempfile::tempdir().expect("configuration root");
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "vendor".into(),
            name: "Vendor".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: format!("http://{address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .expect("provider");

    let discovered = service
        .discover_provider_models("vendor")
        .await
        .expect("discovery");
    assert_eq!(
        discovered[0].native_protocols,
        vec![NativeProtocol::OpenAiResponses]
    );
    assert_eq!(
        discovered[1].native_protocols,
        vec![NativeProtocol::OpenAiChat]
    );
    assert!(discovered[2].native_protocols.is_empty());

    let state = service
        .import_provider_models("vendor", discovered)
        .expect("import");
    assert_eq!(
        state.models[0].native_protocols,
        vec![NativeProtocol::OpenAiResponses]
    );
    assert_eq!(
        state.models[1].native_protocols,
        vec![NativeProtocol::OpenAiChat]
    );
    assert!(state.models[2].native_protocols.is_empty());
}

#[test]
fn duplicate_upstream_native_protocol_metadata_is_rejected() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "vendor".into(),
            name: "Vendor".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: "https://api.vendor.example".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("test-key".into()),
            enabled: true,
            models_url: None,
        })
        .expect("provider");

    let error = service
        .import_provider_models(
            "vendor",
            vec![DiscoveredModel {
                id: "coder".into(),
                owned_by: None,
                native_protocols: vec![
                    NativeProtocol::OpenAiResponses,
                    NativeProtocol::OpenAiResponses,
                ],
            }],
        )
        .expect_err("duplicate upstream metadata must fail");

    assert_eq!(error, "duplicate native protocol metadata for model: coder");
}

#[tokio::test]
async fn model_connection_uses_the_models_verified_protocol_not_the_provider_default() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upstream = Router::new()
        .route(
            "/anthropic/v1/messages",
            post(
                |State(calls): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    calls.lock().unwrap().push(body);
                    Json(json!({
                        "id": "msg_pro",
                        "type": "message",
                        "role": "assistant",
                        "model": "deepseek-v4-pro",
                        "content": [{"type": "text", "text": "OK"}],
                        "stop_reason": "end_turn",
                        "stop_sequence": null,
                        "usage": {"input_tokens": 1, "output_tokens": 1}
                    }))
                },
            ),
        )
        .with_state(calls.clone());
    let upstream_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("upstream listener");
    let upstream_address = upstream_listener.local_addr().expect("upstream address");
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream)
            .await
            .expect("upstream")
    });

    let root = tempfile::tempdir().expect("configuration root");
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: format!("http://{upstream_address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    service
        .save_model(ModelInput {
            id: "deepseek-v4-pro".into(),
            name: "DeepSeek V4 Pro".into(),
            upstream_id: "deepseek-v4-pro".into(),
            provider_id: "deepseek".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
        })
        .expect("model");
    service
        .set_model_native_protocols(
            "deepseek-v4-pro",
            vec![
                NativeProtocol::AnthropicMessages,
                NativeProtocol::OpenAiChat,
            ],
        )
        .expect("verified protocol");

    let gateway = Gateway::new(root.path());
    let status = gateway.status("http://127.0.0.1:1".into());
    let _connection_test = status
        .allow_connection_test("deepseek-v4-pro")
        .expect("connection-test route");
    let gateway_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("gateway listener");
    let gateway_address = gateway_listener.local_addr().expect("gateway address");
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway.router())
            .await
            .expect("gateway")
    });

    service
        .test_model_connection(&format!("http://{gateway_address}"), "deepseek-v4-pro")
        .await
        .expect("connection succeeds through native Anthropic route");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["model"], "deepseek-v4-pro");
}

#[test]
fn deepseek_preset_import_preserves_responses_reasoning_capability() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: "https://api.deepseek.com".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("test-key".into()),
            enabled: true,
            models_url: None,
        })
        .expect("provider");

    let state = service
        .import_provider_models(
            "deepseek",
            vec![DiscoveredModel {
                id: "deepseek-v4-flash".into(),
                owned_by: Some("deepseek".into()),
                native_protocols: vec![],
            }],
        )
        .expect("model import");

    assert_eq!(
        state.models[0].protocol_capabilities,
        vec![ProtocolCapability::ReasoningItems]
    );
    assert_eq!(
        state.models[0].native_protocols,
        vec![
            NativeProtocol::AnthropicMessages,
            NativeProtocol::OpenAiResponses,
            NativeProtocol::OpenAiChat,
        ]
    );
}

#[test]
fn pinned_deepseek_pro_metadata_does_not_claim_unavailable_responses_support() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: "https://api.deepseek.com".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("test-key".into()),
            enabled: true,
            models_url: None,
        })
        .expect("provider");

    let state = service
        .import_provider_models(
            "deepseek",
            vec![DiscoveredModel {
                id: "deepseek-v4-pro".into(),
                owned_by: Some("deepseek".into()),
                native_protocols: vec![],
            }],
        )
        .expect("model import");

    assert_eq!(
        state.models[0].native_protocols,
        vec![
            NativeProtocol::AnthropicMessages,
            NativeProtocol::OpenAiChat,
        ]
    );
}

#[test]
fn model_discovery_does_not_invent_protocol_capabilities() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: "https://api.deepseek.com".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("test-key".into()),
            enabled: true,
            models_url: Some("https://api.deepseek.com/models".into()),
        })
        .expect("provider");

    let state = service
        .import_provider_models(
            "deepseek",
            vec![DiscoveredModel {
                id: "future-model-listed-by-models-endpoint".into(),
                owned_by: Some("deepseek".into()),
                native_protocols: vec![],
            }],
        )
        .expect("model import");

    assert!(state.models[0].protocol_capabilities.is_empty());
}

#[test]
fn explicit_chat_reasoning_metadata_is_applied_to_discovered_models() {
    let root = tempfile::tempdir().expect("configuration root");
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "nvidia-chat".into(),
            name: "Nvidia".into(),
            protocol: Protocol::OpenAiChatCompletions,
            endpoint: "https://integrate.api.nvidia.com/v1".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("test-key".into()),
            enabled: true,
            models_url: None,
        })
        .expect("provider");

    let state = service
        .import_provider_models(
            "nvidia-chat",
            vec![DiscoveredModel {
                id: "moonshotai/kimi-k2.5".into(),
                owned_by: Some("nvidia".into()),
                native_protocols: vec![],
            }],
        )
        .expect("model import");

    assert_eq!(
        state.models[0].protocol_capabilities,
        vec![ProtocolCapability::ReasoningContent]
    );
}

#[tokio::test]
async fn gemini_native_models_use_the_google_catalog_and_api_key_header() {
    let upstream = Router::new()
        .route(
            "/v1beta/models",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get("x-goog-api-key")
                        .and_then(|value| value.to_str().ok()),
                    Some("gemini-key")
                );
                Json(json!({
                    "models": [
                        {"name": "models/gemini-2.5-pro", "displayName": "Gemini 2.5 Pro"},
                        {"name": "models/gemini-2.5-flash", "displayName": "Gemini 2.5 Flash"}
                    ]
                }))
            }),
        )
        .route(
            "/v1beta/models/gemini-2.5-pro:generateContent",
            axum::routing::post(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get("x-goog-api-key")
                        .and_then(|value| value.to_str().ok()),
                    Some("gemini-key")
                );
                Json(json!({"candidates": [{"content": {"parts": [{"text": "OK"}]}}]}))
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, upstream).await.expect("upstream") });
    let root = tempfile::tempdir().expect("configuration root");
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "google".into(),
            name: "Google Gemini".into(),
            protocol: Protocol::GeminiNative,
            endpoint: format!("http://{address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::XApiKey,
            api_key: Some("gemini-key".into()),
            enabled: true,
            models_url: None,
        })
        .expect("provider");

    let discovered = service
        .discover_provider_models("google")
        .await
        .expect("Gemini model discovery");
    assert_eq!(
        discovered
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["gemini-2.5-flash", "gemini-2.5-pro"]
    );
    let imported = service
        .import_provider_models("google", discovered)
        .expect("Gemini model import");
    let model_id = imported
        .models
        .iter()
        .find(|model| model.upstream_id == "gemini-2.5-pro")
        .unwrap()
        .id
        .clone();
    let connected = service
        .test_model_connection("http://127.0.0.1:1", &model_id)
        .await
        .expect("Gemini direct connection");
    assert_eq!(connected.upstream_id, "gemini-2.5-pro");
}
