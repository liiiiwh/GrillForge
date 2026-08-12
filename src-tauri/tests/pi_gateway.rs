use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::post,
};
use grillforge_lib::adapters::pi::{PiAdapter, PiModelSpec, PiPaths, PiRequest};
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use serde_json::{Value, json};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[tokio::test]
async fn pi_route_requires_its_token_and_reaches_the_selected_provider() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upstream = Router::new()
        .route(
            "/v1/chat/completions",
            post(
                |State(calls): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    calls.lock().unwrap().push(body);
                    Json(json!({
                        "id": "chatcmpl_pi",
                        "object": "chat.completion",
                        "model": "deepseek-chat",
                        "choices": [{"index": 0, "message": {"role": "assistant", "content": "pi-ok"}, "finish_reason": "stop"}],
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
            id: "deepseek".into(),
            name: "DeepSeek".into(),
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
            id: "deepseek-chat".into(),
            name: "DeepSeek Chat".into(),
            upstream_id: "deepseek-chat".into(),
            provider_id: "deepseek".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .unwrap();

    let gateway = Gateway::new(temp.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_pi(vec!["deepseek-chat".into()], "pi-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });
    let client = reqwest::Client::new();
    let body = json!({
        "model": "grillforge/deepseek-chat",
        "max_tokens": 16,
        "messages": [{"role": "user", "content": "ping"}]
    });

    let unauthorized = client
        .post(format!("http://{address}/pi/v1/messages"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let unauthorized: Value = unauthorized.json().await.unwrap();
    assert_eq!(
        unauthorized["error"]["message"],
        "Pi gateway authorization failed"
    );

    let response = client
        .post(format!("http://{address}/pi/v1/messages"))
        .header("x-api-key", "pi-token")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["content"][0]["text"], "pi-ok");
    assert_eq!(calls.lock().unwrap()[0]["model"], "deepseek-chat");
}

#[tokio::test]
async fn claude_and_pi_routes_reach_gemini_native_through_the_shared_gateway() {
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
                        "responseId":"gemini-gateway",
                        "modelVersion":"gemini-2.5-pro",
                        "candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"gemini-ok"}]}}],
                        "usageMetadata":{"promptTokenCount":2,"totalTokenCount":3}
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
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .unwrap();
    service.set_main_model(Some("gemini-pro".into())).unwrap();
    let state = service
        .set_pi_model_enabled("gemini-pro".into(), true)
        .unwrap();

    let gateway = Gateway::new(temp.path());
    let status = gateway.status("http://127.0.0.1:1".into());
    status.activate(&state).unwrap();
    status
        .activate_pi(vec!["gemini-pro".into()], "pi-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });
    let body = json!({
        "model":"grillforge/gemini-pro",
        "max_tokens":16,
        "messages":[{"role":"user","content":"ping"}]
    });
    let client = reqwest::Client::new();

    let claude: Value = client
        .post(format!("http://{address}/v1/messages"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pi: Value = client
        .post(format!("http://{address}/pi/v1/messages"))
        .header("x-api-key", "pi-token")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(claude["content"][0]["text"], "gemini-ok");
    assert_eq!(pi["content"][0]["text"], "gemini-ok");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|(headers, body)| {
        headers["x-goog-api-key"] == "gemini-secret"
            && body["contents"][0]["parts"][0]["text"] == "ping"
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an installed Pi CLI; uses only loopback and a dummy gateway token"]
async fn installed_pi_cli_uses_its_real_auth_header_through_grillforge() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upstream = Router::new()
        .route(
            "/v1/chat/completions",
            post(
                |State(calls): State<Arc<Mutex<Vec<Value>>>>,
                 Json(body): Json<Value>| async move {
                    calls.lock().unwrap().push(body);
                    let sse = concat!(
                        "data: {\"id\":\"chatcmpl_pi_cli\",\"object\":\"chat.completion.chunk\",\"model\":\"deepseek-chat\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"pi-ok\"},\"finish_reason\":null}]}\n\n",
                        "data: {\"id\":\"chatcmpl_pi_cli\",\"object\":\"chat.completion.chunk\",\"model\":\"deepseek-chat\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1}}\n\n",
                        "data: [DONE]\n\n"
                    );
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(sse))
                        .unwrap()
                },
            ),
        )
        .with_state(calls.clone());
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let temp = tempfile::tempdir().unwrap();
    let grillforge_root = temp.path().join("grillforge");
    let pi_root = temp.path().join("pi");
    let service = ControlPlaneService::new(&grillforge_root);
    service
        .save_provider(ProviderInput {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
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
            id: "deepseek-chat".into(),
            name: "DeepSeek Chat".into(),
            upstream_id: "deepseek-chat".into(),
            provider_id: "deepseek".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .unwrap();

    let gateway = Gateway::new(&grillforge_root);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let gateway_status = gateway.status(format!("http://{address}"));
    gateway_status
        .activate_pi(vec!["deepseek-chat".into()], "pi-cli-token")
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let model = PiModelSpec::new(
        "grillforge/deepseek-chat",
        "DeepSeek Chat",
        false,
        vec!["text".into()],
        128_000,
        16_384,
    )
    .unwrap();
    PiAdapter::new(
        PiPaths::new(pi_root.join("models.json"), pi_root.join("settings.json")),
        &grillforge_root,
    )
    .apply(
        PiRequest::new(
            format!("http://{address}/pi"),
            "pi-cli-token",
            vec![model],
            Some("grillforge/deepseek-chat".into()),
        )
        .unwrap(),
    )
    .unwrap();

    let pi_root_for_process = pi_root.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new("pi")
            .args([
                "--print",
                "--no-session",
                "--no-tools",
                "--no-extensions",
                "--no-skills",
                "--no-context-files",
                "--offline",
                "reply pi-ok",
            ])
            .env("PI_CODING_AGENT_DIR", pi_root_for_process)
            .stdin(Stdio::null())
            .output()
            .expect("run installed Pi CLI")
    })
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "Pi CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("pi-ok"));
    assert_eq!(calls.lock().unwrap()[0]["model"], "deepseek-chat");
}
