use grillforge_lib::bridge::{OpenAiChatBridge, OpenAiChatCapabilities};
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

struct CapturedRequest {
    path: String,
    headers: HashMap<String, String>,
    body: Value,
}

async fn serve_once_status(
    status: u16,
    response: Value,
) -> (Url, tokio::task::JoinHandle<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut received = Vec::new();
        let header_end = loop {
            let mut chunk = [0; 1024];
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0);
            received.extend_from_slice(&chunk[..count]);
            if let Some(index) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let text = std::str::from_utf8(&received[..header_end]).unwrap();
        let mut lines = text.split("\r\n");
        let path = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_owned();
        let headers: HashMap<String, String> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let length = headers["content-length"].parse::<usize>().unwrap();
        while received.len() - header_end < length {
            let mut chunk = [0; 1024];
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0);
            received.extend_from_slice(&chunk[..count]);
        }
        let body = serde_json::from_slice(&received[header_end..header_end + length]).unwrap();
        let response = serde_json::to_vec(&response).unwrap();
        let reason = if status == 200 { "OK" } else { "Error" };
        let head = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            response.len()
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(&response).await.unwrap();
        CapturedRequest {
            path,
            headers,
            body,
        }
    });
    (
        Url::parse(&format!("http://{address}/chat-prefix")).unwrap(),
        task,
    )
}

async fn serve_once(response: Value) -> (Url, tokio::task::JoinHandle<CapturedRequest>) {
    serve_once_status(200, response).await
}

#[tokio::test]
async fn text_system_and_parameters_round_trip_over_chat_http() {
    let (base_url, captured) = serve_once(json!({
        "id":"chatcmpl_1","model":"qwen-coder",
        "choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}
    }))
    .await;

    let response = OpenAiChatBridge::new(base_url, "chat-secret")
        .complete(json!({
            "model":"qwen-coder","max_tokens":128,
            "system":"Be precise.",
            "messages":[{"role":"user","content":"ping"}],
            "temperature":0.2,"top_p":0.9,"stop_sequences":["STOP"]
        }))
        .await
        .unwrap();

    assert_eq!(response["content"], json!([{"type":"text","text":"pong"}]));
    assert_eq!(response["stop_reason"], "end_turn");
    assert_eq!(
        response["usage"],
        json!({"input_tokens":3,"output_tokens":1})
    );
    let request = captured.await.unwrap();
    assert_eq!(request.path, "/chat-prefix/v1/chat/completions");
    assert_eq!(request.headers["authorization"], "Bearer chat-secret");
    assert_eq!(
        request.body["messages"][0],
        json!({"role":"system","content":"Be precise."})
    );
    assert_eq!(
        request.body["messages"][1],
        json!({"role":"user","content":"ping"})
    );
    assert_eq!(request.body["max_tokens"], 128);
    assert_eq!(request.body["temperature"], 0.2);
    assert_eq!(request.body["top_p"], 0.9);
    assert_eq!(request.body["stop"], json!(["STOP"]));
}

#[tokio::test]
async fn claude_subagent_system_history_is_preserved_for_chat() {
    let (base_url, captured) = serve_once(json!({
        "id":"chatcmpl_system","model":"local",
        "choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":2,"completion_tokens":1}
    }))
    .await;

    OpenAiChatBridge::new(base_url, "chat-secret")
        .complete(json!({
            "model":"local","max_tokens":32,
            "messages":[
                {"role":"user","content":"delegate"},
                {"role":"system","content":[{"type":"text","text":"You are the selected Worker.","cache_control":{"type":"ephemeral"}}]}
            ]
        }))
        .await
        .expect("Claude SubAgent history");

    assert_eq!(
        captured.await.unwrap().body["messages"][1],
        json!({"role":"system","content":"You are the selected Worker."})
    );
}

#[tokio::test]
async fn tools_and_tool_history_round_trip_without_repair() {
    let (base_url, captured) = serve_once(json!({
        "id":"chatcmpl_tools","model":"qwen-coder",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"call_2","type":"function","function":{"name":"write_file","arguments":"{\"path\":\"a.rs\"}"}}]},
            "finish_reason":"tool_calls"}],
        "usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}
    }))
    .await;

    let response = OpenAiChatBridge::new(base_url, "chat-secret")
        .complete(json!({
            "model":"qwen-coder","max_tokens":128,
            "tools":[{"name":"read_file","description":"Read one file","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}],
            "tool_choice":{"type":"tool","name":"read_file"},
            "messages":[
                {"role":"user","content":"inspect"},
                {"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"read_file","input":{"path":"a.rs"}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"source"}]}
            ]
        }))
        .await
        .unwrap();

    assert_eq!(
        response["content"],
        json!([{
            "type":"tool_use","id":"call_2","name":"write_file","input":{"path":"a.rs"}
        }])
    );
    assert_eq!(response["stop_reason"], "tool_use");
    let request = captured.await.unwrap().body;
    assert_eq!(request["tools"][0]["function"]["name"], "read_file");
    assert_eq!(
        request["tools"][0]["function"]["parameters"]["type"],
        "object"
    );
    assert_eq!(request["tool_choice"]["function"]["name"], "read_file");
    assert_eq!(request["messages"][1]["tool_calls"][0]["id"], "call_1");
    assert_eq!(
        request["messages"][1]["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\"a.rs\"}"
    );
    assert_eq!(
        request["messages"][2],
        json!({
            "role":"tool","tool_call_id":"call_1","content":"source"
        })
    );
}

#[tokio::test]
async fn exact_local_endpoint_can_be_called_without_an_authorization_header() {
    let (endpoint, captured) = serve_once(json!({
        "id":"chatcmpl_local","model":"local-coder",
        "choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":1,"completion_tokens":1}
    }))
    .await;

    OpenAiChatBridge::from_endpoint_without_auth(endpoint)
        .complete(json!({
            "model":"local-coder","max_tokens":32,
            "messages":[{"role":"user","content":"ping"}]
        }))
        .await
        .unwrap();

    assert!(
        !captured
            .await
            .unwrap()
            .headers
            .contains_key("authorization")
    );
}

#[tokio::test]
async fn reasoning_content_is_never_inferred_from_the_model_name() {
    let bridge = OpenAiChatBridge::from_endpoint(
        Url::parse("http://127.0.0.1:1/v1/chat/completions").unwrap(),
        "unused",
    );
    let error = bridge
        .complete(json!({
            "model":"deepseek-reasoner","max_tokens":32,
            "messages":[
                {"role":"user","content":"inspect"},
                {"role":"assistant","content":[
                    {"type":"thinking","thinking":"Need a tool"},
                    {"type":"tool_use","id":"call_1","name":"read_file","input":{"path":"a.rs"}}
                ]}
            ]
        }))
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid Anthropic request: thinking blocks require the provider reasoning_content capability"
    );
}

#[tokio::test]
async fn explicit_reasoning_capability_preserves_request_and_response_fields() {
    let (base_url, captured) = serve_once(json!({
        "id":"chat_reasoning","model":"generic-model",
        "choices":[{"index":0,"message":{"role":"assistant","reasoning_content":"Checked it","content":"done"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":4,"completion_tokens":2}
    }))
    .await;

    let response = OpenAiChatBridge::new(base_url, "chat-secret")
        .with_capabilities(OpenAiChatCapabilities {
            reasoning_content: true,
            reasoning_effort: false,
        })
        .complete(json!({
            "model":"generic-model","max_tokens":32,
            "messages":[
                {"role":"user","content":"inspect"},
                {"role":"assistant","content":[
                    {"type":"thinking","thinking":"Need a tool"},
                    {"type":"tool_use","id":"call_1","name":"read_file","input":{"path":"a.rs"}}
                ]}
            ]
        }))
        .await
        .unwrap();

    assert_eq!(
        captured.await.unwrap().body["messages"][1]["reasoning_content"],
        "Need a tool"
    );
    assert_eq!(
        response["content"][0],
        json!({"type":"thinking","thinking":"Checked it"})
    );
    assert_eq!(response["content"][1], json!({"type":"text","text":"done"}));
}

#[tokio::test]
async fn chat_http_error_is_returned_without_retry_or_false_success() {
    let (base_url, captured) = serve_once_status(
        429,
        json!({"error":{"type":"rate_limit_error","message":"quota exhausted"}}),
    )
    .await;

    let error = OpenAiChatBridge::new(base_url, "chat-secret")
        .complete(json!({
            "model":"qwen","max_tokens":32,
            "messages":[{"role":"user","content":"ping"}]
        }))
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Chat Completions upstream returned HTTP 429"
    );
    captured.await.unwrap();
}

#[tokio::test]
async fn chat_error_envelope_inside_http_200_is_not_a_success() {
    let (base_url, captured) = serve_once(json!({
        "error":{"type":"model_error","message":"model unavailable"}
    }))
    .await;

    let error = OpenAiChatBridge::new(base_url, "chat-secret")
        .complete(json!({
            "model":"qwen","max_tokens":32,
            "messages":[{"role":"user","content":"ping"}]
        }))
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Chat Completions upstream failed (model_error): model unavailable"
    );
    captured.await.unwrap();
}

#[tokio::test]
async fn chat_malformed_base64_image_fails_before_network_access() {
    let bridge = OpenAiChatBridge::from_endpoint(
        Url::parse("http://127.0.0.1:1/v1/chat/completions").unwrap(),
        "unused",
    );
    let error = bridge
        .complete(json!({
            "model":"qwen","max_tokens":32,
            "messages":[{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"abc"}}]}]
        }))
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("data must be valid canonical base64")
    );
}

#[tokio::test]
async fn chat_maps_base64_and_url_images_to_image_url_parts() {
    let (base_url, captured) = serve_once(json!({
        "id":"chat_images","model":"vision-model",
        "choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":5,"completion_tokens":1}
    }))
    .await;

    OpenAiChatBridge::new(base_url, "chat-secret")
        .complete(json!({
            "model":"vision-model","max_tokens":32,
            "messages":[{"role":"user","content":[
                {"type":"text","text":"inspect"},
                {"type":"image","source":{"type":"base64","media_type":"image/webp","data":"aGVsbG8="}},
                {"type":"image","source":{"type":"url","url":"https://example.com/image.png"}}
            ]}]
        }))
        .await
        .unwrap();

    let content = &captured.await.unwrap().body["messages"][0]["content"];
    assert_eq!(
        content[1],
        json!({
            "type":"image_url","image_url":{"url":"data:image/webp;base64,aGVsbG8="}
        })
    );
    assert_eq!(
        content[2],
        json!({
            "type":"image_url","image_url":{"url":"https://example.com/image.png"}
        })
    );
}

#[tokio::test]
async fn chat_tool_result_image_is_rejected_instead_of_detached_or_dropped() {
    let bridge = OpenAiChatBridge::from_endpoint(
        Url::parse("http://127.0.0.1:1/v1/chat/completions").unwrap(),
        "unused",
    );
    let error = bridge
        .complete(json!({
            "model":"vision-model","max_tokens":32,
            "messages":[{"role":"user","content":[{
                "type":"tool_result","tool_use_id":"call_1","content":[
                    {"type":"image","source":{"type":"url","url":"https://example.com/tool.png"}}
                ]
            }]}]
        }))
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid Anthropic request: tool_result images cannot be represented losslessly by Chat Completions"
    );
}

#[tokio::test]
async fn chat_document_is_rejected_instead_of_being_dropped() {
    let bridge = OpenAiChatBridge::from_endpoint(
        Url::parse("http://127.0.0.1:1/v1/chat/completions").unwrap(),
        "unused",
    );
    let error = bridge
        .complete(json!({
            "model":"generic-model","max_tokens":32,
            "messages":[{"role":"user","content":[{
                "type":"document","title":"manual.pdf",
                "source":{"type":"url","url":"https://example.com/manual.pdf"}
            }]}]
        }))
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid Anthropic request: document blocks cannot be represented losslessly by Chat Completions"
    );
}

#[tokio::test]
async fn chat_tool_result_document_is_rejected_instead_of_being_dropped() {
    let bridge = OpenAiChatBridge::from_endpoint(
        Url::parse("http://127.0.0.1:1/v1/chat/completions").unwrap(),
        "unused",
    );
    let error = bridge
        .complete(json!({
            "model":"generic-model","max_tokens":32,
            "messages":[{"role":"user","content":[{
                "type":"tool_result","tool_use_id":"call_1","content":[{
                    "type":"document","title":"trace.pdf",
                    "source":{"type":"base64","media_type":"application/pdf","data":"JVBERi0xLjc="}
                }]
            }]}]
        }))
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid Anthropic request: tool_result documents cannot be represented losslessly by Chat Completions"
    );
}

#[tokio::test]
async fn chat_accepts_real_claude_hints_only_with_explicit_reasoning_effort_capability() {
    let (base_url, captured) = serve_once(json!({
        "id":"chat_hints","model":"generic-model",
        "choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":4,"completion_tokens":1}
    }))
    .await;

    OpenAiChatBridge::new(base_url, "chat-secret")
        .with_capabilities(OpenAiChatCapabilities {
            reasoning_content: false,
            reasoning_effort: true,
        })
        .complete(json!({
            "model":"generic-model","max_tokens":32,
            "metadata":{"user_id":"user_redacted"},
            "context_management":{"edits":[{"keep":"all","type":"clear_thinking_20251015"}]},
            "output_config":{"effort":"high"},
            "thinking":{"display":"omitted","type":"adaptive"},
            "messages":[{"role":"user","content":"inspect"}]
        }))
        .await
        .unwrap();

    let request = captured.await.unwrap().body;
    assert_eq!(request["reasoning_effort"], "high");
    assert!(request.get("metadata").is_none());
    assert!(request.get("context_management").is_none());
}
