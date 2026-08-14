use axum::{Json, Router, extract::State, routing::post};
use grillforge_lib::application::{ControlPlaneService, ModelInput, ProviderInput};
use grillforge_lib::configuration::{ConfigurationFiles, ProviderProtocolEndpoint};
use grillforge_lib::core::model::NativeProtocol;
use grillforge_lib::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use grillforge_lib::gateway::Gateway;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

#[derive(Clone, Default)]
struct Calls(Arc<Mutex<Vec<(String, Value)>>>);

#[tokio::test]
async fn chat_ingress_routes_text_and_tools_to_every_verified_native_protocol() {
    let calls = Calls::default();
    let upstream = Router::new()
        .route(
            "/anthropic/v1/messages",
            post(
                |State(calls): State<Calls>, Json(body): Json<Value>| async move {
                    let tools = body.get("tools").is_some();
                    let has_tool_result = body["messages"]
                        .as_array()
                        .is_some_and(|messages| {
                            messages.iter().any(|message| {
                                message["content"].as_array().is_some_and(|blocks| {
                                    blocks.iter().any(|block| block["type"] == "tool_result")
                                })
                            })
                        });
                    calls.0.lock().unwrap().push(("anthropic".into(), body));
                    Json(if has_tool_result {
                        json!({
                            "id":"msg_anthropic_final","type":"message","role":"assistant","model":"anthropic-upstream",
                            "content":[{"type":"text","text":"anthropic-final"}],
                            "stop_reason":"end_turn","stop_sequence":null,
                            "usage":{"input_tokens":6,"output_tokens":2}
                        })
                    } else if tools {
                        json!({
                            "id":"msg_anthropic","type":"message","role":"assistant","model":"anthropic-upstream",
                            "content":[{"type":"tool_use","id":"call_anthropic","name":"lookup","input":{"query":"anthropic"}}],
                            "stop_reason":"tool_use","stop_sequence":null,
                            "usage":{"input_tokens":4,"output_tokens":2}
                        })
                    } else {
                        json!({
                            "id":"msg_anthropic","type":"message","role":"assistant","model":"anthropic-upstream",
                            "content":[{"type":"text","text":"anthropic-text"}],
                            "stop_reason":"end_turn","stop_sequence":null,
                            "usage":{"input_tokens":4,"output_tokens":2}
                        })
                    })
                },
            ),
        )
        .route(
            "/responses/v1/responses",
            post(
                |State(calls): State<Calls>, Json(body): Json<Value>| async move {
                    let tools = body.get("tools").is_some();
                    let has_tool_result = body["input"].as_array().is_some_and(|items| {
                        items
                            .iter()
                            .any(|item| item["type"] == "function_call_output")
                    });
                    calls.0.lock().unwrap().push(("responses".into(), body));
                    Json(if has_tool_result {
                        json!({
                            "id":"resp_responses_final","object":"response","status":"completed","model":"responses-upstream",
                            "output":[{"id":"message_responses_final","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"responses-final","annotations":[]}]}],
                            "usage":{"input_tokens":6,"output_tokens":2,"total_tokens":8}
                        })
                    } else if tools {
                        json!({
                            "id":"resp_responses","object":"response","status":"completed","model":"responses-upstream",
                            "output":[{"id":"fc_responses","type":"function_call","call_id":"call_responses","name":"lookup","arguments":"{\"query\":\"responses\"}","status":"completed"}],
                            "usage":{"input_tokens":4,"output_tokens":2,"total_tokens":6}
                        })
                    } else {
                        json!({
                            "id":"resp_responses","object":"response","status":"completed","model":"responses-upstream",
                            "output":[{"id":"message_responses","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"responses-text","annotations":[]}]}],
                            "usage":{"input_tokens":4,"output_tokens":2,"total_tokens":6}
                        })
                    })
                },
            ),
        )
        .route(
            "/chat/v1/chat/completions",
            post(
                |State(calls): State<Calls>, Json(body): Json<Value>| async move {
                    let tools = body.get("tools").is_some();
                    let has_tool_result = body["messages"]
                        .as_array()
                        .is_some_and(|messages| messages.iter().any(|message| message["role"] == "tool"));
                    calls.0.lock().unwrap().push(("chat".into(), body));
                    Json(if has_tool_result {
                        json!({
                            "id":"chatcmpl_chat_final","object":"chat.completion","model":"chat-upstream",
                            "choices":[{"index":0,"message":{"role":"assistant","content":"chat-final"},"finish_reason":"stop"}],
                            "usage":{"prompt_tokens":6,"completion_tokens":2,"total_tokens":8}
                        })
                    } else if tools {
                        json!({
                            "id":"chatcmpl_chat","object":"chat.completion","model":"chat-upstream",
                            "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_chat","type":"function","function":{"name":"lookup","arguments":"{\"query\":\"chat\"}"}}]},"finish_reason":"tool_calls"}],
                            "usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}
                        })
                    } else {
                        json!({
                            "id":"chatcmpl_chat","object":"chat.completion","model":"chat-upstream",
                            "choices":[{"index":0,"message":{"role":"assistant","content":"chat-text"},"finish_reason":"stop"}],
                            "usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}
                        })
                    })
                },
            ),
        )
        .route(
            "/gemini/v1beta/models/gemini-upstream:generateContent",
            post(
                |State(calls): State<Calls>, Json(body): Json<Value>| async move {
                    let tools = body.get("tools").is_some();
                    let has_tool_result = body["contents"].as_array().is_some_and(|contents| {
                        contents.iter().any(|content| {
                            content["parts"].as_array().is_some_and(|parts| {
                                parts
                                    .iter()
                                    .any(|part| part.get("functionResponse").is_some())
                            })
                        })
                    });
                    calls.0.lock().unwrap().push(("gemini".into(), body));
                    Json(if has_tool_result {
                        json!({
                            "responseId":"gemini_final","modelVersion":"gemini-upstream",
                            "candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"gemini-final"}]} ,"finishReason":"STOP"}],
                            "usageMetadata":{"promptTokenCount":6,"candidatesTokenCount":2,"totalTokenCount":8}
                        })
                    } else if tools {
                        json!({
                            "responseId":"gemini_tool","modelVersion":"gemini-upstream",
                            "candidates":[{"index":0,"content":{"role":"model","parts":[{"functionCall":{"id":"call_gemini","name":"lookup","args":{"query":"gemini"}}}]},"finishReason":"STOP"}],
                            "usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":2,"totalTokenCount":6}
                        })
                    } else {
                        json!({
                            "responseId":"gemini_text","modelVersion":"gemini-upstream",
                            "candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"gemini-text"}]},"finishReason":"STOP"}],
                            "usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":2,"totalTokenCount":6}
                        })
                    })
                },
            ),
        )
        .with_state(calls.clone());
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let root = tempfile::tempdir().unwrap();
    let control = ControlPlaneService::new(root.path());
    for (id, protocol, placement) in [
        (
            "anthropic",
            Protocol::AnthropicMessages,
            ApiKeyPlacement::None,
        ),
        (
            "responses",
            Protocol::OpenAiResponses,
            ApiKeyPlacement::None,
        ),
        (
            "chat",
            Protocol::OpenAiChatCompletions,
            ApiKeyPlacement::None,
        ),
        ("gemini", Protocol::GeminiNative, ApiKeyPlacement::XApiKey),
    ] {
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
            })
            .unwrap();
        control
            .set_model_native_protocols(
                id,
                vec![match protocol {
                    Protocol::AnthropicMessages => NativeProtocol::AnthropicMessages,
                    Protocol::OpenAiResponses => NativeProtocol::OpenAiResponses,
                    Protocol::OpenAiChatCompletions => NativeProtocol::OpenAiChat,
                    Protocol::GeminiNative => NativeProtocol::GeminiNative,
                }],
            )
            .unwrap();
    }
    let files = ConfigurationFiles::new(root.path());
    let mut documents = files.read().unwrap();
    for provider in &mut documents.config.providers {
        let protocol = match provider.protocol {
            Protocol::AnthropicMessages => NativeProtocol::AnthropicMessages,
            Protocol::OpenAiResponses => NativeProtocol::OpenAiResponses,
            Protocol::OpenAiChatCompletions => NativeProtocol::OpenAiChat,
            Protocol::GeminiNative => NativeProtocol::GeminiNative,
        };
        provider.protocol_endpoints = vec![ProviderProtocolEndpoint {
            protocol,
            endpoint: provider.endpoint.clone(),
            endpoint_mode: provider.endpoint_mode,
            api_key_placement: provider.api_key_placement,
        }];
    }
    files
        .save(&documents.config, &documents.models, &documents.agents)
        .unwrap();

    let gateway = Gateway::new(root.path());
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_client(
            "opencode",
            vec![
                "anthropic".into(),
                "responses".into(),
                "chat".into(),
                "gemini".into(),
            ],
            "client-token",
        )
        .unwrap();
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_client(
            "gemini",
            vec![
                "anthropic".into(),
                "responses".into(),
                "chat".into(),
                "gemini".into(),
            ],
            "gemini-token",
        )
        .unwrap();
    gateway
        .status("http://127.0.0.1:1".into())
        .activate_codex(
            vec![
                "anthropic".into(),
                "responses".into(),
                "chat".into(),
                "gemini".into(),
            ],
            "codex-token",
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, gateway.router()).await.unwrap() });

    let client = reqwest::Client::new();
    for model in ["anthropic", "responses", "chat", "gemini"] {
        let mut chat_text_body = json!({
            "model":format!("grillforge/{model}"),
            "messages":[{"role":"user","content":"say text"}],
            "max_tokens":64
        });
        if model == "chat" {
            chat_text_body["service_tier"] = json!("auto");
        }
        let text = client
            .post(format!(
                "http://{address}/chat/opencode/v1/chat/completions"
            ))
            .bearer_auth("client-token")
            .json(&chat_text_body)
            .send()
            .await
            .unwrap();
        let status = text.status();
        let text: Value = text.json().await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{model}: {text}");
        assert_eq!(
            text["choices"][0]["message"]["content"],
            json!(format!("{model}-text")),
            "{model}: {text}"
        );

        let tool = client
            .post(format!(
                "http://{address}/chat/opencode/v1/chat/completions"
            ))
            .bearer_auth("client-token")
            .json(&json!({
                "model":format!("grillforge/{model}"),
                "messages":[{"role":"user","content":"use lookup"}],
                "max_tokens":64,
                "tools":[{"type":"function","function":{"name":"lookup","description":"lookup","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}}]
            }))
            .send()
            .await
            .unwrap();
        let status = tool.status();
        let tool: Value = tool.json().await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{model}: {tool}");
        assert_eq!(tool["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            tool["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "lookup"
        );
        let chat_final = client
            .post(format!(
                "http://{address}/chat/opencode/v1/chat/completions"
            ))
            .bearer_auth("client-token")
            .json(&json!({
                "model":format!("grillforge/{model}"),
                "messages":[
                    {"role":"user","content":"use lookup"},
                    {"role":"assistant","content":null,"tool_calls":tool["choices"][0]["message"]["tool_calls"].clone()},
                    {"role":"tool","tool_call_id":tool["choices"][0]["message"]["tool_calls"][0]["id"].clone(),"content":"lookup-result"}
                ],
                "max_tokens":64,
                "tools":[{"type":"function","function":{"name":"lookup","description":"lookup","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}}]
            }))
            .send()
            .await
            .unwrap();
        let status = chat_final.status();
        let chat_final: Value = chat_final.json().await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{model}: {chat_final}");
        assert_eq!(
            chat_final["choices"][0]["message"]["content"],
            json!(format!("{model}-final"))
        );

        let anthropic_text = client
            .post(format!("http://{address}/clients/opencode/v1/messages"))
            .bearer_auth("client-token")
            .json(&json!({
                "model":format!("grillforge/{model}"),
                "max_tokens":64,
                "messages":[{"role":"user","content":"say text"}]
            }))
            .send()
            .await
            .unwrap();
        let status = anthropic_text.status();
        let anthropic_text: Value = anthropic_text.json().await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{model}: {anthropic_text}");
        assert_eq!(
            anthropic_text["content"][0]["text"],
            json!(format!("{model}-text"))
        );
        let anthropic_tool = client
            .post(format!("http://{address}/clients/opencode/v1/messages"))
            .bearer_auth("client-token")
            .json(&json!({
                "model":format!("grillforge/{model}"),
                "max_tokens":64,
                "messages":[{"role":"user","content":"use lookup"}],
                "tools":[{"name":"lookup","description":"lookup","input_schema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}]
            }))
            .send()
            .await
            .unwrap();
        let status = anthropic_tool.status();
        let anthropic_tool: Value = anthropic_tool.json().await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{model}: {anthropic_tool}");
        assert_eq!(anthropic_tool["stop_reason"], "tool_use");
        assert_eq!(anthropic_tool["content"][0]["name"], "lookup");
        let anthropic_final = client
            .post(format!("http://{address}/clients/opencode/v1/messages"))
            .bearer_auth("client-token")
            .json(&json!({
                "model":format!("grillforge/{model}"),
                "max_tokens":64,
                "messages":[
                    {"role":"user","content":"use lookup"},
                    {"role":"assistant","content":anthropic_tool["content"].clone()},
                    {"role":"user","content":[{"type":"tool_result","tool_use_id":anthropic_tool["content"][0]["id"].clone(),"content":"lookup-result"}]}
                ],
                "tools":[{"name":"lookup","description":"lookup","input_schema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}]
            }))
            .send()
            .await
            .unwrap();
        let status = anthropic_final.status();
        let anthropic_final: Value = anthropic_final.json().await.unwrap();
        assert_eq!(
            status,
            reqwest::StatusCode::OK,
            "{model}: {anthropic_final}"
        );
        assert_eq!(
            anthropic_final["content"][0]["text"],
            json!(format!("{model}-final"))
        );

        let responses_text = client
            .post(format!("http://{address}/codex/v1/responses"))
            .bearer_auth("codex-token")
            .json(&json!({
                "model":format!("grillforge/{model}"),
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"say text"}]}],
                "stream":false,"store":false
            }))
            .send()
            .await
            .unwrap();
        let status = responses_text.status();
        let responses_text: Value = responses_text.json().await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{model}: {responses_text}");
        assert_eq!(
            responses_text["output"][0]["content"][0]["text"],
            json!(format!("{model}-text"))
        );
        let responses_tool = client
            .post(format!("http://{address}/codex/v1/responses"))
            .bearer_auth("codex-token")
            .json(&json!({
                "model":format!("grillforge/{model}"),
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"use lookup"}]}],
                "tools":[{"type":"function","name":"lookup","description":"lookup","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}],
                "stream":false,"store":false
            }))
            .send()
            .await
            .unwrap();
        let status = responses_tool.status();
        let responses_tool: Value = responses_tool.json().await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{model}: {responses_tool}");
        assert_eq!(responses_tool["output"][0]["type"], "function_call");
        assert_eq!(responses_tool["output"][0]["name"], "lookup");
        let responses_final = client
            .post(format!("http://{address}/codex/v1/responses"))
            .bearer_auth("codex-token")
            .json(&json!({
                "model":format!("grillforge/{model}"),
                "input":[
                    {"type":"message","role":"user","content":[{"type":"input_text","text":"use lookup"}]},
                    responses_tool["output"][0].clone(),
                    {"type":"function_call_output","call_id":responses_tool["output"][0]["call_id"].clone(),"output":"lookup-result"}
                ],
                "tools":[{"type":"function","name":"lookup","description":"lookup","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}],
                "stream":false,"store":false
            }))
            .send()
            .await
            .unwrap();
        let status = responses_final.status();
        let responses_final: Value = responses_final.json().await.unwrap();
        assert_eq!(
            status,
            reqwest::StatusCode::OK,
            "{model}: {responses_final}"
        );
        assert_eq!(
            responses_final["output"][0]["content"][0]["text"],
            json!(format!("{model}-final"))
        );

        let mut gemini_text_body = json!({
            "contents":[{"role":"user","parts":[{"text":"say text"}]}],
            "generationConfig":{"maxOutputTokens":64}
        });
        if model == "gemini" {
            gemini_text_body["cachedContent"] = json!("cachedContents/direct-test");
        }
        let gemini_text = client
            .post(format!(
                "http://{address}/gemini/v1beta/models/grillforge--{model}:generateContent"
            ))
            .header("x-goog-api-key", "gemini-token")
            .json(&gemini_text_body)
            .send()
            .await
            .unwrap();
        let status = gemini_text.status();
        let gemini_text: Value = gemini_text.json().await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{model}: {gemini_text}");
        assert_eq!(
            gemini_text["candidates"][0]["content"]["parts"][0]["text"],
            json!(format!("{model}-text"))
        );
        let gemini_tool = client
            .post(format!(
                "http://{address}/gemini/v1beta/models/grillforge--{model}:generateContent"
            ))
            .header("x-goog-api-key", "gemini-token")
            .json(&json!({
                "contents":[{"role":"user","parts":[{"text":"use lookup"}]}],
                "tools":[{"functionDeclarations":[{"name":"lookup","description":"lookup","parametersJsonSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}]}],
                "generationConfig":{"maxOutputTokens":64}
            }))
            .send()
            .await
            .unwrap();
        let status = gemini_tool.status();
        let gemini_tool: Value = gemini_tool.json().await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{model}: {gemini_tool}");
        assert_eq!(
            gemini_tool["candidates"][0]["content"]["parts"][0]["functionCall"]["name"],
            "lookup"
        );
        let function_call =
            gemini_tool["candidates"][0]["content"]["parts"][0]["functionCall"].clone();
        let gemini_final = client
            .post(format!(
                "http://{address}/gemini/v1beta/models/grillforge--{model}:generateContent"
            ))
            .header("x-goog-api-key", "gemini-token")
            .json(&json!({
                "contents":[
                    {"role":"user","parts":[{"text":"use lookup"}]},
                    {"role":"model","parts":[{"functionCall":function_call.clone()}]},
                    {"role":"user","parts":[{"functionResponse":{"id":function_call["id"].clone(),"name":"lookup","response":{"content":"lookup-result"}}}]}
                ],
                "tools":[{"functionDeclarations":[{"name":"lookup","description":"lookup","parametersJsonSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}]}],
                "generationConfig":{"maxOutputTokens":64}
            }))
            .send()
            .await
            .unwrap();
        let status = gemini_final.status();
        let gemini_final: Value = gemini_final.json().await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{model}: {gemini_final}");
        assert_eq!(
            gemini_final["candidates"][0]["content"]["parts"][0]["text"],
            json!(format!("{model}-final"))
        );
    }

    let calls = calls.0.lock().unwrap();
    for protocol in ["anthropic", "responses", "chat", "gemini"] {
        assert_eq!(
            calls.iter().filter(|(seen, _)| seen == protocol).count(),
            12,
            "{protocol} must receive text, tool, and tool-result continuation requests from all four ingress protocols"
        );
    }
}
