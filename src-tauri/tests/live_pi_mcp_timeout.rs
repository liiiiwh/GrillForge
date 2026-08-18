use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::Response,
    routing::post,
};
use grillforge_lib::adapters::pi::detect_pi_cli;
use grillforge_lib::mcp_mount::{McpClientFormat, McpMountManager, McpMountTarget};
use serde_json::{Value, json};
use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

const PI_MCP_TOOL: &str = "mcp_grillforge_pi_slow_tool";

#[derive(Clone, Default)]
struct McpCalls(Arc<Mutex<usize>>);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an installed Pi CLI and pi-mcp-extension; all traffic is loopback-only"]
async fn installed_pi_mcp_extension_allows_a_tool_call_to_run_longer_than_thirty_seconds() {
    let pi_cli = env::var_os("GRILLFORGE_LIVE_PI_CLI")
        .map(PathBuf::from)
        .or_else(|| detect_pi_cli().ok().flatten().map(|detected| detected.path))
        .expect("install Pi or set GRILLFORGE_LIVE_PI_CLI to its executable");
    let extension = env::var_os("GRILLFORGE_LIVE_PI_MCP_EXTENSION")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("home directory")
                .join(".pi/agent/npm/node_modules/pi-mcp-extension/src/index.ts")
        });
    assert!(
        extension.is_file(),
        "install pi-mcp-extension or set GRILLFORGE_LIVE_PI_MCP_EXTENSION to src/index.ts: {}",
        extension.display()
    );

    let mcp_calls = McpCalls::default();
    let mcp = Router::new()
        .route("/mcp/pi", post(mcp_request))
        .with_state(mcp_calls.clone());
    let mcp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mcp_address = mcp_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(mcp_listener, mcp).await.unwrap() });

    let model_requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured_requests = Arc::clone(&model_requests);
    let upstream = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let captured_requests = Arc::clone(&captured_requests);
            async move {
                captured_requests.lock().unwrap().push(body.clone());
                let has_tool_result = body.to_string().contains("slow-tool-finished");
                let sse = if has_tool_result {
                    concat!(
                        "data: {\"id\":\"chatcmpl_final\",\"object\":\"chat.completion.chunk\",\"model\":\"local\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"pi-mcp-finished\"},\"finish_reason\":null}]}\n\n",
                        "data: {\"id\":\"chatcmpl_final\",\"object\":\"chat.completion.chunk\",\"model\":\"local\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                        "data: [DONE]\n\n"
                    )
                } else {
                    concat!(
                        "data: {\"id\":\"chatcmpl_tool\",\"object\":\"chat.completion.chunk\",\"model\":\"local\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_slow\",\"type\":\"function\",\"function\":{\"name\":\"mcp_grillforge_pi_slow_tool\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\n",
                        "data: {\"id\":\"chatcmpl_tool\",\"object\":\"chat.completion.chunk\",\"model\":\"local\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
                        "data: [DONE]\n\n"
                    )
                };
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(sse))
                    .unwrap()
            }
        }),
    );
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let temp = tempfile::tempdir().unwrap();
    let pi_root = temp.path().join("pi-agent");
    let child_home = temp.path().join("home");
    let mcp_config = child_home.join(".pi/agent/mcp.json");
    std::fs::create_dir_all(mcp_config.parent().unwrap()).unwrap();

    std::fs::create_dir_all(&pi_root).unwrap();
    std::fs::write(
        pi_root.join("models.json"),
        serde_json::to_vec_pretty(&json!({
            "providers": {
                "local": {
                    "baseUrl": format!("http://{upstream_address}/v1"),
                    "api": "openai-completions",
                    "apiKey": "loopback-only",
                    "models": [{
                        "id": "local",
                        "name": "Local",
                        "reasoning": false,
                        "input": ["text"],
                        "contextWindow": 8192,
                        "maxTokens": 1024,
                        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
                    }]
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        pi_root.join("settings.json"),
        serde_json::to_vec_pretty(&json!({
            "defaultProvider": "local",
            "defaultModel": "local",
            "enabledModels": ["local"]
        }))
        .unwrap(),
    )
    .unwrap();

    McpMountManager::new(
        temp.path().join("snapshots"),
        [McpMountTarget::new(
            "pi",
            &mcp_config,
            McpClientFormat::PiExtensionJson,
        )],
    )
    .unwrap()
    .mount("pi", &format!("http://{mcp_address}/mcp/pi"), "local-token")
    .unwrap();
    let mounted: Value = serde_json::from_slice(&std::fs::read(&mcp_config).unwrap()).unwrap();
    assert_eq!(mounted["settings"]["requestTimeoutMs"], 10_800_000);

    let started = Instant::now();
    let output = tokio::time::timeout(
        Duration::from_secs(55),
        tokio::task::spawn_blocking(move || {
            Command::new(pi_cli)
                .args(["--print", "--no-session", "--no-extensions", "--extension"])
                .arg(extension)
                .args([
                    "--no-builtin-tools",
                    "--no-skills",
                    "--no-context-files",
                    "--tools",
                    PI_MCP_TOOL,
                    "--offline",
                    "Call the available MCP tool and return its result.",
                ])
                .env("HOME", child_home)
                .env("PI_CODING_AGENT_DIR", pi_root)
                .stdin(Stdio::null())
                .output()
                .expect("run installed Pi CLI")
        }),
    )
    .await
    .expect("Pi CLI did not finish within 55 seconds")
    .unwrap();

    assert!(
        output.status.success(),
        "Pi CLI failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() > Duration::from_secs(30),
        "the MCP tool did not cross the former 30 second timeout"
    );
    assert_eq!(*mcp_calls.0.lock().unwrap(), 1);
    assert!(String::from_utf8_lossy(&output.stdout).contains("pi-mcp-finished"));
    assert_eq!(model_requests.lock().unwrap().len(), 2);
}

async fn mcp_request(State(calls): State<McpCalls>, Json(request): Json<Value>) -> Response<Body> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let result = match request.get("method").and_then(Value::as_str) {
        Some("initialize") => json!({
            "protocolVersion": request
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-03-26"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "slow-loopback", "version": "1"}
        }),
        Some("tools/list") => json!({
            "tools": [{
                "name": "slow_tool",
                "description": "Returns after the former Pi timeout boundary",
                "inputSchema": {"type": "object", "properties": {}}
            }]
        }),
        Some("tools/call") => {
            tokio::time::sleep(Duration::from_secs(31)).await;
            *calls.0.lock().unwrap() += 1;
            json!({"content": [{"type": "text", "text": "slow-tool-finished"}]})
        }
        Some("notifications/initialized") => {
            return Response::builder()
                .status(StatusCode::ACCEPTED)
                .body(Body::empty())
                .unwrap();
        }
        Some(method) => {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "jsonrpc":"2.0",
                        "id":id,
                        "error":{"code":-32601,"message":format!("unknown method: {method}")}
                    })
                    .to_string(),
                ))
                .unwrap();
        }
        None => json!({}),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"jsonrpc":"2.0","id":id,"result":result}).to_string(),
        ))
        .unwrap()
}
