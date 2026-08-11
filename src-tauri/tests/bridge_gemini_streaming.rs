use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::post,
};
use futures::StreamExt;
use grillforge_lib::bridge::GeminiNativeBridge;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use url::Url;

#[tokio::test]
async fn gemini_cumulative_text_stream_becomes_anthropic_deltas_and_usage() {
    type CapturedRequest = Arc<Mutex<Option<(HeaderMap, Value)>>>;

    let captured = CapturedRequest::default();
    let upstream = Router::new()
        .route(
            "/v1beta/models/gemini-2.5-pro:streamGenerateContent",
            post(
                |State(captured): State<CapturedRequest>,
                 headers: HeaderMap,
                 Json(body): Json<Value>| async move {
                    *captured.lock().unwrap() = Some((headers, body));
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(concat!(
                            "data: {\"responseId\":\"gemini-stream\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hel\"}]}}],\"usageMetadata\":{\"promptTokenCount\":4,\"totalTokenCount\":5}}\n\n",
                            "data: {\"responseId\":\"gemini-stream\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"finishReason\":\"STOP\",\"content\":{\"parts\":[{\"text\":\"Hello\"}]}}],\"usageMetadata\":{\"promptTokenCount\":4,\"totalTokenCount\":6}}\n\n"
                        )))
                        .unwrap()
                },
            ),
        )
        .with_state(captured.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let endpoint = Url::parse(&format!(
        "http://{address}/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
    ))
    .unwrap();
    let bridge = GeminiNativeBridge::from_endpoint(endpoint, "stream-secret");

    let stream = bridge
        .stream(json!({
            "model":"gemini-2.5-pro",
            "max_tokens":64,
            "stream":true,
            "messages":[{"role":"user","content":"hello"}]
        }))
        .await
        .expect("Gemini stream");
    let output = stream
        .map(|chunk| String::from_utf8(chunk.expect("valid SSE").to_vec()).unwrap())
        .collect::<Vec<_>>()
        .await
        .join("");

    assert!(output.contains("event: message_start"));
    assert!(output.contains("\"text\":\"Hel\""));
    assert!(output.contains("\"text\":\"lo\""));
    assert!(output.contains("\"stop_reason\":\"end_turn\""));
    assert!(output.contains("\"input_tokens\":4"));
    assert!(output.contains("\"output_tokens\":2"));
    assert!(output.contains("event: message_stop"));
    let captured = captured.lock().unwrap();
    let (headers, body) = captured.as_ref().expect("captured request");
    assert_eq!(headers["x-goog-api-key"], "stream-secret");
    assert_eq!(body["contents"][0]["parts"][0]["text"], "hello");
    assert!(body.get("stream").is_none());
}

#[tokio::test]
async fn gemini_function_call_stream_becomes_anthropic_tool_use() {
    let upstream = Router::new().route(
        "/v1beta/models/gemini:streamGenerateContent",
        post(|| async {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(concat!(
                    "data: {\"responseId\":\"gemini-tool-stream\",\"modelVersion\":\"gemini\",\"candidates\":[{\"finishReason\":\"STOP\",\"content\":{\"parts\":[{\"functionCall\":{\"id\":\"call_stream\",\"name\":\"forecast\",\"args\":{\"city\":\"Oslo\"}}}]}}],\"usageMetadata\":{\"promptTokenCount\":8,\"totalTokenCount\":11}}\n\n"
                )))
                .unwrap()
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let endpoint = Url::parse(&format!(
        "http://{address}/v1beta/models/gemini:streamGenerateContent?alt=sse"
    ))
    .unwrap();
    let bridge = GeminiNativeBridge::from_endpoint(endpoint, "stream-secret");

    let stream = bridge
        .stream(json!({
            "model":"gemini",
            "max_tokens":64,
            "stream":true,
            "messages":[{"role":"user","content":"weather?"}]
        }))
        .await
        .expect("Gemini tool stream");
    let output = stream
        .map(|chunk| String::from_utf8(chunk.expect("valid SSE").to_vec()).unwrap())
        .collect::<Vec<_>>()
        .await
        .join("");

    assert!(output.contains("\"type\":\"tool_use\""));
    assert!(output.contains("\"id\":\"call_stream\""));
    assert!(output.contains("\"name\":\"forecast\""));
    assert!(output.contains("\"type\":\"input_json_delta\""));
    assert!(output.contains("\\\"city\\\":\\\"Oslo\\\""));
    assert!(output.contains("\"stop_reason\":\"tool_use\""));
    assert!(output.contains("\"output_tokens\":3"));
}
