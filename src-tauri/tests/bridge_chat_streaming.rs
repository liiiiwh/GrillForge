use bytes::Bytes;
use futures::{StreamExt, stream};
use grillforge_lib::bridge::{OpenAiChatBridge, OpenAiChatCapabilities, chat_sse_to_anthropic};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::convert::Infallible;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

fn chunk(value: Value) -> Bytes {
    Bytes::from(format!(
        "data: {}\n\n",
        serde_json::to_string(&value).unwrap()
    ))
}

async fn translate(chunks: Vec<Bytes>, capabilities: OpenAiChatCapabilities) -> String {
    chat_sse_to_anthropic(
        stream::iter(chunks.into_iter().map(Ok::<Bytes, Infallible>)),
        capabilities,
    )
    .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
    .collect::<Vec<_>>()
    .await
    .join("")
}

fn data(output: &str) -> Vec<Value> {
    output
        .split("\n\n")
        .filter_map(|block| block.lines().find_map(|line| line.strip_prefix("data: ")))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn count(output: &str, needle: &str) -> usize {
    output.match_indices(needle).count()
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
        let request = serde_json::from_slice(&received[header_end..header_end + length]).unwrap();
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
        (path, headers, request)
    });
    (
        Url::parse(&format!("http://{address}/chat-prefix")).unwrap(),
        task,
    )
}

#[tokio::test]
async fn chat_streaming_bridge_connects_real_http_and_requests_usage() {
    let mut upstream = Vec::new();
    upstream.extend_from_slice(&chunk(json!({"id":"chat_http","model":"qwen","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]})));
    upstream.extend_from_slice(&chunk(json!({"id":"chat_http","model":"qwen","choices":[],"usage":{"prompt_tokens":2,"completion_tokens":1}})));
    upstream.extend_from_slice(b"data: [DONE]\n\n");
    let (base_url, captured) = serve_sse_once(upstream).await;

    let output = OpenAiChatBridge::new(base_url, "chat-stream-secret")
        .stream(json!({
            "model":"qwen","max_tokens":32,"stream":true,
            "messages":[{"role":"user","content":"ping"}]
        }))
        .await
        .unwrap()
        .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
        .collect::<Vec<_>>()
        .await
        .join("");

    assert!(output.contains("event: message_stop"));
    let (path, headers, request) = captured.await.unwrap();
    assert_eq!(path, "/chat-prefix/v1/chat/completions");
    assert_eq!(headers["authorization"], "Bearer chat-stream-secret");
    assert_eq!(request["stream"], true);
    assert_eq!(request["stream_options"]["include_usage"], true);
}

#[tokio::test]
async fn text_stream_uses_usage_only_tail_before_one_terminal_pair() {
    let output = translate(
        vec![
            chunk(json!({"id":"chat_1","model":"qwen","choices":[{"index":0,"delta":{"role":"assistant","content":"Hel"},"finish_reason":null}]})),
            chunk(json!({"id":"chat_1","model":"qwen","choices":[{"index":0,"delta":{"content":"lo"},"finish_reason":"stop"}]})),
            chunk(json!({"id":"chat_1","model":"qwen","choices":[],"usage":{"prompt_tokens":9,"completion_tokens":2}})),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ],
        OpenAiChatCapabilities::default(),
    )
    .await;

    assert!(output.contains("event: message_start"));
    assert!(output.contains("\"text\":\"Hel\""));
    assert!(output.contains("\"text\":\"lo\""));
    assert_eq!(count(&output, "event: content_block_stop"), 1);
    assert_eq!(count(&output, "event: message_delta"), 1);
    assert_eq!(count(&output, "event: message_stop"), 1);
    assert!(output.contains("\"input_tokens\":9"));
    assert!(output.contains("\"output_tokens\":2"));
}

#[tokio::test]
async fn null_content_and_reasoning_deltas_are_empty_not_protocol_errors() {
    let output = translate(
        vec![
            chunk(json!({"id":"chat_null","model":"deepseek-v4-flash","choices":[{"index":0,"delta":{"role":"assistant","content":null,"reasoning_content":null},"finish_reason":null}]})),
            chunk(json!({"id":"chat_null","model":"deepseek-v4-flash","choices":[{"index":0,"delta":{"content":"OK"},"finish_reason":"stop"}]})),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ],
        OpenAiChatCapabilities::default(),
    )
    .await;

    assert!(output.contains("\"text\":\"OK\""));
    assert_eq!(count(&output, "event: message_stop"), 1);
    assert!(!output.contains("event: error"));
}

#[tokio::test]
async fn interleaved_tool_calls_are_routed_by_chat_index() {
    let output = translate(
        vec![
            chunk(json!({"id":"chat_tools","model":"qwen","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_0","type":"function","function":{"name":"first"}},{"index":1,"id":"call_1","type":"function","function":{"name":"second"}}]},"finish_reason":null}]})),
            chunk(json!({"id":"chat_tools","model":"qwen","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{\"b\":2}"}},{"index":0,"function":{"arguments":"{\"a\":1}"}}]},"finish_reason":"tool_calls"}]})),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ],
        OpenAiChatCapabilities::default(),
    )
    .await;
    let events = data(&output);
    let first = events
        .iter()
        .find(|event| event.pointer("/content_block/id") == Some(&json!("call_0")))
        .unwrap()["index"]
        .as_u64()
        .unwrap();
    let second = events
        .iter()
        .find(|event| event.pointer("/content_block/id") == Some(&json!("call_1")))
        .unwrap()["index"]
        .as_u64()
        .unwrap();
    assert!(events.iter().any(|event| event["index"] == first
        && event.pointer("/delta/partial_json") == Some(&json!("{\"a\":1}"))));
    assert!(events.iter().any(|event| event["index"] == second
        && event.pointer("/delta/partial_json") == Some(&json!("{\"b\":2}"))));
    assert!(output.contains("\"stop_reason\":\"tool_use\""));
}

#[tokio::test]
async fn duplicate_finish_reasons_emit_one_terminal_pair() {
    let output = translate(
        vec![
            chunk(json!({"id":"chat_1","model":"qwen","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]})),
            chunk(json!({"id":"chat_1","model":"qwen","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ],
        OpenAiChatCapabilities::default(),
    )
    .await;
    assert_eq!(count(&output, "event: message_delta"), 1);
    assert_eq!(count(&output, "event: message_stop"), 1);
    assert!(!output.contains("event: error"));
}

#[tokio::test]
async fn clean_eof_without_finish_reason_is_an_error() {
    let output = translate(
        vec![chunk(json!({"id":"chat_1","model":"qwen","choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":null}]}))],
        OpenAiChatCapabilities::default(),
    )
    .await;
    assert_eq!(count(&output, "event: error"), 1);
    assert!(output.contains("without finish_reason"));
    assert!(!output.contains("event: message_stop"));
}

#[tokio::test]
async fn reasoning_content_requires_explicit_capability() {
    let input = vec![chunk(
        json!({"id":"chat_1","model":"qwen","choices":[{"index":0,"delta":{"reasoning_content":"think"},"finish_reason":"stop"}]}),
    )];
    let disabled = translate(input.clone(), OpenAiChatCapabilities::default()).await;
    assert!(disabled.contains("event: error"));
    let enabled = translate(
        input,
        OpenAiChatCapabilities {
            reasoning_content: true,
            reasoning_effort: false,
        },
    )
    .await;
    assert!(enabled.contains("\"type\":\"thinking_delta\""));
    assert!(enabled.contains("\"thinking\":\"think\""));
    assert!(!enabled.contains("event: error"));
}

#[tokio::test]
async fn malformed_json_stops_at_the_first_error() {
    let output = translate(
        vec![
            Bytes::from_static(b"data: {bad-json}\n\n"),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ],
        OpenAiChatCapabilities::default(),
    )
    .await;
    assert_eq!(count(&output, "event: error"), 1);
    assert!(!output.contains("event: message_stop"));
}

#[tokio::test]
async fn chinese_utf8_can_cross_transport_chunks() {
    let mut raw = Vec::new();
    raw.extend_from_slice(&chunk(json!({"id":"chat_1","model":"qwen","choices":[{"index":0,"delta":{"content":"你好"},"finish_reason":"stop"}]})));
    raw.extend_from_slice(b"data: [DONE]\n\n");
    let split = raw
        .windows(3)
        .position(|window| window == "你".as_bytes())
        .unwrap()
        + 1;
    let output = translate(
        vec![
            Bytes::copy_from_slice(&raw[..split]),
            Bytes::copy_from_slice(&raw[split..]),
        ],
        OpenAiChatCapabilities::default(),
    )
    .await;
    assert!(output.contains("你好"));
    assert!(!output.contains('\u{fffd}'));
    assert!(!output.contains("event: error"));
}

#[tokio::test]
async fn upstream_error_event_is_preserved_once_and_stops() {
    let output = translate(
        vec![
            chunk(
                json!({"error":{"type":"server_error","message":"backend exploded"},"choices":[]}),
            ),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ],
        OpenAiChatCapabilities::default(),
    )
    .await;
    assert_eq!(count(&output, "event: error"), 1);
    assert!(output.contains("server_error"));
    assert!(output.contains("backend exploded"));
    assert!(!output.contains("event: message_stop"));
}
