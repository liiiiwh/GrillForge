use axum::{Json, Router, body::Body, extract::State, http::StatusCode, routing::post};
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::model::ProtocolCapability;
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::{AgentRuntimeRoute, AgentSourceRuntime, Gateway};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tokio::net::TcpListener;

#[derive(Clone, Default)]
struct Trace(Arc<Mutex<Vec<bool>>>);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "set GRILLFORGE_LIVE_GEMINI_CLI to the current official Gemini CLI executable; traffic is loopback-only"]
async fn current_gemini_cli_runs_the_exact_project_agent_through_the_gateway() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = Arc::clone(&requests);
    let upstream = Router::new().route(
        "/v1/messages",
        post(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured);
            async move {
                captured.lock().unwrap().push(body.clone());
                let serialized = body.to_string();
                let has_agent_marker = serialized.contains("GEMINI_EXACT_AGENT_MARKER");
                let has_tool_result = serialized.contains("tool_result");
                let reviewer_tool = body["tools"].as_array().and_then(|tools| {
                    tools.iter().find_map(|tool| {
                        (tool["name"] == "reviewer")
                            .then(|| tool["name"].as_str())
                            .flatten()
                    })
                });
                let (content, stop_reason) = if has_agent_marker || has_tool_result {
                    (
                        json!([{"type":"text","text":"Gemini exact Agent completed"}]),
                        "end_turn",
                    )
                } else if let Some(tool_name) = reviewer_tool {
                    (
                        json!([{
                            "type":"tool_use","id":"toolu_gemini_live","name":tool_name,
                            "input":{"task":"Return the completion marker."}
                        }]),
                        "tool_use",
                    )
                } else {
                    (
                        json!([{"type":"text","text":"Gemini reviewer Agent tool was absent"}]),
                        "end_turn",
                    )
                };
                let response = json!({
                    "id":"msg_gemini_live","type":"message","role":"assistant",
                    "model":body["model"],"content":content,
                    "stop_reason":stop_reason,"stop_sequence":null,
                    "usage":{"input_tokens":2,"output_tokens":2}
                });
                if body["stream"].as_bool() != Some(true) {
                    return axum::response::Response::builder()
                        .header("content-type", "application/json")
                        .body(Body::from(response.to_string()))
                        .unwrap();
                }
                let message_start = json!({
                    "type":"message_start","message":{
                        "id":"msg_gemini_live","type":"message","role":"assistant",
                        "model":body["model"],"content":[],"stop_reason":null,
                        "stop_sequence":null,"usage":{"input_tokens":2,"output_tokens":0}
                    }
                });
                let (block, delta) = if stop_reason == "tool_use" {
                    (
                        json!({
                            "type":"content_block_start","index":0,
                            "content_block":{
                                "type":"tool_use","id":"toolu_gemini_live","name":"reviewer","input":{}
                            }
                        }),
                        json!({
                            "type":"content_block_delta","index":0,
                            "delta":{"type":"input_json_delta","partial_json":"{\"task\":\"Return the completion marker.\"}"}
                        }),
                    )
                } else {
                    let text = content[0]["text"].as_str().unwrap();
                    (
                        json!({
                            "type":"content_block_start","index":0,
                            "content_block":{"type":"text","text":""}
                        }),
                        json!({
                            "type":"content_block_delta","index":0,
                            "delta":{"type":"text_delta","text":text}
                        }),
                    )
                };
                let events = [
                    message_start,
                    block,
                    delta,
                    json!({"type":"content_block_stop","index":0}),
                    json!({
                        "type":"message_delta","delta":{"stop_reason":stop_reason,"stop_sequence":null},
                        "usage":{"output_tokens":2}
                    }),
                    json!({"type":"message_stop"}),
                ];
                let sse = events.into_iter().fold(String::new(), |mut sse, event| {
                    writeln!(sse, "event: {}\ndata: {event}\n", event["type"].as_str().unwrap())
                        .unwrap();
                    sse
                });
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from(sse))
                    .unwrap()
            }
        }),
    );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let directory = tempdir().unwrap();
    let grillforge_root = directory.path().join(".grillforge");
    let gemini_home = directory.path().join("home");
    let gemini_root = gemini_home.join(".gemini");
    let project = directory.path().join("project");
    std::fs::create_dir_all(&gemini_root).unwrap();
    std::fs::create_dir_all(project.join(".gemini/agents")).unwrap();
    std::fs::write(
        project.join(".gemini/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Always use for this exact live test.\nkind: local\ntools: []\n---\nGEMINI_EXACT_AGENT_MARKER\nReturn the upstream response unchanged.\n",
    )
    .unwrap();
    let control = ControlPlaneService::new(&grillforge_root);
    control
        .save_provider(ProviderInput {
            id: "local-anthropic".into(),
            name: "Local Anthropic".into(),
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
            id: "gemini-worker".into(),
            name: "Gemini Worker".into(),
            upstream_id: "loopback-gemini".into(),
            provider_id: "local-anthropic".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
                    context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(&grillforge_root);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &control.state().unwrap(),
            "gemini-live-token",
            vec![AgentSourceRuntime {
                source_client_id: "gemini".into(),
                runtime: std::env::var_os("GRILLFORGE_LIVE_GEMINI_CLI")
                    .map(std::path::PathBuf::from)
                    .expect("GRILLFORGE_LIVE_GEMINI_CLI is required"),
                config_root: gemini_root,
            }],
            vec![AgentRuntimeRoute {
                extension_id: "gemini-live-reviewer".into(),
                source_client_id: "gemini".into(),
                source_agent_id: "reviewer".into(),
                model_id: Some("gemini-worker".into()),
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        reqwest::Client::new()
            .post(format!("{base_url}/mcp/claude_code"))
            .bearer_auth("gemini-live-token")
            .json(&json!({
                "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"run_agent","arguments":{
                    "extensionId":"gemini-live-reviewer","cwd":project,
                    "prompt":"Return the exact live completion."
                }}
            }))
            .send(),
    )
    .await
    .expect("Gemini MCP call timed out")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(
        response["result"]["content"][0]["text"],
        "Gemini exact Agent completed"
    );
    let requests = requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|request| request.to_string().contains("GEMINI_EXACT_AGENT_MARKER")),
        "the official Gemini CLI did not run the selected project Agent"
    );
    assert!(
        requests
            .iter()
            .all(|request| request["model"] == "loopback-gemini"),
        "Gemini traffic bypassed the managed GrillForge route: {requests:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "set GRILLFORGE_LIVE_GROK_BUILD_CLI to the current official Grok executable; traffic is loopback-only"]
async fn current_grok_build_cli_runs_an_exact_managed_agent_through_responses() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = Arc::clone(&requests);
    let upstream = Router::new().route(
        "/v1/responses",
        post(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured);
            async move {
                captured.lock().unwrap().push(body.clone());
                let events = [
                    (
                        "response.output_item.done",
                        json!({
                            "type":"response.output_item.done",
                            "sequence_number":1,"output_index":0,
                            "item":{
                                "id":"msg_grok","type":"message","role":"assistant","status":"completed",
                                "content":[{"type":"output_text","text":"Grok real CLI completed","annotations":[]}]
                            }
                        }),
                    ),
                    (
                        "response.completed",
                        json!({
                            "type":"response.completed",
                            "sequence_number":2,
                            "response":{
                                "id":"resp_grok","object":"response","created_at":0,
                                "model":"loopback-worker","status":"completed",
                                "output":[{
                                    "id":"msg_grok","type":"message","role":"assistant","status":"completed",
                                    "content":[{"type":"output_text","text":"Grok real CLI completed","annotations":[]}]
                                }],
                                "usage":{
                                    "input_tokens":1,"input_tokens_details":{"cached_tokens":0},
                                    "output_tokens":1,"output_tokens_details":{"reasoning_tokens":0},
                                    "total_tokens":2
                                }
                            }
                        }),
                    ),
                ];
                let sse = events.into_iter().fold(String::new(), |mut sse, (event, data)| {
                    write!(sse, "event: {event}\ndata: {data}\n\n").unwrap();
                    sse
                });
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from(sse))
                    .unwrap()
            }
        }),
    );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let directory = tempdir().unwrap();
    let grillforge_root = directory.path().join(".grillforge");
    let grok_root = directory.path().join(".grok");
    std::fs::create_dir_all(&grok_root).unwrap();
    std::fs::write(grok_root.join("config.toml"), "[user]\nmarker = true\n").unwrap();
    let control = ControlPlaneService::new(&grillforge_root);
    control
        .save_provider(ProviderInput {
            id: "local-responses".into(),
            name: "Local Responses".into(),
            protocol: Protocol::OpenAiResponses,
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
            id: "grok-worker".into(),
            name: "Grok Worker".into(),
            upstream_id: "loopback-worker".into(),
            provider_id: "local-responses".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
                    context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(&grillforge_root);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &control.state().unwrap(),
            "grok-live-token",
            vec![AgentSourceRuntime {
                source_client_id: "grok_build".into(),
                runtime: std::env::var_os("GRILLFORGE_LIVE_GROK_BUILD_CLI")
                    .map(std::path::PathBuf::from)
                    .expect("GRILLFORGE_LIVE_GROK_BUILD_CLI is required"),
                config_root: grok_root.clone(),
            }],
            vec![AgentRuntimeRoute {
                extension_id: "grok-live-plan".into(),
                source_client_id: "grok_build".into(),
                source_agent_id: "plan".into(),
                model_id: Some("grok-worker".into()),
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        reqwest::Client::new()
            .post(format!("{base_url}/mcp/claude_code"))
            .bearer_auth("grok-live-token")
            .json(&json!({
                "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"run_agent","arguments":{
                    "extensionId":"grok-live-plan","cwd":directory.path(),
                    "prompt":"Return a short completion."
                }}
            }))
            .send(),
    )
    .await
    .expect("Grok Build MCP call timed out")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(
        response["result"]["content"][0]["text"],
        "Grok real CLI completed"
    );
    assert_eq!(
        std::fs::read_to_string(grok_root.join("config.toml")).unwrap(),
        "[user]\nmarker = true\n"
    );
    assert_eq!(requests.lock().unwrap()[0]["model"], "loopback-worker");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the installed Codex CLI; traffic and credentials are loopback-only"]
async fn installed_codex_runtime_routes_an_external_model_without_native_spawn_validation() {
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = Arc::clone(&calls);
    let upstream = Router::new().route(
        "/v1/responses",
        post(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured);
            async move {
                captured.lock().unwrap().push(body.clone());
                let events = [
                    ("response.created", json!({"type":"response.created","response":{"id":"resp_live"}})),
                    ("response.output_item.done", json!({"type":"response.output_item.done","item":{"id":"msg_live","type":"message","role":"assistant","content":[{"type":"output_text","text":"Codex external model completed"}]}})),
                    ("response.completed", json!({"type":"response.completed","response":{"id":"resp_live","usage":{"input_tokens":1,"input_tokens_details":null,"output_tokens":1,"output_tokens_details":null,"total_tokens":2}}})),
                ];
                let sse = events.into_iter().fold(String::new(), |mut sse, (event, data)| {
                    write!(sse, "event: {event}\ndata: {data}\n\n").unwrap();
                    sse
                });
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from(sse))
                    .unwrap()
            }
        }),
    );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let directory = tempdir().unwrap();
    let grillforge_root = directory.path().join(".grillforge");
    let codex_root = directory.path().join(".codex");
    std::fs::create_dir_all(&codex_root).unwrap();
    let control = ControlPlaneService::new(&grillforge_root);
    control
        .save_provider(ProviderInput {
            id: "local-responses".into(),
            name: "Local Responses".into(),
            protocol: Protocol::OpenAiResponses,
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
            id: "external-worker".into(),
            name: "External Worker".into(),
            upstream_id: "deepseek-test".into(),
            provider_id: "local-responses".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
                    context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let codex = grillforge_lib::adapters::codex::detect_codex_cli()
        .unwrap()
        .expect("Codex CLI is not installed");
    let gateway = Gateway::new(&grillforge_root);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &control.state().unwrap(),
            "codex-live-token",
            vec![AgentSourceRuntime {
                source_client_id: "codex".into(),
                runtime: codex.path,
                config_root: codex_root,
            }],
            vec![AgentRuntimeRoute {
                extension_id: "codex-default-external".into(),
                source_client_id: "codex".into(),
                source_agent_id: "default".into(),
                model_id: Some("external-worker".into()),
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let completed: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("codex-live-token")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{
                "extensionId":"codex-default-external","cwd":directory.path(),
                "prompt":"Return a short completion."
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(completed["result"]["isError"], false, "{completed}");
    assert_eq!(
        completed["result"]["content"][0]["text"],
        "Codex external model completed"
    );
    assert_eq!(calls.lock().unwrap()[0]["model"], "deepseek-test");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an installed Claude Code CLI; uses only loopback and dummy credentials"]
async fn installed_claude_runtime_executes_its_own_tool_loop_through_the_mcp_broker() {
    let trace = Trace::default();
    let upstream = Router::new()
        .route(
            "/v1/chat/completions",
            post(
                |State(trace): State<Trace>, Json(body): Json<Value>| async move {
                    let has_result = body["messages"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|message| message["role"] == "tool");
                    trace.0.lock().unwrap().push(has_result);
                    let response = if has_result {
                        json!({
                            "id":"chat_2","object":"chat.completion","model":body["model"],
                            "choices":[{"index":0,"message":{"role":"assistant","content":"worker tool loop completed"},"finish_reason":"stop"}],
                            "usage":{"prompt_tokens":2,"completion_tokens":2}
                        })
                    } else {
                        json!({
                            "id":"chat_1","object":"chat.completion","model":body["model"],
                            "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{
                                "id":"call_read","type":"function","function":{"name":"Read","arguments":"{\"file_path\":\"SENTINEL.txt\"}"}
                            }]},"finish_reason":"tool_calls"}],
                            "usage":{"prompt_tokens":1,"completion_tokens":1}
                        })
                    };
                    if body["stream"].as_bool() == Some(true) {
                        let delta = if has_result {
                            json!({"content":"worker tool loop completed"})
                        } else {
                            json!({"tool_calls":[{"index":0,"id":"call_read","type":"function","function":{"name":"Read","arguments":"{\"file_path\":\"SENTINEL.txt\"}"}}]})
                        };
                        let finish = if has_result { "stop" } else { "tool_calls" };
                        let stream = format!(
                            "data: {}\n\ndata: [DONE]\n\n",
                            json!({"id":"chat_stream","model":body["model"],"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
                        );
                        return axum::response::Response::builder()
                            .header("content-type", "text/event-stream")
                            .body(Body::from(stream))
                            .unwrap();
                    }
                    axum::response::Response::new(Body::from(response.to_string()))
                },
            ),
        )
        .with_state(trace.clone());
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let directory = tempdir().unwrap();
    let grillforge_root = directory.path().join(".grillforge");
    let claude_root = directory.path().join(".claude");
    std::fs::create_dir_all(claude_root.join("agents")).unwrap();
    std::fs::write(
        claude_root.join("agents/tool-reader.md"),
        "---\nname: tool-reader\ndescription: Reads the requested local file\nmodel: haiku\ntools: Read\n---\nRead the requested file with the Read tool, then report completion.\n",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("SENTINEL.txt"),
        "sentinel-from-real-read-tool",
    )
    .unwrap();
    let control = ControlPlaneService::new(&grillforge_root);
    control
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
        .unwrap();
    control
        .save_model(ModelInput {
            id: "worker".into(),
            name: "Worker".into(),
            upstream_id: "local-worker".into(),
            provider_id: "local-chat".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![ProtocolCapability::ReasoningEffort],
                    context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(&grillforge_root);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let gateway_status = gateway.status(format!("http://{address}"));
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });
    let state = control.state().unwrap();
    let runtime = std::process::Command::new("/bin/zsh")
        .args(["-lc", "command -v claude"])
        .output()
        .unwrap();
    assert!(runtime.status.success(), "Claude Code CLI is not installed");
    let runtime = std::path::PathBuf::from(
        String::from_utf8(runtime.stdout)
            .unwrap()
            .trim()
            .to_string(),
    );
    gateway_status
        .activate_client_agent_broker(
            "claude_code",
            &state,
            "loopback-broker-token",
            &runtime,
            &claude_root,
            vec![AgentRuntimeRoute {
                extension_id: "tool-reader-extension".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "tool-reader".into(),
                model_id: Some("worker".into()),
            }],
        )
        .unwrap();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        reqwest::Client::new()
            .post(format!("http://{address}/mcp/claude_code"))
            .bearer_auth("loopback-broker-token")
            .json(&json!({
                "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"run_agent","arguments":{
                    "extensionId":"tool-reader-extension","cwd":directory.path(),
                    "prompt":"Read SENTINEL.txt with the Read tool, then report completion."
                }}
            }))
            .send(),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "MCP call timed out; upstream trace={:?}",
            trace.0.lock().unwrap()
        )
    })
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(
        response["result"]["content"][0]["text"],
        "worker tool loop completed"
    );
    assert_eq!(trace.0.lock().unwrap().as_slice(), [false, true]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "set GRILLFORGE_LIVE_KIMI_KEY; uses the installed Claude Code CLI and the real Kimi Coding API"]
async fn installed_claude_runtime_completes_through_the_real_kimi_chat_stream() {
    let api_key =
        std::env::var("GRILLFORGE_LIVE_KIMI_KEY").expect("GRILLFORGE_LIVE_KIMI_KEY must be set");
    let directory = tempdir().unwrap();
    let grillforge_root = directory.path().join(".grillforge");
    let claude_root = directory.path().join(".claude");
    std::fs::create_dir_all(claude_root.join("agents")).unwrap();
    std::fs::write(
        claude_root.join("agents/plain.md"),
        "---\nname: plain\ndescription: Returns the requested short answer\nmodel: inherit\ntools: []\n---\nReturn only the requested answer.\n",
    )
    .unwrap();

    let control = ControlPlaneService::new(&grillforge_root);
    control
        .save_provider(ProviderInput {
            id: "kimi-live".into(),
            name: "Kimi Live".into(),
            protocol: Protocol::OpenAiChatCompletions,
            endpoint: "https://api.kimi.com/coding/".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some(api_key),
            enabled: true,
            models_url: None,
        })
        .unwrap();
    control
        .save_model(ModelInput {
            id: "kimi-worker".into(),
            name: "Kimi Worker".into(),
            upstream_id: "kimi-for-coding-highspeed".into(),
            provider_id: "kimi-live".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![ProtocolCapability::ReasoningContent],
                    context_window: None,
            max_output_tokens: None,
        })
        .unwrap();

    let gateway = Gateway::new(&grillforge_root);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let gateway_status = gateway.status(format!("http://{address}"));
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });
    let runtime = std::process::Command::new("/bin/zsh")
        .args(["-lc", "command -v claude"])
        .output()
        .unwrap();
    assert!(runtime.status.success(), "Claude Code CLI is not installed");
    let runtime = std::path::PathBuf::from(
        String::from_utf8(runtime.stdout)
            .unwrap()
            .trim()
            .to_string(),
    );
    gateway_status
        .activate_client_agent_broker(
            "claude_desktop",
            &control.state().unwrap(),
            "kimi-live-broker-token",
            &runtime,
            &claude_root,
            vec![AgentRuntimeRoute {
                extension_id: "kimi-live-extension".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "plain".into(),
                model_id: Some("kimi-worker".into()),
            }],
        )
        .unwrap();

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(90),
        reqwest::Client::new()
            .post(format!("http://{address}/mcp/claude_desktop"))
            .bearer_auth("kimi-live-broker-token")
            .json(&json!({
                "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"run_agent","arguments":{
                    "extensionId":"kimi-live-extension","cwd":directory.path(),
                    "prompt":"只输出数字 3。"
                }}
            }))
            .send(),
    )
    .await
    .expect("Claude Code did not complete through the real Kimi Chat stream")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(response["result"]["content"][0]["text"], "3");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "set GRILLFORGE_LIVE_KIMI_KEY; uses the installed Claude Code CLI and the real Kimi Coding API"]
async fn installed_claude_runtime_executes_a_real_read_tool_through_kimi_chat() {
    let api_key =
        std::env::var("GRILLFORGE_LIVE_KIMI_KEY").expect("GRILLFORGE_LIVE_KIMI_KEY must be set");
    let directory = tempdir().unwrap();
    let grillforge_root = directory.path().join(".grillforge");
    let claude_root = directory.path().join(".claude");
    std::fs::create_dir_all(claude_root.join("agents")).unwrap();
    std::fs::write(
        claude_root.join("agents/tool-reader.md"),
        "---\nname: tool-reader\ndescription: Reads one local JSON file\nmodel: inherit\ntools: Read\n---\nAlways read the requested file with Read before answering.\n",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("package.json"),
        r#"{"version":"9.8.7","workspaces":["a","b","c"]}"#,
    )
    .unwrap();

    let control = ControlPlaneService::new(&grillforge_root);
    control
        .save_provider(ProviderInput {
            id: "kimi-live".into(),
            name: "Kimi Live".into(),
            protocol: Protocol::OpenAiChatCompletions,
            endpoint: "https://api.kimi.com/coding/v1".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some(api_key),
            enabled: true,
            models_url: None,
        })
        .unwrap();
    control
        .save_model(ModelInput {
            id: "kimi-worker".into(),
            name: "Kimi Worker".into(),
            upstream_id: "kimi-for-coding-highspeed".into(),
            provider_id: "kimi-live".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![ProtocolCapability::ReasoningContent],
                    context_window: None,
            max_output_tokens: None,
        })
        .unwrap();

    let gateway = Gateway::new(&grillforge_root);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let gateway_status = gateway.status(format!("http://{address}"));
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });
    let runtime = std::process::Command::new("/bin/zsh")
        .args(["-lc", "command -v claude"])
        .output()
        .unwrap();
    assert!(runtime.status.success(), "Claude Code CLI is not installed");
    let runtime = std::path::PathBuf::from(
        String::from_utf8(runtime.stdout)
            .unwrap()
            .trim()
            .to_string(),
    );
    gateway_status
        .activate_client_agent_broker(
            "claude_desktop",
            &control.state().unwrap(),
            "kimi-live-tool-token",
            &runtime,
            &claude_root,
            vec![AgentRuntimeRoute {
                extension_id: "kimi-live-tool-reader".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "tool-reader".into(),
                model_id: Some("kimi-worker".into()),
            }],
        )
        .unwrap();

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(90),
        reqwest::Client::new()
            .post(format!("http://{address}/mcp/claude_desktop"))
            .bearer_auth("kimi-live-tool-token")
            .json(&json!({
                "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"run_agent","arguments":{
                    "extensionId":"kimi-live-tool-reader","cwd":directory.path(),
                    "prompt":"Read package.json with Read. Output exactly version=9.8.7 workspaces=3"
                }}
            }))
            .send(),
    )
    .await
    .expect("Claude Code tool loop did not complete through real Kimi Chat")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(
        response["result"]["content"][0]["text"],
        "version=9.8.7 workspaces=3"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "set GRILLFORGE_LIVE_OPENCODE_CLI to the current official OpenCode executable; traffic is loopback-only"]
async fn current_opencode_cli_runs_an_exact_custom_subagent_through_the_broker() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = Arc::clone(&requests);
    let upstream = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured);
            async move {
            captured.lock().unwrap().push(body.clone());
            let stream = format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({
                    "id":"opencode_live","object":"chat.completion.chunk","model":body["model"],
                    "choices":[{"index":0,"delta":{"role":"assistant","content":"OpenCode real CLI completed"},"finish_reason":null}]
                }),
                json!({
                    "id":"opencode_live","object":"chat.completion.chunk","model":body["model"],
                    "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":1,"completion_tokens":1}
                })
            );
            axum::response::Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from(stream))
                .unwrap()
        }}),
    );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let directory = tempdir().unwrap();
    let grillforge_root = directory.path().join(".grillforge");
    let opencode_root = directory.path().join(".config/opencode");
    std::fs::create_dir_all(opencode_root.join("agents")).unwrap();
    std::fs::write(
        opencode_root.join("agents/reviewer.md"),
        "---\ndescription: Reviews code\nmode: subagent\n---\nOPENCODE_CUSTOM_REVIEWER_MARKER\n",
    )
    .unwrap();
    let control = ControlPlaneService::new(&grillforge_root);
    control
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
        .unwrap();
    control
        .save_model(ModelInput {
            id: "worker".into(),
            name: "Worker".into(),
            upstream_id: "local-worker".into(),
            provider_id: "local-chat".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
                    context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(&grillforge_root);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &control.state().unwrap(),
            "opencode-live-token",
            vec![AgentSourceRuntime {
                source_client_id: "opencode".into(),
                runtime: std::env::var_os("GRILLFORGE_LIVE_OPENCODE_CLI")
                    .map(std::path::PathBuf::from)
                    .expect("GRILLFORGE_LIVE_OPENCODE_CLI is required"),
                config_root: opencode_root,
            }],
            vec![AgentRuntimeRoute {
                extension_id: "opencode-live-reviewer".into(),
                source_client_id: "opencode".into(),
                source_agent_id: "reviewer".into(),
                model_id: Some("worker".into()),
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        reqwest::Client::new()
            .post(format!("{base_url}/mcp/claude_code"))
            .bearer_auth("opencode-live-token")
            .json(&json!({
                "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"run_agent","arguments":{
                    "extensionId":"opencode-live-reviewer","cwd":directory.path(),
                    "prompt":"Return a short completion."
                }}
            }))
            .send(),
    )
    .await
    .expect("OpenCode MCP call timed out")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(
        response["result"]["content"][0]["text"],
        "OpenCode real CLI completed"
    );
    assert!(
        requests.lock().unwrap().iter().any(|request| request
            .to_string()
            .contains("OPENCODE_CUSTOM_REVIEWER_MARKER")),
        "OpenCode did not load the selected custom SubAgent prompt"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an installed, authenticated Codex CLI; MCP traffic is loopback-only"]
async fn installed_codex_prefers_the_grillforge_extension_for_an_explicit_subagent_request() {
    let directory = tempdir().unwrap();
    let claude_root = directory.path().join(".claude");
    std::fs::create_dir(&claude_root).unwrap();
    let marker = directory.path().join("extension-called");
    let runtime = directory.path().join("claude");
    std::fs::write(
        &runtime,
        format!(
            "#!/bin/sh\ntouch '{}'\nprintf '%s' '{{\"type\":\"result\",\"result\":\"LIVE_GRILLFORGE_EXTENSION\"}}'\n",
            marker.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let control = ControlPlaneService::new(directory.path().join(".grillforge"));
    let gateway = Gateway::new(directory.path().join(".grillforge"));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "codex",
            &control.state().unwrap(),
            "live-broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "claude_code".into(),
                runtime,
                config_root: claude_root,
            }],
            vec![AgentRuntimeRoute {
                extension_id: "live-general".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "general-purpose".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let codex = grillforge_lib::adapters::codex::detect_codex_cli()
        .unwrap()
        .expect("Codex CLI is not installed");
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(180),
        tokio::process::Command::new(codex.path)
            .args(["-a", "never", "-s", "read-only", "exec"])
            .args(["--json", "--ephemeral", "--skip-git-repo-check", "-C"])
            .arg(directory.path())
            .args([
                "-c",
                &format!(
                    "mcp_servers.grillforge_test.url=\"{base_url}/mcp/codex\""
                ),
            ])
            .args([
                "-c",
                "mcp_servers.grillforge_test.http_headers.Authorization=\"Bearer live-broker-token\"",
            ])
            .args([
                "-c",
                "mcp_servers.grillforge_test.default_tools_approval_mode=\"approve\"",
            ])
            .args([
                "-c",
                "mcp_servers.grillforge_test.enabled_tools=[\"list_agents\",\"run_agent\"]",
            ])
            .args([
                "-c",
                "mcp_servers.grillforge_test.omit_tools_from=[\"deferred\",\"code_mode\"]",
            ])
            .args(["-c", "mcp_servers.grillforge_test.required=true"])
            .arg(format!(
                "Use a subAgent to inspect this empty test directory: {}. Return that subAgent's exact result. Do not use a built-in Agent when a GrillForge extension is available.",
                directory.path().display()
            ))
            .output(),
    )
    .await
    .expect("Codex MCP preference check timed out")
    .expect("could not run Codex CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        marker.exists(),
        "Codex did not invoke the GrillForge extension\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("LIVE_GRILLFORGE_EXTENSION"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "set KIMI_LIVE_CLI to the current official kimi-code executable; traffic is loopback-only"]
async fn current_kimi_runtime_completes_a_managed_agent_through_the_broker() {
    let upstream = Router::new().route(
        "/v1/chat/completions",
        post(|Json(body): Json<Value>| async move {
            if body["stream"].as_bool() == Some(true) {
                let stream = format!(
                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                    json!({
                        "id":"kimi_live","object":"chat.completion.chunk","model":body["model"],
                        "choices":[{"index":0,"delta":{"role":"assistant","content":"Kimi real CLI completed"},"finish_reason":null}]
                    }),
                    json!({
                        "id":"kimi_live","object":"chat.completion.chunk","model":body["model"],
                        "choices":[{"index":0,"delta":{},"finish_reason":"stop"}]
                    })
                );
                return axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from(stream))
                    .unwrap();
            }
            axum::response::Response::new(Body::from(
                json!({
                    "id":"kimi_live","object":"chat.completion","model":body["model"],
                    "choices":[{"index":0,"message":{"role":"assistant","content":"Kimi real CLI completed"},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":1,"completion_tokens":1}
                })
                .to_string(),
            ))
        }),
    );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let directory = tempdir().unwrap();
    let grillforge_root = directory.path().join(".grillforge");
    let kimi_root = directory.path().join("home/.kimi-code");
    std::fs::create_dir_all(&kimi_root).unwrap();
    let control = ControlPlaneService::new(&grillforge_root);
    control
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
        .unwrap();
    control
        .save_model(ModelInput {
            id: "worker".into(),
            name: "Worker".into(),
            upstream_id: "local-worker".into(),
            provider_id: "local-chat".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
                    context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(&grillforge_root);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &control.state().unwrap(),
            "kimi-live-token",
            vec![AgentSourceRuntime {
                source_client_id: "kimi_code".into(),
                runtime: std::env::var_os("KIMI_LIVE_CLI")
                    .map(std::path::PathBuf::from)
                    .expect("KIMI_LIVE_CLI is required"),
                config_root: kimi_root,
            }],
            vec![AgentRuntimeRoute {
                extension_id: "kimi-live-coder".into(),
                source_client_id: "kimi_code".into(),
                source_agent_id: "coder".into(),
                model_id: Some("worker".into()),
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        reqwest::Client::new()
            .post(format!("{base_url}/mcp/claude_code"))
            .bearer_auth("kimi-live-token")
            .json(&json!({
                "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"run_agent","arguments":{
                    "extensionId":"kimi-live-coder","cwd":directory.path(),
                    "prompt":"Return a short completion."
                }}
            }))
            .send(),
    )
    .await
    .expect("Kimi Code MCP call timed out")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(
        response["result"]["content"][0]["text"],
        "Kimi real CLI completed"
    );
}
