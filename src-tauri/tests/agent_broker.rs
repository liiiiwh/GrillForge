#![cfg(unix)]

use axum::http::StatusCode;
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::{AgentRuntimeRoute, AgentSourceRuntime, Gateway};
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
    assert!(instructions.contains("默认先调用 list_agents"));
    assert!(instructions.contains("run_agent"));
    assert!(instructions.contains("用户明确要求使用原生 Agent"));

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

    let response = client
        .post(format!("{base_url}/mcp/claude_code"))
        .bearer_auth("broker-secret")
        .json(&json!({
            "jsonrpc":"2.0",
            "id":3,
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
            "params":{"name":"run_agent","arguments":{
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
    assert_eq!(
        response["result"]["content"][0]["text"],
        "pi child completed"
    );
    let argv = fs::read_to_string(argv_log).unwrap();
    assert!(argv.starts_with(&format!("{}|", pi_root.display())));
    assert!(argv.contains("--mode json -p --no-session"));
    assert!(argv.contains("--tools read,grep,find"));
    assert!(argv.contains("Task: Review this"));
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
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"run_agent","arguments":{"extensionId":"pi-reviewer","cwd":project,"prompt":"Review"}}}))
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
case "$*" in *"agents.reviewer.config_file="*) ;; *) exit 31 ;; esac
case "$*" in *"agents.default_subagent_model=grillforge/worker"*) ;; *) exit 32 ;; esac
case "$*" in *"agent_type reviewer"*"fork_turns none"*) ;; *) exit 33 ;; esac
case "$*" in *"model_providers.grillforge_agent.base_url="*) ;; *) exit 34 ;; esac
test -n "$GRILLFORGE_AGENT_TOKEN" || exit 35
for argument in "$@"; do
  case "$argument" in
    agents.reviewer.config_file=*)
      config=${{argument#*=}}
      config=${{config#\"}}
      config=${{config%\"}}
      grep -q 'PROJECT_ROLE_MARKER' "$config" || exit 36
      grep -q 'USER_ROLE_MARKER' "$config" && exit 38
      grep -q 'model = "grillforge/worker"' "$config" || exit 37
      grep -q 'model_provider = "grillforge_agent"' "$config" || exit 39
      cp "$config" {argv}.effective
      ;;
  esac
done
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
            vec![AgentRuntimeRoute {
                extension_id: "codex-reviewer".into(),
                source_client_id: "codex".into(),
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
            "params":{"name":"run_agent","arguments":{
                "extensionId":"codex-reviewer","cwd":project,"prompt":"Review this"
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
        "codex child completed"
    );
    let argv = fs::read_to_string(&argv_log).unwrap();
    assert!(argv.contains("agents.reviewer.config_file="));
    assert!(argv.contains("agents.default_subagent_model=grillforge/worker"));
    let effective = fs::read_to_string(format!("{}.effective", argv_log.display())).unwrap();
    assert!(effective.contains("PROJECT_ROLE_MARKER"));
    assert!(!effective.contains("USER_ROLE_MARKER"));
    assert!(effective.contains("model = \"grillforge/worker\""));
    assert!(effective.contains("model_provider = \"grillforge_agent\""));
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

#[tokio::test]
async fn opencode_agent_source_is_selected_exactly_with_an_isolated_managed_model() {
    let directory = tempfile::tempdir().unwrap();
    let opencode_root = directory.path().join("opencode-config");
    fs::create_dir_all(opencode_root.join("agents")).unwrap();
    let agent_file = opencode_root.join("agents/reviewer.md");
    let original_agent =
        "---\ndescription: Reviews code\nmode: all\nmodel: native/model\n---\nPrivate prompt\n";
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
            "params":{"name":"run_agent","arguments":{
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
    assert_eq!(
        response["result"]["content"][0]["text"],
        "OpenCode child completed"
    );
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
    assert_eq!(fs::read_to_string(agent_file).unwrap(), original_agent);
}

#[tokio::test]
async fn opencode_primary_agent_uses_native_configuration_without_managed_injection() {
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
                extension_id: "opencode-build".into(),
                source_client_id: "opencode".into(),
                source_agent_id: "build".into(),
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
            "params":{"name":"run_agent","arguments":{
                "extensionId":"opencode-build","cwd":directory.path(),"prompt":"Build this"
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
        "native build completed"
    );
    let argv = fs::read_to_string(argv_log).unwrap();
    assert!(argv.contains("--agent\nbuild\n"));
    assert!(argv.ends_with("Build this\n"));
    assert!(!argv.contains("--model\n"));
    assert_eq!(fs::read_to_string(env_log).unwrap(), "unset");
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
previous=""
for argument in "$@"; do
  if [ "$previous" = "--config-file" ]; then cp "$argument" '{config}'; fi
  previous="$argument"
done
printf '%s' 'Kimi managed child completed'
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
                extension_id: "kimi-okabe".into(),
                source_client_id: "kimi_code".into(),
                source_agent_id: "okabe".into(),
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
            "params":{"name":"run_agent","arguments":{
                "extensionId":"kimi-okabe","cwd":directory.path(),"prompt":"Review this"
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
        "Kimi managed child completed"
    );
    let argv = fs::read_to_string(argv_log).unwrap();
    assert!(argv.contains("--agent\nokabe\n"));
    assert!(argv.contains("--model\ngrillforge/worker\n"));
    assert!(argv.contains("--config-file\n"));
    assert!(argv.contains("--quiet\n"));
    assert!(argv.contains("--prompt\nReview this\n"));
    assert!(!argv.contains("broker-token"));
    let effective = fs::read_to_string(config_log).unwrap();
    assert!(effective.contains("telemetry = false"));
    assert!(effective.contains("default_model = \"grillforge/worker\""));
    assert!(effective.contains("[providers.grillforge_agent]"));
    assert!(effective.contains("type = \"anthropic\""));
    assert!(effective.contains(&format!("base_url = \"{base_url}/agent-runtime/v1\"")));
    assert!(effective.contains("[models.\"grillforge/worker\"]"));
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
printf '%s' 'Kimi native child completed'
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
                extension_id: "kimi-default".into(),
                source_client_id: "kimi_code".into(),
                source_agent_id: "default".into(),
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
            "params":{"name":"run_agent","arguments":{
                "extensionId":"kimi-default","cwd":directory.path(),"prompt":"Inspect this"
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
        "Kimi native child completed"
    );
    let argv = fs::read_to_string(argv_log).unwrap();
    assert!(argv.contains("--agent\ndefault\n"));
    assert!(!argv.contains("--config-file\n"));
    assert!(!argv.contains("--model\n"));
}
