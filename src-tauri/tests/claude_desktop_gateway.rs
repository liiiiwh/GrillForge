use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::{Gateway, RouteSpec};
use serde_json::{Value, json};
use std::fs;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<(HeaderMap, Value)>>>);

async fn serve(router: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, router).await.expect("server") });
    format!("http://{address}")
}

fn save_provider_and_model(
    service: &ControlPlaneService,
    protocol: Protocol,
    endpoint: String,
    placement: ApiKeyPlacement,
    api_key: Option<&str>,
) {
    service
        .save_provider(ProviderInput {
            id: "provider".into(),
            name: "Provider".into(),
            protocol,
            endpoint,
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: placement,
            api_key: api_key.map(str::to_string),
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    service
        .save_model(ModelInput {
            id: "private-model".into(),
            name: "Private Model".into(),
            upstream_id: "upstream-secret-name".into(),
            provider_id: "provider".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
        })
        .expect("model");
}

fn route(route_id: &str) -> RouteSpec {
    RouteSpec {
        route_id: route_id.into(),
        model_id: "private-model".into(),
        label_override: Some("本地编码模型".into()),
        supports_1m: true,
    }
}

async fn desktop_gateway(gateway: Gateway) -> String {
    serve(gateway.router()).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Claude Client's embedded Code binary; uses only loopback and dummy credentials"]
async fn embedded_claude_client_code_routes_a_named_worker_through_threep_gateway() {
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
                        let model = body["model"].as_str().expect("upstream model");
                        let has_tool_result = body["messages"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|message| message["content"].as_array())
                            .flatten()
                            .any(|block| block["type"] == "tool_result");
                        let (content, stop_reason) = if model == "upstream-main" && !has_tool_result
                        {
                            (
                                json!([{
                                    "type": "tool_use",
                                    "id": "toolu_desktop_worker_e2e",
                                    "name": "Agent",
                                    "input": {
                                        "description": "Run GrillForge Worker",
                                        "prompt": "Return the loopback result",
                                        "subagent_type": "grillforge-worker-worker",
                                        "run_in_background": false
                                    }
                                }]),
                                "tool_use",
                            )
                        } else {
                            (
                                json!([{"type":"text","text":format!("loopback {model}")}]),
                                "end_turn",
                            )
                        };
                        Json(json!({
                            "id": "msg_desktop_worker_e2e",
                            "type": "message",
                            "role": "assistant",
                            "model": model,
                            "content": content,
                            "stop_reason": stop_reason,
                            "stop_sequence": null,
                            "usage": {"input_tokens": 1, "output_tokens": 1}
                        }))
                    },
                ),
            )
            .with_state(capture.clone());
    let upstream_url = serve(upstream).await;
    let directory = tempfile::tempdir().expect("temporary roots");
    let service = ControlPlaneService::new(directory.path());
    service
        .save_provider(ProviderInput {
            id: "loopback".into(),
            name: "Loopback".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: upstream_url,
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    for (id, upstream_id) in [("main", "upstream-main"), ("worker", "upstream-worker")] {
        service
            .save_model(ModelInput {
                id: id.into(),
                name: id.into(),
                upstream_id: upstream_id.into(),
                provider_id: "loopback".into(),
                capabilities: vec!["coding".into()],
                protocol_capabilities: vec![],
            })
            .expect("model");
    }
    let gateway = Gateway::new(directory.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_claude_desktop(
            vec![
                RouteSpec {
                    route_id: "claude-sonnet-5".into(),
                    model_id: "main".into(),
                    label_override: None,
                    supports_1m: false,
                },
                RouteSpec {
                    route_id: "grillforge/worker".into(),
                    model_id: "worker".into(),
                    label_override: None,
                    supports_1m: false,
                },
            ],
            "desktop-dummy-token",
        )
        .expect("activate routes");
    let gateway_url = desktop_gateway(gateway).await;

    let claude_root = directory.path().join("claude");
    fs::create_dir_all(claude_root.join("agents")).expect("Agent directory");
    fs::write(claude_root.join("settings.json"), "{}\n").expect("settings");
    fs::write(
        claude_root.join("agents/grillforge-worker-worker.md"),
        "---\nname: grillforge-worker-worker\ndescription: Loopback Worker\nmodel: grillforge/worker\n---\nReturn the requested result.\n",
    )
    .expect("Agent definition");
    let binary = std::env::var_os("GRILLFORGE_CLAUDE_DESKTOP_CODE_BIN")
        .expect("GRILLFORGE_CLAUDE_DESKTOP_CODE_BIN must point to Claude Client's Code binary");
    let process_root = claude_root.clone();
    let endpoint = format!("{gateway_url}/claude-desktop");
    let output = tokio::task::spawn_blocking(move || {
        let mut child = Command::new(binary)
            .args([
                "--print",
                "--no-session-persistence",
                "--output-format",
                "json",
                "--model",
                "claude-sonnet-5",
                "Delegate this task to grillforge-worker-worker.",
            ])
            .current_dir(&process_root)
            .env("CLAUDE_CONFIG_DIR", &process_root)
            .env("CLAUDE_CODE_ENTRYPOINT", "claude-desktop-3p")
            .env("CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST", "1")
            .env("ANTHROPIC_BASE_URL", endpoint)
            .env("ANTHROPIC_AUTH_TOKEN", "desktop-dummy-token")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start embedded Claude Code");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if child.try_wait().expect("poll Claude Code").is_some() {
                return child.wait_with_output().expect("Claude Code output");
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill timed-out Claude Code");
                return child.wait_with_output().expect("reap Claude Code");
            }
            thread::sleep(Duration::from_millis(20));
        }
    })
    .await
    .expect("join Claude Code");

    assert!(
        output.status.success(),
        "embedded Claude Code failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let models = capture
        .0
        .lock()
        .expect("capture")
        .iter()
        .map(|(_, body)| body["model"].as_str().expect("model").to_string())
        .fold(Vec::new(), |mut models, model| {
            if models.last() != Some(&model) {
                models.push(model);
            }
            models
        });
    assert_eq!(
        models,
        ["upstream-main", "upstream-worker", "upstream-main"]
    );
}

#[tokio::test]
async fn desktop_models_require_exact_bearer_token_and_only_expose_safe_routes() {
    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());
    save_provider_and_model(
        &service,
        Protocol::OpenAiChatCompletions,
        "http://127.0.0.1:9".into(),
        ApiKeyPlacement::None,
        None,
    );
    let gateway = Gateway::new(directory.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_claude_desktop(
            vec![
                route("claude-sonnet-4-6"),
                route("grillforge/private-model"),
            ],
            "local-token",
        )
        .expect("activate desktop");
    let base_url = desktop_gateway(gateway).await;
    let client = reqwest::Client::new();

    for authorization in [None, Some("Bearer wrong"), Some("bearer local-token")] {
        let mut request = client.get(format!("{base_url}/claude-desktop/v1/models"));
        if let Some(value) = authorization {
            request = request.header("authorization", value);
        }
        let response = request.send().await.expect("unauthorized response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = response.text().await.expect("error body");
        assert!(!body.contains("private-model"));
        assert!(!body.contains("upstream-secret-name"));
        assert!(!body.contains("provider"));
        assert!(!body.contains("local-token"));
    }
    let unauthorized_post = client
        .post(format!("{base_url}/claude-desktop/v1/messages"))
        .body("not-json")
        .send()
        .await
        .expect("unauthorized POST");
    assert_eq!(unauthorized_post.status(), StatusCode::UNAUTHORIZED);

    let response = client
        .get(format!("{base_url}/claude-desktop/v1/models"))
        .bearer_auth("local-token")
        .send()
        .await
        .expect("model response");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("model JSON");
    assert_eq!(
        body,
        json!({
            "data": [{
                "type": "model",
                "id": "claude-sonnet-4-6",
                "created_at": "2024-01-01T00:00:00Z",
                "supports1m": true
            }],
            "has_more": false,
            "first_id": "claude-sonnet-4-6",
            "last_id": "claude-sonnet-4-6"
        })
    );
    let serialized = body.to_string();
    assert!(!serialized.contains("private-model"));
    assert!(!serialized.contains("upstream-secret-name"));
    assert!(!serialized.contains("provider"));
}

#[tokio::test]
async fn desktop_code_worker_route_maps_through_openai_compatible_bridge() {
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
                            "id": "chatcmpl_desktop",
                            "object": "chat.completion",
                            "model": "upstream-secret-name",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "mapped"},
                                "finish_reason": "stop"
                            }],
                            "usage": {"prompt_tokens": 2, "completion_tokens": 1}
                        }))
                    },
                ),
            )
            .with_state(capture.clone());
    let upstream_url = serve(upstream).await;
    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());
    save_provider_and_model(
        &service,
        Protocol::OpenAiChatCompletions,
        upstream_url,
        ApiKeyPlacement::Bearer,
        Some("provider-secret"),
    );
    let gateway = Gateway::new(directory.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_claude_desktop(
            vec![
                route("claude-sonnet-4-6"),
                route("grillforge/private-model"),
            ],
            "desktop-token",
        )
        .expect("activate desktop");
    let base_url = desktop_gateway(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{base_url}/claude-desktop/v1/messages"))
        .bearer_auth("desktop-token")
        .json(&json!({
            "model": "grillforge/private-model",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("desktop response");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("Anthropic JSON");
    assert_eq!(body["content"][0]["text"], "mapped");
    let calls = capture.0.lock().expect("captured calls");
    assert_eq!(calls[0].1["model"], "upstream-secret-name");
    assert_eq!(calls[0].0["authorization"], "Bearer provider-secret");
    assert_ne!(calls[0].0["authorization"], "Bearer desktop-token");
}

#[tokio::test]
async fn desktop_messages_map_safe_route_through_anthropic_provider() {
    let capture = Capture::default();
    let upstream =
        Router::new()
            .route(
                "/v1/messages",
                post(
                    |State(capture): State<Capture>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        capture.0.lock().expect("capture").push((headers, body));
                        Json(json!({
                            "id": "msg_desktop",
                            "type": "message",
                            "role": "assistant",
                            "model": "upstream-secret-name",
                            "content": [{"type": "text", "text": "native"}],
                            "stop_reason": "end_turn",
                            "usage": {"input_tokens": 2, "output_tokens": 1}
                        }))
                    },
                ),
            )
            .with_state(capture.clone());
    let upstream_url = serve(upstream).await;
    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());
    save_provider_and_model(
        &service,
        Protocol::AnthropicMessages,
        upstream_url,
        ApiKeyPlacement::XApiKey,
        Some("anthropic-secret"),
    );
    let gateway = Gateway::new(directory.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_claude_desktop(vec![route("claude-opus-4-8")], "desktop-token")
        .expect("activate desktop");
    let base_url = desktop_gateway(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{base_url}/claude-desktop/v1/messages"))
        .bearer_auth("desktop-token")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "claude-opus-4-8",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("desktop response");
    assert_eq!(response.status(), StatusCode::OK);
    let calls = capture.0.lock().expect("captured calls");
    assert_eq!(calls[0].1["model"], "upstream-secret-name");
    assert_eq!(calls[0].0["x-api-key"], "anthropic-secret");
    assert!(calls[0].0.get("authorization").is_none());
}

#[tokio::test]
async fn unknown_route_fails_before_upstream_and_failed_activation_keeps_previous_state() {
    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());
    save_provider_and_model(
        &service,
        Protocol::OpenAiChatCompletions,
        "http://127.0.0.1:9".into(),
        ApiKeyPlacement::None,
        None,
    );
    let gateway = Gateway::new(directory.path());
    let status = gateway.status("http://127.0.0.1:1".into());
    status
        .activate_claude_desktop(vec![route("claude-sonnet-4-6")], "old-token")
        .expect("initial activation");
    let error = status
        .activate_claude_desktop(
            vec![RouteSpec {
                route_id: "not-desktop-safe".into(),
                model_id: "missing-model".into(),
                label_override: None,
                supports_1m: false,
            }],
            "new-token",
        )
        .expect_err("invalid activation");
    assert!(error.contains("Claude-safe"));
    let base_url = desktop_gateway(gateway).await;
    let client = reqwest::Client::new();

    let unknown = client
        .post(format!("{base_url}/claude-desktop/v1/messages"))
        .bearer_auth("old-token")
        .json(&json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 8,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("unknown route response");
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    assert!(
        unknown
            .text()
            .await
            .expect("body")
            .contains("unknown Claude Desktop route")
    );

    let models = client
        .get(format!("{base_url}/claude-desktop/v1/models"))
        .bearer_auth("old-token")
        .send()
        .await
        .expect("preserved state");
    assert_eq!(models.status(), StatusCode::OK);
    let body: Value = models.json().await.expect("models");
    assert_eq!(body["data"][0]["id"], "claude-sonnet-4-6");
}

#[tokio::test]
async fn cli_and_desktop_activation_and_deactivation_are_independent() {
    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());
    save_provider_and_model(
        &service,
        Protocol::OpenAiChatCompletions,
        "http://127.0.0.1:9".into(),
        ApiKeyPlacement::None,
        None,
    );
    let gateway = Gateway::new(directory.path());
    let status = gateway.status("http://127.0.0.1:1".into());
    status
        .activate(
            &service
                .set_main_model(Some("private-model".into()))
                .expect("CLI state"),
        )
        .expect("CLI activation");
    status
        .activate_claude_desktop(vec![route("claude-sonnet-4-6")], "desktop-token")
        .expect("desktop activation");
    let base_url = desktop_gateway(gateway).await;
    let client = reqwest::Client::new();

    status.deactivate_claude_desktop();
    let cli_still_active = client
        .post(format!("{base_url}/v1/messages"))
        .json(&json!({
            "model": "grillforge/private-model",
            "max_tokens": 8,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("CLI response");
    assert_eq!(cli_still_active.status(), StatusCode::BAD_GATEWAY);

    status
        .activate_claude_desktop(vec![route("claude-sonnet-4-6")], "desktop-token")
        .expect("reactivate desktop");
    status.deactivate();
    let desktop_still_active = client
        .get(format!("{base_url}/claude-desktop/v1/models"))
        .bearer_auth("desktop-token")
        .send()
        .await
        .expect("desktop models");
    assert_eq!(desktop_still_active.status(), StatusCode::OK);

    status.deactivate_claude_desktop();
    let cli_inactive = client
        .post(format!("{base_url}/v1/messages"))
        .json(&json!({
            "model": "grillforge/private-model",
            "max_tokens": 8,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("CLI response");
    assert_eq!(cli_inactive.status(), StatusCode::BAD_REQUEST);
    assert!(
        cli_inactive
            .text()
            .await
            .expect("body")
            .contains("inactive GrillForge route")
    );

    let desktop = client
        .get(format!("{base_url}/claude-desktop/v1/models"))
        .bearer_auth("desktop-token")
        .send()
        .await
        .expect("desktop response");
    assert_eq!(desktop.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn desktop_activation_rejects_duplicate_routes_unknown_models_and_disabled_providers() {
    let directory = tempfile::tempdir().expect("configuration directory");
    let service = ControlPlaneService::new(directory.path());
    save_provider_and_model(
        &service,
        Protocol::OpenAiChatCompletions,
        "http://127.0.0.1:9".into(),
        ApiKeyPlacement::None,
        None,
    );
    let status = Gateway::new(directory.path()).status("http://127.0.0.1:1".into());
    let duplicate = status
        .activate_claude_desktop(
            vec![route("claude-sonnet-4-6"), route("claude-sonnet-4-6")],
            "token",
        )
        .expect_err("duplicate route");
    assert!(duplicate.contains("duplicate Claude Desktop route"));

    let mut unknown = route("claude-opus-4-8");
    unknown.model_id = "unknown".into();
    let error = status
        .activate_claude_desktop(vec![unknown], "token")
        .expect_err("unknown model");
    assert!(error.contains("unknown model"));

    service
        .update_provider(ProviderInput {
            id: "provider".into(),
            name: "Provider".into(),
            protocol: Protocol::OpenAiChatCompletions,
            endpoint: "http://127.0.0.1:9".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: false,
            models_url: None,
        })
        .expect("disable provider");
    let error = status
        .activate_claude_desktop(vec![route("claude-haiku-4-5")], "token")
        .expect_err("disabled provider");
    assert!(error.contains("disabled provider"));
}
