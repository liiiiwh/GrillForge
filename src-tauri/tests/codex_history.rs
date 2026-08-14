use futures::{StreamExt, stream};
use grillforge_lib::bridge::{BridgeError, CodexHistoryStore, record_codex_sse};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn restores_the_previous_tool_call_before_a_codex_tool_output() {
    let history = CodexHistoryStore::default();
    history
        .record_response(&json!({
            "id":"resp_1",
            "output":[{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"README.md\"}","reasoning_content":"inspect"}]
        }))
        .await;
    let mut request = json!({
        "previous_response_id":"resp_1",
        "input":[{"type":"function_call_output","call_id":"call_1","output":"hello"}]
    });

    assert_eq!(history.enrich_request(&mut request).await, 1);
    assert_eq!(request["input"][0]["type"], "function_call");
    assert_eq!(request["input"][0]["reasoning_content"], "inspect");
    assert_eq!(request["input"][1]["type"], "function_call_output");
}

#[tokio::test]
async fn records_streamed_tool_calls_for_the_next_request() {
    let history = Arc::new(CodexHistoryStore::default());
    let body = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_s\"}}\n\n",
        "event: response.output_item.done\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_s\",\"name\":\"read\",\"arguments\":\"{}\"}}\n\n"
    );
    let stream = stream::iter([Ok::<_, BridgeError>(body.into())]);
    record_codex_sse(stream, history.clone())
        .collect::<Vec<_>>()
        .await;
    let mut request =
        json!({"input":[{"type":"function_call_output","call_id":"call_s","output":"ok"}]});
    assert_eq!(history.enrich_request(&mut request).await, 1);
    assert_eq!(request["input"][0]["name"], "read");
}
