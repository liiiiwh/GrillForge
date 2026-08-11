use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::Response,
    routing::post,
};
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::model::ProtocolCapability;
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use grillforge_lib::integration::IntegrationService;
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;
use tokio::net::TcpListener;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an installed Claude Code CLI; uses only loopback and dummy credentials"]
async fn installed_claude_cli_reaches_a_responses_worker_through_grillforge() {
    let native_capture = Capture::default();
    let native = Router::new()
        .route(
            "/v1/messages",
            post(
                |State(capture): State<Capture>, Json(body): Json<Value>| async move {
                    let has_tool_result = body["messages"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|message| message["content"].as_array())
                        .flatten()
                        .any(|block| block["type"] == "tool_result");
                    capture.0.lock().expect("capture").push(format!(
                        "model={} stream={} tool_result={has_tool_result}",
                        body["model"].as_str().unwrap_or("<missing>"),
                        body["stream"].as_bool().unwrap_or(false),
                    ));
                    let is_worker = body["model"]
                        .as_str()
                        .is_some_and(|model| model.starts_with("upstream-worker"));
                    let (content, stop_reason) = if has_tool_result || is_worker {
                        (
                            json!([{"type":"text","text":"main received worker result"}]),
                            "end_turn",
                        )
                    } else {
                        (
                            json!([{
                                "type":"tool_use",
                                "id":"toolu_grillforge_gateway_e2e",
                                "name":"Agent",
                                "input":{
                                    "description":"Run Responses Worker",
                                    "prompt":"Return a concise loopback result",
                                    "subagent_type":"grillforge-worker-worker-a",
                                    "run_in_background":false
                                }
                            }]),
                            "tool_use",
                        )
                    };
                    Json(json!({
                        "id":"msg_native_e2e",
                        "type":"message",
                        "role":"assistant",
                        "model":body["model"],
                        "content":content,
                        "stop_reason":stop_reason,
                        "stop_sequence":null,
                        "usage":{"input_tokens":1,"output_tokens":1}
                    }))
                },
            ),
        )
        .with_state(native_capture.clone());
    let native_listener = TcpListener::bind("127.0.0.1:0").await.expect("native mock");
    let native_address = native_listener.local_addr().expect("native address");
    tokio::spawn(async move {
        axum::serve(native_listener, native)
            .await
            .expect("serve native mock")
    });

    let capture = Capture::default();
    let responses = Router::new()
        .route(
            "/v1/responses",
            post(
                |State(capture): State<Capture>, Json(body): Json<Value>| async move {
                    capture
                        .0
                        .lock()
                        .expect("capture")
                        .push(body["model"].as_str().expect("model").to_string());
                    if body["stream"].as_bool() != Some(true) {
                        return Response::builder()
                            .status(StatusCode::OK)
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(Body::from(
                                json!({
                                    "id":"resp_probe",
                                    "status":"completed",
                                    "model":body["model"],
                                    "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"probe ok"}]}],
                                    "usage":{"input_tokens":1,"output_tokens":1}
                                })
                                .to_string(),
                            ))
                            .expect("JSON response");
                    }
                    let sse = [
                        ("response.created", json!({"type":"response.created","response":{"id":"resp_e2e","model":body["model"]}})),
                        ("response.output_item.added", json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg_worker","type":"message","role":"assistant","content":[]}})),
                        ("response.content_part.added", json!({"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":""}})),
                        ("response.output_text.delta", json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"worker reached through Responses"})),
                        ("response.content_part.done", json!({"type":"response.content_part.done","output_index":0,"content_index":0,"part":{"type":"output_text","text":"worker reached through Responses"}})),
                        ("response.output_item.done", json!({"type":"response.output_item.done","output_index":0,"item":{"id":"msg_worker","type":"message","role":"assistant","content":[{"type":"output_text","text":"worker reached through Responses"}]}})),
                        ("response.completed", json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":2,"output_tokens":4}}})),
                    ]
                    .into_iter()
                    .fold(String::new(), |mut output, (event, data)| {
                        write!(output, "event: {event}\ndata: {data}\n\n")
                            .expect("write SSE fixture");
                        output
                    });
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(sse))
                        .expect("SSE response")
                },
            ),
        )
        .with_state(capture.clone());
    let responses_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Responses mock");
    let responses_address = responses_listener.local_addr().expect("Responses address");
    tokio::spawn(async move {
        axum::serve(responses_listener, responses)
            .await
            .expect("serve Responses mock")
    });

    let directory = tempdir().expect("temporary roots");
    let grillforge_root = directory.path().join("grillforge");
    let claude_root = directory.path().join("claude");
    let control = ControlPlaneService::new(&grillforge_root);
    control
        .save_provider(ProviderInput {
            id: "responses".into(),
            name: "Responses".into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: format!("http://{responses_address}"),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    for id in ["worker-a", "worker-b"] {
        control
            .save_model(ModelInput {
                id: id.into(),
                name: id.into(),
                upstream_id: format!("upstream-{id}"),
                provider_id: "responses".into(),
                capabilities: vec!["coding".into()],
                protocol_capabilities: vec![ProtocolCapability::ReasoningItems],
            })
            .expect("model");
        control.set_worker(id.into(), true).expect("worker");
    }
    control.set_worker_mode(true).expect("worker mode");

    let gateway = Gateway::new(&grillforge_root);
    let gateway_listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let gateway_address = gateway_listener.local_addr().expect("gateway address");
    let gateway_status = gateway.status(format!("http://{gateway_address}"));
    gateway_status
        .set_native_base_url(&format!("http://{native_address}"))
        .expect("native route");
    tokio::spawn(async move {
        axum::serve(gateway_listener, gateway.router())
            .await
            .expect("serve gateway")
    });

    let integration = IntegrationService::new(&claude_root, &grillforge_root);
    integration
        .apply(&control.state().expect("state"), &gateway_status.base_url)
        .expect("apply integration");
    gateway_status
        .activate(&control.state().expect("active state"))
        .expect("activate routes");
    let probe = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway_status.base_url))
        .json(&json!({
            "model":"grillforge/worker-a",
            "max_tokens":8,
            "messages":[{"role":"user","content":"probe"}]
        }))
        .send()
        .await
        .expect("gateway probe");
    assert_eq!(probe.status(), StatusCode::OK);
    capture.0.lock().expect("probe capture").clear();

    let claude_root_for_process = claude_root.clone();
    let native_trace = native_capture.clone();
    let responses_trace = capture.clone();
    let output = tokio::task::spawn_blocking(move || {
        let mut child = Command::new("claude")
            .args([
                "--print",
                "--no-session-persistence",
                "--output-format",
                "json",
                "delegate to worker-a",
            ])
            .current_dir(&claude_root_for_process)
            .env("CLAUDE_CONFIG_DIR", &claude_root_for_process)
            .env("ANTHROPIC_MODEL", "main-loopback")
            .env("ANTHROPIC_API_KEY", "local-dummy-key")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start installed Claude Code CLI");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if child.try_wait().expect("poll Claude CLI").is_some() {
                return child.wait_with_output().expect("Claude output");
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill timed-out Claude CLI");
                let output = child.wait_with_output().expect("reap Claude CLI");
                panic!(
                    "Claude CLI gateway E2E exceeded 20 seconds; native={:?}; responses={:?}; stderr={}",
                    native_trace.0.lock().expect("native trace"),
                    responses_trace.0.lock().expect("responses trace"),
                    String::from_utf8_lossy(&output.stderr),
                );
            }
            thread::sleep(Duration::from_millis(20));
        }
    })
    .await
    .expect("join Claude process");

    assert!(
        output.status.success(),
        "Claude CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        capture.0.lock().expect("captured models").as_slice(),
        ["upstream-worker-a"],
        "native trace: {:?}",
        native_capture.0.lock().expect("native trace")
    );
    integration.disable().expect("restore Claude configuration");
}
