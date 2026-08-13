use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::Response,
    routing::post,
};
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[tokio::test]
async fn gemini_generate_content_routes_through_openai_chat() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upstream = Router::new()
        .route(
            "/v1/chat/completions",
            post(
                |State(calls): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    calls.lock().unwrap().push(body);
                    Json(json!({
                        "id":"chatcmpl-gemini-client","object":"chat.completion","model":"upstream-coder",
                        "choices":[{"index":0,"message":{"role":"assistant","content":"routed"},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}
                    }))
                },
            ),
        )
        .with_state(calls.clone());
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let root = tempfile::tempdir().unwrap();
    let control = ControlPlaneService::new(root.path());
    control
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
    control
        .save_model(ModelInput {
            id: "coder".into(),
            name: "Coder".into(),
            upstream_id: "upstream-coder".into(),
            provider_id: "chat".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
        })
        .unwrap();
    let gateway = Gateway::new(root.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_client("gemini", vec!["coder".into()], "gemini-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = reqwest::Client::new()
        .post(format!(
            "http://{address}/gemini/v1beta/models/grillforge--coder:generateContent"
        ))
        .header("x-goog-api-key", "gemini-token")
        .json(&json!({
            "contents":[{"role":"user","parts":[{"text":"ping"}]}],
            "generationConfig":{"maxOutputTokens":64}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["candidates"][0]["content"]["parts"][0]["text"],
        "routed"
    );
    assert_eq!(body["candidates"][0]["finishReason"], "STOP");
    assert_eq!(calls.lock().unwrap()[0]["model"], "upstream-coder");
    assert_eq!(calls.lock().unwrap()[0]["messages"][0]["content"], "ping");
}

#[tokio::test]
async fn gemini_ingress_routes_through_native_responses_and_anthropic_protocols() {
    let upstream = Router::new()
        .route(
            "/v1/responses",
            post(|| async {
                Json(json!({
                    "id":"resp_1","status":"completed","model":"responses-upstream",
                    "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"responses-routed"}]}],
                    "usage":{"input_tokens":3,"output_tokens":2}
                }))
            }),
        )
        .route(
            "/v1/messages",
            post(|| async {
                Json(json!({
                    "id":"msg_1","type":"message","role":"assistant","model":"anthropic-upstream",
                    "content":[{"type":"text","text":"anthropic-routed"}],
                    "stop_reason":"end_turn","stop_sequence":null,
                    "usage":{"input_tokens":3,"output_tokens":2}
                }))
            }),
        )
        .route(
            "/v1beta/models/gemini-upstream:generateContent",
            post(|| async {
                Json(json!({
                    "responseId":"gemini_1","modelVersion":"gemini-upstream",
                    "candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"gemini-routed"}]},"finishReason":"STOP"}],
                    "usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5}
                }))
            }),
        );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let root = tempfile::tempdir().unwrap();
    let control = ControlPlaneService::new(root.path());
    for (id, protocol) in [
        ("responses", Protocol::OpenAiResponses),
        ("anthropic", Protocol::AnthropicMessages),
        ("gemini", Protocol::GeminiNative),
    ] {
        let (api_key_placement, api_key) = if protocol == Protocol::GeminiNative {
            (ApiKeyPlacement::XApiKey, Some("gemini-secret".into()))
        } else {
            (ApiKeyPlacement::None, None)
        };
        control
            .save_provider(ProviderInput {
                id: id.into(),
                name: id.into(),
                protocol,
                endpoint: format!("http://{upstream_address}"),
                endpoint_mode: EndpointMode::BaseUrl,
                api_key_placement,
                api_key,
                enabled: true,
                models_url: None,
            })
            .unwrap();
        control
            .save_model(ModelInput {
                id: id.into(),
                name: id.into(),
                upstream_id: format!("{id}-upstream"),
                provider_id: id.into(),
                capabilities: vec![],
                protocol_capabilities: vec![],
            })
            .unwrap();
    }
    let gateway = Gateway::new(root.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_client(
            "gemini",
            vec!["responses".into(), "anthropic".into(), "gemini".into()],
            "gemini-token",
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    for (model, expected) in [
        ("responses", "responses-routed"),
        ("anthropic", "anthropic-routed"),
        ("gemini", "gemini-routed"),
    ] {
        let response = reqwest::Client::new()
            .post(format!(
                "http://{address}/gemini/v1beta/models/grillforge--{model}:generateContent"
            ))
            .header("x-goog-api-key", "gemini-token")
            .json(&json!({
                "contents":[{"role":"user","parts":[{"text":"ping"}]}],
                "generationConfig":{"maxOutputTokens":64}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK, "{model}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(
            body["candidates"][0]["content"]["parts"][0]["text"], expected,
            "{model}: {body}"
        );
    }
}

#[tokio::test]
async fn gemini_stream_generate_content_returns_data_only_sse() {
    let upstream = Router::new().route(
        "/v1/messages",
        post(|| async {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(concat!(
                    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_stream\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"anthropic-upstream\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n",
                    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"streamed\"}}\n\n",
                    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
                    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
                )))
                .unwrap()
        }),
    );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let root = tempfile::tempdir().unwrap();
    let control = ControlPlaneService::new(root.path());
    control
        .save_provider(ProviderInput {
            id: "anthropic".into(),
            name: "Anthropic".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: format!("http://{upstream_address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .unwrap();
    control
        .save_model(ModelInput {
            id: "worker".into(),
            name: "Worker".into(),
            upstream_id: "anthropic-upstream".into(),
            provider_id: "anthropic".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
        })
        .unwrap();
    let gateway = Gateway::new(root.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_client("gemini", vec!["worker".into()], "gemini-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = reqwest::Client::new()
        .post(format!(
            "http://{address}/gemini/v1beta/models/grillforge--worker:streamGenerateContent?alt=sse"
        ))
        .header("x-goog-api-key", "gemini-token")
        .json(&json!({
            "contents":[{"role":"user","parts":[{"text":"ping"}]}],
            "generationConfig":{"maxOutputTokens":64}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let output = response.text().await.unwrap();
    assert!(output.contains(r#""text":"streamed""#), "{output}");
    assert!(output.contains(r#""finishReason":"STOP""#), "{output}");
    assert!(!output.contains("event:"), "{output}");
}
