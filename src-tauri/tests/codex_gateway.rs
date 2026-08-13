use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::configuration::{ConfigurationFiles, ProviderProtocolEndpoint};
use grillforge_lib::core::model::NativeProtocol;
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[tokio::test]
async fn routing_uses_the_selected_protocols_own_endpoint() {
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let upstream = Router::new()
        .route(
            "/responses/v1/responses",
            post({
                let calls = calls.clone();
                move || {
                    let calls = calls.clone();
                    async move {
                        calls.lock().unwrap().push("responses".into());
                        Json(json!({"id":"resp_direct","object":"response","status":"completed","output":[]}))
                    }
                }
            }),
        )
        .route(
            "/chat/v1/chat/completions",
            post({
                let calls = calls.clone();
                move || {
                    let calls = calls.clone();
                    async move {
                        calls.lock().unwrap().push("chat".into());
                        Json(json!({
                            "id":"chat_bridge","object":"chat.completion","model":"bridged",
                            "choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],
                            "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}
                        }))
                    }
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(root.path());
    service
        .save_provider(ProviderInput {
            id: "vendor".into(),
            name: "Vendor".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: format!("http://{upstream_address}/responses"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .unwrap();
    for (id, protocol) in [
        ("direct", NativeProtocol::OpenAiResponses),
        ("bridged", NativeProtocol::OpenAiChat),
    ] {
        service
            .save_model(ModelInput {
                id: id.into(),
                name: id.into(),
                upstream_id: id.into(),
                provider_id: "vendor".into(),
                capabilities: vec![],
                protocol_capabilities: vec![],
            })
            .unwrap();
        service
            .set_model_native_protocols(id, vec![protocol])
            .unwrap();
    }
    let files = ConfigurationFiles::new(root.path());
    let mut documents = files.read().unwrap();
    documents.config.providers[0].protocol_endpoints = vec![
        ProviderProtocolEndpoint {
            protocol: NativeProtocol::OpenAiResponses,
            endpoint: format!("http://{upstream_address}/responses"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
        },
        ProviderProtocolEndpoint {
            protocol: NativeProtocol::OpenAiChat,
            endpoint: format!("http://{upstream_address}/chat"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
        },
    ];
    files
        .save(&documents.config, &documents.models, &documents.agents)
        .unwrap();
    let gateway = Gateway::new(root.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_codex(vec!["direct".into(), "bridged".into()], "token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });
    for model in ["direct", "bridged"] {
        let response = reqwest::Client::new()
            .post(format!("http://{address}/codex/v1/responses"))
            .bearer_auth("token")
            .json(&json!({"model":format!("grillforge/{model}"),"input":"ping","stream":false,"store":false}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    assert_eq!(*calls.lock().unwrap(), ["responses", "chat"]);
}

#[tokio::test]
async fn codex_route_requires_its_token_and_forwards_responses_without_client_auth() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upstream = Router::new()
        .route(
            "/v1/responses",
            post(
                |State(calls): State<Arc<Mutex<Vec<Value>>>>,
                 Json(body): Json<Value>| async move {
                    calls.lock().unwrap().push(body);
                    Json(json!({
                        "id": "resp_deepseek",
                        "object": "response",
                        "status": "completed",
                        "model": "deepseek-v4-flash",
                        "output": [{
                            "id": "msg_1",
                            "type": "message",
                            "role": "assistant",
                            "status": "completed",
                            "content": [{"type": "output_text", "text": "codex-ok", "annotations": []}]
                        }],
                        "usage": {"input_tokens": 2, "output_tokens": 1, "total_tokens": 3}
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
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: format!("http://{upstream_address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("upstream-token".into()),
            enabled: true,
            models_url: None,
        })
        .unwrap();
    service
        .save_model(ModelInput {
            id: "deepseek-v4-flash".into(),
            name: "DeepSeek V4 Flash".into(),
            upstream_id: "deepseek-v4-flash".into(),
            provider_id: "deepseek".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .unwrap();

    let gateway = Gateway::new(temp.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_codex(vec!["deepseek-v4-flash".into()], "codex-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let body = json!({
        "model": "grillforge/deepseek-v4-flash",
        "instructions": "reply briefly",
        "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "ping"}]}],
        "stream": false,
        "store": false
    });
    let client = reqwest::Client::new();
    let unauthorized = client
        .post(format!("http://{address}/codex/v1/responses"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let response = client
        .post(format!("http://{address}/codex/v1/responses"))
        .bearer_auth("codex-token")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["output"][0]["content"][0]["text"], "codex-ok");
    assert_eq!(calls.lock().unwrap()[0]["model"], "deepseek-v4-flash");
}

#[tokio::test]
async fn same_provider_routes_flash_to_responses_and_pro_to_its_verified_chat_protocol() {
    #[derive(Clone, Default)]
    struct ProtocolCalls {
        responses: Arc<AtomicUsize>,
        chat: Arc<AtomicUsize>,
    }
    let calls = ProtocolCalls::default();
    let upstream = Router::new()
        .route(
            "/v1/responses",
            post(
                |State(calls): State<ProtocolCalls>, Json(body): Json<Value>| async move {
                    calls.responses.fetch_add(1, Ordering::SeqCst);
                    if body["model"] == "deepseek-v4-pro" {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({"error":{"message":"Responses model unavailable"}})),
                        )
                            .into_response();
                    }
                    Json(json!({
                        "id": "resp_flash",
                        "status": "completed",
                        "model": "deepseek-v4-flash",
                        "output": [{
                            "type": "message",
                            "content": [{"type": "output_text", "text": "flash-ok"}]
                        }],
                        "usage": {"input_tokens": 1, "output_tokens": 1}
                    }))
                    .into_response()
                },
            ),
        )
        .route(
            "/v1/chat/completions",
            post(
                |State(calls): State<ProtocolCalls>, Json(body): Json<Value>| async move {
                    calls.chat.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(body["model"], "deepseek-v4-pro");
                    Json(json!({
                        "id": "chat_pro",
                        "object": "chat.completion",
                        "created": 1,
                        "model": "deepseek-v4-pro",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "pro-ok"},
                            "finish_reason": "stop"
                        }],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
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
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: format!("http://{upstream_address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("loopback-token".into()),
            enabled: true,
            models_url: None,
        })
        .unwrap();
    for (id, protocol) in [
        ("deepseek-v4-flash", NativeProtocol::OpenAiResponses),
        ("deepseek-v4-pro", NativeProtocol::OpenAiChat),
    ] {
        service
            .save_model(ModelInput {
                id: id.into(),
                name: id.into(),
                upstream_id: id.into(),
                provider_id: "deepseek".into(),
                capabilities: vec![],
                protocol_capabilities: vec![],
            })
            .unwrap();
        service
            .set_model_native_protocols(id, vec![protocol])
            .unwrap();
    }
    let gateway = Gateway::new(temp.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_codex(
            vec!["deepseek-v4-flash".into(), "deepseek-v4-pro".into()],
            "codex-token",
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });
    let client = reqwest::Client::new();
    for (model, expected) in [
        ("deepseek-v4-flash", "flash-ok"),
        ("deepseek-v4-pro", "pro-ok"),
    ] {
        let response: Value = client
            .post(format!("http://{address}/codex/v1/responses"))
            .bearer_auth("codex-token")
            .json(&json!({
                "model": format!("grillforge/{model}"),
                "instructions": "reply",
                "input": [{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}],
                "stream": false
            }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["output"][0]["content"][0]["text"], expected);
    }
    assert_eq!(calls.responses.load(Ordering::SeqCst), 1);
    assert_eq!(calls.chat.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn codex_route_converts_responses_to_chat_and_back() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upstream = Router::new()
        .route(
            "/v1/chat/completions",
            post(
                |State(calls): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    calls.lock().unwrap().push(body);
                    Json(json!({
                        "id": "chatcmpl_1",
                        "object": "chat.completion",
                        "created": 1,
                        "model": "kimi-code",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "chat-ok"},
                            "finish_reason": "stop"
                        }],
                        "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
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
            id: "kimi-code".into(),
            name: "Kimi Code".into(),
            upstream_id: "kimi-code".into(),
            provider_id: "chat".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .unwrap();

    let gateway = Gateway::new(temp.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_codex(vec!["kimi-code".into()], "codex-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/codex/v1/responses"))
        .bearer_auth("codex-token")
        .json(&json!({
            "model": "grillforge/kimi-code",
            "instructions": "reply briefly",
            "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "ping"}]}],
            "stream": false,
            "store": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["output"][0]["content"][0]["text"], "chat-ok");
    let call = &calls.lock().unwrap()[0];
    assert_eq!(call["model"], "kimi-code");
    assert_eq!(call["messages"][0]["role"], "system");
    assert_eq!(call["messages"][1]["content"], "ping");
}

#[tokio::test]
async fn codex_chat_route_streams_responses_events() {
    let upstream = Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            let sse = concat!(
                "data: {\"id\":\"chatcmpl_stream\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"kimi-code\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"stream-ok\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"chatcmpl_stream\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"kimi-code\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1}}\n\n",
                "data: [DONE]\n\n"
            );
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(sse))
                .unwrap()
        }),
    );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

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
            id: "kimi-code".into(),
            name: "Kimi Code".into(),
            upstream_id: "kimi-code".into(),
            provider_id: "chat".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .unwrap();
    let gateway = Gateway::new(temp.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_codex(vec!["kimi-code".into()], "codex-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/codex/v1/responses"))
        .bearer_auth("codex-token")
        .json(&json!({"model":"grillforge/kimi-code","input":"ping","stream":true,"store":false}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let body = response.text().await.unwrap();
    assert!(body.contains("event: response.output_text.delta"));
    assert!(body.contains("\"delta\":\"stream-ok\""));
    assert!(body.contains("event: response.completed"));
}

#[tokio::test]
async fn codex_route_converts_responses_text_to_anthropic_and_back() {
    type AnthropicCalls = Arc<Mutex<Vec<(HeaderMap, Value)>>>;

    let calls = AnthropicCalls::default();
    let upstream = Router::new()
        .route(
            "/v1/messages",
            post(
                |State(calls): State<AnthropicCalls>,
                 headers: HeaderMap,
                 Json(body): Json<Value>| async move {
                    calls.lock().unwrap().push((headers, body));
                    Json(json!({
                        "id":"msg_anthropic_1","type":"message","role":"assistant",
                        "model":"claude-sonnet","stop_reason":"end_turn",
                        "content":[{"type":"text","text":"anthropic-ok"}],
                        "usage":{"input_tokens":3,"output_tokens":2}
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
            id: "anthropic".into(),
            name: "Anthropic".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: format!("http://{upstream_address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::XApiKey,
            api_key: Some("anthropic-secret".into()),
            enabled: true,
            models_url: None,
        })
        .unwrap();
    service
        .save_model(ModelInput {
            id: "claude-sonnet".into(),
            name: "Claude Sonnet".into(),
            upstream_id: "claude-sonnet".into(),
            provider_id: "anthropic".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .unwrap();

    let gateway = Gateway::new(temp.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_codex(vec!["claude-sonnet".into()], "codex-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/codex/v1/responses"))
        .bearer_auth("codex-token")
        .json(&json!({
            "model":"grillforge/claude-sonnet",
            "instructions":"reply briefly",
            "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}],
            "stream":false,"store":false
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["output"][0]["content"][0]["text"], "anthropic-ok");
    assert_eq!(response["usage"]["input_tokens"], 3);
    assert_eq!(response["usage"]["output_tokens"], 2);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0["x-api-key"], "anthropic-secret");
    assert_eq!(calls[0].0["anthropic-version"], "2023-06-01");
    assert_eq!(calls[0].1["model"], "claude-sonnet");
    assert_eq!(calls[0].1["system"], "reply briefly");
    assert_eq!(calls[0].1["messages"][0]["content"][0]["text"], "ping");
}

#[tokio::test]
async fn codex_route_converts_responses_through_gemini_native_and_back() {
    type GeminiCalls = Arc<Mutex<Vec<(HeaderMap, Value)>>>;
    let calls = GeminiCalls::default();
    let upstream = Router::new()
        .route(
            "/v1beta/models/gemini-2.5-pro:generateContent",
            post(
                |State(calls): State<GeminiCalls>,
                 headers: HeaderMap,
                 Json(body): Json<Value>| async move {
                    calls.lock().unwrap().push((headers, body));
                    Json(json!({
                        "responseId":"gemini-codex","modelVersion":"gemini-2.5-pro",
                        "candidates":[{"finishReason":"STOP","content":{"role":"model","parts":[{"text":"gemini-codex-ok"}]}}],
                        "usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":2,"totalTokenCount":6}
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
            id: "gemini".into(),
            name: "Gemini".into(),
            protocol: Protocol::GeminiNative,
            endpoint: format!("http://{upstream_address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::XApiKey,
            api_key: Some("gemini-secret".into()),
            enabled: true,
            models_url: None,
        })
        .unwrap();
    service
        .save_model(ModelInput {
            id: "gemini-pro".into(),
            name: "Gemini Pro".into(),
            upstream_id: "gemini-2.5-pro".into(),
            provider_id: "gemini".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
        })
        .unwrap();
    service
        .set_model_native_protocols("gemini-pro", vec![NativeProtocol::GeminiNative])
        .unwrap();
    let gateway = Gateway::new(temp.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_codex(vec!["gemini-pro".into()], "codex-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/codex/v1/responses"))
        .bearer_auth("codex-token")
        .json(&json!({
            "model":"grillforge/gemini-pro","input":"ping","stream":false,"store":false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(
        response["output"][0]["content"][0]["text"],
        "gemini-codex-ok"
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0["x-goog-api-key"], "gemini-secret");
    assert_eq!(calls[0].1["contents"][0]["parts"][0]["text"], "ping");
}

#[tokio::test]
async fn codex_route_streams_anthropic_text_as_responses_events() {
    let upstream = Router::new().route(
        "/v1/messages",
        post(|| async {
            let sse = concat!(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_stream\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":5,\"output_tokens\":0}}}\n\n",
                "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"stream-anthropic\"}}\n\n",
                "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
            );
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(sse))
                .unwrap()
        }),
    );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let temp = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(temp.path());
    service
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
    service
        .save_model(ModelInput {
            id: "claude-sonnet".into(),
            name: "Claude Sonnet".into(),
            upstream_id: "claude-sonnet".into(),
            provider_id: "anthropic".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .unwrap();
    let gateway = Gateway::new(temp.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_codex(vec!["claude-sonnet".into()], "codex-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/codex/v1/responses"))
        .bearer_auth("codex-token")
        .json(
            &json!({"model":"grillforge/claude-sonnet","input":"ping","stream":true,"store":false}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let body = response.text().await.unwrap();
    assert!(body.contains("event: response.created"));
    assert!(body.contains("event: response.output_text.delta"));
    assert!(body.contains("\"delta\":\"stream-anthropic\""));
    assert!(body.contains("event: response.output_item.done"));
    assert!(body.contains("event: response.completed"));
    assert!(body.contains("\"input_tokens\":5"));
    assert!(body.contains("\"output_tokens\":2"));
}

#[tokio::test]
async fn codex_anthropic_route_surfaces_the_first_upstream_error_without_retry() {
    let calls = Arc::new(AtomicUsize::new(0));
    let upstream = Router::new()
        .route(
            "/v1/messages",
            post(|State(calls): State<Arc<AtomicUsize>>| async move {
                calls.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({
                        "type":"error",
                        "error":{"type":"rate_limit_error","message":"quota exhausted"}
                    })),
                )
            }),
        )
        .with_state(calls.clone());
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let temp = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(temp.path());
    service
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
    service
        .save_model(ModelInput {
            id: "claude-sonnet".into(),
            name: "Claude Sonnet".into(),
            upstream_id: "claude-sonnet".into(),
            provider_id: "anthropic".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .unwrap();
    let gateway = Gateway::new(temp.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_codex(vec!["claude-sonnet".into()], "codex-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/codex/v1/responses"))
        .bearer_auth("codex-token")
        .json(&json!({"model":"grillforge/claude-sonnet","input":"ping","stream":false,"store":false}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["type"], "rate_limit_error");
    assert_eq!(body["error"]["message"], "quota exhausted");
}
