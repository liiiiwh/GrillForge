use bytes::Bytes;
use futures::{StreamExt, stream};
use grillforge_lib::bridge::{
    chat_sse_to_codex_responses_with_context, chat_to_codex_response_with_context,
    codex_response_to_chat_with_context,
};
use serde_json::json;

#[test]
fn custom_and_tool_search_calls_round_trip_through_chat() {
    let request = json!({
        "model":"upstream-chat",
        "tools":[
            {"type":"custom","name":"apply_patch","description":"Apply a patch"},
            {"type":"tool_search"}
        ],
        "tool_choice":{"type":"custom","name":"apply_patch"},
        "input":[
            {"type":"custom_tool_call","call_id":"call_patch","name":"apply_patch","input":"*** Begin Patch\n*** End Patch"},
            {"type":"custom_tool_call_output","call_id":"call_patch","output":"done"},
            {"type":"tool_search_call","call_id":"call_search","arguments":{"query":"mail"}},
            {"type":"tool_search_output","call_id":"call_search","output":"loaded"},
            {"type":"message","role":"user","content":"continue"}
        ]
    });
    let (chat, context) = codex_response_to_chat_with_context(request).unwrap();
    assert_eq!(chat["tools"][0]["function"]["name"], "apply_patch");
    assert_eq!(chat["tools"][1]["function"]["name"], "tool_search");
    assert_eq!(chat["tool_choice"]["function"]["name"], "apply_patch");
    assert_eq!(
        chat["messages"][0]["tool_calls"][0]["function"]["arguments"],
        r#"{"input":"*** Begin Patch\n*** End Patch"}"#
    );

    let custom = chat_to_codex_response_with_context(json!({
        "id":"chatcmpl_1","model":"upstream-chat","choices":[{"message":{"tool_calls":[{
            "id":"call_new","type":"function","function":{"name":"apply_patch","arguments":"{\"input\":\"PATCH\"}"}
        }]}}]
    }), &context).unwrap();
    assert_eq!(custom["output"][0]["type"], "custom_tool_call");
    assert_eq!(custom["output"][0]["input"], "PATCH");

    let search = chat_to_codex_response_with_context(json!({
        "id":"chatcmpl_2","model":"upstream-chat","choices":[{"message":{"tool_calls":[{
            "id":"call_search_2","type":"function","function":{"name":"tool_search","arguments":"{\"query\":\"calendar\"}"}
        }]}}]
    }), &context).unwrap();
    assert_eq!(search["output"][0]["type"], "tool_search_call");
    assert_eq!(search["output"][0]["arguments"]["query"], "calendar");
}

#[test]
fn dynamically_loaded_namespace_tool_round_trips_through_chat() {
    let request = json!({
        "model":"upstream-chat",
        "tools":[{"type":"tool_search"}],
        "input":[{
            "type":"tool_search_output","call_id":"search","tools":[{
                "type":"namespace","name":"mcp__mail","tools":[{
                    "type":"function","name":"search","parameters":{"type":"object"}
                }]
            }]
        }, {"type":"message","role":"user","content":"search"}]
    });
    let (chat, context) = codex_response_to_chat_with_context(request).unwrap();
    assert!(
        chat["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "mcp__mail__search")
    );
    let response = chat_to_codex_response_with_context(json!({
        "id":"chatcmpl_3","model":"upstream-chat","choices":[{"message":{"tool_calls":[{
            "id":"call_mail","type":"function","function":{"name":"mcp__mail__search","arguments":"{\"query\":\"unread\"}"}
        }]}}]
    }), &context).unwrap();
    assert_eq!(response["output"][0]["namespace"], "mcp__mail");
    assert_eq!(response["output"][0]["name"], "search");
}

#[tokio::test]
async fn streaming_custom_tool_restores_custom_events() {
    let (_, context) = codex_response_to_chat_with_context(json!({
        "model":"upstream-chat","stream":true,
        "tools":[{"type":"custom","name":"apply_patch"}],
        "input":"patch"
    }))
    .unwrap();
    let source = stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(
        b"data: {\"id\":\"chatcmpl_stream\",\"model\":\"upstream-chat\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_patch\",\"function\":{\"name\":\"apply_patch\",\"arguments\":\"{\\\"input\\\":\\\"PATCH\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n"
    ))]);
    let output = chat_sse_to_codex_responses_with_context(source, context)
        .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
        .collect::<Vec<_>>()
        .await
        .join("");
    assert!(output.contains("response.custom_tool_call_input.done"));
    assert!(output.contains("\"type\":\"custom_tool_call\""));
    assert!(output.contains("\"input\":\"PATCH\""));
}
