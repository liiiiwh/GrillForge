use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::core::model::ProtocolCapability;
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use serde_json::{Value, json};
use std::env;
use std::time::Duration;
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio::time::timeout;

#[tokio::test]
#[ignore = "uses an explicitly configured real Provider and may incur Provider charges"]
async fn configured_real_provider_streams_text_and_tool_use_through_the_gateway() {
    let protocol = required("GRILLFORGE_LIVE_PROTOCOL");
    let protocol = match protocol.as_str() {
        "anthropic_messages" => Protocol::AnthropicMessages,
        "open_ai_responses" => Protocol::OpenAiResponses,
        "open_ai_chat_completions" => Protocol::OpenAiChatCompletions,
        other => panic!("unsupported GRILLFORGE_LIVE_PROTOCOL: {other}"),
    };
    let placement = match env::var("GRILLFORGE_LIVE_API_KEY_PLACEMENT")
        .unwrap_or_else(|_| match protocol {
            Protocol::AnthropicMessages => "x_api_key".into(),
            Protocol::OpenAiResponses | Protocol::OpenAiChatCompletions => "bearer".into(),
            Protocol::GeminiNative => "x_api_key".into(),
        })
        .as_str()
    {
        "bearer" => ApiKeyPlacement::Bearer,
        "x_api_key" => ApiKeyPlacement::XApiKey,
        other => panic!("unsupported GRILLFORGE_LIVE_API_KEY_PLACEMENT: {other}"),
    };
    let endpoint_mode = match env::var("GRILLFORGE_LIVE_ENDPOINT_MODE")
        .unwrap_or_else(|_| "base_url".into())
        .as_str()
    {
        "base_url" => EndpointMode::BaseUrl,
        "exact_url" => EndpointMode::ExactUrl,
        other => panic!("unsupported GRILLFORGE_LIVE_ENDPOINT_MODE: {other}"),
    };
    let protocol_capabilities = env::var("GRILLFORGE_LIVE_PROTOCOL_CAPABILITIES")
        .unwrap_or_default()
        .split(',')
        .filter(|value| !value.trim().is_empty())
        .map(|value| match value.trim() {
            "reasoning_items" => ProtocolCapability::ReasoningItems,
            "reasoning_content" => ProtocolCapability::ReasoningContent,
            "reasoning_effort" => ProtocolCapability::ReasoningEffort,
            other => panic!("unsupported live protocol capability: {other}"),
        })
        .collect();

    let root = tempdir().expect("temporary configuration");
    let control = ControlPlaneService::new(root.path());
    control
        .save_provider(ProviderInput {
            id: "live".into(),
            name: "Live Provider".into(),
            protocol,
            endpoint: required("GRILLFORGE_LIVE_ENDPOINT"),
            endpoint_mode,
            api_key_placement: placement,
            api_key: Some(required("GRILLFORGE_LIVE_API_KEY")),
            enabled: true,
            models_url: None,
        })
        .expect("live Provider configuration");
    control
        .save_model(ModelInput {
            id: "live-model".into(),
            name: "Live Model".into(),
            upstream_id: required("GRILLFORGE_LIVE_MODEL"),
            provider_id: "live".into(),
            capabilities: vec!["coding".into()],
            protocol_capabilities,
        })
        .expect("live model configuration");
    let state = control
        .set_main_model(Some("live-model".into()))
        .expect("live main route");

    let gateway = Gateway::new(root.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("gateway");
    let address = listener.local_addr().expect("gateway address");
    gateway
        .status(format!("http://{address}"))
        .activate(&state)
        .expect("activate live route");
    tokio::spawn(async move {
        axum::serve(listener, gateway.router())
            .await
            .expect("serve gateway")
    });

    let client = reqwest::Client::new();
    let text_response = timeout(
        Duration::from_secs(90),
        client
            .post(format!("http://{address}/v1/messages"))
            .json(&json!({
                "model":"grillforge/live-model",
                "max_tokens":64,
                "messages":[{"role":"user","content":"Reply with exactly OK."}]
            }))
            .send(),
    )
    .await
    .expect("live text request timed out")
    .expect("live text request");
    if text_response.status() != 200 {
        let status = text_response.status();
        let body = text_response
            .text()
            .await
            .unwrap_or_else(|error| format!("<failed to read error body: {error}>"));
        panic!("live text request returned {status}: {body}");
    }
    let text: Value = timeout(Duration::from_secs(90), text_response.json())
        .await
        .expect("live text response body timed out")
        .expect("Anthropic text response");
    assert_eq!(text["type"], "message");
    assert!(
        text["content"]
            .as_array()
            .is_some_and(|content| !content.is_empty())
    );

    let tool_choice = match env::var("GRILLFORGE_LIVE_TOOL_CHOICE")
        .unwrap_or_else(|_| "any".into())
        .as_str()
    {
        "any" => json!({"type":"any"}),
        "auto" => json!({"type":"auto"}),
        other => panic!("unsupported GRILLFORGE_LIVE_TOOL_CHOICE: {other}"),
    };
    let tool_response = timeout(
        Duration::from_secs(90),
        client
            .post(format!("http://{address}/v1/messages"))
            .json(&json!({
            "model":"grillforge/live-model",
            "max_tokens":256,
            "stream":true,
            "messages":[{"role":"user","content":"You must use the report_result tool exactly once. Do not answer with text."}],
            "tools":[{
                "name":"report_result",
                "description":"Report the requested result.",
                "input_schema":{
                    "type":"object",
                    "properties":{"result":{"type":"string"}},
                    "required":["result"]
                }
            }],
            "tool_choice":tool_choice
        }))
            .send(),
    )
    .await
    .expect("live streaming tool request timed out")
    .expect("live streaming tool request");
    if tool_response.status() != 200 {
        let status = tool_response.status();
        let body = tool_response
            .text()
            .await
            .unwrap_or_else(|error| format!("<failed to read error body: {error}>"));
        panic!("live tool request returned {status}: {body}");
    }
    let stream = timeout(Duration::from_secs(90), tool_response.text())
        .await
        .expect("live streaming tool response body timed out")
        .expect("Anthropic SSE");
    let events: Vec<_> = stream
        .lines()
        .filter_map(|line| line.strip_prefix("event: "))
        .collect();
    let errors: Vec<Value> = stream
        .split("\n\n")
        .filter(|block| block.lines().any(|line| line == "event: error"))
        .filter_map(|block| {
            block
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .and_then(|data| serde_json::from_str(data).ok())
        })
        .collect();
    assert!(
        !stream.contains("event: error"),
        "unexpected error event; event sequence: {events:?}; errors: {errors:?}"
    );
    assert!(
        stream.contains("event: message_start"),
        "missing message_start; event sequence: {events:?}"
    );
    assert!(
        stream.contains("event: message_stop"),
        "missing message_stop; event sequence: {events:?}"
    );
    let require_tool = env::var("GRILLFORGE_LIVE_REQUIRE_TOOL")
        .unwrap_or_else(|_| "true".into())
        .parse::<bool>()
        .expect("GRILLFORGE_LIVE_REQUIRE_TOOL must be true or false");
    if require_tool {
        assert!(stream.contains("\"type\":\"tool_use\""));
    }
}

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} must be set for the live Provider test"))
}
