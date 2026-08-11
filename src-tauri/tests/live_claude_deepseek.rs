use axum::{Json, Router, extract::State, routing::post};
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput, SubAgentInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::{Gateway, RouteSpec};
use grillforge_lib::integration::IntegrationService;
use serde_json::{Value, json};
use std::env;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;
use tokio::net::TcpListener;

#[derive(Clone, Default)]
struct NativeTrace(Arc<Mutex<Vec<usize>>>);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses an installed Claude Code CLI and a real DeepSeek API key"]
async fn installed_claude_cli_reaches_two_deepseek_workers_through_grillforge() {
    let api_key = env::var("GRILLFORGE_LIVE_API_KEY")
        .expect("GRILLFORGE_LIVE_API_KEY must be set for the live Claude test");
    let trace = NativeTrace::default();
    let native = Router::new()
        .route(
            "/v1/messages",
            post(
                |State(trace): State<NativeTrace>, Json(body): Json<Value>| async move {
                    let tool_results = body["messages"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|message| message["content"].as_array())
                        .flatten()
                        .filter(|block| block["type"] == "tool_result")
                        .count();
                    trace.0.lock().expect("native trace").push(tool_results);
                    let (content, stop_reason) = match tool_results {
                        0 => (json!([{
                                "type":"tool_use",
                                "id":"toolu_grillforge_live_deepseek_flash",
                                "name":"Agent",
                                "input":{
                                    "description":"Run DeepSeek Flash Worker",
                                    "prompt":"Reply with one concise sentence confirming the Flash route.",
                                    "subagent_type":"grillforge-worker-deepseek-flash",
                                    "run_in_background":false
                                }
                            }]), "tool_use"),
                        1 => (json!([{
                                "type":"tool_use",
                                "id":"toolu_grillforge_live_deepseek_pro",
                                "name":"Agent",
                                "input":{
                                    "description":"Run DeepSeek Pro Worker",
                                    "prompt":"Reply with one concise sentence confirming the Pro route.",
                                    "subagent_type":"grillforge-worker-deepseek-pro",
                                    "run_in_background":false
                                }
                            }]), "tool_use"),
                        _ => (
                            json!([{"type":"text","text":"main received both DeepSeek worker results"}]),
                            "end_turn",
                        ),
                    };
                    Json(json!({
                        "id":"msg_native_live_deepseek",
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
        .with_state(trace.clone());
    let native_listener = TcpListener::bind("127.0.0.1:0").await.expect("native mock");
    let native_address = native_listener.local_addr().expect("native address");
    tokio::spawn(async move {
        axum::serve(native_listener, native)
            .await
            .expect("serve native mock")
    });

    let directory = tempdir().expect("temporary roots");
    let grillforge_root = directory.path().join("grillforge");
    let claude_root = directory.path().join("claude");
    std::fs::create_dir_all(&claude_root).expect("Claude config root");
    let control = ControlPlaneService::new(&grillforge_root);
    control
        .save_provider(ProviderInput {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: "https://api.deepseek.com/anthropic".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some(api_key),
            enabled: true,
            models_url: Some("https://api.deepseek.com/models".into()),
        })
        .expect("DeepSeek provider");
    for (id, name, upstream_id, capabilities) in [
        (
            "deepseek-flash",
            "DeepSeek V4 Flash",
            "deepseek-v4-flash",
            vec!["coding".into(), "fast".into()],
        ),
        (
            "deepseek-pro",
            "DeepSeek V4 Pro",
            "deepseek-v4-pro",
            vec!["coding".into(), "reasoning".into()],
        ),
    ] {
        control
            .save_model(ModelInput {
                id: id.into(),
                name: name.into(),
                upstream_id: upstream_id.into(),
                provider_id: "deepseek".into(),
                capabilities: vec!["coding".into()],
                protocol_capabilities: Vec::new(),
            })
            .expect("DeepSeek model");
        control
            .save_subagent(SubAgentInput {
                id: id.into(),
                name: name.into(),
                model_id: id.into(),
                capabilities,
                enabled: true,
            })
            .expect("DeepSeek SubAgent");
    }

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
    let state = control.state().expect("state");
    integration
        .apply(&state, &gateway_status.base_url)
        .expect("apply integration");
    gateway_status.activate(&state).expect("activate routes");

    let process_root = claude_root.clone();
    let trace_for_timeout = trace.clone();
    let output = tokio::task::spawn_blocking(move || {
        let mut child = Command::new("claude")
            .args([
                "--print",
                "--no-session-persistence",
                "--output-format",
                "json",
                "delegate once to the DeepSeek Flash worker and once to the DeepSeek Pro worker",
            ])
            .current_dir(&process_root)
            .env("CLAUDE_CONFIG_DIR", &process_root)
            .env("ANTHROPIC_MODEL", "main-loopback")
            .env("ANTHROPIC_API_KEY", "untrusted-inbound-dummy")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start installed Claude Code CLI");
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if child.try_wait().expect("poll Claude CLI").is_some() {
                return child.wait_with_output().expect("Claude output");
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill timed-out Claude CLI");
                let output = child.wait_with_output().expect("reap Claude CLI");
                panic!(
                    "Claude CLI live DeepSeek E2E exceeded 120 seconds; native trace={:?}; stderr={}",
                    trace_for_timeout.0.lock().expect("native trace"),
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
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("grillforge/deepseek-flash"));
    assert!(stdout.contains("grillforge/deepseek-pro"));
    let mut normalized_trace = trace.0.lock().expect("native trace").clone();
    normalized_trace.dedup();
    assert_eq!(normalized_trace, [0, 1, 2]);
    integration.disable().expect("restore Claude configuration");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "uses Claude Client's embedded Code binary and a real DeepSeek API key; never contacts Anthropic"]
async fn claude_client_code_reaches_deepseek_worker_through_official_3p_gateway() {
    let api_key = env::var("GRILLFORGE_LIVE_API_KEY")
        .expect("GRILLFORGE_LIVE_API_KEY must be set for the live Claude Client test");
    let claude = env::var("GRILLFORGE_CLAUDE_DESKTOP_CODE_BIN")
        .expect("GRILLFORGE_CLAUDE_DESKTOP_CODE_BIN must point to Claude Client's Code binary");
    let directory = tempdir().expect("temporary roots");
    let grillforge_root = directory.path().join("grillforge");
    let claude_root = directory.path().join("claude");
    std::fs::create_dir_all(&claude_root).expect("Claude config root");
    let control = ControlPlaneService::new(&grillforge_root);
    control
        .save_provider(ProviderInput {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: "https://api.deepseek.com/anthropic".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some(api_key),
            enabled: true,
            models_url: Some("https://api.deepseek.com/models".into()),
        })
        .expect("DeepSeek provider");
    control
        .save_model(ModelInput {
            id: "deepseek-v4-flash".into(),
            name: "DeepSeek V4 Flash".into(),
            upstream_id: "deepseek-v4-flash".into(),
            provider_id: "deepseek".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: Vec::new(),
        })
        .expect("DeepSeek model");

    let gateway = Gateway::new(&grillforge_root);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("gateway address");
    let status = gateway.status(format!("http://{address}"));
    status
        .activate_claude_desktop(
            vec![
                RouteSpec {
                    route_id: "claude-sonnet-5".into(),
                    model_id: "deepseek-v4-flash".into(),
                    label_override: Some("DeepSeek V4 Flash".into()),
                    supports_1m: false,
                },
                RouteSpec {
                    route_id: "grillforge/deepseek-v4-flash".into(),
                    model_id: "deepseek-v4-flash".into(),
                    label_override: None,
                    supports_1m: false,
                },
            ],
            "desktop-local-dummy-token",
        )
        .expect("activate Claude Client routes");
    tokio::spawn(async move {
        axum::serve(listener, gateway.router())
            .await
            .expect("serve gateway")
    });

    let process_root = claude_root.clone();
    let gateway_url = format!("http://{address}/claude-desktop");
    let output = tokio::task::spawn_blocking(move || {
        let mut child = Command::new(claude)
            .args([
                "--print",
                "--no-session-persistence",
                "--output-format",
                "json",
                "--model",
                "grillforge/deepseek-v4-flash",
                "Reply with exactly: DESKTOP_3P_OK",
            ])
            .current_dir(&process_root)
            .env("CLAUDE_CONFIG_DIR", &process_root)
            .env("CLAUDE_CODE_ENTRYPOINT", "claude-desktop-3p")
            .env("CLAUDE_CODE_OAUTH_TOKEN", "desktop-local-dummy-token")
            .env("ANTHROPIC_BASE_URL", gateway_url)
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start Claude Client Code binary");
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if child.try_wait().expect("poll Claude Client Code").is_some() {
                return child.wait_with_output().expect("Claude Client output");
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill timed-out Claude Client Code");
                let output = child.wait_with_output().expect("reap Claude Client Code");
                panic!(
                    "Claude Client Code 3P E2E exceeded 120 seconds: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            thread::sleep(Duration::from_millis(20));
        }
    })
    .await
    .expect("join Claude Client Code process");

    assert!(
        output.status.success(),
        "Claude Client Code failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("DESKTOP_3P_OK"),
        "unexpected output: {stdout}"
    );
    assert!(
        stdout.contains("grillforge/deepseek-v4-flash"),
        "model usage did not contain the worker alias: {stdout}"
    );
}
