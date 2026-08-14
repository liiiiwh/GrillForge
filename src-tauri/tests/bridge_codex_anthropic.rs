use bytes::Bytes;
use futures::{StreamExt, stream};
use grillforge_lib::bridge::{
    CodexAnthropicCapabilities, anthropic_sse_to_codex_responses,
    anthropic_sse_to_codex_responses_with_context, anthropic_to_codex_response,
    anthropic_to_codex_response_with_context, codex_response_to_anthropic,
    codex_response_to_anthropic_with_context,
};
use serde_json::json;
use std::convert::Infallible;

#[test]
fn empty_codex_tool_list_is_omitted_from_anthropic_request() {
    let request = codex_response_to_anthropic(
        json!({"model":"claude-sonnet","input":"ping","tools":[]}),
        CodexAnthropicCapabilities::default(),
    )
    .expect("an empty Codex tool list is valid");

    assert!(request.get("tools").is_none());
}

#[test]
fn tool_call_and_result_history_round_trip_without_being_flattened() {
    let capabilities = CodexAnthropicCapabilities::default();
    let request = codex_response_to_anthropic(
        json!({
            "model":"claude-sonnet","max_output_tokens":1024,"store":false,
            "include":["reasoning.encrypted_content"],
            "parallel_tool_calls":false,
            "prompt_cache_key":"session-1",
            "service_tier":"priority",
            "tools":[{"type":"function","name":"weather","description":"Get weather","strict":false,"parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}],
            "tool_choice":{"type":"function","name":"weather"},
            "input":[
                {"role":"user","content":"weather?"},
                {"type":"function_call","id":"fc_1","call_id":"call_1","name":"weather","arguments":"{\"city\":\"Tokyo\"}","status":"completed"},
                {"type":"function_call_output","call_id":"call_1","output":"sunny"}
            ]
        }),
        capabilities,
    )
    .unwrap();

    assert_eq!(request["tools"][0]["name"], "weather");
    assert_eq!(request["tools"][0]["strict"], false);
    assert_eq!(
        request["tool_choice"],
        json!({"type":"tool","name":"weather","disable_parallel_tool_use":true})
    );
    assert!(request.get("include").is_none());
    assert!(request.get("prompt_cache_key").is_none());
    assert_eq!(request["messages"][1]["role"], "assistant");
    assert_eq!(request["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(
        request["messages"][1]["content"][0]["input"]["city"],
        "Tokyo"
    );
    assert_eq!(request["messages"][2]["role"], "user");
    assert_eq!(request["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(request["messages"][2]["content"][0]["content"], "sunny");

    let response = anthropic_to_codex_response(
        json!({
            "id":"msg_tool","type":"message","role":"assistant","model":"claude-sonnet",
            "content":[{"type":"tool_use","id":"call_2","name":"weather","input":{"city":"Paris"}}],
            "stop_reason":"tool_use","usage":{"input_tokens":9,"output_tokens":4}
        }),
        capabilities,
    )
    .unwrap();
    assert_eq!(response["output"][0]["type"], "function_call");
    assert_eq!(response["output"][0]["call_id"], "call_2");
    assert_eq!(response["output"][0]["name"], "weather");
    assert_eq!(response["output"][0]["arguments"], "{\"city\":\"Paris\"}");
}

#[test]
fn signed_thinking_round_trips_as_summary_and_opaque_encrypted_content_only_when_enabled() {
    let capabilities = CodexAnthropicCapabilities { reasoning: true };
    let response = anthropic_to_codex_response(
        json!({
            "id":"msg_reasoning","type":"message","role":"assistant","model":"claude-sonnet",
            "content":[
                {"type":"thinking","thinking":"Check the file.","signature":"anthropic-secret-signature"},
                {"type":"tool_use","id":"call_1","name":"read_file","input":{"path":"a.rs"}}
            ],
            "stop_reason":"tool_use","usage":{"input_tokens":7,"output_tokens":3}
        }),
        capabilities,
    )
    .unwrap();

    assert_eq!(response["output"][0]["type"], "reasoning");
    assert_eq!(
        response["output"][0]["summary"][0]["text"],
        "Check the file."
    );
    let encrypted = response["output"][0]["encrypted_content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(encrypted.starts_with("grillforge-anthropic-thinking-v1:"));
    assert!(!encrypted.contains("anthropic-secret-signature"));

    let replay = codex_response_to_anthropic(
        json!({
            "model":"claude-sonnet","max_output_tokens":32768,"store":false,
            "reasoning":{"effort":"high"},
            "input":[
                {"type":"message","role":"user","content":[{"type":"input_text","text":"inspect"}]},
                response["output"][0].clone(),
                response["output"][1].clone(),
                {"type":"function_call_output","call_id":"call_1","output":"contents"}
            ]
        }),
        capabilities,
    )
    .unwrap();
    assert_eq!(
        replay["thinking"],
        json!({"type":"enabled","budget_tokens":16384})
    );
    assert_eq!(replay["messages"][1]["content"][0]["type"], "thinking");
    assert_eq!(
        replay["messages"][1]["content"][0]["signature"],
        "anthropic-secret-signature"
    );

    let error = anthropic_to_codex_response(
        json!({
            "id":"msg_reasoning","type":"message","role":"assistant","model":"claude-sonnet",
            "content":[{"type":"thinking","thinking":"private","signature":"sig"}],
            "stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}
        }),
        CodexAnthropicCapabilities::default(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid Codex Responses bridge response: Anthropic thinking requires the explicit reasoning capability"
    );
}

#[tokio::test]
async fn streamed_tool_call_preserves_arguments_and_completed_item() {
    let events = [
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_tool_stream","model":"claude-sonnet","usage":{"input_tokens":4,"output_tokens":0}}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_1","name":"read_file","input":{}}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a.rs\"}"}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":3}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ];
    let chunks = events.into_iter().map(|(name, data)| {
        Ok::<_, Infallible>(Bytes::from(format!("event: {name}\ndata: {data}\n\n")))
    });
    let output = anthropic_sse_to_codex_responses(
        stream::iter(chunks),
        CodexAnthropicCapabilities::default(),
    )
    .map(|event| String::from_utf8(event.unwrap().to_vec()).unwrap())
    .collect::<Vec<_>>()
    .await
    .join("");

    assert!(output.contains("event: response.output_item.added"));
    assert!(output.contains("\"type\":\"function_call\""));
    assert!(output.contains("event: response.function_call_arguments.delta"));
    assert!(output.contains("{\\\"path\\\":\\\"a.rs\\\"}"));
    assert!(output.contains("event: response.function_call_arguments.done"));
    assert!(output.contains("event: response.output_item.done"));
    assert!(output.contains("event: response.completed"));
}

#[tokio::test]
async fn anthropic_stream_error_becomes_one_responses_error_event_and_stops() {
    let chunks = [
        Ok::<_, Infallible>(Bytes::from_static(
            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"slow down\"}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        )),
    ];
    let output = anthropic_sse_to_codex_responses(
        stream::iter(chunks),
        CodexAnthropicCapabilities::default(),
    )
    .map(|event| String::from_utf8(event.unwrap().to_vec()).unwrap())
    .collect::<Vec<_>>()
    .await
    .join("");

    assert_eq!(output.matches("event: error").count(), 1);
    assert!(output.contains("\"type\":\"rate_limit_error\""));
    assert!(output.contains("\"message\":\"slow down\""));
    assert!(!output.contains("event: response.completed"));
}

#[tokio::test]
async fn interleaved_reasoning_and_tool_blocks_are_routed_by_anthropic_index() {
    let events = [
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_interleaved","model":"claude-sonnet","usage":{"input_tokens":6,"output_tokens":0}}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Inspect first."}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_1","name":"read_file","input":{}}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a.rs\"}"}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"anthropic-signature"}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":1}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":5}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ];
    let chunks = events.into_iter().map(|(name, data)| {
        Ok::<_, Infallible>(Bytes::from(format!("event: {name}\ndata: {data}\n\n")))
    });
    let output = anthropic_sse_to_codex_responses(
        stream::iter(chunks),
        CodexAnthropicCapabilities { reasoning: true },
    )
    .map(|event| String::from_utf8(event.unwrap().to_vec()).unwrap())
    .collect::<Vec<_>>()
    .await
    .join("");

    assert!(output.contains("event: response.reasoning_summary_text.delta"));
    assert!(output.contains("\"delta\":\"Inspect first.\""));
    assert!(output.contains("grillforge-anthropic-thinking-v1:"));
    assert!(!output.contains("anthropic-signature"));
    assert!(output.contains("event: response.function_call_arguments.delta"));
    assert!(output.contains("\"output_index\":1"));
    assert_eq!(
        output.matches("event: response.output_item.done").count(),
        2
    );
    assert!(output.contains("event: response.completed"));
}

#[tokio::test]
async fn streamed_reasoning_fails_before_exposing_thinking_without_capability() {
    let chunks = [Ok::<_, Infallible>(Bytes::from_static(
        b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"secret-thought\"}}\n\n",
    ))];
    let results = anthropic_sse_to_codex_responses(
        stream::iter(chunks),
        CodexAnthropicCapabilities::default(),
    )
    .collect::<Vec<_>>()
    .await;

    let error = results.last().unwrap().as_ref().unwrap_err();
    assert!(error.to_string().contains("explicit reasoning capability"));
    assert!(
        results
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .all(|event| !String::from_utf8_lossy(event).contains("secret-thought"))
    );
}

#[test]
fn image_and_document_inputs_preserve_media_in_messages_and_tool_results() {
    let request = codex_response_to_anthropic(
        json!({
            "model":"claude-sonnet","max_output_tokens":2048,"store":false,
            "input":[
                {"role":"user","content":[
                    {"type":"input_text","text":"inspect"},
                    {"type":"input_image","image_url":"data:image/png;base64,aGVsbG8=","detail":"high"},
                    {"type":"input_image","image_url":"https://example.com/image.webp"},
                    {"type":"input_file","filename":"trace.pdf","file_data":"data:application/pdf;base64,JVBERi0xLjc="},
                    {"type":"input_file","filename":"manual.pdf","file_url":"https://example.com/manual.pdf"}
                ]},
                {"type":"function_call","call_id":"call_1","name":"capture","arguments":"{}","status":"completed"},
                {"type":"function_call_output","call_id":"call_1","output":[
                    {"type":"input_text","text":"artifact"},
                    {"type":"input_image","image_url":"data:image/jpeg;base64,aGVsbG8="},
                    {"type":"input_file","filename":"tool.pdf","file_data":"data:application/pdf;base64,JVBERi0xLjc="}
                ]}
            ]
        }),
        CodexAnthropicCapabilities::default(),
    )
    .unwrap();

    let content = request["messages"][0]["content"].as_array().unwrap();
    assert_eq!(
        content[1],
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}})
    );
    assert_eq!(
        content[2],
        json!({"type":"image","source":{"type":"url","url":"https://example.com/image.webp"}})
    );
    assert_eq!(
        content[3],
        json!({"type":"document","title":"trace.pdf","source":{"type":"base64","media_type":"application/pdf","data":"JVBERi0xLjc="}})
    );
    assert_eq!(
        content[4],
        json!({"type":"document","title":"manual.pdf","source":{"type":"url","url":"https://example.com/manual.pdf"}})
    );
    let tool_content = request["messages"][2]["content"][0]["content"]
        .as_array()
        .unwrap();
    assert_eq!(tool_content[1]["type"], "image");
    assert_eq!(tool_content[2]["type"], "document");
}

#[test]
fn declared_custom_tool_round_trips_verbatim_without_name_guessing() {
    let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-old\n+new\n*** End Patch";
    let (request, context) = codex_response_to_anthropic_with_context(
        json!({
            "model":"claude-sonnet","store":false,
            "tools":[{"type":"custom","name":"apply_patch","description":"Apply a patch","format":{"type":"grammar","syntax":"lark","definition":"start: /[\\s\\S]+/"}}],
            "tool_choice":{"type":"custom","name":"apply_patch"},
            "input":[
                {"role":"user","content":"edit"},
                {"type":"custom_tool_call","call_id":"call_old","name":"apply_patch","input":patch,"status":"completed"},
                {"type":"custom_tool_call_output","call_id":"call_old","output":"Done!"}
            ]
        }),
        CodexAnthropicCapabilities::default(),
    ).unwrap();
    assert_eq!(
        request["tools"][0]["input_schema"]["required"],
        json!(["input"])
    );
    assert!(
        request["tools"][0]["description"]
            .as_str()
            .unwrap()
            .contains("\"format\"")
    );
    assert_eq!(
        request["messages"][1]["content"][0]["input"]["input"],
        patch
    );
    assert_eq!(
        request["tool_choice"],
        json!({"type":"tool","name":"apply_patch"})
    );

    let response = anthropic_to_codex_response_with_context(
        json!({
            "id":"msg_custom","model":"claude-sonnet",
            "content":[{"type":"tool_use","id":"call_new","name":"apply_patch","input":{"input":patch}}],
            "stop_reason":"tool_use","usage":{"input_tokens":2,"output_tokens":3}
        }),
        CodexAnthropicCapabilities::default(),
        &context,
    ).unwrap();
    assert_eq!(response["output"][0]["type"], "custom_tool_call");
    assert_eq!(response["output"][0]["input"], patch);
    assert!(response["output"][0].get("arguments").is_none());
}

#[tokio::test]
async fn declared_custom_tool_stream_emits_custom_input_events() {
    let (_, context) = codex_response_to_anthropic_with_context(
        json!({"model":"claude-sonnet","store":false,"input":"edit","tools":[{"type":"custom","name":"apply_patch","format":{"type":"grammar","syntax":"lark","definition":"start: /.+/"}}]}),
        CodexAnthropicCapabilities::default(),
    ).unwrap();
    let chunks = [Ok::<_, Infallible>(Bytes::from_static(
        b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"apply_patch\",\"input\":{}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"input\\\":\\\"line1\\\\nline2\\\"}\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":3}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ))];
    let output = anthropic_sse_to_codex_responses_with_context(
        stream::iter(chunks),
        CodexAnthropicCapabilities::default(),
        context,
    )
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .flat_map(Result::unwrap)
    .collect::<Vec<_>>();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("event: response.custom_tool_call_input.done"));
    assert!(output.contains("\"type\":\"custom_tool_call\""));
    assert!(!output.contains("response.function_call_arguments.done"));
}

#[test]
fn unsupported_hosted_and_structured_features_fail_before_conversion() {
    for unsupported in [
        json!({"model":"claude","input":"x","tools":[{"type":"web_search"}]}),
        json!({"model":"claude","input":"x","tools":[{"type":"namespace","name":"mcp"}]}),
        json!({"model":"claude","input":"x","text":{"format":{"type":"json_schema","name":"out","strict":true,"schema":{"type":"object"}}}}),
    ] {
        codex_response_to_anthropic(unsupported, CodexAnthropicCapabilities::default())
            .expect_err("unsupported Codex feature must fail before an upstream request exists");
    }
}

#[test]
fn tool_search_round_trips_through_anthropic() {
    let (request, context) = codex_response_to_anthropic_with_context(
        json!({
            "model":"claude-sonnet","store":false,
            "tools":[{"type":"tool_search"}],
            "tool_choice":{"type":"tool_search"},
            "input":[
                {"role":"user","content":"find a tool"},
                {"type":"tool_search_call","call_id":"search_old","arguments":{"query":"calendar"},"status":"completed"},
                {"type":"tool_search_output","call_id":"search_old","output":"loaded"}
            ]
        }),
        CodexAnthropicCapabilities::default(),
    )
    .unwrap();
    assert_eq!(request["tools"][0]["name"], "tool_search");
    assert_eq!(
        request["tool_choice"],
        json!({"type":"tool","name":"tool_search"})
    );
    assert_eq!(request["messages"][1]["content"][0]["name"], "tool_search");
    assert_eq!(request["messages"][2]["content"][0]["type"], "tool_result");

    let response = anthropic_to_codex_response_with_context(
        json!({
            "id":"msg_search","model":"claude-sonnet",
            "content":[{"type":"tool_use","id":"search_new","name":"tool_search","input":{"query":"mail"}}],
            "stop_reason":"tool_use","usage":{"input_tokens":2,"output_tokens":3}
        }),
        CodexAnthropicCapabilities::default(),
        &context,
    )
    .unwrap();
    assert_eq!(response["output"][0]["type"], "tool_search_call");
    assert_eq!(response["output"][0]["arguments"], json!({"query":"mail"}));
}

#[tokio::test]
async fn streamed_tool_search_restores_codex_events() {
    let (_, context) = codex_response_to_anthropic_with_context(
        json!({"model":"claude-sonnet","store":false,"input":"find","tools":[{"type":"tool_search"}]}),
        CodexAnthropicCapabilities::default(),
    )
    .unwrap();
    let chunks = [Ok::<_, Infallible>(Bytes::from_static(
        b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"tool_search\",\"input\":{}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"mail\\\"}\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":3}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ))];
    let output = anthropic_sse_to_codex_responses_with_context(
        stream::iter(chunks),
        CodexAnthropicCapabilities::default(),
        context,
    )
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .flat_map(Result::unwrap)
    .collect::<Vec<_>>();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("response.tool_search_call.completed"));
    assert!(output.contains("\"type\":\"tool_search_call\""));
}
