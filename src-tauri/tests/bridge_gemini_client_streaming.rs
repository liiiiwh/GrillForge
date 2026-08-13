use bytes::Bytes;
use futures::{StreamExt, stream};
use grillforge_lib::bridge::anthropic_sse_to_gemini;

#[tokio::test]
async fn anthropic_text_and_finish_events_become_gemini_sse_chunks() {
    let source = stream::iter(vec![Ok::<_, std::convert::Infallible>(Bytes::from_static(
        br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"coder","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":3,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#,
    ))]);

    let output = anthropic_sse_to_gemini(source)
        .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
        .collect::<Vec<_>>()
        .await
        .join("");

    assert!(output.contains(r#""parts":[{"text":"hello"}]"#), "{output}");
    assert!(output.contains(r#""finishReason":"STOP""#), "{output}");
    assert!(output.contains(r#""promptTokenCount":3"#), "{output}");
    assert!(output.contains(r#""candidatesTokenCount":2"#), "{output}");
    assert!(
        !output.contains("event:"),
        "Gemini SSE uses data-only events"
    );
}

#[tokio::test]
async fn anthropic_tool_deltas_become_a_gemini_function_call() {
    let source = stream::iter(vec![Ok::<_, std::convert::Infallible>(Bytes::from_static(
        br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_tool","type":"message","role":"assistant","model":"coder","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":4,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_1","name":"read_file","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"src/main.rs\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#,
    ))]);

    let output = anthropic_sse_to_gemini(source)
        .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
        .collect::<Vec<_>>()
        .await
        .join("");

    assert!(
        output.contains(
            r#""functionCall":{"args":{"path":"src/main.rs"},"id":"call_1","name":"read_file"}"#
        ),
        "{output}"
    );
    assert!(output.contains(r#""finishReason":"STOP""#), "{output}");
}
