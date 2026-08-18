use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[tokio::test]
async fn named_client_route_is_isolated_by_client_token_and_model_pool() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upstream = Router::new()
        .route(
            "/v1/chat/completions",
            post(
                |State(calls): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    calls.lock().unwrap().push(body);
                    Json(json!({
                        "id": "chatcmpl-client",
                        "object": "chat.completion",
                        "model": "coder",
                        "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 2, "completion_tokens": 1}
                    }))
                },
            ),
        )
        .with_state(calls.clone());
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let temp = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(temp.path());
    service
        .save_provider(ProviderInput {
            id: "provider".into(),
            name: "Provider".into(),
            protocol: Protocol::OpenAiChatCompletions,
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
            id: "coder".into(),
            name: "Coder".into(),
            upstream_id: "coder-upstream".into(),
            provider_id: "provider".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
                    context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(temp.path());
    let status = gateway.status("http://127.0.0.1:1".into());
    let removed = ["open", "claw"].concat();
    assert!(
        status
            .activate_client(&removed, vec!["coder".into()], "removed-token")
            .unwrap_err()
            .contains("unsupported")
    );
    status
        .activate_client("opencode", vec!["coder".into()], "client-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });
    let body = json!({
        "model": "grillforge/coder",
        "max_tokens": 16,
        "messages": [{"role": "user", "content": "ping"}]
    });
    let client = reqwest::Client::new();

    assert_eq!(
        client
            .post(format!("http://{address}/clients/hermes/v1/messages"))
            .bearer_auth("client-token")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .post(format!("http://{address}/clients/opencode/v1/messages"))
            .bearer_auth("wrong")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = client
        .post(format!("http://{address}/clients/opencode/v1/messages"))
        .bearer_auth("client-token")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["content"][0]["text"],
        "ok"
    );
    assert_eq!(calls.lock().unwrap()[0]["model"], "coder-upstream");
}

#[test]
fn anthropic_ingress_clients_accept_gemini_native_models_for_local_bridging() {
    let temp = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(temp.path());
    service
        .save_provider(ProviderInput {
            id: "gemini".into(),
            name: "Gemini".into(),
            protocol: Protocol::GeminiNative,
            endpoint: "https://generativelanguage.googleapis.com".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::XApiKey,
            api_key: Some("secret".into()),
            enabled: true,
            models_url: None,
        })
        .unwrap();
    service
        .save_model(ModelInput {
            id: "gemini-model".into(),
            name: "Gemini Model".into(),
            upstream_id: "gemini-upstream".into(),
            provider_id: "gemini".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
                    context_window: None,
            max_output_tokens: None,
        })
        .unwrap();

    Gateway::new(temp.path())
        .status("http://127.0.0.1:1".into())
        .activate_client("opencode", vec!["gemini-model".into()], "token")
        .unwrap();
}

#[tokio::test]
async fn responses_ingress_client_routes_a_chat_only_model_through_the_bridge() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upstream = Router::new()
        .route(
            "/v1/chat/completions",
            post(|State(calls): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                calls.lock().unwrap().push(body);
                Json(json!({
                    "id":"chatcmpl-grok","object":"chat.completion","model":"chat-upstream",
                    "choices":[{"index":0,"message":{"role":"assistant","content":"bridged"},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}
                }))
            }),
        )
        .with_state(calls.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let temp = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(temp.path());
    service
        .save_provider(ProviderInput {
            id: "chat".into(),
            name: "Chat".into(),
            protocol: Protocol::OpenAiChatCompletions,
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
            id: "chat-model".into(),
            name: "Chat Model".into(),
            upstream_id: "chat-upstream".into(),
            provider_id: "chat".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
                    context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(temp.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_response_client("grok-build", vec!["chat-model".into()], "token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = reqwest::Client::new()
        .post(format!(
            "http://{address}/responses/grok-build/v1/responses"
        ))
        .bearer_auth("token")
        .json(&json!({"model":"grillforge/chat-model","input":"ping","stream":false,"store":false}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["output"][0]["content"][0]["text"], "bridged");
    assert_eq!(calls.lock().unwrap()[0]["model"], "chat-upstream");
}
