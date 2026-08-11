use bytes::Bytes;
use futures::{StreamExt, stream};
use grillforge_lib::bridge::{
    OpenAiResponsesBridge, OpenAiResponsesCapabilities, responses_sse_to_anthropic,
    responses_sse_to_anthropic_with_capabilities,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::convert::Infallible;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

fn upstream_event(name: &str, data: Value) -> Bytes {
    Bytes::from(format!(
        "event: {name}\ndata: {}\n\n",
        serde_json::to_string(&data).unwrap()
    ))
}

async fn translate(chunks: Vec<Bytes>) -> String {
    responses_sse_to_anthropic(stream::iter(
        chunks.into_iter().map(Ok::<Bytes, Infallible>),
    ))
    .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
    .collect::<Vec<_>>()
    .await
    .join("")
}

async fn translate_with_reasoning(chunks: Vec<Bytes>) -> String {
    responses_sse_to_anthropic_with_capabilities(
        stream::iter(chunks.into_iter().map(Ok::<Bytes, Infallible>)),
        OpenAiResponsesCapabilities {
            reasoning_items: true,
        },
    )
    .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
    .collect::<Vec<_>>()
    .await
    .join("")
}

fn created() -> Bytes {
    upstream_event(
        "response.created",
        json!({"type":"response.created","response":{"id":"resp_1","model":"gpt-5"}}),
    )
}

fn completed() -> Bytes {
    upstream_event(
        "response.completed",
        json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":8,"output_tokens":4}}}),
    )
}

fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

fn output_data(output: &str) -> Vec<Value> {
    output
        .split("\n\n")
        .filter_map(|block| block.lines().find_map(|line| line.strip_prefix("data: ")))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect()
}

async fn serve_sse_once(
    body: Vec<u8>,
) -> (
    Url,
    tokio::task::JoinHandle<(String, HashMap<String, String>, Value)>,
) {
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
        let headers_text = std::str::from_utf8(&received[..header_end]).unwrap();
        let mut lines = headers_text.split("\r\n");
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
        let content_length = headers["content-length"].parse::<usize>().unwrap();
        while received.len() - header_end < content_length {
            let mut chunk = [0; 1024];
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0);
            received.extend_from_slice(&chunk[..count]);
        }
        let request: Value =
            serde_json::from_slice(&received[header_end..header_end + content_length]).unwrap();
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
        (path, headers, request)
    });
    (Url::parse(&format!("http://{address}")).unwrap(), task)
}

#[tokio::test]
async fn streaming_bridge_connects_request_and_response_over_real_http() {
    let mut upstream = Vec::new();
    upstream.extend_from_slice(&created());
    upstream.extend_from_slice(&completed());
    let (base_url, captured) = serve_sse_once(upstream).await;
    let bridge = OpenAiResponsesBridge::new(base_url, "stream-secret");

    let output = bridge
        .stream(json!({
            "model":"gpt-5",
            "max_tokens":128,
            "messages":[{"role":"user","content":[
                {"type":"text","text":"ping"},
                {"type":"document","title":"manual.pdf","source":{"type":"url","url":"https://example.com/manual.pdf"}}
            ]}],
            "stream":true
        }))
        .await
        .unwrap()
        .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
        .collect::<Vec<_>>()
        .await
        .join("");

    assert!(output.contains("event: message_start"));
    assert!(output.contains("event: message_stop"));
    let (path, headers, request) = captured.await.unwrap();
    assert_eq!(path, "/v1/responses");
    assert_eq!(headers["authorization"], "Bearer stream-secret");
    assert_eq!(request["stream"], true);
    assert_eq!(request["input"][0]["content"][0]["text"], "ping");
    assert_eq!(
        request["input"][0]["content"][1],
        json!({
            "type":"input_file","filename":"manual.pdf",
            "file_url":"https://example.com/manual.pdf"
        })
    );
}

#[tokio::test]
async fn translates_responses_text_lifecycle() {
    let output = translate(vec![
        upstream_event(
            "response.created",
            json!({"type":"response.created","response":{"id":"resp_1","model":"gpt-5"}}),
        ),
        upstream_event(
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg_1","type":"message","role":"assistant","content":[]}}),
        ),
        upstream_event(
            "response.content_part.added",
            json!({"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":""}}),
        ),
        upstream_event(
            "response.output_text.delta",
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"hello"}),
        ),
        upstream_event(
            "response.content_part.done",
            json!({"type":"response.content_part.done","output_index":0,"content_index":0,"part":{"type":"output_text","text":"hello"}}),
        ),
        upstream_event(
            "response.output_item.done",
            json!({"type":"response.output_item.done","output_index":0,"item":{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}}),
        ),
        upstream_event(
            "response.completed",
            json!({"type":"response.completed","response":{"id":"resp_1","model":"gpt-5","status":"completed","usage":{"input_tokens":7,"output_tokens":2}}}),
        ),
    ])
    .await;

    assert!(output.contains("event: message_start"));
    assert!(output.contains("\"id\":\"resp_1\""));
    assert!(output.contains("event: content_block_start"));
    assert!(output.contains("\"type\":\"text_delta\""));
    assert!(output.contains("\"text\":\"hello\""));
    assert!(output.contains("event: content_block_stop"));
    assert!(output.contains("\"input_tokens\":7"));
    assert!(output.contains("\"output_tokens\":2"));
    assert!(output.contains("event: message_stop"));
}

#[tokio::test]
async fn routes_interleaved_tool_arguments_by_item_id() {
    let output = translate(vec![
        created(),
        upstream_event(
            "response.output_item.added",
            json!({"type":"response.output_item.added","item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"first"}}),
        ),
        upstream_event(
            "response.output_item.added",
            json!({"type":"response.output_item.added","item":{"id":"fc_2","type":"function_call","call_id":"call_2","name":"second"}}),
        ),
        upstream_event(
            "response.function_call_arguments.delta",
            json!({"type":"response.function_call_arguments.delta","item_id":"fc_2","delta":"{\"b\":2}"}),
        ),
        upstream_event(
            "response.function_call_arguments.delta",
            json!({"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"a\":1}"}),
        ),
        upstream_event(
            "response.function_call_arguments.done",
            json!({"type":"response.function_call_arguments.done","item_id":"fc_1"}),
        ),
        upstream_event(
            "response.function_call_arguments.done",
            json!({"type":"response.function_call_arguments.done","item_id":"fc_2"}),
        ),
        completed(),
    ])
    .await;

    let events = output_data(&output);
    assert!(
        events
            .iter()
            .any(|event| { event["index"] == 1 && event["delta"]["partial_json"] == "{\"b\":2}" })
    );
    assert!(
        events
            .iter()
            .any(|event| { event["index"] == 0 && event["delta"]["partial_json"] == "{\"a\":1}" })
    );
    assert_eq!(occurrences(&output, "event: content_block_stop"), 2);
    assert!(output.contains("\"stop_reason\":\"tool_use\""));
}

#[tokio::test]
async fn uses_tool_arguments_from_done_when_no_deltas_arrived() {
    let output = translate(vec![
        created(),
        upstream_event(
            "response.output_item.added",
            json!({"type":"response.output_item.added","item":{"id":"fc_done","type":"function_call","call_id":"call_done","name":"lookup"}}),
        ),
        upstream_event(
            "response.output_item.done",
            json!({"type":"response.output_item.done","item":{"id":"fc_done","type":"function_call","arguments":"{\"q\":\"rust\"}"}}),
        ),
        completed(),
    ])
    .await;

    assert!(output.contains("\"partial_json\":\"{\\\"q\\\":\\\"rust\\\"}\""));
    assert_eq!(occurrences(&output, "event: content_block_stop"), 1);
    assert!(output.contains("event: message_stop"));
}

#[tokio::test]
async fn maps_response_failed_to_one_anthropic_error_and_stops() {
    let output = translate(vec![
        created(),
        upstream_event(
            "response.failed",
            json!({"type":"response.failed","response":{"status":"failed","error":{"type":"server_error","message":"backend exploded"}}}),
        ),
        completed(),
    ])
    .await;

    assert_eq!(occurrences(&output, "event: error"), 1);
    assert!(output.contains("\"type\":\"server_error\""));
    assert!(output.contains("backend exploded"));
    assert!(!output.contains("event: message_stop"));
}

#[tokio::test]
async fn reports_clean_eof_while_tool_arguments_are_partial() {
    let output = translate(vec![
        created(),
        upstream_event(
            "response.output_item.added",
            json!({"type":"response.output_item.added","item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"exec"}}),
        ),
        upstream_event(
            "response.function_call_arguments.delta",
            json!({"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"cmd\":"}),
        ),
    ])
    .await;

    assert_eq!(occurrences(&output, "event: error"), 1);
    assert!(output.contains("ended before tool arguments completed"));
    assert!(!output.contains("event: message_stop"));
}

#[tokio::test]
async fn preserves_chinese_utf8_split_across_transport_chunks() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&created());
    bytes.extend_from_slice(&upstream_event(
        "response.content_part.added",
        json!({"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":""}}),
    ));
    bytes.extend_from_slice(&upstream_event(
        "response.output_text.delta",
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"你好"}),
    ));
    bytes.extend_from_slice(&upstream_event(
        "response.output_text.done",
        json!({"type":"response.output_text.done","output_index":0,"content_index":0}),
    ));
    bytes.extend_from_slice(&completed());
    let split = bytes
        .windows("你".len())
        .position(|window| window == "你".as_bytes())
        .unwrap()
        + 1;

    let output = translate(vec![
        Bytes::copy_from_slice(&bytes[..split]),
        Bytes::copy_from_slice(&bytes[split..]),
    ])
    .await;

    assert!(output.contains("你好"));
    assert!(!output.contains('\u{fffd}'));
    assert!(!output.contains("event: error"));
}

#[tokio::test]
async fn emits_terminal_events_only_once() {
    let output = translate(vec![
        created(),
        completed(),
        Bytes::from_static(b"this trailing block is deliberately malformed\n\n"),
    ])
    .await;

    assert_eq!(occurrences(&output, "event: message_stop"), 1);
    assert_eq!(occurrences(&output, "event: message_delta"), 1);
    assert!(!output.contains("event: error"));
}

#[tokio::test]
async fn malformed_json_is_a_first_error_and_stops() {
    let output = translate(vec![
        created(),
        Bytes::from_static(b"event: response.output_text.delta\ndata: {not-json}\n\n"),
        completed(),
    ])
    .await;

    assert_eq!(occurrences(&output, "event: error"), 1);
    assert!(output.contains("must be valid JSON"));
    assert!(!output.contains("event: message_stop"));
}

#[tokio::test]
async fn malformed_sse_is_a_first_error_and_stops() {
    let output = translate(vec![
        created(),
        Bytes::from_static(b"event response.output_text.delta\ndata: {}\n\n"),
        completed(),
    ])
    .await;

    assert_eq!(occurrences(&output, "event: error"), 1);
    assert!(output.contains("line must contain"));
    assert!(!output.contains("event: message_stop"));
}

#[tokio::test]
async fn streams_reasoning_summary_and_opaque_replay_signature_when_enabled() {
    let output = translate_with_reasoning(vec![
        created(),
        upstream_event(
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":0,"item":{"id":"rs_1","type":"reasoning","status":"in_progress","summary":[]}}),
        ),
        upstream_event(
            "response.reasoning_summary_part.added",
            json!({"type":"response.reasoning_summary_part.added","item_id":"rs_1","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":""}}),
        ),
        upstream_event(
            "response.reasoning_summary_text.delta",
            json!({"type":"response.reasoning_summary_text.delta","item_id":"rs_1","output_index":0,"summary_index":0,"delta":"Need a tool."}),
        ),
        upstream_event(
            "response.reasoning_summary_text.done",
            json!({"type":"response.reasoning_summary_text.done","item_id":"rs_1","output_index":0,"summary_index":0,"text":"Need a tool."}),
        ),
        upstream_event(
            "response.reasoning_summary_part.done",
            json!({"type":"response.reasoning_summary_part.done","item_id":"rs_1","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":"Need a tool."}}),
        ),
        upstream_event(
            "response.output_item.done",
            json!({"type":"response.output_item.done","output_index":0,"item":{"id":"rs_1","type":"reasoning","status":"completed","summary":[{"type":"summary_text","text":"Need a tool."}],"encrypted_content":"opaque-ciphertext"}}),
        ),
        completed(),
    ])
    .await;

    assert!(output.contains("\"type\":\"thinking\""));
    assert!(output.contains("\"type\":\"thinking_delta\""));
    assert!(output.contains("\"thinking\":\"Need a tool.\""));
    assert!(output.contains("\"type\":\"signature_delta\""));
    assert!(output.contains("grillforge-openai-reasoning-v1:"));
    assert!(!output.contains("opaque-ciphertext"));
    assert_eq!(occurrences(&output, "event: content_block_stop"), 1);
    assert_eq!(occurrences(&output, "event: message_stop"), 1);
    assert!(!output.contains("event: error"));
}

#[tokio::test]
async fn deepseek_reasoning_text_stream_is_buffered_into_an_opaque_block() {
    let output = translate_with_reasoning(vec![
        created(),
        upstream_event(
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":0,"item":{"id":"rs_deepseek","type":"reasoning","status":"in_progress","summary":[],"content":[]}}),
        ),
        upstream_event(
            "response.content_part.added",
            json!({"type":"response.content_part.added","item_id":"rs_deepseek","output_index":0,"content_index":0,"part":{"type":"reasoning_text","text":""}}),
        ),
        upstream_event(
            "response.reasoning_text.delta",
            json!({"type":"response.reasoning_text.delta","item_id":"rs_deepseek","output_index":0,"content_index":0,"delta":"private thought"}),
        ),
        upstream_event(
            "response.reasoning_text.done",
            json!({"type":"response.reasoning_text.done","item_id":"rs_deepseek","output_index":0,"content_index":0,"text":"private thought"}),
        ),
        upstream_event(
            "response.content_part.done",
            json!({"type":"response.content_part.done","item_id":"rs_deepseek","output_index":0,"content_index":0,"part":{"type":"reasoning_text","text":"private thought"}}),
        ),
        upstream_event(
            "response.output_item.done",
            json!({"type":"response.output_item.done","output_index":0,"item":{"id":"rs_deepseek","type":"reasoning","status":"completed","summary":[],"content":[{"type":"reasoning_text","text":"private thought"}]}}),
        ),
        completed(),
    ])
    .await;

    assert!(output.contains("\"type\":\"redacted_thinking\""));
    assert!(output.contains("grillforge-openai-reasoning-v1:"));
    assert!(!output.contains("private thought"));
    assert_eq!(occurrences(&output, "event: content_block_stop"), 1);
    assert_eq!(occurrences(&output, "event: message_stop"), 1);
    assert!(!output.contains("event: error"));
}

#[tokio::test]
async fn rejects_reasoning_before_exposing_summary_without_explicit_capability() {
    let output = translate(vec![
        created(),
        upstream_event(
            "response.output_item.added",
            json!({"type":"response.output_item.added","output_index":0,"item":{"id":"rs_1","type":"reasoning","status":"in_progress","summary":[]}}),
        ),
        upstream_event(
            "response.reasoning_summary_text.delta",
            json!({"type":"response.reasoning_summary_text.delta","item_id":"rs_1","output_index":0,"summary_index":0,"delta":"private thought"}),
        ),
        completed(),
    ])
    .await;

    assert_eq!(occurrences(&output, "event: error"), 1);
    assert!(output.contains("reasoning items require the provider capability"));
    assert!(!output.contains("private thought"));
    assert!(!output.contains("event: message_stop"));
}
