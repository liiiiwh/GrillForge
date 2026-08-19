use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use grillforge_lib::application::{
    ControlPlaneService, ModelInput, ModelWithNativeProtocolsInput, ProviderInput,
};
use grillforge_lib::core::model::{NativeProtocol, ProtocolCapability};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use serde_json::{Value, json};
use std::env;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::timeout;

#[tokio::test]
#[ignore = "uses the explicitly supplied DeepSeek key and sends bounded live protocol probes"]
async fn live_deepseek_sync_records_protocols_and_connects_both_v4_models() {
    let key =
        env::var("GRILLFORGE_LIVE_DEEPSEEK_KEY").expect("GRILLFORGE_LIVE_DEEPSEEK_KEY must be set");
    let root = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: "https://api.deepseek.com".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some(key),
            enabled: true,
            models_url: None,
        })
        .unwrap();

    let synchronized = timeout(
        Duration::from_secs(180),
        service.sync_provider_models("deepseek"),
    )
    .await
    .expect("DeepSeek protocol synchronization timed out")
    .expect("DeepSeek protocol synchronization");
    let provider = synchronized
        .providers
        .iter()
        .find(|provider| provider.id == "deepseek")
        .unwrap();
    assert!(!provider.protocol_endpoints.is_empty());
    for upstream_id in ["deepseek-v4-flash", "deepseek-v4-pro"] {
        let model = synchronized
            .models
            .iter()
            .find(|model| model.upstream_id == upstream_id)
            .unwrap_or_else(|| panic!("DeepSeek catalog did not return {upstream_id}"));
        assert!(
            !model.native_protocols.is_empty(),
            "{upstream_id} must support at least one probed protocol"
        );
    }

    let gateway = Gateway::new(root.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let gateway_base_url = format!("http://{address}");
    tokio::spawn({
        let router = gateway.router();
        async move { axum::serve(listener, router).await.unwrap() }
    });
    for upstream_id in ["deepseek-v4-flash", "deepseek-v4-pro"] {
        let model_id = service
            .state()
            .unwrap()
            .models
            .into_iter()
            .find(|model| model.upstream_id == upstream_id)
            .unwrap()
            .id;
        let state = service.set_main_model(Some(model_id.clone())).unwrap();
        gateway
            .status(gateway_base_url.clone())
            .activate(&state)
            .unwrap();
        timeout(
            Duration::from_secs(90),
            service.test_model_connection(&gateway_base_url, &model_id),
        )
        .await
        .unwrap_or_else(|_| panic!("{upstream_id} connection test timed out"))
        .unwrap_or_else(|error| panic!("{upstream_id} connection test failed: {error}"));
    }
}

#[tokio::test]
#[ignore = "uses the explicitly supplied Kimi key and sends bounded live protocol probes"]
async fn live_kimi_sync_records_reasoning_and_connects_every_discovered_model() {
    let key = env::var("GRILLFORGE_LIVE_KIMI_KEY").expect("GRILLFORGE_LIVE_KIMI_KEY must be set");
    let root = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(root.path());
    let synchronized = timeout(
        Duration::from_secs(180),
        service.save_provider_with_model_check(ProviderInput {
            id: "kimi-for-coding".into(),
            name: "Kimi For Coding".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: "https://api.kimi.com/coding/".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some(key),
            enabled: true,
            models_url: None,
        }),
    )
    .await
    .expect("Kimi synchronization timed out")
    .expect("Kimi synchronization");
    let kimi_models = synchronized
        .models
        .iter()
        .filter(|model| model.provider_id == "kimi-for-coding")
        .collect::<Vec<_>>();
    assert!(!kimi_models.is_empty(), "Kimi returned no models");
    for model in &kimi_models {
        assert!(model.native_protocols.contains(&NativeProtocol::OpenAiChat));
        assert!(
            model
                .protocol_capabilities
                .contains(&ProtocolCapability::ReasoningContent),
            "{} did not expose reasoning_content",
            model.upstream_id
        );
    }

    let gateway = Gateway::new(root.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    tokio::spawn({
        let router = gateway.router();
        async move { axum::serve(listener, router).await.unwrap() }
    });
    for model in kimi_models {
        let _route = gateway
            .status(base_url.clone())
            .allow_connection_test(&model.id)
            .unwrap();
        timeout(
            Duration::from_secs(90),
            service.test_model_connection(&base_url, &model.id),
        )
        .await
        .unwrap_or_else(|_| panic!("{} connection timed out", model.upstream_id))
        .unwrap_or_else(|error| panic!("{} connection failed: {error}", model.upstream_id));
    }
}

#[tokio::test]
async fn sync_probes_each_discovered_model_on_each_provider_protocol_once() {
    #[derive(Clone, Default)]
    struct Calls(Arc<Mutex<Vec<(String, String)>>>);

    async fn models() -> Json<Value> {
        Json(json!({"data": [{"id": "alpha"}, {"id": "beta"}]}))
    }

    async fn probe(
        State(calls): State<Calls>,
        uri: axum::http::Uri,
        Json(body): Json<Value>,
    ) -> (axum::http::StatusCode, Json<Value>) {
        let model = body["model"]
            .as_str()
            .map(str::to_string)
            .or_else(|| {
                uri.path()
                    .strip_prefix("/v1beta/models/")
                    .and_then(|value| value.strip_suffix(":generateContent"))
                    .map(str::to_string)
            })
            .unwrap_or_default();
        calls
            .0
            .lock()
            .unwrap()
            .push((uri.path().into(), model.clone()));
        match (uri.path(), model.as_str()) {
            ("/v1/responses", "alpha") => (
                axum::http::StatusCode::OK,
                Json(
                    json!({"id":"resp_alpha","object":"response","status":"completed","output":[]}),
                ),
            ),
            ("/v1/chat/completions", "alpha" | "beta") => (
                axum::http::StatusCode::OK,
                Json(
                    json!({"id":"chat","object":"chat.completion","choices":[{"message":{"role":"assistant","content":"OK","reasoning_content":"checked"},"finish_reason":"stop"}]}),
                ),
            ),
            _ => (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":{"message":"unsupported"}})),
            ),
        }
    }

    let calls = Calls::default();
    let upstream = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/responses", post(probe))
        .route("/v1/chat/completions", post(probe))
        .route("/v1/messages", post(probe))
        .route("/v1beta/models/{operation}", post(probe))
        .with_state(calls.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let root = tempfile::tempdir().unwrap();
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
        .unwrap();

    let state = service
        .sync_provider_models("vendor")
        .await
        .expect("one bounded protocol probe per model and surface");

    let provider = state
        .providers
        .iter()
        .find(|item| item.id == "vendor")
        .unwrap();
    assert_eq!(
        provider
            .protocol_endpoints
            .iter()
            .map(|entry| entry.protocol)
            .collect::<Vec<_>>(),
        vec![NativeProtocol::OpenAiResponses, NativeProtocol::OpenAiChat]
    );
    let alpha = state.models.iter().find(|item| item.id == "alpha").unwrap();
    assert_eq!(
        alpha.native_protocols,
        vec![NativeProtocol::OpenAiResponses, NativeProtocol::OpenAiChat]
    );
    assert_eq!(
        alpha.unsupported_native_protocols,
        vec![
            NativeProtocol::AnthropicMessages,
            NativeProtocol::GeminiNative,
        ]
    );
    let beta = state.models.iter().find(|item| item.id == "beta").unwrap();
    assert_eq!(beta.native_protocols, vec![NativeProtocol::OpenAiChat]);
    assert_eq!(
        beta.protocol_capabilities,
        vec![ProtocolCapability::ReasoningContent]
    );
    assert_eq!(
        beta.unsupported_native_protocols,
        vec![
            NativeProtocol::AnthropicMessages,
            NativeProtocol::OpenAiResponses,
            NativeProtocol::GeminiNative,
        ]
    );

    let calls = calls.0.lock().unwrap();
    for model in ["alpha", "beta"] {
        for path in [
            "/v1/messages",
            "/v1/responses",
            "/v1/chat/completions",
            &format!("/v1beta/models/{model}:generateContent"),
        ] {
            assert_eq!(
                calls
                    .iter()
                    .filter(|(actual_path, actual_model)| {
                        actual_path == path && actual_model == model
                    })
                    .count(),
                1,
                "{model} must be probed once through {path}"
            );
        }
    }
}

#[tokio::test]
async fn adding_kimi_preset_checks_its_pinned_model_when_listing_is_unavailable() {
    async fn anthropic_probe(Json(body): Json<Value>) -> (axum::http::StatusCode, Json<Value>) {
        assert_eq!(body["model"], "kimi-for-coding");
        (
            axum::http::StatusCode::OK,
            Json(json!({
                "id": "msg_kimi",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "OK"}],
                "model": "kimi-for-coding",
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 1, "output_tokens": 1}
            })),
        )
    }

    let upstream = Router::new()
        .route("/coding/v1/messages", post(anthropic_probe))
        .fallback(|| async {
            (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error":{"message":"not found"}})),
            )
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let root = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(root.path());
    let state = service
        .save_provider_with_model_check(ProviderInput {
            id: "kimi-for-coding".into(),
            name: "Kimi For Coding".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: format!("http://{address}/coding/"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .await
        .expect("a preset without /models must validate its pinned model");

    let provider = state
        .providers
        .iter()
        .find(|provider| provider.id == "kimi-for-coding")
        .unwrap();
    assert_eq!(
        provider
            .protocol_endpoints
            .iter()
            .map(|entry| entry.protocol)
            .collect::<Vec<_>>(),
        vec![NativeProtocol::AnthropicMessages]
    );
    let model = state
        .models
        .iter()
        .find(|model| model.upstream_id == "kimi-for-coding")
        .unwrap();
    assert_eq!(
        model.native_protocols,
        vec![NativeProtocol::AnthropicMessages]
    );
}

#[tokio::test]
async fn adding_the_same_upstream_models_under_two_providers_namespaces_the_second_routes() {
    async fn models() -> Json<Value> {
        Json(json!({"data": [{"id": "shared-model"}]}))
    }
    async fn chat_probe() -> Json<Value> {
        Json(json!({
            "id": "chat",
            "object": "chat.completion",
            "choices": [{"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]
        }))
    }
    let upstream = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat_probe))
        .fallback(|| async {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":{"message":"unsupported"}})),
            )
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let root = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(root.path());
    for (id, name) in [("vendor", "Vendor"), ("vendor-2", "Vendor 2")] {
        service
            .save_provider_with_model_check(ProviderInput {
                id: id.into(),
                name: name.into(),
                protocol: Protocol::OpenAiChatCompletions,
                endpoint: format!("http://{address}"),
                endpoint_mode: EndpointMode::BaseUrl,
                api_key_placement: ApiKeyPlacement::None,
                api_key: None,
                enabled: true,
                models_url: None,
            })
            .await
            .expect("each credential profile must own independent model routes");
    }

    let state = service.state().unwrap();
    assert_eq!(state.models.len(), 2);
    assert_eq!(state.models[0].id, "shared-model");
    assert_eq!(state.models[1].id, "vendor-2-shared-model");
    assert_ne!(state.models[0].route_alias, state.models[1].route_alias);
    assert_eq!(state.models[0].upstream_id, state.models[1].upstream_id);
}

#[tokio::test]
async fn sync_records_observed_chat_reasoning_before_the_model_is_routed() {
    let upstream = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data": [{"id": "reasoning-model"}]})) }),
        )
        .route(
            "/v1/chat/completions",
            post(|Json(body): Json<Value>| async move {
                Json(json!({
                    "id": "chat_reasoning",
                    "object": "chat.completion",
                    "model": body["model"],
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": "OK",
                            "reasoning_content": "checked"
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                }))
            }),
        )
        .fallback(|| async {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":{"message":"unsupported"}})),
            )
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let root = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "reasoning-vendor".into(),
            name: "Reasoning Vendor".into(),
            protocol: Protocol::OpenAiChatCompletions,
            endpoint: format!("http://{address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    service
        .save_model(ModelInput {
            id: "reasoning-model".into(),
            name: "Reasoning Model".into(),
            upstream_id: "reasoning-model".into(),
            provider_id: "reasoning-vendor".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
            context_window: None,
            max_output_tokens: None,
        })
        .expect("existing model without protocol capability");
    let state = service
        .sync_provider_models("reasoning-vendor")
        .await
        .expect("synchronization repairs the existing model facts");
    let model = state
        .models
        .iter()
        .find(|model| model.upstream_id == "reasoning-model")
        .unwrap();
    assert_eq!(
        model.protocol_capabilities,
        vec![ProtocolCapability::ReasoningContent]
    );

    let gateway = Gateway::new(root.path());
    let gateway_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_address = gateway_listener.local_addr().unwrap();
    let _connection_test = gateway
        .status(format!("http://{gateway_address}"))
        .allow_connection_test(&model.id)
        .unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway.router())
            .await
            .unwrap()
    });

    service
        .test_model_connection(&format!("http://{gateway_address}"), &model.id)
        .await
        .expect("observed reasoning_content must be accepted by the bridge");
}

#[tokio::test]
async fn adding_a_provider_keeps_configuration_unchanged_when_models_cannot_be_checked() {
    let upstream = Router::new().fallback(|| async { axum::http::StatusCode::NOT_FOUND });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let root = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(root.path());
    let error = service
        .save_provider_with_model_check(ProviderInput {
            id: "unknown-vendor".into(),
            name: "Unknown Vendor".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: format!("http://{address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .await
        .unwrap_err();
    assert!(error.contains("model discovery returned HTTP 404"));
    let state = service.state().unwrap();
    assert!(state.providers.is_empty());
    assert!(state.models.is_empty());
}

#[tokio::test]
async fn sync_fails_fast_on_authentication_without_persisting_partial_probe_facts() {
    let upstream = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data": [{"id": "alpha"}]})) }),
        )
        .route(
            "/v1/responses",
            post(|| async {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    Json(json!({"error":{"message":"bad key"}})),
                )
            }),
        )
        .fallback(|| async {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":{"message":"unsupported"}})),
            )
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "vendor".into(),
            name: "Vendor".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: format!("http://{address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("redacted-secret".into()),
            enabled: true,
            models_url: None,
        })
        .unwrap();

    let error = service.sync_provider_models("vendor").await.unwrap_err();
    assert!(error.contains("authentication failed"));
    assert!(!error.contains("redacted-secret"));
    assert!(service.state().unwrap().models.is_empty());
}

#[tokio::test]
async fn derived_protocol_auth_rejection_is_recorded_as_unsupported() {
    async fn route(uri: axum::http::Uri) -> (axum::http::StatusCode, Json<Value>) {
        match uri.path() {
            "/v1/chat/completions" => (
                axum::http::StatusCode::OK,
                Json(json!({"choices":[{"message":{"role":"assistant","content":"OK"}}]})),
            ),
            "/v1/responses" => (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(json!({"error":{"message":"wrong auth shape"}})),
            ),
            _ => (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":{"message":"unsupported"}})),
            ),
        }
    }
    let upstream = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data": [{"id": "alpha"}]})) }),
        )
        .fallback(post(route));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "vendor".into(),
            name: "Vendor".into(),
            protocol: Protocol::OpenAiChatCompletions,
            endpoint: format!("http://{address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("secret".into()),
            enabled: true,
            models_url: None,
        })
        .unwrap();
    let state = service.sync_provider_models("vendor").await.unwrap();
    assert_eq!(
        state.models[0].native_protocols,
        vec![NativeProtocol::OpenAiChat]
    );
    assert!(
        state.models[0]
            .unsupported_native_protocols
            .contains(&NativeProtocol::OpenAiResponses)
    );
}

#[test]
fn manually_added_models_require_explicit_verified_native_protocols() {
    let temp = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(temp.path());
    service
        .save_provider(ProviderInput {
            id: "provider".into(),
            name: "Provider".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: "https://example.com".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("secret".into()),
            enabled: true,
            models_url: None,
        })
        .unwrap();
    let model = ModelInput {
        id: "manual".into(),
        name: "Manual".into(),
        upstream_id: "manual-upstream".into(),
        provider_id: "provider".into(),
        capabilities: vec![],
        protocol_capabilities: vec![],
        context_window: None,
        max_output_tokens: None,
    };

    assert!(
        service
            .save_model_with_native_protocols(ModelWithNativeProtocolsInput {
                model: model.clone(),
                native_protocols: vec![],
            })
            .unwrap_err()
            .contains("at least one verified native protocol")
    );
    let state = service
        .save_model_with_native_protocols(ModelWithNativeProtocolsInput {
            model,
            native_protocols: vec![NativeProtocol::OpenAiChat],
        })
        .unwrap();
    assert_eq!(
        state
            .models
            .iter()
            .find(|model| model.id == "manual")
            .unwrap()
            .native_protocols,
        vec![NativeProtocol::OpenAiChat]
    );
}

#[test]
fn saving_or_updating_a_provider_does_not_claim_unprobed_protocol_support() {
    let root = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(root.path());
    let provider = ProviderInput {
        id: "provider".into(),
        name: "Provider".into(),
        protocol: Protocol::OpenAiChatCompletions,
        endpoint: "https://example.com".into(),
        endpoint_mode: EndpointMode::BaseUrl,
        api_key_placement: ApiKeyPlacement::Bearer,
        api_key: Some("secret".into()),
        enabled: true,
        models_url: None,
    };
    let state = service.save_provider(provider.clone()).unwrap();
    assert!(state.providers[0].protocol_endpoints.is_empty());
    service
        .save_model_with_native_protocols(ModelWithNativeProtocolsInput {
            model: ModelInput {
                id: "model".into(),
                name: "Model".into(),
                upstream_id: "model".into(),
                provider_id: "provider".into(),
                capabilities: vec![],
                protocol_capabilities: vec![],
                context_window: None,
                max_output_tokens: None,
            },
            native_protocols: vec![NativeProtocol::OpenAiChat],
        })
        .unwrap();
    let state = service
        .update_provider(ProviderInput {
            name: "Renamed".into(),
            api_key: None,
            ..provider
        })
        .unwrap();
    assert!(state.providers[0].protocol_endpoints.is_empty());
    assert!(state.models[0].native_protocols.is_empty());
    assert!(state.models[0].unsupported_native_protocols.is_empty());
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
            endpoint: format!("http://{upstream_address}/anthropic"),
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
            context_window: None,
            max_output_tokens: None,
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

#[tokio::test]
async fn stale_model_protocol_facts_are_not_silently_overridden() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upstream = Router::new()
        .route(
            "/v1/responses",
            post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"message":"pro does not support Responses"}})),
                )
            }),
        )
        .route(
            "/anthropic/v1/messages",
            post({
                let calls = calls.clone();
                move |Json(body): Json<Value>| {
                    let calls = calls.clone();
                    async move {
                        calls.lock().unwrap().push(body);
                        Json(json!({
                            "id":"msg_pro","type":"message","role":"assistant","model":"deepseek-v4-pro",
                            "content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn",
                            "usage":{"input_tokens":1,"output_tokens":1}
                        }))
                    }
                }
            }),
        );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let root = tempfile::tempdir().unwrap();
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
        .unwrap();
    service
        .save_model(ModelInput {
            id: "deepseek-v4-pro".into(),
            name: "DeepSeek V4 Pro".into(),
            upstream_id: "deepseek-v4-pro".into(),
            provider_id: "deepseek".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
            context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    service
        .set_model_native_protocols("deepseek-v4-pro", vec![NativeProtocol::OpenAiResponses])
        .expect("simulate stale metadata from an older build");

    let gateway = Gateway::new(root.path());
    let gateway_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_address = gateway_listener.local_addr().unwrap();
    let _connection_test = gateway
        .status(format!("http://{gateway_address}"))
        .allow_connection_test("deepseek-v4-pro")
        .unwrap();
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway.router())
            .await
            .unwrap()
    });

    let error = service
        .test_model_connection(&format!("http://{gateway_address}"), "deepseek-v4-pro")
        .await
        .expect_err("stale explicit facts must fail instead of using hidden compatibility data");
    assert!(error.contains("pro does not support Responses"));
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn sync_records_a_reported_context_window_and_never_overwrites_an_edited_one() {
    async fn models() -> Json<Value> {
        Json(json!({"data": [
            {"id": "reports", "context_length": 262144},
            {"id": "silent"}
        ]}))
    }
    async fn probe() -> Json<Value> {
        Json(json!({"id":"chat","object":"chat.completion",
            "choices":[{"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]}))
    }
    let upstream = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(probe))
        .route("/v1/responses", post(probe))
        .route("/v1/messages", post(probe));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let root = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "vendor".into(),
            name: "Vendor".into(),
            protocol: Protocol::OpenAiChatCompletions,
            endpoint: format!("http://{address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .unwrap();

    let state = service.sync_provider_models("vendor").await.expect("sync");
    let window = |state: &grillforge_lib::application::ControlPlaneState, upstream_id: &str| {
        state
            .models
            .iter()
            .find(|model| model.upstream_id == upstream_id)
            .unwrap_or_else(|| panic!("model {upstream_id}"))
            .context_window
    };
    assert_eq!(window(&state, "reports"), Some(262144));
    // A provider that publishes nothing leaves the model unknown rather than
    // handing a client an invented window.
    assert_eq!(window(&state, "silent"), None);

    // The operator fills in the one the provider does not publish.
    let silent = state
        .models
        .iter()
        .find(|model| model.upstream_id == "silent")
        .unwrap()
        .clone();
    let state = service
        .update_model(ModelInput {
            id: silent.id.clone(),
            name: silent.name.clone(),
            upstream_id: silent.upstream_id.clone(),
            provider_id: silent.provider_id.clone(),
            capabilities: silent.capabilities.clone(),
            protocol_capabilities: silent.protocol_capabilities.clone(),
            context_window: Some(64000),
            max_output_tokens: Some(8192),
        })
        .expect("operator supplied window");
    assert_eq!(window(&state, "silent"), Some(64000));

    let state = service
        .sync_provider_models("vendor")
        .await
        .expect("resync");
    assert_eq!(
        window(&state, "silent"),
        Some(64000),
        "a re-sync must not discard it"
    );
    assert_eq!(window(&state, "reports"), Some(262144));
}
