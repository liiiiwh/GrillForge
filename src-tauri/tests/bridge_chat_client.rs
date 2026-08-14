use bytes::Bytes;
use futures::{StreamExt, stream};
use grillforge_lib::bridge::{
    anthropic_response_to_chat, anthropic_sse_to_chat, chat_request_to_anthropic,
};
use serde_json::{Value, json};
use std::convert::Infallible;

#[test]
fn chat_tool_history_round_trips_through_anthropic_messages() {
    let request = chat_request_to_anthropic(json!({
        "model":"grillforge/worker",
        "max_tokens":128,
        "messages":[
            {"role":"system","content":"Use tools precisely."},
            {"role":"user","content":"Read package.json"},
            {"role":"assistant","content":null,"reasoning_content":"Need the file.","reasoning_signature":"opaque-signature","tool_calls":[{
                "id":"call_read","type":"function",
                "function":{"name":"Read","arguments":"{\"file_path\":\"/tmp/package.json\"}"}
            }]},
            {"role":"tool","tool_call_id":"call_read","content":"{\"version\":\"1.0.0\"}"}
        ],
        "tools":[{"type":"function","function":{
            "name":"Read","description":"Read one file",
            "parameters":{"type":"object","properties":{"file_path":{"type":"string"}},"required":["file_path"]}
        }}],
        "tool_choice":"auto"
    }))
    .unwrap();

    assert_eq!(request["system"], "Use tools precisely.");
    assert_eq!(request["tools"][0]["name"], "Read");
    assert_eq!(
        request["messages"][1]["content"][0],
        json!({
            "type":"thinking","thinking":"Need the file.","signature":"opaque-signature"
        })
    );
    assert_eq!(
        request["messages"][1]["content"][1],
        json!({"type":"tool_use","id":"call_read","name":"Read","input":{"file_path":"/tmp/package.json"}})
    );
    assert_eq!(
        request["messages"][2]["content"][0],
        json!({"type":"tool_result","tool_use_id":"call_read","content":"{\"version\":\"1.0.0\"}"})
    );

    let response = anthropic_response_to_chat(json!({
        "id":"msg_tool","type":"message","role":"assistant","model":"worker",
        "content":[
            {"type":"thinking","thinking":"Need the file.","signature":"opaque-signature"},
            {"type":"tool_use","id":"call_next","name":"Write","input":{"path":"a.txt"}}
        ],
        "stop_reason":"tool_use","stop_sequence":null,
        "usage":{"input_tokens":9,"output_tokens":3}
    }))
    .unwrap();
    assert_eq!(response["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(
        response["choices"][0]["message"]["reasoning_content"],
        "Need the file."
    );
    assert_eq!(
        response["choices"][0]["message"]["reasoning_signature"],
        "opaque-signature"
    );
    assert_eq!(
        response["choices"][0]["message"]["tool_calls"][0]["function"],
        json!({"name":"Write","arguments":"{\"path\":\"a.txt\"}"})
    );
}

#[tokio::test]
async fn anthropic_tool_stream_becomes_chat_tool_deltas() {
    let source = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_stream\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"worker\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Need a file.\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"opaque-signature\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_read\",\"name\":\"Read\",\"input\":{}}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"file_\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"path\\\":\\\"a.txt\\\"}\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":3}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
    );
    let output = anthropic_sse_to_chat(stream::iter([Ok::<_, Infallible>(Bytes::from(source))]))
        .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
        .collect::<Vec<_>>()
        .await
        .join("");
    let chunks = output
        .split("\n\n")
        .filter_map(|block| block.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str::<Value>(data).unwrap())
        .collect::<Vec<_>>();

    assert!(chunks.iter().any(|chunk| {
        chunk.pointer("/choices/0/delta/tool_calls/0/function/name") == Some(&json!("Read"))
    }));
    assert!(chunks.iter().any(|chunk| {
        chunk.pointer("/choices/0/delta/reasoning_content") == Some(&json!("Need a file."))
    }));
    assert!(chunks.iter().any(|chunk| {
        chunk.pointer("/choices/0/delta/reasoning_signature") == Some(&json!("opaque-signature"))
    }));
    assert!(chunks.iter().any(|chunk| {
        chunk.pointer("/choices/0/delta/tool_calls/0/function/arguments")
            == Some(&json!("{\"file_"))
    }));
    assert!(
        chunks.iter().any(|chunk| {
            chunk.pointer("/choices/0/finish_reason") == Some(&json!("tool_calls"))
        })
    );
    assert!(output.ends_with("data: [DONE]\n\n"));
}
