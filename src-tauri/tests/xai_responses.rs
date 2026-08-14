use grillforge_lib::bridge::sanitize_xai_responses_request;
use serde_json::json;

#[test]
fn xai_sanitizer_lifts_tools_and_removes_codex_private_fields() {
    let mut request = json!({
        "model":"vendor/grok-4.5",
        "prompt_cache_retention":"24h",
        "safety_identifier":"private",
        "presence_penalty":1,
        "external_web_access":true,
        "input":[
            {"type":"reasoning","content":null,"summary":[]},
            {"type":"additional_tools","tools":[{"type":"function","name":"loaded"}]}
        ],
        "tools":[
            {"type":"function","name":"kept","parameters":{"external_web_access":false}},
            {"type":"custom","name":"removed"},
            {"type":"tool_search"}
        ],
        "tool_choice":{"type":"custom","name":"removed"}
    });
    assert!(sanitize_xai_responses_request(&mut request));
    let encoded = request.to_string();
    assert!(!encoded.contains("external_web_access"));
    assert!(request.get("prompt_cache_retention").is_none());
    assert!(request.get("presence_penalty").is_none());
    assert!(request["input"][0].get("content").is_none());
    let names = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool.get("name").and_then(|name| name.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(names, ["kept", "loaded"]);
    assert!(request.get("tool_choice").is_none());
    assert!(!sanitize_xai_responses_request(&mut request));
}
