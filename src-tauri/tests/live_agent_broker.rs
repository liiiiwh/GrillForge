use axum::{Json, Router, body::Body, extract::State, http::StatusCode, routing::post};
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::model::ProtocolCapability;
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::{AgentRuntimeRoute, Gateway};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tokio::net::TcpListener;

#[derive(Clone, Default)]
struct Trace(Arc<Mutex<Vec<bool>>>);

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
