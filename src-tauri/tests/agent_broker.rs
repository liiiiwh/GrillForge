#![cfg(unix)]

use axum::http::StatusCode;
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::{AgentRuntimeRoute, Gateway};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tokio::net::TcpListener;

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

    let response = client
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-secret")
        .json(&json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params": {
                "name":"run_agent",
                "arguments": {
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
    assert_eq!(
        body["result"]["content"][0]["text"],
        "child runtime completed"
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
            "params":{"name":"run_agent","arguments":{
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
    assert_eq!(
        response["result"]["content"][0]["text"],
        "native runtime completed"
    );

    let override_attempt: Value = reqwest::Client::new()
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("native-token")
        .json(&json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"run_agent","arguments":{
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
