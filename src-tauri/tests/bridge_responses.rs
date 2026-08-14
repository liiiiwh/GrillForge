use grillforge_lib::bridge::{OpenAiResponsesBridge, OpenAiResponsesCapabilities};
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

async fn serve_once(
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
            assert!(count > 0, "request ended before its headers");
            received.extend_from_slice(&chunk[..count]);
            if let Some(position) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break position + 4;
            }
        };

        let header_text = std::str::from_utf8(&received[..header_end]).unwrap();
        let mut lines = header_text.split("\r\n");
        let request_line = lines.next().unwrap();
        let path = request_line.split_whitespace().nth(1).unwrap().to_string();
        let headers: HashMap<String, String> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        let content_length = headers
            .get("content-length")
            .unwrap()
            .parse::<usize>()
            .unwrap();

        while received.len() - header_end < content_length {
            let mut chunk = [0; 1024];
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0, "request ended before its body");
            received.extend_from_slice(&chunk[..count]);
        }
        let body =
            serde_json::from_slice(&received[header_end..header_end + content_length]).unwrap();

        let response_body = serde_json::to_vec(&response).unwrap();
        let reason = if status == 200 { "OK" } else { "Error" };
        let response_head = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            response_body.len()
        );
        socket.write_all(response_head.as_bytes()).await.unwrap();
        socket.write_all(&response_body).await.unwrap();

        CapturedRequest {
            path,
            headers,
            body,
        }
    });

    (Url::parse(&format!("http://{address}")).unwrap(), task)
}

fn valid_request() -> Value {
    json!({
        "model": "gpt-5-codex",
        "max_tokens": 128,
        "messages": [{"role": "user", "content": "ping"}]
    })
}

fn valid_response() -> Value {
    json!({
        "id": "resp_1",
        "status": "completed",
        "model": "gpt-5-codex",
        "output": [{"type":"message","content":[{"type":"output_text","text":"pong"}]}],
        "usage": {"input_tokens": 3, "output_tokens": 1}
    })
}

#[tokio::test]
async fn empty_claude_tool_list_is_omitted_from_responses_request() {
    let (base_url, captured) = serve_once(200, valid_response()).await;
    let mut request = valid_request();
    request["tools"] = json!([]);

    OpenAiResponsesBridge::new(base_url, "responses-secret")
        .complete(request)
        .await
        .expect("an empty Claude tool list is valid");

    let captured = captured.await.unwrap();
    assert!(captured.body.get("tools").is_none());
}

#[tokio::test]
async fn claude_subagent_system_history_is_preserved_as_a_system_input_message() {
    let (base_url, captured) = serve_once(200, valid_response()).await;
    let mut request = valid_request();
    request["messages"] = json!([
        {"role":"user","content":"delegate"},
        {"role":"system","content":[{"type":"text","text":"You are the selected Worker.","cache_control":{"type":"ephemeral"}}]}
    ]);

    OpenAiResponsesBridge::new(base_url, "test-secret")
        .complete(request)
        .await
        .expect("Claude SubAgent history");

    let body = captured.await.unwrap().body;
    assert_eq!(body["input"][1]["role"], "system");
    assert_eq!(body["input"][1]["content"][0]["type"], "input_text");
}

#[tokio::test]
async fn base_url_preserves_custom_prefix_and_appends_responses_path() {
    let (mut base_url, captured) = serve_once(200, valid_response()).await;
    base_url.set_path("/openai");

    OpenAiResponsesBridge::new(base_url, "test-secret")
        .complete(valid_request())
        .await
        .unwrap();

    assert_eq!(captured.await.unwrap().path, "/openai/v1/responses");
}

#[tokio::test]
async fn base_url_deduplicates_adjacent_v1_segments() {
    let (mut base_url, captured) = serve_once(200, valid_response()).await;
    base_url.set_path("/openai/v1/v1");

    OpenAiResponsesBridge::new(base_url, "test-secret")
        .complete(valid_request())
        .await
        .unwrap();

    assert_eq!(captured.await.unwrap().path, "/openai/v1/responses");
}

#[tokio::test]
async fn exact_endpoint_is_used_without_rewriting() {
    let (mut endpoint, captured) = serve_once(200, valid_response()).await;
    endpoint.set_path("/gateway/custom-response-route");

    OpenAiResponsesBridge::from_endpoint(endpoint, "test-secret")
        .complete(valid_request())
        .await
        .unwrap();

    assert_eq!(
        captured.await.unwrap().path,
        "/gateway/custom-response-route"
    );
}

#[tokio::test]
async fn anthropic_text_request_reaches_responses_endpoint_and_returns_anthropic_message() {
    let (base_url, captured) = serve_once(
        200,
        json!({
            "id": "resp_1",
            "status": "completed",
            "model": "gpt-5-codex",
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "pong"}]
            }],
            "usage": {"input_tokens": 3, "output_tokens": 1}
        }),
    )
    .await;
    let bridge = OpenAiResponsesBridge::new(base_url, "test-secret");

    let response = bridge
        .complete(json!({
            "model": "gpt-5-codex",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .await
        .expect("valid Responses request");

    assert_eq!(
        response,
        json!({
            "id": "resp_1",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "pong"}],
            "model": "gpt-5-codex",
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {"input_tokens": 3, "output_tokens": 1}
        })
    );

    let request = captured.await.unwrap();
    assert_eq!(request.path, "/v1/responses");
    assert_eq!(
        request.headers.get("authorization").unwrap(),
        "Bearer test-secret"
    );
    assert_eq!(
        request.body,
        json!({
            "model": "gpt-5-codex",
            "max_output_tokens": 128,
            "input": [{
                "role": "user",
                "content": [{"type": "input_text", "text": "ping"}]
            }]
        })
    );
}

#[tokio::test]
async fn system_and_text_blocks_are_preserved_in_responses_request() {
    let (base_url, captured) = serve_once(
        200,
        json!({
            "id": "resp_blocks",
            "status": "completed",
            "model": "gpt-5-codex",
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "done"}]
            }],
            "usage": {"input_tokens": 8, "output_tokens": 1}
        }),
    )
    .await;
    let bridge = OpenAiResponsesBridge::new(base_url, "test-secret");

    bridge
        .complete(json!({
            "model": "gpt-5-codex",
            "max_tokens": 128,
            "system": [
                {"type": "text", "text": "You are concise."},
                {"type": "text", "text": "Answer exactly."}
            ],
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "first"},
                    {"type": "text", "text": "second"}
                ]
            }]
        }))
        .await
        .expect("system and text blocks are supported");

    assert_eq!(
        captured.await.unwrap().body,
        json!({
            "model": "gpt-5-codex",
            "max_output_tokens": 128,
            "instructions": "You are concise.\n\nAnswer exactly.",
            "input": [{
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "first"},
                    {"type": "input_text", "text": "second"}
                ]
            }]
        })
    );
}

#[tokio::test]
async fn tool_definitions_and_choice_are_preserved_in_responses_request() {
    let (base_url, captured) = serve_once(
        200,
        json!({
            "id": "resp_tools",
            "status": "completed",
            "model": "gpt-5-codex",
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "checking"}]
            }],
            "usage": {"input_tokens": 12, "output_tokens": 1}
        }),
    )
    .await;
    let bridge = OpenAiResponsesBridge::new(base_url, "test-secret");

    bridge
        .complete(json!({
            "model": "gpt-5-codex",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": "read it"}],
            "tools": [{
                "name": "Read",
                "description": "Read one file",
                "input_schema": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                }
            }],
            "tool_choice": {"type": "any"}
        }))
        .await
        .expect("tool declaration is supported");

    let body = captured.await.unwrap().body;
    assert_eq!(
        body["tools"],
        json!([{
            "type": "function",
            "name": "Read",
            "description": "Read one file",
            "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }
        }])
    );
    assert_eq!(body["tool_choice"], json!("required"));
}

#[tokio::test]
async fn tool_choice_without_tools_fails_before_network_access() {
    let bridge =
        OpenAiResponsesBridge::new(Url::parse("http://127.0.0.1:1").unwrap(), "test-secret");

    let error = bridge
        .complete(json!({
            "model": "gpt-5-codex",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": "ping"}],
            "tool_choice": {"type": "any"}
        }))
        .await
        .expect_err("tool_choice without tools must fail before network access");

    assert_eq!(
        error.to_string(),
        "invalid Anthropic request: tool_choice requires a non-empty tools array"
    );
}

#[tokio::test]
async fn tool_history_and_function_call_round_trip_without_agent_specific_repair() {
    let (base_url, captured) = serve_once(
        200,
        json!({
            "id": "resp_tool_call",
            "status": "completed",
            "model": "gpt-5-codex",
            "output": [{
                "type": "function_call",
                "call_id": "call_2",
                "name": "Read",
                "arguments": "{\"path\":\"src/lib.rs\"}"
            }],
            "usage": {"input_tokens": 20, "output_tokens": 4}
        }),
    )
    .await;
    let bridge = OpenAiResponsesBridge::new(base_url, "test-secret");

    let response = bridge
        .complete(json!({
            "model": "gpt-5-codex",
            "max_tokens": 128,
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {"type": "text", "text": "I need a file."},
                        {
                            "type": "tool_use",
                            "id": "call_1",
                            "name": "Read",
                            "input": {"path": "README.md"}
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "call_1",
                        "content": "README contents"
                    }]
                }
            ]
        }))
        .await
        .expect("tool history is supported");

    assert_eq!(
        captured.await.unwrap().body["input"],
        json!([
            {
                "role": "assistant",
                "content": [{"type": "output_text", "text": "I need a file."}]
            },
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "Read",
                "arguments": "{\"path\":\"README.md\"}"
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "README contents"
            }
        ])
    );
    assert_eq!(
        response["content"],
        json!([{
            "type": "tool_use",
            "id": "call_2",
            "name": "Read",
            "input": {"path": "src/lib.rs"}
        }])
    );
    assert_eq!(response["stop_reason"], json!("tool_use"));
}

#[tokio::test]
async fn responses_usage_is_split_into_anthropic_cache_buckets() {
    let (base_url, _) = serve_once(
        200,
        json!({
            "id": "resp_usage",
            "status": "completed",
            "model": "gpt-5-codex",
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "done"}]
            }],
            "usage": {
                "input_tokens": 20,
                "output_tokens": 3,
                "input_tokens_details": {
                    "cached_tokens": 4,
                    "cache_write_tokens": 3
                }
            }
        }),
    )
    .await;
    let bridge = OpenAiResponsesBridge::new(base_url, "test-secret");

    let response = bridge
        .complete(json!({
            "model": "gpt-5-codex",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .await
        .expect("usage is valid");

    assert_eq!(
        response["usage"],
        json!({
            "input_tokens": 13,
            "output_tokens": 3,
            "cache_read_input_tokens": 4,
            "cache_creation_input_tokens": 3
        })
    );
}

#[tokio::test]
async fn failed_responses_envelope_preserves_a_safe_upstream_error() {
    let (base_url, _) = serve_once(
        200,
        json!({
            "id": "resp_failed",
            "status": "failed",
            "error": {
                "type": "rate_limit_error",
                "message": "quota exhausted\nretry later"
            },
            "output": []
        }),
    )
    .await;
    let bridge = OpenAiResponsesBridge::new(base_url, "test-secret");

    let error = bridge
        .complete(json!({
            "model": "gpt-5-codex",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .await
        .expect_err("failed envelope must not become an empty success");

    assert_eq!(
        error.to_string(),
        "Responses upstream failed (rate_limit_error): quota exhausted retry later"
    );
}

#[tokio::test]
async fn responses_error_envelope_wins_over_a_success_like_status() {
    let (base_url, _) = serve_once(
        200,
        json!({
            "id": "resp_error",
            "status": "completed",
            "error": {
                "code": "server_error",
                "message": "backend unavailable"
            },
            "output": [],
            "usage": {"input_tokens": 1, "output_tokens": 0}
        }),
    )
    .await;
    let bridge = OpenAiResponsesBridge::new(base_url, "test-secret");

    let error = bridge
        .complete(json!({
            "model": "gpt-5-codex",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .await
        .expect_err("an error envelope must win over status");

    assert_eq!(
        error.to_string(),
        "Responses upstream failed (server_error): backend unavailable"
    );
}

#[tokio::test]
async fn unsupported_anthropic_fields_fail_instead_of_being_dropped() {
    let bridge =
        OpenAiResponsesBridge::new(Url::parse("http://127.0.0.1:1").unwrap(), "test-secret");

    let error = bridge
        .complete(json!({
            "model": "gpt-5-codex",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": "ping"}],
            "temperature": 0.2
        }))
        .await
        .expect_err("unsupported fields must fail before network access");

    assert_eq!(
        error.to_string(),
        "invalid Anthropic request: unsupported field: temperature"
    );
}

#[tokio::test]
async fn unsupported_message_fields_fail_instead_of_being_dropped() {
    let bridge =
        OpenAiResponsesBridge::new(Url::parse("http://127.0.0.1:1").unwrap(), "test-secret");

    let error = bridge
        .complete(json!({
            "model": "gpt-5-codex",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": "ping", "name": "ignored"}]
        }))
        .await
        .expect_err("unsupported message fields must fail before network access");

    assert_eq!(
        error.to_string(),
        "invalid Anthropic request: unsupported field: messages[0].name"
    );
}

#[tokio::test]
async fn upstream_401_is_returned_without_retry_or_false_success() {
    let (base_url, captured) = serve_once(
        401,
        json!({"error": {"type": "authentication_error", "message": "invalid key"}}),
    )
    .await;
    let bridge = OpenAiResponsesBridge::new(base_url, "test-secret");

    let error = bridge
        .complete(json!({
            "model": "gpt-5-codex",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .await
        .expect_err("401 must not become an Anthropic success response");

    assert_eq!(
        error.to_string(),
        "Responses upstream failed (authentication_error): invalid key"
    );
    assert_eq!(captured.await.unwrap().path, "/v1/responses");
}

#[tokio::test]
async fn upstream_429_is_returned_without_retry_or_false_success() {
    let (base_url, captured) = serve_once(
        429,
        json!({"error": {"type": "rate_limit_error", "message": "quota exhausted"}}),
    )
    .await;
    let bridge = OpenAiResponsesBridge::new(base_url, "test-secret");

    let error = bridge
        .complete(json!({
            "model": "gpt-5-codex",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .await
        .expect_err("429 must not become an Anthropic success response");

    assert_eq!(
        error.to_string(),
        "Responses upstream failed (rate_limit_error): quota exhausted"
    );
    assert_eq!(captured.await.unwrap().path, "/v1/responses");
}

#[tokio::test]
async fn upstream_model_rollout_error_is_preserved_as_the_actionable_cause() {
    let (base_url, captured) = serve_once(
        400,
        json!({"error": {
            "type": "invalid_request_error",
            "message": "Codex integration with deepseek-v4-pro is not available yet; use deepseek-v4-flash"
        }}),
    )
    .await;
    let bridge = OpenAiResponsesBridge::new(base_url, "test-secret");

    let error = bridge
        .complete(json!({
            "model": "deepseek-v4-pro",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .await
        .expect_err("model rollout errors must remain actionable");

    assert_eq!(
        error.to_string(),
        "Responses upstream failed (invalid_request_error): Codex integration with deepseek-v4-pro is not available yet; use deepseek-v4-flash"
    );
    assert_eq!(captured.await.unwrap().path, "/v1/responses");
}

#[tokio::test]
async fn exact_local_responses_endpoint_can_omit_authorization() {
    let (endpoint, captured) = serve_once(200, valid_response()).await;

    OpenAiResponsesBridge::from_endpoint_without_auth(endpoint)
        .complete(valid_request())
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
async fn responses_maps_base64_and_url_images_without_changing_the_source() {
    let (base_url, captured) = serve_once(200, valid_response()).await;

    OpenAiResponsesBridge::new(base_url, "test-secret")
        .complete(json!({
            "model":"gpt-5-codex","max_tokens":64,
            "messages":[{"role":"user","content":[
                {"type":"text","text":"inspect"},
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}},
                {"type":"image","source":{"type":"url","url":"https://example.com/image.webp?size=2"}}
            ]}]
        }))
        .await
        .unwrap();

    let content = &captured.await.unwrap().body["input"][0]["content"];
    assert_eq!(
        content[1],
        json!({
            "type":"input_image","image_url":"data:image/png;base64,aGVsbG8="
        })
    );
    assert_eq!(
        content[2],
        json!({
            "type":"input_image","image_url":"https://example.com/image.webp?size=2"
        })
    );
}

#[tokio::test]
async fn responses_tool_result_preserves_supported_image_parts() {
    let (base_url, captured) = serve_once(200, valid_response()).await;

    OpenAiResponsesBridge::new(base_url, "test-secret")
        .complete(json!({
            "model":"gpt-5-codex","max_tokens":64,
            "messages":[{"role":"user","content":[{
                "type":"tool_result","tool_use_id":"call_1","content":[
                    {"type":"text","text":"screenshot"},
                    {"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"aGVsbG8="}}
                ]
            }]}]
        }))
        .await
        .unwrap();

    assert_eq!(
        captured.await.unwrap().body["input"][0]["output"],
        json!([
            {"type":"input_text","text":"screenshot"},
            {"type":"input_image","image_url":"data:image/jpeg;base64,aGVsbG8="}
        ])
    );
}

#[tokio::test]
async fn malformed_image_source_fails_before_network_access() {
    let bridge = OpenAiResponsesBridge::new(Url::parse("http://127.0.0.1:1").unwrap(), "unused");
    let error = bridge
        .complete(json!({
            "model":"gpt-5-codex","max_tokens":64,
            "messages":[{"role":"user","content":[{
                "type":"image","source":{"type":"base64","media_type":"image/svg+xml","data":"not base64"}
            }]}]
        }))
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("media_type must be image/jpeg, image/png, image/gif, or image/webp")
    );
}

#[tokio::test]
async fn responses_maps_base64_and_url_documents_to_input_file() {
    let (base_url, captured) = serve_once(200, valid_response()).await;

    OpenAiResponsesBridge::new(base_url, "test-secret")
        .complete(json!({
            "model":"gpt-5-codex","max_tokens":64,
            "messages":[{"role":"user","content":[
                {"type":"document","title":"trace.pdf","source":{"type":"base64","media_type":"application/pdf","data":"JVBERi0xLjc="}},
                {"type":"document","filename":"manual.pdf","source":{"type":"url","url":"https://example.com/manual.pdf"}}
            ]}]
        }))
        .await
        .unwrap();

    let content = &captured.await.unwrap().body["input"][0]["content"];
    assert_eq!(
        content[0],
        json!({
            "type":"input_file","filename":"trace.pdf",
            "file_data":"data:application/pdf;base64,JVBERi0xLjc="
        })
    );
    assert_eq!(
        content[1],
        json!({
            "type":"input_file","filename":"manual.pdf",
            "file_url":"https://example.com/manual.pdf"
        })
    );
}

#[tokio::test]
async fn responses_tool_result_preserves_supported_document_parts() {
    let (base_url, captured) = serve_once(200, valid_response()).await;

    OpenAiResponsesBridge::new(base_url, "test-secret")
        .complete(json!({
            "model":"gpt-5-codex","max_tokens":64,
            "messages":[{"role":"user","content":[{
                "type":"tool_result","tool_use_id":"call_1","content":[
                    {"type":"text","text":"trace"},
                    {"type":"document","title":"trace.pdf","source":{"type":"base64","media_type":"application/pdf","data":"JVBERi0xLjc="}}
                ]
            }]}]
        }))
        .await
        .unwrap();

    assert_eq!(
        captured.await.unwrap().body["input"][0]["output"],
        json!([
            {"type":"input_text","text":"trace"},
            {"type":"input_file","filename":"trace.pdf","file_data":"data:application/pdf;base64,JVBERi0xLjc="}
        ])
    );
}

#[tokio::test]
async fn malformed_document_source_fails_before_network_access() {
    let bridge = OpenAiResponsesBridge::new(Url::parse("http://127.0.0.1:1").unwrap(), "unused");
    let error = bridge
        .complete(json!({
            "model":"gpt-5-codex","max_tokens":64,
            "messages":[{"role":"user","content":[{
                "type":"document","title":"fake.pdf",
                "source":{"type":"base64","media_type":"text/plain","data":"aGVsbG8="}
            }]}]
        }))
        .await
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("media_type must be application/pdf")
    );
}

#[tokio::test]
async fn encrypted_responses_reasoning_round_trips_through_an_opaque_signature() {
    let reasoning_item = json!({
        "id":"rs_1","type":"reasoning","status":"completed",
        "summary":[{"type":"summary_text","text":"Need a tool."}],
        "encrypted_content":"opaque-ciphertext"
    });
    let (base_url, first_request) = serve_once(200, json!({
        "id":"resp_reasoning","status":"completed","model":"gpt-5",
        "output":[
            reasoning_item.clone(),
            {"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"a.rs\"}"}
        ],
        "usage":{"input_tokens":5,"output_tokens":2}
    }))
    .await;
    let capabilities = OpenAiResponsesCapabilities {
        reasoning_items: true,
    };
    let response = OpenAiResponsesBridge::new(base_url, "test-secret")
        .with_capabilities(capabilities)
        .complete(valid_request())
        .await
        .unwrap();
    first_request.await.unwrap();

    assert_eq!(response["content"][0]["type"], "thinking");
    assert_eq!(response["content"][0]["thinking"], "Need a tool.");
    let signature = response["content"][0]["signature"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(signature.starts_with("grillforge-openai-reasoning-v1:"));
    assert!(!signature.contains("opaque-ciphertext"));

    let (base_url, replayed) = serve_once(200, valid_response()).await;
    OpenAiResponsesBridge::new(base_url, "test-secret")
        .with_capabilities(capabilities)
        .complete(json!({
            "model":"gpt-5","max_tokens":64,
            "messages":[
                {"role":"user","content":"inspect"},
                {"role":"assistant","content":[
                    {"type":"thinking","thinking":"Need a tool.","signature":signature},
                    {"type":"tool_use","id":"call_1","name":"read_file","input":{"path":"a.rs"}}
                ]}
            ]
        }))
        .await
        .unwrap();

    let input = replayed.await.unwrap().body["input"].clone();
    assert_eq!(input[1], reasoning_item);
    assert_eq!(input[2]["type"], "function_call");
}

#[tokio::test]
async fn null_encrypted_reasoning_keeps_its_summary_without_a_signature() {
    let (base_url, captured) = serve_once(
        200,
        json!({
            "id":"resp_k3","status":"completed","model":"k3",
            "output":[
                {
                    "id":"rs_k3","type":"reasoning","status":"completed",
                    "summary":[{"type":"summary_text","text":"Checked the request."}],
                    "encrypted_content":null
                },
                {
                    "type":"message","role":"assistant","status":"completed",
                    "content":[{"type":"output_text","text":"OK","annotations":[]}]
                }
            ],
            "usage":{"input_tokens":2,"output_tokens":1}
        }),
    )
    .await;

    let response = OpenAiResponsesBridge::new(base_url, "test-secret")
        .with_capabilities(OpenAiResponsesCapabilities {
            reasoning_items: true,
        })
        .complete(valid_request())
        .await
        .expect("null encrypted_content is an absent optional value");
    captured.await.unwrap();

    assert_eq!(response["content"][0]["type"], "thinking");
    assert_eq!(response["content"][0]["thinking"], "Checked the request.");
    assert!(response["content"][0].get("signature").is_none());
    assert_eq!(response["content"][1]["text"], "OK");
}

#[tokio::test]
async fn non_string_encrypted_reasoning_is_rejected() {
    let (base_url, captured) = serve_once(
        200,
        json!({
            "id":"resp_bad","status":"completed","model":"k3",
            "output":[{
                "id":"rs_bad","type":"reasoning","status":"completed",
                "summary":[{"type":"summary_text","text":"Checked."}],
                "encrypted_content":{"unexpected":true}
            }],
            "usage":{"input_tokens":2,"output_tokens":1}
        }),
    )
    .await;

    let error = OpenAiResponsesBridge::new(base_url, "test-secret")
        .with_capabilities(OpenAiResponsesCapabilities {
            reasoning_items: true,
        })
        .complete(valid_request())
        .await
        .unwrap_err();
    captured.await.unwrap();

    assert!(
        error
            .to_string()
            .contains("reasoning item.encrypted_content must be a string or null")
    );
}

#[tokio::test]
async fn deepseek_reasoning_content_is_kept_opaque_and_replayable() {
    let reasoning_item = json!({
        "id":"rs_deepseek","type":"reasoning","status":"completed",
        "summary":[],
        "content":[{"type":"reasoning_text","text":"private chain of thought"}]
    });
    let (base_url, captured) = serve_once(
        200,
        json!({
            "id":"resp_deepseek","status":"completed","model":"deepseek-v4-flash",
            "output":[
                reasoning_item.clone(),
                {"type":"message","content":[{"type":"output_text","text":"OK"}]}
            ],
            "usage":{"input_tokens":5,"output_tokens":2}
        }),
    )
    .await;
    let capabilities = OpenAiResponsesCapabilities {
        reasoning_items: true,
    };

    let response = OpenAiResponsesBridge::new(base_url, "test-secret")
        .with_capabilities(capabilities)
        .complete(valid_request())
        .await
        .unwrap();
    captured.await.unwrap();

    assert_eq!(response["content"][0]["type"], "redacted_thinking");
    assert_eq!(response["content"][1]["text"], "OK");
    let signature = response["content"][0]["data"].as_str().unwrap();
    assert!(signature.starts_with("grillforge-openai-reasoning-v1:"));
    assert!(!signature.contains("private chain of thought"));

    let (base_url, replayed) = serve_once(200, valid_response()).await;
    OpenAiResponsesBridge::new(base_url, "test-secret")
        .with_capabilities(capabilities)
        .complete(json!({
            "model":"deepseek-v4-flash","max_tokens":64,
            "messages":[
                {"role":"user","content":"continue"},
                {"role":"assistant","content":[
                    {"type":"redacted_thinking","data":signature}
                ]}
            ]
        }))
        .await
        .unwrap();

    assert_eq!(replayed.await.unwrap().body["input"][1], reasoning_item);
}

#[tokio::test]
async fn responses_reasoning_requires_explicit_capability_instead_of_model_guessing() {
    let (base_url, captured) = serve_once(200, json!({
        "id":"resp_reasoning","status":"completed","model":"gpt-5",
        "output":[{"id":"rs_1","type":"reasoning","summary":[{"type":"summary_text","text":"summary"}],"encrypted_content":"opaque"}],
        "usage":{"input_tokens":2,"output_tokens":1}
    }))
    .await;

    let error = OpenAiResponsesBridge::new(base_url, "test-secret")
        .complete(valid_request())
        .await
        .unwrap_err();
    captured.await.unwrap();

    assert_eq!(
        error.to_string(),
        "invalid Responses response: reasoning items require the provider capability"
    );
}

#[tokio::test]
async fn real_claude_transport_hints_are_validated_and_reasoning_is_capability_gated() {
    let (base_url, captured) = serve_once(200, valid_response()).await;

    OpenAiResponsesBridge::new(base_url, "test-secret")
        .with_capabilities(OpenAiResponsesCapabilities {
            reasoning_items: true,
        })
        .complete(json!({
            "model":"gpt-5","max_tokens":64,
            "metadata":{"user_id":"user_redacted"},
            "context_management":{"edits":[{"keep":"all","type":"clear_thinking_20251015"}]},
            "output_config":{"effort":"high"},
            "thinking":{"display":"omitted","type":"adaptive"},
            "system":"Be precise.",
            "tools":[{"name":"read_file","input_schema":{"type":"object","properties":{}}}],
            "messages":[{"role":"user","content":"inspect"}]
        }))
        .await
        .unwrap();

    let request = captured.await.unwrap().body;
    assert_eq!(request["reasoning"], json!({"effort":"high"}));
    assert!(request.get("metadata").is_none());
    assert!(request.get("context_management").is_none());
    assert!(request.get("thinking").is_none());
    assert!(request.get("output_config").is_none());
}

#[tokio::test]
async fn incomplete_responses_are_returned_as_anthropic_max_tokens_results() {
    let (base_url, captured) = serve_once(200, json!({
        "id":"resp_incomplete","status":"incomplete","model":"limited",
        "incomplete_details":{"reason":"max_output_tokens"},
        "output":[{"id":"msg_1","type":"message","role":"assistant","status":"incomplete","content":[{"type":"output_text","text":"partial","annotations":[]}]}],
        "usage":{"input_tokens":2,"output_tokens":4}
    }))
    .await;

    let response = OpenAiResponsesBridge::new(base_url, "test-secret")
        .complete(valid_request())
        .await
        .unwrap();
    captured.await.unwrap();

    assert_eq!(response["content"][0]["text"], "partial");
    assert_eq!(response["stop_reason"], "max_tokens");
}
