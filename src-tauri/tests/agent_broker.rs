#![cfg(unix)]

use axum::http::StatusCode;
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::{AgentRuntimeRoute, AgentSourceRuntime, Gateway};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tokio::net::TcpListener;

/// A completed run reports {runId, status, result}; tests assert the result.
fn agent_result(response: &Value) -> String {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("agent payload");
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|payload| payload["result"].as_str().map(str::to_string))
        .unwrap_or_else(|| text.to_string())
}

#[tokio::test]
async fn a_running_agent_reports_progress_without_leaking_the_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = directory.path().join("claude");
    fs::write(
        &runtime,
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"checked one file\"}]}}'\nsleep 0.1\nprintf '%s\\n' '{\"type\":\"result\",\"result\":\"final-only-result\"}'\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();

    let service = ControlPlaneService::new(directory.path());
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    gateway
        .status(format!("http://{address}"))
        .activate_client_agent_broker(
            "codex",
            &service.state().unwrap(),
            "progress-token",
            &runtime,
            directory.path(),
            vec![AgentRuntimeRoute {
                extension_id: "reviewer".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "general-purpose".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let call = |id: i32, name: &str, arguments: Value| {
        reqwest::Client::new()
            .post(format!("http://{address}/mcp/codex"))
            .bearer_auth("progress-token")
            .json(&json!({
                "jsonrpc":"2.0","id":id,"method":"tools/call",
                "params":{"name":name,"arguments":arguments}
            }))
            .send()
    };

    // Starting returns a handle at once, so the caller keeps its turn.
    let started: Value = call(
        7,
        "run_agent",
        json!({
            "extensionId":"reviewer",
            "cwd":directory.path(),
            "prompt":"SECRET PROMPT MUST NOT ENTER PROGRESS"
        }),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let handle: Value =
        serde_json::from_str(started["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(handle["status"], "running");
    // The handle says what is still owed, at the point where the caller decides
    // whether its turn is finished.
    assert!(
        handle["next"]
            .as_str()
            .expect("collect obligation")
            .contains("call get_agent_result with this runId")
    );
    let run_id = handle["runId"].as_str().unwrap().to_string();

    // Collecting waits only as long as it was asked to and returns the one result.
    let collected: Value = call(
        8,
        "get_agent_result",
        json!({"runId":run_id,"waitSeconds":120}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(collected["result"]["isError"], false, "{collected}");
    assert_eq!(agent_result(&collected), "final-only-result");

    // The prompt never travels back as progress or result.
    assert!(
        !collected.to_string().contains("SECRET PROMPT"),
        "{collected}"
    );

    // A collected run is gone; collecting twice is an error, not a second result.
    let again: Value = call(9, "get_agent_result", json!({"runId":run_id}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(again["result"]["isError"], true, "{again}");
}

#[tokio::test]
async fn client_scoped_mcp_broker_resolves_the_extension_and_launches_child_only_routing() {
    let directory = tempfile::tempdir().expect("temporary directory");
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
            id: "worker".into(),
            name: "Worker".into(),
            upstream_id: "worker-upstream".into(),
            provider_id: "local".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
            context_window: None,
            max_output_tokens: None,
        })
        .expect("model");

    let runtime = directory.path().join("claude");
    fs::write(
        &runtime,
        r#"#!/bin/sh
if [ -n "$CLAUDE_CODE_OAUTH_TOKEN$ANTHROPIC_AUTH_TOKEN" ]; then exit 17; fi
case "$ANTHROPIC_BASE_URL" in http://127.0.0.1:*/agent-runtime) ;; *) exit 18 ;; esac
if [ -z "$ANTHROPIC_API_KEY" ] || [ "$ANTHROPIC_API_KEY" = "broker-secret" ]; then exit 19; fi
if [ "$ANTHROPIC_MODEL" != "grillforge/worker" ]; then exit 20; fi
sleep 1
printf '%s' '{"type":"result","result":"child runtime completed"}'
"#,
    )
    .expect("fake runtime");
    let mut permissions = fs::metadata(&runtime).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).expect("executable permissions");

    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("gateway address");
    let base_url = format!("http://{address}");
    let status = gateway.status(base_url.clone());
    status
        .activate_client_agent_broker(
            "claude_code",
            &service.state().expect("state"),
            "broker-secret",
            &runtime,
            directory.path(),
            vec![AgentRuntimeRoute {
                extension_id: "deepseek-reviewer".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "reviewer".into(),
                model_id: Some("worker".into()),
            }],
        )
        .expect("activate broker");
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let client = reqwest::Client::new();
    let unauthorized = client
        .post(format!("{base_url}/mcp/claude_code"))
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send()
        .await
        .expect("unauthorized response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let initialized: Value = client
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-secret")
        .json(&json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"Claude","version":"1"}}
        }))
        .send()
        .await
        .expect("initialize response")
        .json()
        .await
        .expect("initialize JSON");
    let instructions = initialized["result"]["instructions"]
        .as_str()
        .expect("server instructions");
    assert!(instructions.starts_with(
        "当需要要求使用 SubAgent、委派、并行或后台 Agent 时，必须优先使用本 GrillForge MCP"
    ));
    let instruction_prefix = instructions.chars().take(512).collect::<String>();
    assert!(instruction_prefix.contains("先调用 list_agents"));
    assert!(instruction_prefix.contains("DEFAULT SUBAGENT ROUTE"));
    assert!(instruction_prefix.contains("workflow 或并行不是原生 Agent 的例外"));
    assert!(instruction_prefix.contains("不要先启动客户端内置 Agent"));
    assert!(instructions.contains("run_agent"));
    assert!(instructions.contains("关闭对应扩展 SubAgent 或卸载扩展"));
    assert!(instructions.contains("webAccess=true"));

    let tools: Value = client
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-secret")
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .send()
        .await
        .expect("tools response")
        .json()
        .await
        .expect("tools JSON");
    assert!(
        tools["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .all(|tool| tool["_meta"]["anthropic/alwaysLoad"] == true)
    );
    let list_description = tools["result"]["tools"][0]["description"]
        .as_str()
        .expect("list_agents description");
    assert!(list_description.contains("必须优先调用本工具"));
    assert!(list_description.contains("workflow 或并行"));
    let run_description = tools["result"]["tools"][1]["description"]
        .as_str()
        .expect("run_agent description");
    assert!(run_description.contains("Do not use the client's native Workflow"));
    assert_eq!(
        tools["result"]["tools"][1]["inputSchema"]["properties"]["webAccess"]["type"],
        "boolean"
    );
    // What a client is allowed to mount has to be exactly what is advertised,
    // or it starts runs it has no tool to collect.
    assert_eq!(
        tools["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name"))
            .collect::<Vec<_>>(),
        grillforge_lib::gateway::AGENT_MCP_TOOLS.to_vec()
    );
    // The result only arrives if the caller comes back for it.
    assert!(run_description.contains("The result reaches you only through get_agent_result"));

    let listed: Value = client
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-secret")
        .json(&json!({
            "jsonrpc":"2.0","id":22,"method":"tools/call",
            "params":{"name":"list_agents","arguments":{}}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed: Value =
        serde_json::from_str(listed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(listed[0]["webAccessSupported"], true);

    let response = client
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-secret")
        .json(&json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params": {
                "name":"run_agent",
                "arguments": {"waitSeconds":120,
                    "extensionId":"deepseek-reviewer",
                    "cwd": directory.path(),
                    "prompt":"Inspect the project"
                }
            }
        }))
        .send()
        .await
        .expect("MCP response");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("MCP JSON");
    assert_eq!(body["result"]["isError"], false);
    assert_eq!(agent_result(&body), "child runtime completed");
}

#[tokio::test]
async fn workflow_can_run_independent_extension_agents_concurrently() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = directory.path().join("claude");
    let starts = directory.path().join("starts");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
printf 'started\n' >> '{}'
i=0
while [ $i -lt 300 ]; do
  i=$((i+1))
  [ "$(wc -l < '{}')" -ge 2 ] && break
  sleep 0.1
done
[ "$(wc -l < '{}')" -ge 2 ] || exit 31
printf '%s' '{{"type":"result","result":"parallel child completed"}}'
"#,
            starts.display(),
            starts.display(),
            starts.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();

    let service = ControlPlaneService::new(directory.path());
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker(
            "codex",
            &service.state().unwrap(),
            "parallel-token",
            &runtime,
            directory.path(),
            vec![AgentRuntimeRoute {
                extension_id: "parallel-worker".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "general-purpose".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let call = |id| {
        reqwest::Client::new()
            .post(format!("{base_url}/mcp/codex"))
            .bearer_auth("parallel-token")
            .json(&json!({
                "jsonrpc":"2.0","id":id,"method":"tools/call",
                "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                    "extensionId":"parallel-worker","cwd":directory.path(),"prompt":"Inspect"
                }}
            }))
            .send()
    };
    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(45), async {
        tokio::join!(call(1), call(2))
    })
    .await
    .expect("parallel calls did not overlap");
    for response in [first.unwrap(), second.unwrap()] {
        let response: Value = response.json().await.unwrap();
        assert_eq!(response["result"]["isError"], false, "{response}");
        assert_eq!(agent_result(&response), "parallel child completed");
    }
}

#[tokio::test]
async fn claude_extension_enables_native_web_tools_only_for_an_explicit_web_request() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = directory.path().join("claude");
    let argv_log = directory.path().join("argv");
    fs::create_dir(directory.path().join(".claude")).unwrap();
    fs::write(
        &runtime,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s' '{{\"type\":\"result\",\"result\":\"done\"}}'\n",
            argv_log.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();

    let service = ControlPlaneService::new(directory.path());
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "codex",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "claude_code".into(),
                runtime,
                config_root: directory.path().join(".claude"),
            }],
            vec![AgentRuntimeRoute {
                extension_id: "claude-general".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "general-purpose".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/codex"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"claude-general",
                "cwd":directory.path(),
                "prompt":"Inspect a public GitHub repository",
                "webAccess":true
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    let argv = fs::read_to_string(&argv_log).unwrap();
    assert!(
        argv.contains("--allowedTools\nWebSearch,WebFetch\n"),
        "{argv}"
    );

    let response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/codex"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"claude-general",
                "cwd":directory.path(),
                "prompt":"Inspect only local files",
                "webAccess":false
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    let argv = fs::read_to_string(&argv_log).unwrap();
    // The permission mode approves every tool, so a refused call must withhold the
    // web tools explicitly rather than merely leave them ungranted.
    assert!(argv.contains("--permission-mode\nauto\n"), "{argv}");
    assert!(
        argv.contains("--disallowedTools\nWebSearch,WebFetch\n"),
        "{argv}"
    );
    assert!(!argv.contains("--allowedTools"), "{argv}");
    assert!(
        argv.contains(&format!(
            "Working directory: {}",
            directory.path().display()
        )),
        "{argv}"
    );

    let invalid: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/codex"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"claude-general",
                "cwd":directory.path(),
                "prompt":"Inspect public docs",
                "webAccess":"yes"
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(invalid["result"]["isError"], true);
    assert_eq!(
        agent_result(&invalid),
        "run_agent webAccess must be a boolean"
    );
}

#[tokio::test]
async fn agent_runtime_endpoint_fails_closed_without_its_broker_token() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("gateway address");
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response = reqwest::Client::new()
        .post(format!("http://{address}/agent-runtime/v1/messages"))
        .json(&json!({
            "model":"grillforge/worker",
            "max_tokens":8,
            "messages":[{"role":"user","content":"ping"}]
        }))
        .send()
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("Agent broker gateway authorization failed")
    );
}

#[tokio::test]
async fn native_extension_uses_the_local_runtime_configuration_without_model_injection() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = directory.path().join("claude");
    fs::write(
        &runtime,
        r#"#!/bin/sh
if [ -n "$CLAUDE_CODE_OAUTH_TOKEN$ANTHROPIC_API_KEY$ANTHROPIC_AUTH_TOKEN$ANTHROPIC_BASE_URL$ANTHROPIC_MODEL" ]; then exit 21; fi
case "$*" in *"--model"*) exit 22 ;; esac
case "$*" in *"--agent local-reviewer"*) ;; *) exit 23 ;; esac
printf '%s' '{"type":"result","result":"native runtime completed"}'
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();

    let service = ControlPlaneService::new(directory.path());
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker(
            "claude_code",
            &service.state().unwrap(),
            "native-token",
            &runtime,
            directory.path(),
            vec![AgentRuntimeRoute {
                extension_id: "native-reviewer".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "local-reviewer".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("native-token")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"native-reviewer",
                "cwd":directory.path(),
                "prompt":"Review"
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(agent_result(&response), "native runtime completed");

    let override_attempt: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("native-token")
        .json(&json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"native-reviewer",
                "runtime":"codex",
                "modelRoute":"grillforge/other",
                "cwd":directory.path(),
                "prompt":"Review"
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(override_attempt["result"]["isError"], true);
    assert!(
        override_attempt["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("does not accept")
    );
}

#[tokio::test]
async fn pi_extension_runs_the_selected_local_agent_with_pi_owned_tools_and_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let pi_root = directory.path().join("pi-home");
    let project = directory.path().join("project");
    fs::create_dir_all(&pi_root).unwrap();
    fs::create_dir_all(project.join(".pi/agents")).unwrap();
    fs::write(
        project.join(".pi/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\ntools: read, grep, find\n---\nPI_AGENT_PRIVATE_PROMPT\n",
    )
    .unwrap();
    let argv_log = directory.path().join("pi-argv.log");
    let prompt_log = directory.path().join("pi-prompt.log");
    let runtime = directory.path().join("pi");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$PI_CODING_AGENT_DIR|$*" > {argv}
previous=''
for argument in "$@"; do
  if [ "$previous" = '--append-system-prompt' ]; then cp "$argument" {prompt}; fi
  previous="$argument"
done
printf '%s\n' '{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":"pi child completed"}}],"stopReason":"stop"}}}}'
"#,
            argv = argv_log.display(),
            prompt = prompt_log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();

    let service = ControlPlaneService::new(directory.path());
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "pi".into(),
                runtime,
                config_root: pi_root.clone(),
            }],
            vec![AgentRuntimeRoute {
                extension_id: "pi-reviewer".into(),
                source_client_id: "pi".into(),
                source_agent_id: "reviewer".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"pi-reviewer","cwd":project,"prompt":"Review this"
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(agent_result(&response), "pi child completed");
    let argv = fs::read_to_string(&argv_log).unwrap();
    assert!(argv.starts_with(&format!("{}|", pi_root.display())));
    assert!(argv.contains("--mode json -p --no-session"));
    assert!(argv.contains("--tools read,grep,find"));
    assert!(argv.contains("Task: Review this"));

    let web_response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"pi-reviewer","cwd":project,"prompt":"Research public docs",
                "webAccess":true
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(web_response["result"]["isError"], false, "{web_response}");

    let refusal: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"pi-reviewer","cwd":project,"prompt":"Stay offline",
                "webAccess":false
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Pi has no switch to withhold the network, so only a refusal can fail.
    assert_eq!(refusal["result"]["isError"], true);
    assert!(
        refusal["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("cannot withhold native web access")
    );
    assert_eq!(
        fs::read_to_string(prompt_log).unwrap().trim(),
        "PI_AGENT_PRIVATE_PROMPT"
    );
}

#[tokio::test]
async fn managed_pi_extension_uses_an_isolated_anthropic_route_without_changing_pi_home() {
    let directory = tempfile::tempdir().unwrap();
    let pi_root = directory.path().join("pi-home");
    let project = directory.path().join("project");
    fs::create_dir_all(pi_root.join("agents")).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(
        pi_root.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\nmodel: native/model\n---\nReviewer prompt\n",
    )
    .unwrap();
    let original_models = pi_root.join("models.json");
    fs::write(&original_models, "{\"native\":true}\n").unwrap();
    let effective_models = directory.path().join("effective-models.json");
    let argv_log = directory.path().join("pi-managed-argv.log");
    let runtime = directory.path().join("pi");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
test "$PI_CODING_AGENT_DIR" != "{pi_root}" || exit 41
cp "$PI_CODING_AGENT_DIR/models.json" {models}
printf '%s\n' "$*" > {argv}
printf '%s\n' '{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":"managed pi completed"}}],"stopReason":"stop"}}}}'
"#,
            pi_root = pi_root.display(),
            models = effective_models.display(),
            argv = argv_log.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();

    let service = ControlPlaneService::new(directory.path());
    service
        .save_provider(ProviderInput {
            id: "local".into(),
            name: "Local".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: "http://127.0.0.1:9".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .unwrap();
    service
        .save_model(ModelInput {
            id: "worker".into(),
            name: "Worker".into(),
            upstream_id: "worker-upstream".into(),
            provider_id: "local".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
            context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "pi".into(),
                runtime,
                config_root: pi_root.clone(),
            }],
            vec![AgentRuntimeRoute {
                extension_id: "pi-reviewer".into(),
                source_client_id: "pi".into(),
                source_agent_id: "reviewer".into(),
                model_id: Some("worker".into()),
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new().post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"run_agent","arguments":{"waitSeconds":120,"extensionId":"pi-reviewer","cwd":project,"prompt":"Review"}}}))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(
        fs::read_to_string(&original_models).unwrap(),
        "{\"native\":true}\n"
    );
    let models: Value =
        serde_json::from_str(&fs::read_to_string(effective_models).unwrap()).unwrap();
    let provider = &models["providers"]["grillforge_agent"];
    assert_eq!(provider["api"], "anthropic-messages");
    assert_eq!(provider["models"][0]["id"], "grillforge/worker");
    assert!(
        provider["baseUrl"]
            .as_str()
            .unwrap()
            .ends_with("/agent-runtime")
    );
    assert_ne!(provider["apiKey"], "broker-token");
    let argv = fs::read_to_string(argv_log).unwrap();
    assert!(argv.contains("--model grillforge_agent/grillforge/worker"));
    assert!(!argv.contains("native/model"));
}

#[tokio::test]
async fn managed_grok_build_extension_selects_the_exact_agent_with_isolated_responses_config() {
    let directory = tempfile::tempdir().unwrap();
    let grok_root = directory.path().join("grok-home");
    let project = directory.path().join("project");
    fs::create_dir_all(&grok_root).unwrap();
    fs::create_dir_all(&project).unwrap();
    let original_config = grok_root.join("config.toml");
    fs::write(&original_config, "[user]\nmarker = true\n").unwrap();
    let effective_config = directory.path().join("effective-grok.toml");
    let argv_log = directory.path().join("grok-argv.log");
    let runtime = directory.path().join("grok");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
if [ "$1" = inspect ]; then
  printf '%s\n' '{{"agents":[{{"name":"plan","description":"Plans changes","source":{{"type":"builtin"}}}}]}}'
  exit 0
fi
test "$GROK_HOME" != "{grok_root}" || exit 51
test -n "$GRILLFORGE_GROK_BUILD_API_KEY" || exit 52
cp "$GROK_HOME/config.toml" {config}
printf '%s\n' "$*" > {argv}
printf '%s\n' '{{"text":"grok child completed","stopReason":"end_turn","num_turns":1}}'
"#,
            grok_root = grok_root.display(),
            config = effective_config.display(),
            argv = argv_log.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();

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
        .unwrap();
    service
        .save_model(ModelInput {
            id: "worker".into(),
            name: "Worker".into(),
            upstream_id: "worker-upstream".into(),
            provider_id: "local".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
            context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "grok_build".into(),
                runtime,
                config_root: grok_root.clone(),
            }],
            vec![AgentRuntimeRoute {
                extension_id: "grok-reviewer".into(),
                source_client_id: "grok_build".into(),
                source_agent_id: "plan".into(),
                model_id: Some("worker".into()),
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new().post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"run_agent","arguments":{"waitSeconds":120,"extensionId":"grok-reviewer","cwd":project,"prompt":"Plan this"}}}))
        .send().await.unwrap().json().await.unwrap();

    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(agent_result(&response), "grok child completed");
    assert_eq!(
        fs::read_to_string(original_config).unwrap(),
        "[user]\nmarker = true\n"
    );
    let config = fs::read_to_string(effective_config).unwrap();
    assert!(config.contains("base_url = \"http://127.0.0.1:"));
    assert!(config.contains("/agent-runtime/v1\""));
    assert!(config.contains("model = \"grillforge/worker\""));
    assert!(config.contains("api_backend = \"responses\""));
    assert!(config.contains("env_key = \"GRILLFORGE_GROK_BUILD_API_KEY\""));
    assert!(!config.contains("broker-token"));
    let argv = fs::read_to_string(argv_log).unwrap();
    assert_eq!(
        argv,
        "--permission-mode auto --agent plan -p Plan this --output-format json --model grillforge\n"
    );
}

#[tokio::test]
async fn codex_extension_uses_the_selected_role_config_and_managed_model_route() {
    let directory = tempfile::tempdir().unwrap();
    let codex_root = directory.path().join("home/.codex");
    fs::create_dir_all(codex_root.join("agents")).unwrap();
    fs::write(
        codex_root.join("agents/reviewer.toml"),
        "name = \"reviewer\"\ndescription = \"Reviews\"\ndeveloper_instructions = \"USER_ROLE_MARKER\"\nmodel = \"native-model\"\n",
    )
    .unwrap();
    let project = directory.path().join("project");
    fs::create_dir_all(project.join(".codex/agents")).unwrap();
    fs::write(
        project.join(".codex/agents/reviewer.toml"),
        "name = \"reviewer\"\ndescription = \"Project reviews\"\ndeveloper_instructions = \"PROJECT_ROLE_MARKER\"\nmodel = \"project-native-model\"\nmodel_provider = \"openai\"\n",
    )
    .unwrap();
    let argv_log = directory.path().join("codex-argv");
    let runtime = directory.path().join("codex");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > {argv}
case "$*" in *"agents.reviewer.config_file="*) exit 31 ;; esac
case "$*" in *"PROJECT_ROLE_MARKER"*"Task:"*"Review this"*) ;; *) exit 33 ;; esac
case "$*" in *"model_providers.grillforge_agent.base_url="*) ;; *) exit 34 ;; esac
test -n "$GRILLFORGE_AGENT_TOKEN" || exit 35
printf '%s\n' '{{"type":"item.completed","item":{{"type":"agent_message","text":"codex child completed"}}}}' '{{"type":"turn.completed"}}'
"#,
            argv = argv_log.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();

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
        .unwrap();
    service
        .save_model(ModelInput {
            id: "worker".into(),
            name: "Worker".into(),
            upstream_id: "worker-upstream".into(),
            provider_id: "local".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
            context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "codex".into(),
                runtime: runtime.clone(),
                config_root: codex_root,
            }],
            vec![
                AgentRuntimeRoute {
                    extension_id: "codex-reviewer".into(),
                    source_client_id: "codex".into(),
                    source_agent_id: "reviewer".into(),
                    model_id: Some("worker".into()),
                },
                AgentRuntimeRoute {
                    extension_id: "codex-worker".into(),
                    source_client_id: "codex".into(),
                    source_agent_id: "worker".into(),
                    model_id: Some("worker".into()),
                },
            ],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"codex-reviewer","cwd":project,"prompt":"Review this",
                "webAccess":true
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(agent_result(&response), "codex child completed");
    let argv = fs::read_to_string(&argv_log).unwrap();
    assert!(
        argv.starts_with("-s\nworkspace-write\n-a\nnever\n--search\nexec\n"),
        "{argv}"
    );
    assert!(!argv.contains("agents.reviewer.config_file="));
    assert!(!argv.contains("agents.default_subagent_model="));
    assert!(argv.contains("model=grillforge/worker"));
    assert!(argv.contains("model_provider=grillforge_agent"));
    assert!(argv.contains("PROJECT_ROLE_MARKER"));
    assert!(!argv.contains("USER_ROLE_MARKER"));
    assert!(!argv.contains("spawn_agent"));

    let unsupported: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"codex-worker","cwd":project,"prompt":"Review this"
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unsupported["result"]["isError"], true, "{unsupported}");
    assert!(
        unsupported["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("validates its native SubAgent model catalog"),
        "{unsupported}"
    );
}

#[tokio::test]
async fn native_codex_custom_agent_keeps_the_exact_spawn_agent_path() {
    let directory = tempfile::tempdir().unwrap();
    let codex_root = directory.path().join(".codex");
    fs::create_dir_all(codex_root.join("agents")).unwrap();
    let agent_file = codex_root.join("agents/reviewer.toml");
    fs::write(
        &agent_file,
        "name = \"reviewer\"\ndescription = \"Native reviewer\"\ndeveloper_instructions = \"Review carefully\"\n",
    )
    .unwrap();
    let argv_log = directory.path().join("codex-native-argv");
    let runtime = directory.path().join("codex");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > {argv}
case "$*" in *"--enable multi_agent"*) ;; *) exit 51 ;; esac
case "$*" in *"agents.reviewer.config_file=\"{agent}\""*) ;; *) exit 52 ;; esac
case "$*" in *"spawn_agent tool exactly once with agent_type reviewer"*) ;; *) exit 53 ;; esac
case "$*" in *"model_provider=grillforge_agent"*) exit 54 ;; esac
printf '%s\n' '{{"type":"item.completed","item":{{"type":"agent_message","text":"native codex child completed"}}}}' '{{"type":"turn.completed"}}'
"#,
            argv = argv_log.display(),
            agent = agent_file.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();

    let service = ControlPlaneService::new(directory.path());
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "codex".into(),
                runtime,
                config_root: codex_root,
            }],
            vec![AgentRuntimeRoute {
                extension_id: "native-codex-reviewer".into(),
                source_client_id: "codex".into(),
                source_agent_id: "reviewer".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"native-codex-reviewer","cwd":directory.path(),"prompt":"Review"
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(agent_result(&response), "native codex child completed");
    let argv = fs::read_to_string(argv_log).unwrap();
    assert!(argv.contains("--enable\nmulti_agent"), "{argv}");
    assert!(!argv.contains("grillforge_agent"), "{argv}");
}

#[tokio::test]
async fn client_agent_lists_update_independently_without_remounting_mcp() {
    let directory = tempfile::tempdir().expect("temporary directory");
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
        .unwrap();
    for id in ["one", "two"] {
        service
            .save_model(ModelInput {
                id: id.into(),
                name: id.into(),
                upstream_id: id.into(),
                provider_id: "local".into(),
                capabilities: vec!["coding".into()],
                protocol_capabilities: vec![],
                context_window: None,
                max_output_tokens: None,
            })
            .unwrap();
    }
    let runtime = directory.path().join("claude");
    fs::write(&runtime, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    let status = gateway.status(base_url.clone());
    let state = service.state().unwrap();
    status
        .activate_client_agent_broker(
            "claude_code",
            &state,
            "claude-token",
            &runtime,
            directory.path(),
            vec![AgentRuntimeRoute {
                extension_id: "one".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "reviewer".into(),
                model_id: Some("one".into()),
            }],
        )
        .unwrap();
    status
        .activate_client_agent_broker(
            "codex",
            &state,
            "codex-token",
            &runtime,
            directory.path(),
            vec![AgentRuntimeRoute {
                extension_id: "one".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "reviewer".into(),
                model_id: Some("one".into()),
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    status
        .activate_client_agent_broker(
            "claude_code",
            &state,
            "claude-token",
            &runtime,
            directory.path(),
            vec![AgentRuntimeRoute {
                extension_id: "two".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "reviewer".into(),
                model_id: Some("two".into()),
            }],
        )
        .unwrap();

    let call_list = |client_id: &'static str, token: &'static str| {
        let client = reqwest::Client::new();
        let url = format!("{base_url}/mcp/{client_id}");
        async move {
            client
                .post(url)
                .bearer_auth(token)
                .json(&json!({
                    "jsonrpc":"2.0","id":1,"method":"tools/call",
                    "params":{"name":"list_agents","arguments":{}}
                }))
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let claude = call_list("claude_code", "claude-token").await;
    let claude_list: Value =
        serde_json::from_str(claude["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(claude_list[0]["extensionId"], "two");
    let codex = call_list("codex", "codex-token").await;
    let codex_list: Value =
        serde_json::from_str(codex["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(codex_list[0]["extensionId"], "one");
    let cross_client = reqwest::Client::new()
        .post(format!("{base_url}/mcp/codex"))
        .bearer_auth("claude-token")
        .json(&json!({"jsonrpc":"2.0","id":9,"method":"tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(cross_client.status(), StatusCode::UNAUTHORIZED);

    status.deactivate_client_agent_broker("claude_code");
    let unavailable = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("claude-token")
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        call_list("codex", "codex-token").await["result"]["isError"],
        false
    );
}

#[tokio::test]
async fn opencode_subagent_source_is_selected_exactly_with_an_isolated_managed_model() {
    let directory = tempfile::tempdir().unwrap();
    let opencode_root = directory.path().join("opencode-config");
    fs::create_dir_all(opencode_root.join("agents")).unwrap();
    let agent_file = opencode_root.join("agents/reviewer.md");
    let original_agent = "---\ndescription: Reviews code\nmode: subagent\nmodel: native/model\n---\nPrivate prompt\n";
    fs::write(&agent_file, original_agent).unwrap();
    let argv_log = directory.path().join("opencode.argv");
    let env_log = directory.path().join("opencode.env");
    let runtime = directory.path().join("opencode");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > '{argv}'
printf '%s' "$OPENCODE_CONFIG_CONTENT" > '{env}'
printf '%s\n' '{{"type":"text","part":{{"type":"text","text":"OpenCode child completed","time":{{"end":1}}}}}}'
"#,
            argv = argv_log.display(),
            env = env_log.display(),
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();
    let service = ControlPlaneService::new(directory.path());
    service
        .save_provider(ProviderInput {
            id: "local".into(),
            name: "Local".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: "http://127.0.0.1:9".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .unwrap();
    service
        .save_model(ModelInput {
            id: "worker".into(),
            name: "Worker".into(),
            upstream_id: "worker-upstream".into(),
            provider_id: "local".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
            context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "opencode".into(),
                runtime,
                config_root: opencode_root,
            }],
            vec![AgentRuntimeRoute {
                extension_id: "opencode-reviewer".into(),
                source_client_id: "opencode".into(),
                source_agent_id: "reviewer".into(),
                model_id: Some("worker".into()),
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"opencode-reviewer","cwd":directory.path(),"prompt":"Review this"
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(agent_result(&response), "OpenCode child completed");
    let argv = fs::read_to_string(argv_log).unwrap();
    assert!(argv.contains("run\n"));
    assert!(argv.contains("--format\njson\n"));
    assert!(argv.contains("--model\ngrillforge_agent/grillforge/worker\n"));
    assert!(argv.contains("--agent\nreviewer\n"));
    assert!(argv.ends_with("Review this\n"));
    let config: Value = serde_json::from_slice(&fs::read(env_log).unwrap()).unwrap();
    assert_eq!(
        config["provider"]["grillforge_agent"]["options"]["baseURL"],
        format!("{base_url}/agent-runtime/v1")
    );
    assert_eq!(
        config["provider"]["grillforge_agent"]["models"]["grillforge/worker"]["name"],
        "GrillForge worker"
    );
    assert_eq!(
        config["agent"]["reviewer"]["model"],
        "grillforge_agent/grillforge/worker"
    );
    assert_eq!(config["agent"]["reviewer"]["mode"], "primary");
    assert_eq!(fs::read_to_string(agent_file).unwrap(), original_agent);
}

#[tokio::test]
async fn opencode_builtin_subagent_uses_native_model_through_isolated_promotion() {
    let directory = tempfile::tempdir().unwrap();
    let opencode_root = directory.path().join("opencode-config");
    fs::create_dir_all(&opencode_root).unwrap();
    let argv_log = directory.path().join("opencode-native.argv");
    let env_log = directory.path().join("opencode-native.env");
    let runtime = directory.path().join("opencode-native");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > '{argv}'
printf '%s' "${{OPENCODE_CONFIG_CONTENT-unset}}" > '{env}'
printf '%s\n' '{{"type":"text","part":{{"type":"text","text":"native build completed","time":{{"end":1}}}}}}'
"#,
            argv = argv_log.display(),
            env = env_log.display(),
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();
    let service = ControlPlaneService::new(directory.path());
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "opencode".into(),
                runtime,
                config_root: opencode_root,
            }],
            vec![AgentRuntimeRoute {
                extension_id: "opencode-general".into(),
                source_client_id: "opencode".into(),
                source_agent_id: "general".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"opencode-general","cwd":directory.path(),"prompt":"Research this"
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(agent_result(&response), "native build completed");
    let argv = fs::read_to_string(argv_log).unwrap();
    assert!(argv.contains("--agent\ngeneral\n"));
    assert!(argv.ends_with("Research this\n"));
    assert!(!argv.contains("--model\n"));
    let config: Value = serde_json::from_slice(&fs::read(env_log).unwrap()).unwrap();
    assert_eq!(config["agent"]["general"]["mode"], "primary");
}

#[tokio::test]
async fn kimi_builtin_agent_uses_an_isolated_managed_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let kimi_root = directory.path().join("kimi-config");
    fs::create_dir_all(&kimi_root).unwrap();
    let config_file = kimi_root.join("config.toml");
    let original_config = r#"telemetry = false
default_model = "native"

[providers.native]
type = "anthropic"
base_url = "https://native.invalid"
api_key = "native-secret"

[models.native]
provider = "native"
model = "native-model"
max_context_size = 100000
"#;
    fs::write(&config_file, original_config).unwrap();
    let argv_log = directory.path().join("kimi.argv");
    let config_log = directory.path().join("kimi.effective.toml");
    let runtime = directory.path().join("kimi");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > '{argv}'
test "$GRILLFORGE_AGENT_CHILD" = "1" || exit 41
test "$KIMI_CODE_NO_AUTO_UPDATE" = "1" || exit 42
test "$KIMI_DISABLE_TELEMETRY" = "1" || exit 43
test "$KIMI_CODE_EXPERIMENTAL_SECONDARY_MODEL" = "1" || exit 44
cp "$KIMI_CODE_HOME/config.toml" '{config}'
printf '%s\n' '{{"role":"meta","content":"ignored"}}'
printf '%s\n' '{{"role":"assistant","content":"Kimi managed child completed"}}'
"#,
            argv = argv_log.display(),
            config = config_log.display(),
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();
    let service = ControlPlaneService::new(directory.path());
    service
        .save_provider(ProviderInput {
            id: "local".into(),
            name: "Local".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: "http://127.0.0.1:9".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::None,
            api_key: None,
            enabled: true,
            models_url: None,
        })
        .unwrap();
    service
        .save_model(ModelInput {
            id: "worker".into(),
            name: "Worker".into(),
            upstream_id: "worker-upstream".into(),
            provider_id: "local".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
            context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "kimi_code".into(),
                runtime,
                config_root: kimi_root,
            }],
            vec![AgentRuntimeRoute {
                extension_id: "kimi-coder".into(),
                source_client_id: "kimi_code".into(),
                source_agent_id: "coder".into(),
                model_id: Some("worker".into()),
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"kimi-coder","cwd":directory.path(),"prompt":"Review this"
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(agent_result(&response), "Kimi managed child completed");
    let argv = fs::read_to_string(argv_log).unwrap();
    assert!(argv.contains("--agent\ncoder\n"));
    assert!(argv.contains("--model\ngrillforge/worker\n"));
    assert!(argv.contains("--prompt\nReview this\n"));
    assert!(argv.contains("--output-format\nstream-json\n"));
    assert!(!argv.contains("--config-file\n"));
    assert!(!argv.contains("--quiet\n"));
    assert!(!argv.contains("--work-dir\n"));
    assert!(!argv.contains("broker-token"));
    let effective = fs::read_to_string(config_log).unwrap();
    assert!(effective.contains("telemetry = false"));
    assert!(effective.contains("default_model = \"grillforge/worker\""));
    assert!(effective.contains("[providers.grillforge_agent]"));
    assert!(effective.contains("type = \"anthropic\""));
    assert!(effective.contains(&format!("base_url = \"{base_url}/agent-runtime/v1\"")));
    assert!(effective.contains("[models.\"grillforge/worker\"]"));
    assert!(effective.contains("capabilities = [\"tool_use\"]"));
    assert!(effective.contains("[secondary_model]"));
    assert!(effective.contains("force = true"));
    let effective_document = effective.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(
        effective_document["experimental"]["secondary-model"].as_bool(),
        Some(true)
    );
    assert_eq!(fs::read_to_string(config_file).unwrap(), original_config);
}

#[tokio::test]
async fn kimi_builtin_agent_uses_native_configuration_without_managed_injection() {
    let directory = tempfile::tempdir().unwrap();
    let kimi_root = directory.path().join("kimi-config");
    fs::create_dir_all(&kimi_root).unwrap();
    let argv_log = directory.path().join("kimi-native.argv");
    let runtime = directory.path().join("kimi-native");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > '{argv}'
printf '%s\n' '{{"role":"assistant","content":"Kimi native child completed"}}'
"#,
            argv = argv_log.display(),
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();
    let service = ControlPlaneService::new(directory.path());
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "kimi_code".into(),
                runtime,
                config_root: kimi_root,
            }],
            vec![AgentRuntimeRoute {
                extension_id: "kimi-explore".into(),
                source_client_id: "kimi_code".into(),
                source_agent_id: "explore".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"kimi-explore","cwd":directory.path(),"prompt":"Inspect this"
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(agent_result(&response), "Kimi native child completed");
    let argv = fs::read_to_string(argv_log).unwrap();
    assert!(argv.contains("--agent\nexplore\n"));
    assert!(argv.contains("--output-format\nstream-json\n"));
    assert!(!argv.contains("--config-file\n"));
    assert!(!argv.contains("--model\n"));
}

#[tokio::test]
async fn gemini_agent_source_is_invoked_exactly_with_at_syntax() {
    let directory = tempfile::tempdir().unwrap();
    let gemini_home = directory.path().join("home");
    let gemini_root = gemini_home.join(".gemini");
    let project = directory.path().join("project");
    fs::create_dir_all(&gemini_root).unwrap();
    fs::create_dir_all(project.join(".gemini/agents")).unwrap();
    fs::write(
        project.join(".gemini/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nReview.\n",
    )
    .unwrap();
    let argv_log = directory.path().join("gemini.argv");
    let home_log = directory.path().join("gemini.home");
    let settings_log = directory.path().join("gemini.settings.json");
    let runtime = directory.path().join("gemini");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > '{argv}'
printf '%s' "$GEMINI_CLI_HOME" > '{home}'
cp "$GEMINI_CLI_SYSTEM_SETTINGS_PATH" '{settings}'
test "$GEMINI_MODEL" = 'grillforge--worker' || exit 21
case "$GOOGLE_GEMINI_BASE_URL" in http://127.0.0.1:*/agent-runtime/gemini) ;; *) exit 22 ;; esac
test -n "$GEMINI_API_KEY" || exit 23
printf '%s' '{{"response":"Gemini child completed","stats":{{}}}}'
"#,
            argv = argv_log.display(),
            home = home_log.display(),
            settings = settings_log.display(),
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();
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
        .unwrap();
    service
        .save_model(ModelInput {
            id: "worker".into(),
            name: "Worker".into(),
            upstream_id: "worker-upstream".into(),
            provider_id: "local".into(),
            capabilities: vec![],
            protocol_capabilities: vec![],
            context_window: None,
            max_output_tokens: None,
        })
        .unwrap();
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "gemini".into(),
                runtime,
                config_root: gemini_root,
            }],
            vec![AgentRuntimeRoute {
                extension_id: "gemini-reviewer".into(),
                source_client_id: "gemini".into(),
                source_agent_id: "reviewer".into(),
                model_id: Some("worker".into()),
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"gemini-reviewer","cwd":project,"prompt":"Review this"
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(agent_result(&response), "Gemini child completed");
    assert_eq!(
        fs::read_to_string(argv_log).unwrap(),
        "--approval-mode\nauto_edit\n--skip-trust\n--output-format\njson\n-p\n@reviewer Review this\n"
    );
    assert_eq!(
        fs::read_to_string(home_log).unwrap(),
        gemini_home.display().to_string()
    );
    let settings: Value = serde_json::from_str(&fs::read_to_string(settings_log).unwrap()).unwrap();
    assert_eq!(settings["model"]["name"], "grillforge--worker");
    assert_eq!(settings["general"]["maxAttempts"], 1);
    assert_eq!(settings["general"]["retryFetchErrors"], false);
    assert_eq!(
        settings["modelConfigs"]["customOverrides"][0]["match"]["model"],
        "grillforge--worker"
    );
    assert_eq!(
        settings["modelConfigs"]["customOverrides"][0]["modelConfig"]["generateContentConfig"]["maxOutputTokens"],
        8192
    );
    assert_eq!(
        settings["agents"]["overrides"]["reviewer"]["modelConfig"]["model"],
        "grillforge--worker"
    );
    assert_eq!(
        settings["agents"]["overrides"]["reviewer"]["modelConfig"]["generateContentConfig"]["maxOutputTokens"],
        8192
    );
    assert_eq!(
        settings["security"]["auth"]["selectedType"],
        "gemini-api-key"
    );
}

#[tokio::test]
async fn kimi_custom_agent_is_selected_by_its_exact_agent_file() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let kimi_root = home.join(".kimi-code");
    let custom_agent = kimi_root.join("agents/reviewer.md");
    fs::create_dir_all(custom_agent.parent().unwrap()).unwrap();
    fs::write(
        &custom_agent,
        "---\nname: reviewer\ndescription: Exact reviewer\n---\nReview carefully.\n",
    )
    .unwrap();
    let argv_log = directory.path().join("kimi-custom.argv");
    let runtime = directory.path().join("kimi-custom");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > '{argv}'
printf '%s\n' '{{"role":"assistant","content":"Kimi custom child completed"}}'
"#,
            argv = argv_log.display(),
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();
    let service = ControlPlaneService::new(directory.path());
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "claude_code",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "kimi_code".into(),
                runtime,
                config_root: kimi_root,
            }],
            vec![AgentRuntimeRoute {
                extension_id: "kimi-reviewer".into(),
                source_client_id: "kimi_code".into(),
                source_agent_id: "reviewer".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let response: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-token")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"kimi-reviewer","cwd":directory.path(),"prompt":"Inspect this"
            }}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(agent_result(&response), "Kimi custom child completed");
    let argv = fs::read_to_string(argv_log).unwrap();
    assert!(argv.contains("--agent-file\n"));
    assert!(argv.contains(&format!("{}\n", custom_agent.display())));
    assert!(!argv.contains("--agent\nreviewer\n"));
}

#[tokio::test]
async fn a_failed_agent_runtime_reports_the_error_it_wrote_to_stdout() {
    let directory = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(directory.path());

    let runtime = directory.path().join("claude");
    fs::write(
        &runtime,
        r#"#!/bin/sh
printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"API Error: 502 invalid Anthropic request"}'
exit 1
"#,
    )
    .expect("fake runtime");
    let mut permissions = fs::metadata(&runtime).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).expect("executable permissions");

    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("gateway address");
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker(
            "claude_code",
            &service.state().expect("state"),
            "broker-secret",
            &runtime,
            directory.path(),
            vec![AgentRuntimeRoute {
                extension_id: "native-reviewer".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "reviewer".into(),
                model_id: None,
            }],
        )
        .expect("activate broker");
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let body: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-secret")
        .json(&json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"native-reviewer",
                "cwd": directory.path(),
                "prompt":"Inspect the project"
            }}
        }))
        .send()
        .await
        .expect("MCP response")
        .json()
        .await
        .expect("MCP JSON");

    assert_eq!(body["result"]["isError"], true);
    let text = body["result"]["content"][0]["text"]
        .as_str()
        .expect("error text");
    assert!(
        text.contains("API Error: 502 invalid Anthropic request"),
        "{text}"
    );
}

#[tokio::test]
async fn a_managed_child_is_told_the_models_real_context_window() {
    let directory = tempfile::tempdir().unwrap();
    let service = ControlPlaneService::new(directory.path());
    service
        .save_provider(ProviderInput {
            id: "local".into(),
            name: "Local".into(),
            protocol: Protocol::AnthropicMessages,
            endpoint: "http://127.0.0.1:9/anthropic".into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("provider-secret".into()),
            enabled: true,
            models_url: None,
        })
        .expect("provider");
    service
        .save_model(ModelInput {
            id: "wide".into(),
            name: "Wide".into(),
            upstream_id: "wide-upstream".into(),
            provider_id: "local".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities: vec![],
            context_window: Some(262_144),
            max_output_tokens: None,
        })
        .expect("model");

    // The child fails unless it was handed the window recorded for its model.
    let runtime = directory.path().join("claude");
    fs::write(
        &runtime,
        r#"#!/bin/sh
if [ "$CLAUDE_CODE_MAX_CONTEXT_TOKENS" != "262144" ]; then
  printf '%s\n' '{"type":"result","is_error":true,"result":"window was '"$CLAUDE_CODE_MAX_CONTEXT_TOKENS"'"}'
  exit 1
fi
printf '%s' '{"type":"result","result":"child saw the real window"}'
"#,
    )
    .expect("fake runtime");
    let mut permissions = fs::metadata(&runtime).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).expect("executable permissions");

    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("gateway address");
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker(
            "claude_code",
            &service.state().expect("state"),
            "broker-secret",
            &runtime,
            directory.path(),
            vec![AgentRuntimeRoute {
                extension_id: "wide-reviewer".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "reviewer".into(),
                model_id: Some("wide".into()),
            }],
        )
        .expect("activate broker");
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let body: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-secret")
        .json(&json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{"waitSeconds":120,
                "extensionId":"wide-reviewer",
                "cwd": directory.path(),
                "prompt":"Inspect the project"
            }}
        }))
        .send()
        .await
        .expect("MCP response")
        .json()
        .await
        .expect("MCP JSON");

    assert_eq!(body["result"]["isError"], false, "{body}");
    assert_eq!(agent_result(&body), "child saw the real window");
}

#[tokio::test]
async fn a_client_publishes_the_permission_modes_it_accepts_and_rejects_the_rest() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = directory.path().join("claude");
    let argv_log = directory.path().join("argv");
    fs::create_dir(directory.path().join(".claude")).unwrap();
    fs::write(
        &runtime,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s' '{{\"type\":\"result\",\"result\":\"done\"}}'\n",
            argv_log.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();

    let service = ControlPlaneService::new(directory.path());
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "codex",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "claude_code".into(),
                runtime,
                config_root: directory.path().join(".claude"),
            }],
            vec![AgentRuntimeRoute {
                extension_id: "claude-general".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "general-purpose".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let call = |id: i32, name: &str, arguments: Value| {
        reqwest::Client::new()
            .post(format!("{base_url}/mcp/codex"))
            .bearer_auth("broker-token")
            .json(&json!({
                "jsonrpc":"2.0","id":id,"method":"tools/call",
                "params":{"name":name,"arguments":arguments}
            }))
            .send()
    };

    // The caller can see what this Agent's client accepts before choosing.
    let listed: Value = call(1, "list_agents", json!({}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed: Value =
        serde_json::from_str(listed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(listed[0]["defaultPermissionMode"], "auto");
    let modes = listed[0]["permissionModes"].as_array().unwrap();
    assert!(modes.iter().any(|mode| mode == "plan"), "{modes:?}");
    assert!(
        modes.iter().any(|mode| mode == "bypassPermissions"),
        "{modes:?}"
    );

    // A named mode reaches the runtime.
    let response: Value = call(
        2,
        "run_agent",
        json!({
            "extensionId":"claude-general","cwd":directory.path(),
            "prompt":"Plan only","permissionMode":"plan","waitSeconds":120
        }),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    let argv = fs::read_to_string(&argv_log).unwrap();
    assert!(argv.starts_with("--permission-mode\nplan\n"), "{argv}");

    // A mode the client does not accept fails before the Agent is launched.
    let rejected: Value = call(
        3,
        "run_agent",
        json!({
            "extensionId":"claude-general","cwd":directory.path(),
            "prompt":"Anything","permissionMode":"workspace-write"
        }),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(rejected["result"]["isError"], true, "{rejected}");
    let text = rejected["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("unsupported claude_code permission mode"),
        "{text}"
    );
    assert!(text.contains("available:"), "{text}");
}

#[tokio::test]
async fn a_stopped_run_is_cancelled_and_forgotten() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = directory.path().join("claude");
    fs::create_dir(directory.path().join(".claude")).unwrap();
    fs::write(&runtime, "#!/bin/sh\nsleep 120\n").unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();

    let service = ControlPlaneService::new(directory.path());
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "codex",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "claude_code".into(),
                runtime,
                config_root: directory.path().join(".claude"),
            }],
            vec![AgentRuntimeRoute {
                extension_id: "slow".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "general-purpose".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let call = |id: i32, name: &str, arguments: Value| {
        reqwest::Client::new()
            .post(format!("{base_url}/mcp/codex"))
            .bearer_auth("broker-token")
            .json(&json!({
                "jsonrpc":"2.0","id":id,"method":"tools/call",
                "params":{"name":name,"arguments":arguments}
            }))
            .send()
    };

    let started: Value = call(
        1,
        "run_agent",
        json!({"extensionId":"slow","cwd":directory.path(),"prompt":"take your time"}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let handle: Value =
        serde_json::from_str(started["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let run_id = handle["runId"].as_str().unwrap().to_string();

    // A long run does not hold the caller: checking without waiting returns at once.
    let pending: Value = call(2, "get_agent_result", json!({"runId":run_id}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pending: Value =
        serde_json::from_str(pending["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(pending["status"], "running");

    let stopped: Value = call(3, "stop_agent", json!({"runId":run_id}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stopped["result"]["isError"], false, "{stopped}");

    let gone: Value = call(4, "get_agent_result", json!({"runId":run_id}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(gone["result"]["isError"], true, "{gone}");
}

#[tokio::test]
async fn a_child_permission_prompt_is_relayed_to_the_delegating_agent() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = directory.path().join("claude");
    let decision_log = directory.path().join("decision");
    fs::create_dir(directory.path().join(".claude")).unwrap();
    // The child uses the very config GrillForge hands it to raise a prompt.
    fs::write(
        &runtime,
        format!(
            r#"#!/usr/bin/env python3
import json, sys, urllib.request, time
argv = sys.argv[1:]
config = json.loads(argv[argv.index("--mcp-config") + 1])
server = config["mcpServers"]["grillforge_permission"]
url = server["env"]["GRILLFORGE_MCP_URL"]
token = server["env"]["GRILLFORGE_MCP_TOKEN"]
body = json.dumps({{
    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
    "params": {{"name": "approve", "arguments": {{
        "tool_name": "Write", "input": {{"file_path": "/tmp/x", "content": "y"}}
    }}}}
}}).encode()
request = urllib.request.Request(url, data=body, method="POST")
request.add_header("Content-Type", "application/json")
request.add_header("Authorization", "Bearer " + token)
with urllib.request.urlopen(request, timeout=120) as response:
    payload = json.loads(response.read().decode())
decision = json.loads(payload["result"]["content"][0]["text"])
open("{log}", "w").write(json.dumps(decision))
print(json.dumps({{"type": "result", "result": "child saw " + decision["behavior"]}}))
"#,
            log = decision_log.to_str().unwrap()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&runtime).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runtime, permissions).unwrap();

    let service = ControlPlaneService::new(directory.path());
    let gateway = Gateway::new(directory.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    gateway
        .status(base_url.clone())
        .activate_client_agent_broker_with_sources(
            "codex",
            &service.state().unwrap(),
            "broker-token",
            vec![AgentSourceRuntime {
                source_client_id: "claude_code".into(),
                runtime,
                config_root: directory.path().join(".claude"),
            }],
            vec![AgentRuntimeRoute {
                extension_id: "worker".into(),
                source_client_id: "claude_code".into(),
                source_agent_id: "general-purpose".into(),
                model_id: None,
            }],
        )
        .unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let call = |id: i32, name: &str, arguments: Value| {
        reqwest::Client::new()
            .post(format!("{base_url}/mcp/codex"))
            .bearer_auth("broker-token")
            .json(&json!({
                "jsonrpc":"2.0","id":id,"method":"tools/call",
                "params":{"name":name,"arguments":arguments}
            }))
            .send()
    };

    let started: Value = call(
        1,
        "run_agent",
        json!({"extensionId":"worker","cwd":directory.path(),"prompt":"do work"}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let handle: Value =
        serde_json::from_str(started["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let run_id = handle["runId"].as_str().unwrap().to_string();

    // Another caller cannot raise or answer a prompt for this run.
    let unauthorized = reqwest::Client::new()
        .post(format!("{base_url}/mcp/agent-permission/{run_id}"))
        .bearer_auth("not-the-secret")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    // The delegating Agent sees the prompt and decides.
    let mut request_id = None;
    for _ in 0..100 {
        let status: Value = call(2, "get_agent_result", json!({"runId":run_id}))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let payload: Value =
            serde_json::from_str(status["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        if payload["status"] == "awaiting_permission" {
            assert_eq!(payload["pendingPermissions"][0]["toolName"], "Write");
            request_id = payload["pendingPermissions"][0]["requestId"]
                .as_str()
                .map(str::to_string);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let request_id = request_id.expect("the prompt reached the delegating Agent");

    let answered: Value = call(
        3,
        "answer_agent_permission",
        json!({"requestId":request_id,"behavior":"allow"}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(answered["result"]["isError"], false, "{answered}");

    // The decision reached the child, which acted on it.
    let collected: Value = call(
        4,
        "get_agent_result",
        json!({"runId":run_id,"waitSeconds":60}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(collected["result"]["isError"], false, "{collected}");
    assert_eq!(agent_result(&collected), "child saw allow");
    let logged: Value = serde_json::from_str(&fs::read_to_string(&decision_log).unwrap()).unwrap();
    assert_eq!(logged["behavior"], "allow");
}
