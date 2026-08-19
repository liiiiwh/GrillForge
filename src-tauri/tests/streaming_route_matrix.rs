use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::Response,
    routing::post,
};
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::configuration::{ConfigurationFiles, ProviderProtocolEndpoint};
use grillforge_lib::core::model::NativeProtocol;
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[derive(Clone, Default)]
struct Calls(Arc<Mutex<Vec<String>>>);

fn event(name: &str, value: Value) -> String {
    format!("event: {name}\ndata: {value}\n\n")
}

fn native_stream(protocol: &str, tool: bool) -> String {
    match (protocol, tool) {
        ("anthropic", false) => [
            event("message_start", json!({"type":"message_start","message":{"id":"msg_stream","type":"message","role":"assistant","model":"anthropic-upstream","content":[],"stop_reason":null,"usage":{"input_tokens":3,"output_tokens":0}}})),
            event("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
            event("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"stream-text"}})),
            event("content_block_stop", json!({"type":"content_block_stop","index":0})),
            event("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":2}})),
            event("message_stop", json!({"type":"message_stop"})),
        ].concat(),
        ("anthropic", true) => [
            event("message_start", json!({"type":"message_start","message":{"id":"msg_stream_tool","type":"message","role":"assistant","model":"anthropic-upstream","content":[],"stop_reason":null,"usage":{"input_tokens":3,"output_tokens":0}}})),
            event("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_stream","name":"lookup","input":{}}})),
            event("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"stream\"}"}})),
            event("content_block_stop", json!({"type":"content_block_stop","index":0})),
            event("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":2}})),
            event("message_stop", json!({"type":"message_stop"})),
        ].concat(),
        ("responses", false) => [
            event("response.created", json!({"type":"response.created","response":{"id":"resp_stream","object":"response","status":"in_progress","model":"responses-upstream","output":[]}})),
            event("response.output_item.added", json!({"type":"response.output_item.added","output_index":0,"item":{"id":"message_stream","type":"message","role":"assistant","status":"in_progress","content":[]}})),
            event("response.content_part.added", json!({"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}})),
            event("response.output_text.delta", json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"stream-text"})),
            event("response.output_text.done", json!({"type":"response.output_text.done","output_index":0,"content_index":0,"text":"stream-text"})),
            event("response.content_part.done", json!({"type":"response.content_part.done","output_index":0,"content_index":0,"part":{"type":"output_text","text":"stream-text","annotations":[]}})),
            event("response.output_item.done", json!({"type":"response.output_item.done","output_index":0,"item":{"id":"message_stream","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"stream-text","annotations":[]}]}})),
            event("response.completed", json!({"type":"response.completed","response":{"id":"resp_stream","object":"response","status":"completed","model":"responses-upstream","output":[{"id":"message_stream","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"stream-text","annotations":[]}]}],"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}})),
        ].concat(),
        ("responses", true) => [
            event("response.created", json!({"type":"response.created","response":{"id":"resp_stream_tool","object":"response","status":"in_progress","model":"responses-upstream","output":[]}})),
            event("response.output_item.added", json!({"type":"response.output_item.added","output_index":0,"item":{"id":"fc_stream","type":"function_call","status":"in_progress","call_id":"call_stream","name":"lookup","arguments":""}})),
            event("response.function_call_arguments.delta", json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_stream","delta":"{\"query\":\"stream\"}"})),
            event("response.function_call_arguments.done", json!({"type":"response.function_call_arguments.done","output_index":0,"item_id":"fc_stream","arguments":"{\"query\":\"stream\"}"})),
            event("response.output_item.done", json!({"type":"response.output_item.done","output_index":0,"item":{"id":"fc_stream","type":"function_call","status":"completed","call_id":"call_stream","name":"lookup","arguments":"{\"query\":\"stream\"}"}})),
            event("response.completed", json!({"type":"response.completed","response":{"id":"resp_stream_tool","object":"response","status":"completed","model":"responses-upstream","output":[{"id":"fc_stream","type":"function_call","status":"completed","call_id":"call_stream","name":"lookup","arguments":"{\"query\":\"stream\"}"}],"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}})),
        ].concat(),
        ("chat", false) => [
            format!("data: {}\n\n", json!({"id":"chatcmpl_stream","object":"chat.completion.chunk","model":"chat-upstream","choices":[{"index":0,"delta":{"role":"assistant","content":"stream-text"},"finish_reason":null}]})),
            format!("data: {}\n\n", json!({"id":"chatcmpl_stream","object":"chat.completion.chunk","model":"chat-upstream","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})),
            format!("data: {}\n\n", json!({"id":"chatcmpl_stream","object":"chat.completion.chunk","model":"chat-upstream","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}})),
            "data: [DONE]\n\n".into(),
        ].concat(),
        ("chat", true) => [
            format!("data: {}\n\n", json!({"id":"chatcmpl_stream_tool","object":"chat.completion.chunk","model":"chat-upstream","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_stream","type":"function","function":{"name":"lookup","arguments":"{\"query\":\"stream\"}"}}]},"finish_reason":null}]})),
            format!("data: {}\n\n", json!({"id":"chatcmpl_stream_tool","object":"chat.completion.chunk","model":"chat-upstream","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]})),
            format!("data: {}\n\n", json!({"id":"chatcmpl_stream_tool","object":"chat.completion.chunk","model":"chat-upstream","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}})),
            "data: [DONE]\n\n".into(),
        ].concat(),
        ("gemini", false) => format!("data: {}\n\n", json!({"responseId":"gemini_stream","modelVersion":"gemini-upstream","candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"stream-text"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5}})),
        ("gemini", true) => format!("data: {}\n\n", json!({"responseId":"gemini_stream_tool","modelVersion":"gemini-upstream","candidates":[{"index":0,"content":{"role":"model","parts":[{"functionCall":{"id":"call_stream","name":"lookup","args":{"query":"stream"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5}})),
        _ => unreachable!(),
    }
}

fn uses_tools(protocol: &str, body: &Value) -> bool {
    match protocol {
        "anthropic" | "chat" => body.get("tools").is_some(),
        "responses" => body.get("tools").is_some(),
        "gemini" => body.get("tools").is_some(),
        _ => false,
    }
}

async fn upstream(protocol: &'static str, calls: Calls, body: Value) -> Response {
    let tool = uses_tools(protocol, &body);
    calls.0.lock().unwrap().push(protocol.to_string());
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(native_stream(protocol, tool)))
        .unwrap()
}

fn provider_protocol(protocol: NativeProtocol) -> (Protocol, ApiKeyPlacement) {
    match protocol {
        NativeProtocol::AnthropicMessages => (Protocol::AnthropicMessages, ApiKeyPlacement::None),
        NativeProtocol::OpenAiResponses => (Protocol::OpenAiResponses, ApiKeyPlacement::None),
        NativeProtocol::OpenAiChat => (Protocol::OpenAiChatCompletions, ApiKeyPlacement::None),
        NativeProtocol::GeminiNative => (Protocol::GeminiNative, ApiKeyPlacement::XApiKey),
    }
}

async fn request_stream(
    client: &reqwest::Client,
    address: std::net::SocketAddr,
    ingress: &str,
    model: &str,
    tool: bool,
) -> String {
    let tool_name =
        json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]});
    let response = match ingress {
        "anthropic" => {
            let mut body = json!({"model":format!("grillforge/{model}"),"max_tokens":64,"stream":true,"messages":[{"role":"user","content":"stream"}]});
            if tool {
                body["tools"] =
                    json!([{"name":"lookup","description":"lookup","input_schema":tool_name}]);
            }
            client
                .post(format!("http://{address}/clients/opencode/v1/messages"))
                .bearer_auth("client-token")
                .json(&body)
                .send()
                .await
                .unwrap()
        }
        "responses" => {
            let mut body = json!({"model":format!("grillforge/{model}"),"stream":true,"store":false,"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"stream"}]}]});
            if tool {
                body["tools"] = json!([{"type":"function","name":"lookup","description":"lookup","parameters":tool_name}]);
            }
            client
                .post(format!("http://{address}/codex/v1/responses"))
                .bearer_auth("codex-token")
                .json(&body)
                .send()
                .await
                .unwrap()
        }
        "chat" => {
            let mut body = json!({"model":format!("grillforge/{model}"),"stream":true,"messages":[{"role":"user","content":"stream"}]});
            if tool {
                body["tools"] = json!([{"type":"function","function":{"name":"lookup","description":"lookup","parameters":tool_name}}]);
            }
            client
                .post(format!(
                    "http://{address}/chat/opencode/v1/chat/completions"
                ))
                .bearer_auth("client-token")
                .json(&body)
                .send()
                .await
                .unwrap()
        }
        "gemini" => {
            let mut body = json!({"contents":[{"role":"user","parts":[{"text":"stream"}]}],"generationConfig":{"maxOutputTokens":64}});
            if tool {
                body["tools"] = json!([{"functionDeclarations":[{"name":"lookup","description":"lookup","parametersJsonSchema":tool_name}]}]);
            }
            client.post(format!("http://{address}/gemini/v1beta/models/grillforge--{model}:streamGenerateContent?alt=sse")).header("x-goog-api-key", "gemini-token").json(&body).send().await.unwrap()
        }
        _ => unreachable!(),
    };
    let status = response.status();
    let output = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{ingress}->{model}: {output}");
    output
}

#[tokio::test]
async fn every_ingress_streams_text_and_tools_through_every_native_protocol() {
    let calls = Calls::default();
    let upstream_router = Router::new()
        .route(
            "/anthropic/v1/messages",
            post(|State(calls): State<Calls>, Json(body): Json<Value>| {
                upstream("anthropic", calls, body)
            }),
        )
        .route(
            "/responses/v1/responses",
            post(|State(calls): State<Calls>, Json(body): Json<Value>| {
                upstream("responses", calls, body)
            }),
        )
        .route(
            "/chat/v1/chat/completions",
            post(|State(calls): State<Calls>, Json(body): Json<Value>| {
                upstream("chat", calls, body)
            }),
        )
        .route(
            "/gemini/v1beta/models/gemini-upstream:streamGenerateContent",
            post(|State(calls): State<Calls>, Json(body): Json<Value>| {
                upstream("gemini", calls, body)
            }),
        )
        .with_state(calls.clone());
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream_router)
            .await
            .unwrap()
    });

    let root = tempfile::tempdir().unwrap();
    let control = ControlPlaneService::new(root.path());
    for (id, native) in [
        ("anthropic", NativeProtocol::AnthropicMessages),
        ("responses", NativeProtocol::OpenAiResponses),
        ("chat", NativeProtocol::OpenAiChat),
        ("gemini", NativeProtocol::GeminiNative),
    ] {
        let (protocol, placement) = provider_protocol(native);
        control
            .save_provider(ProviderInput {
                id: id.into(),
                name: id.into(),
                protocol,
                endpoint: format!("http://{upstream_address}/{id}"),
                endpoint_mode: EndpointMode::BaseUrl,
                api_key_placement: placement,
                api_key: (placement == ApiKeyPlacement::XApiKey).then(|| "secret".into()),
                enabled: true,
                models_url: None,
            })
            .unwrap();
        control
            .save_model(ModelInput {
                id: id.into(),
                name: id.into(),
                upstream_id: format!("{id}-upstream"),
                provider_id: id.into(),
                capabilities: vec![],
                protocol_capabilities: vec![],
                context_window: None,
                max_output_tokens: None,
            })
            .unwrap();
        control
            .set_model_native_protocols(id, vec![native])
            .unwrap();
    }
    let files = ConfigurationFiles::new(root.path());
    let mut documents = files.read().unwrap();
    for provider in &mut documents.config.providers {
        let native = match provider.protocol {
            Protocol::AnthropicMessages => NativeProtocol::AnthropicMessages,
            Protocol::OpenAiResponses => NativeProtocol::OpenAiResponses,
            Protocol::OpenAiChatCompletions => NativeProtocol::OpenAiChat,
            Protocol::GeminiNative => NativeProtocol::GeminiNative,
        };
        provider.protocol_endpoints = vec![ProviderProtocolEndpoint {
            protocol: native,
            endpoint: provider.endpoint.clone(),
            endpoint_mode: provider.endpoint_mode,
            api_key_placement: provider.api_key_placement,
        }];
    }
    files
        .save(&documents.config, &documents.models, &documents.agents)
        .unwrap();

    let gateway = Gateway::new(root.path());
    let models = vec![
        "anthropic".into(),
        "responses".into(),
        "chat".into(),
        "gemini".into(),
    ];
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_client("opencode", models.clone(), "client-token")
        .unwrap();
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_client("gemini", models.clone(), "gemini-token")
        .unwrap();
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_codex(models, "codex-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let client = reqwest::Client::new();
    for ingress in ["anthropic", "responses", "chat", "gemini"] {
        for model in ["anthropic", "responses", "chat", "gemini"] {
            let text = request_stream(&client, address, ingress, model, false).await;
            assert!(text.contains("stream-text"), "{ingress}->{model}: {text}");
            let tool = request_stream(&client, address, ingress, model, true).await;
            let marker = match ingress {
                "anthropic" => "tool_use",
                "responses" => "function_call",
                "chat" => "tool_calls",
                "gemini" => "functionCall",
                _ => unreachable!(),
            };
            assert!(tool.contains(marker), "{ingress}->{model}: {tool}");
            assert!(tool.contains("lookup"), "{ingress}->{model}: {tool}");
        }
    }
    let calls = calls.0.lock().unwrap();
    for protocol in ["anthropic", "responses", "chat", "gemini"] {
        assert_eq!(
            calls
                .iter()
                .filter(|seen| seen.as_str() == protocol)
                .count(),
            8
        );
    }
}
