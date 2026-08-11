use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::post,
};
use bytes::Bytes;
use futures::StreamExt;
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<(HeaderMap, Value)>>>);

#[tokio::test]
async fn a_saved_but_not_applied_model_route_fails_closed() {
    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());
    service
        .save_provider(ProviderInput {
            id: "local".into(),
            name: "Local".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: "http://127.0.0.1:9".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    service
        .save_model(ModelInput {
            id: "saved-only".into(),
            name: "Saved Only".into(),
            upstream_id: "saved-only".into(),
            provider_id: "local".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .expect("model");
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("gateway address");
    tokio::spawn(async move {
        axum::serve(listener, gateway.router())
            .await
            .expect("serve gateway")
    });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/v1/messages"))
        .json(&json!({
            "model":"grillforge/saved-only",
            "max_tokens":8,
            "messages":[{"role":"user","content":"ping"}]
        }))
        .send()
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        response
            .text()
            .await
            .expect("body")
            .contains("inactive GrillForge route")
    );
}

#[tokio::test]
async fn route_alias_reaches_exact_responses_provider_and_upstream_model() {
    let capture = Capture::default();
    let upstream =
        Router::new()
            .route(
                "/v1/responses",
                post(
                    |State(capture): State<Capture>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        capture.0.lock().expect("capture").push((headers, body));
                        Json(json!({
                            "id": "resp_1",
                            "status": "completed",
                            "model": "gpt-upstream",
                            "output": [{
                                "type": "message",
                                "content": [{"type": "output_text", "text": "connected"}]
                            }],
                            "usage": {"input_tokens": 3, "output_tokens": 1}
                        }))
                    },
                ),
            )
            .with_state(capture.clone());
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.expect("upstream");
    let upstream_address = upstream_listener.local_addr().expect("upstream address");
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream)
            .await
            .expect("serve upstream")
    });

    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());
    service
        .save_provider(ProviderInput {
            id: "openai".into(),
            name: "OpenAI".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: format!("http://{upstream_address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("provider-secret".into()),
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    service
        .save_model(ModelInput {
            id: "worker".into(),
            name: "Worker".into(),
            upstream_id: "gpt-upstream".into(),
            provider_id: "openai".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .expect("model");

    let gateway = Gateway::new(directory.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate(
            &service
                .set_main_model(Some("worker".into()))
                .expect("activate model"),
        )
        .expect("active routes");
    service
        .update_model(ModelInput {
            id: "worker".into(),
            name: "Draft Worker".into(),
            upstream_id: "draft-upstream-must-not-go-live".into(),
            provider_id: "openai".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .expect("edit inactive draft");
    service
        .update_provider(ProviderInput {
            id: "openai".into(),
            name: "Draft OpenAI".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: format!("http://{upstream_address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("draft-secret-must-not-go-live".into()),
            enabled: true,
            models_url: None,
        })
        .expect("edit inactive Provider draft");
    let gateway_listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let gateway_address = gateway_listener.local_addr().expect("gateway address");
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway.router())
            .await
            .expect("serve gateway")
    });

    let response = reqwest::Client::new()
        .post(format!("http://{gateway_address}/v1/messages"))
        .bearer_auth("claude-subscription-token-must-not-leak")
        .json(&json!({
            "model": "grillforge/worker",
            "max_tokens": 32,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("gateway response");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("Anthropic JSON");
    assert_eq!(body["content"][0]["text"], "connected");

    let calls = capture.0.lock().expect("captured calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1["model"], "gpt-upstream");
    assert_eq!(calls[0].0["authorization"], "Bearer provider-secret");
    assert_ne!(
        calls[0].0["authorization"],
        "Bearer claude-subscription-token-must-not-leak"
    );
}

#[tokio::test]
async fn chat_route_reaches_local_no_auth_provider_without_leaking_claude_auth() {
    let capture = Capture::default();
    let upstream =
        Router::new()
            .route(
                "/v1/chat/completions",
                post(
                    |State(capture): State<Capture>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        capture.0.lock().expect("capture").push((headers, body));
                        Json(json!({
                            "id": "chatcmpl_1",
                            "object": "chat.completion",
                            "model": "local-upstream",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "local"},
                                "finish_reason": "stop"
                            }],
                            "usage": {"prompt_tokens": 2, "completion_tokens": 1}
                        }))
                    },
                ),
            )
            .with_state(capture.clone());
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.expect("upstream");
    let upstream_address = upstream_listener.local_addr().expect("upstream address");
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream)
            .await
            .expect("serve upstream")
    });

    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());
    service
        .save_provider(ProviderInput {
            id: "local-chat".into(),
            name: "Local Chat".into(),
            protocol: Protocol::OpenAiChatCompletions,
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
            id: "local-worker".into(),
            name: "Local Worker".into(),
            upstream_id: "local-upstream".into(),
            provider_id: "local-chat".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .expect("model");
    let gateway = Gateway::new(directory.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate(
            &service
                .set_main_model(Some("local-worker".into()))
                .expect("activate model"),
        )
        .expect("active routes");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("gateway address");
    tokio::spawn(async move {
        axum::serve(listener, gateway.router())
            .await
            .expect("serve gateway")
    });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/v1/messages"))
        .bearer_auth("claude-token-must-not-leak")
        .json(&json!({
            "model": "grillforge/local-worker",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("gateway response");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("Anthropic JSON");
    assert_eq!(body["content"][0]["text"], "local");
    let calls = capture.0.lock().expect("captured calls");
    assert_eq!(calls[0].1["model"], "local-upstream");
    assert!(!calls[0].0.contains_key("authorization"));
}

#[tokio::test]
async fn native_model_forwards_claude_authorization_without_initializing_managed_state() {
    let capture = Capture::default();
    let upstream =
        Router::new()
            .route(
                "/v1/messages",
                post(
                    |State(capture): State<Capture>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        capture
                            .0
                            .lock()
                            .expect("capture")
                            .push((headers, body.clone()));
                        Json(json!({
                            "id": "msg_native",
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "text", "text": "native"}],
                            "model": body["model"],
                            "stop_reason": "end_turn",
                            "stop_sequence": null,
                            "usage": {"input_tokens": 1, "output_tokens": 1}
                        }))
                    },
                ),
            )
            .with_state(capture.clone());
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.expect("upstream");
    let upstream_address = upstream_listener.local_addr().expect("upstream address");
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream)
            .await
            .expect("serve upstream")
    });

    let directory = tempfile::tempdir().expect("configuration directory");
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("gateway address");
    let status = gateway.status(format!("http://{address}"));
    status
        .set_native_base_url(&format!("http://{upstream_address}"))
        .expect("native mock URL");
    tokio::spawn(async move {
        axum::serve(listener, gateway.router())
            .await
            .expect("serve gateway")
    });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/v1/messages?beta=true"))
        .bearer_auth("native-oauth")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "claude-native",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("native response");

    assert_eq!(response.status(), 200);
    let calls = capture.0.lock().expect("captured calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0["authorization"], "Bearer native-oauth");
    assert_eq!(calls[0].0["anthropic-version"], "2023-06-01");
    assert_eq!(calls[0].1["model"], "claude-native");
    assert!(!directory.path().join("config.yaml").exists());
}

#[test]
fn native_route_cannot_point_back_to_the_gateway() {
    let gateway = Gateway::new("unused");
    let status = gateway.status("http://127.0.0.1:15721".into());

    let error = status
        .set_native_base_url("http://127.0.0.1:15721/")
        .expect_err("self route must fail");

    assert_eq!(
        error,
        "native Anthropic base URL points back to the GrillForge gateway"
    );
}

#[tokio::test]
async fn anthropic_provider_replaces_inbound_auth_with_its_own_key() {
    let capture = Capture::default();
    let upstream =
        Router::new()
            .route(
                "/v1/messages",
                post(
                    |State(capture): State<Capture>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        capture
                            .0
                            .lock()
                            .expect("capture")
                            .push((headers, body.clone()));
                        Json(json!({
                            "id": "msg_external",
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "text", "text": "external"}],
                            "model": body["model"],
                            "stop_reason": "end_turn",
                            "stop_sequence": null,
                            "usage": {"input_tokens": 1, "output_tokens": 1}
                        }))
                    },
                ),
            )
            .with_state(capture.clone());
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.expect("upstream");
    let upstream_address = upstream_listener.local_addr().expect("upstream address");
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream)
            .await
            .expect("serve upstream")
    });

    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());
    service
        .save_provider(ProviderInput {
            id: "anthropic".into(),
            name: "Anthropic".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: format!("http://{upstream_address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::XApiKey,
            api_key: Some("provider-key".into()),
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    service
        .save_model(ModelInput {
            id: "claude-worker".into(),
            name: "Claude Worker".into(),
            upstream_id: "claude-upstream".into(),
            provider_id: "anthropic".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .expect("model");

    let gateway = Gateway::new(directory.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate(
            &service
                .set_main_model(Some("claude-worker".into()))
                .expect("activate model"),
        )
        .expect("active routes");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("gateway address");
    tokio::spawn(async move {
        axum::serve(listener, gateway.router())
            .await
            .expect("serve gateway")
    });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/v1/messages"))
        .bearer_auth("native-oauth-must-be-dropped")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "grillforge/claude-worker",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("external response");

    assert_eq!(response.status(), 200);
    let calls = capture.0.lock().expect("captured calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0["x-api-key"], "provider-key");
    assert!(!calls[0].0.contains_key("authorization"));
    assert_eq!(calls[0].1["model"], "claude-upstream");
}

#[tokio::test]
async fn anthropic_sse_is_forwarded_as_a_stream_without_buffer_conversion() {
    let upstream = Router::new().route(
        "/v1/messages",
        post(|| async {
            let chunks = async_stream::stream! {
                yield Ok::<_, std::io::Error>(Bytes::from_static(
                    b"event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
                ));
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                yield Ok(Bytes::from_static(
                    b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                ));
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(chunks))
                .unwrap()
        }),
    );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.expect("upstream");
    let upstream_address = upstream_listener.local_addr().expect("upstream address");
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream)
            .await
            .expect("serve upstream")
    });

    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());
    service
        .save_provider(ProviderInput {
            id: "anthropic".into(),
            name: "Anthropic".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: format!("http://{upstream_address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("provider-key".into()),
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    service
        .save_model(ModelInput {
            id: "stream-worker".into(),
            name: "Stream Worker".into(),
            upstream_id: "claude-upstream".into(),
            provider_id: "anthropic".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .expect("model");

    let gateway = Gateway::new(directory.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate(
            &service
                .set_main_model(Some("stream-worker".into()))
                .expect("activate model"),
        )
        .expect("active routes");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("gateway address");
    tokio::spawn(async move {
        axum::serve(listener, gateway.router())
            .await
            .expect("serve gateway")
    });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/v1/messages"))
        .json(&json!({
            "model": "grillforge/stream-worker",
            "max_tokens": 16,
            "stream": true,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("stream response");

    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let mut chunks = response.bytes_stream();
    let first = tokio::time::timeout(std::time::Duration::from_millis(150), chunks.next())
        .await
        .expect("first SSE event must arrive before upstream completes")
        .expect("first SSE chunk")
        .expect("valid first SSE chunk");
    assert!(String::from_utf8_lossy(&first).contains("message_start"));
    let body = chunks
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .fold(Vec::new(), |mut output, chunk| {
            output.extend_from_slice(&chunk);
            output
        });
    assert!(String::from_utf8(body).unwrap().contains("message_stop"));
}

#[tokio::test]
async fn responses_sse_is_converted_to_anthropic_events_through_the_gateway() {
    let responses_sse = [
        ("response.created", json!({"type":"response.created","response":{"id":"resp_1","model":"gpt-upstream"}})),
        ("response.output_item.added", json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg_1","type":"message","role":"assistant","content":[]}})),
        ("response.content_part.added", json!({"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":""}})),
        ("response.output_text.delta", json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"hello"})),
        ("response.content_part.done", json!({"type":"response.content_part.done","output_index":0,"content_index":0,"part":{"type":"output_text","text":"hello"}})),
        ("response.output_item.done", json!({"type":"response.output_item.done","output_index":0,"item":{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}})),
        ("response.completed", json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":2,"output_tokens":1}}})),
    ]
    .into_iter()
    .fold(String::new(), |mut output, (event, data)| {
        write!(output, "event: {event}\ndata: {data}\n\n").expect("write SSE fixture");
        output
    });
    let upstream = Router::new().route(
        "/v1/responses",
        post(move || {
            let body = responses_sse.clone();
            async move {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(body))
                    .unwrap()
            }
        }),
    );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.expect("upstream");
    let upstream_address = upstream_listener.local_addr().expect("upstream address");
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream)
            .await
            .expect("serve upstream")
    });

    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());
    service
        .save_provider(ProviderInput {
            id: "openai".into(),
            name: "OpenAI".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: format!("http://{upstream_address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("provider-key".into()),
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    service
        .save_model(ModelInput {
            id: "responses-worker".into(),
            name: "Responses Worker".into(),
            upstream_id: "gpt-upstream".into(),
            provider_id: "openai".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .expect("model");
    let gateway = Gateway::new(directory.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate(
            &service
                .set_main_model(Some("responses-worker".into()))
                .expect("activate model"),
        )
        .expect("active routes");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("gateway address");
    tokio::spawn(async move {
        axum::serve(listener, gateway.router())
            .await
            .expect("serve gateway")
    });

    let output = reqwest::Client::new()
        .post(format!("http://{address}/v1/messages"))
        .json(&json!({
            "model": "grillforge/responses-worker",
            "max_tokens": 16,
            "stream": true,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("stream response")
        .text()
        .await
        .expect("Anthropic SSE");

    assert!(output.contains("event: message_start"));
    assert!(output.contains("event: content_block_delta"));
    assert!(output.contains("\"text\":\"hello\""));
    assert!(output.contains("event: message_stop"));
    assert!(!output.contains("response.output_text.delta"));
}
