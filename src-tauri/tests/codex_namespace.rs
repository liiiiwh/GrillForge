use bytes::Bytes;
use futures::{StreamExt, stream};
use grillforge_lib::bridge::{
    flatten_codex_namespaces, restore_codex_namespace_sse, restore_codex_namespaces,
};
use serde_json::{Value, json};

fn request() -> Value {
    json!({
        "model":"gpt",
        "tools":[
            {"type":"function","name":"plain","parameters":{"type":"object"}},
            {"type":"namespace","name":"mcp__files__","tools":[
                {"type":"function","name":"read","description":"Read a file","parameters":{"type":"object"}}
            ]}
        ],
        "input":[{"type":"function_call","namespace":"mcp__files__","name":"read","call_id":"call-1","arguments":"{}"}],
        "tool_choice":{"type":"namespace","name":"mcp__files__"}
    })
}

#[test]
fn flattens_namespace_tools_and_restores_the_response() {
    let mut request = request();
    let map = flatten_codex_namespaces(&mut request).unwrap();
    assert_eq!(request["tools"][1]["type"], "function");
    assert_eq!(request["tools"][1]["name"], "mcp__files____read");
    assert_eq!(request["input"][0]["name"], "mcp__files____read");
    assert!(request["input"][0].get("namespace").is_none());
    assert_eq!(request["tool_choice"], "auto");

    let mut response = json!({"output":[{
        "type":"function_call","name":"mcp__files____read","call_id":"call-1","arguments":"{}"
    }]});
    assert!(restore_codex_namespaces(&mut response, &map));
    assert_eq!(response["output"][0]["name"], "read");
    assert_eq!(response["output"][0]["namespace"], "mcp__files__");
}

#[test]
fn rejects_flat_name_collisions() {
    let mut request = json!({"tools":[
        {"type":"function","name":"mcp__files____read","parameters":{}},
        {"type":"namespace","name":"mcp__files__","tools":[
            {"type":"function","name":"read","parameters":{}}
        ]}
    ]});
    assert!(flatten_codex_namespaces(&mut request).is_err());
}

#[test]
fn promotes_and_flattens_tools_loaded_by_tool_search() {
    let mut request = json!({
        "model":"gpt",
        "tools":[{"type":"tool_search"}],
        "input":[{
            "type":"tool_search_output","call_id":"search-1","output":"loaded",
            "tools":[{"type":"namespace","name":"calendar","tools":[{
                "type":"function","name":"create","parameters":{"type":"object"}
            }]}]
        }]
    });
    let map = flatten_codex_namespaces(&mut request).unwrap();
    assert_eq!(request["tools"][1]["type"], "function");
    assert_eq!(request["tools"][1]["name"], "calendar__create");
    let mut response = json!({"output":[{
        "type":"function_call","name":"calendar__create","call_id":"call-1","arguments":"{}"
    }]});
    assert!(restore_codex_namespaces(&mut response, &map));
    assert_eq!(response["output"][0]["namespace"], "calendar");
    assert_eq!(response["output"][0]["name"], "create");
}

#[tokio::test]
async fn restores_namespace_in_streamed_response_items() {
    let mut request = request();
    let map = flatten_codex_namespaces(&mut request).unwrap();
    let event = json!({
        "type":"response.output_item.done",
        "item":{"type":"function_call","name":"mcp__files____read","call_id":"call-1","arguments":"{}"}
    });
    let source = stream::iter([Ok::<_, grillforge_lib::bridge::BridgeError>(Bytes::from(
        format!("event: response.output_item.done\ndata: {event}\n\n"),
    ))]);
    let bytes = restore_codex_namespace_sse(source, map)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .concat();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("\\\"name\\\":\\\"read\\\"") || text.contains("\"name\":\"read\""));
    assert!(text.contains("\"namespace\":\"mcp__files__\""));
}
