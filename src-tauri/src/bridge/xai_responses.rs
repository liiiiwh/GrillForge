// xAI Responses request normalization adapted from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use serde_json::Value;
use std::collections::HashSet;

const SUPPORTED_TOOL_TYPES: &[&str] = &[
    "function",
    "web_search",
    "x_search",
    "image_generation",
    "collections_search",
    "file_search",
    "code_execution",
    "code_interpreter",
    "mcp",
    "shell",
];

/// Removes Codex/OpenAI-private request fields rejected by xAI's strict
/// Responses schema. The transform is deterministic and idempotent.
pub fn sanitize_xai_responses_request(body: &mut Value) -> bool {
    if !body.is_object() {
        return false;
    }
    let mut changed = false;
    for field in ["prompt_cache_retention", "safety_identifier"] {
        changed |= body
            .as_object_mut()
            .and_then(|object| object.remove(field))
            .is_some();
    }
    if targets_grok_45(body) {
        for field in [
            "presence_penalty",
            "presencePenalty",
            "frequency_penalty",
            "frequencyPenalty",
            "stop",
        ] {
            changed |= body
                .as_object_mut()
                .and_then(|object| object.remove(field))
                .is_some();
        }
    }
    changed |= remove_recursive(body, "external_web_access");
    changed |= promote_additional_tools(body);
    changed |= strip_null_reasoning_content(body);
    changed |= filter_tools(body);
    changed
}

fn targets_grok_45(body: &Value) -> bool {
    body.get("model")
        .and_then(Value::as_str)
        .and_then(|model| model.rsplit('/').next())
        .is_some_and(|model| model.trim().eq_ignore_ascii_case("grok-4.5"))
}

fn remove_recursive(value: &mut Value, field: &str) -> bool {
    match value {
        Value::Object(object) => {
            let mut changed = object.remove(field).is_some();
            for child in object.values_mut() {
                changed |= remove_recursive(child, field);
            }
            changed
        }
        Value::Array(items) => items.iter_mut().fold(false, |changed, child| {
            changed | remove_recursive(child, field)
        }),
        _ => false,
    }
}

fn promote_additional_tools(body: &mut Value) -> bool {
    let Some(input) = body.get("input").and_then(Value::as_array).cloned() else {
        return false;
    };
    if !input
        .iter()
        .any(|item| item.get("type").and_then(Value::as_str) == Some("additional_tools"))
    {
        return false;
    }
    let mut tools = body
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut seen = tools.iter().map(tool_key).collect::<HashSet<_>>();
    let mut filtered = Vec::with_capacity(input.len());
    for item in input {
        if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
            if let Some(additional) = item.get("tools").and_then(Value::as_array) {
                for tool in additional {
                    if seen.insert(tool_key(tool)) {
                        tools.push(tool.clone());
                    }
                }
            }
        } else {
            filtered.push(item);
        }
    }
    body["input"] = Value::Array(filtered);
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    true
}

fn tool_key(tool: &Value) -> String {
    let kind = tool.get("type").and_then(Value::as_str).unwrap_or("");
    if let Some(name) = tool.get("name").and_then(Value::as_str) {
        return format!("{kind}\0{name}");
    }
    if kind == "mcp" {
        if let Some(label) = tool.get("server_label").and_then(Value::as_str) {
            return format!("mcp\0{label}");
        }
    }
    tool.to_string()
}

fn strip_null_reasoning_content(body: &mut Value) -> bool {
    let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    for item in input {
        if item.get("type").and_then(Value::as_str) == Some("reasoning")
            && matches!(item.get("content"), Some(Value::Null))
        {
            item.as_object_mut()
                .expect("reasoning item is an object")
                .remove("content");
            changed = true;
        }
    }
    changed
}

fn filter_tools(body: &mut Value) -> bool {
    let Some(original) = body.get("tools").and_then(Value::as_array).cloned() else {
        return false;
    };
    let tools = original
        .iter()
        .filter(|tool| {
            tool.get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| SUPPORTED_TOOL_TYPES.contains(&kind))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut changed = tools.len() != original.len();
    if changed {
        if tools.is_empty() {
            body.as_object_mut()
                .expect("request object")
                .remove("tools");
        } else {
            body["tools"] = Value::Array(tools.clone());
        }
    }
    if body.get("tool_choice").is_some()
        && should_drop_choice(body.get("tool_choice").expect("present"), &tools)
    {
        body.as_object_mut()
            .expect("request object")
            .remove("tool_choice");
        changed = true;
    }
    changed
}

fn should_drop_choice(choice: &Value, tools: &[Value]) -> bool {
    if tools.is_empty() {
        return true;
    }
    let Some(choice) = choice.as_object() else {
        return false;
    };
    let kind = choice.get("type").and_then(Value::as_str).unwrap_or("");
    if !SUPPORTED_TOOL_TYPES.contains(&kind) {
        return !kind.is_empty();
    }
    if kind != "function" {
        return false;
    }
    let name = choice.get("name").and_then(Value::as_str).or_else(|| {
        choice
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
    });
    name.is_some_and(|name| {
        !tools.iter().any(|tool| {
            tool.get("type").and_then(Value::as_str) == Some("function")
                && tool.get("name").and_then(Value::as_str).or_else(|| {
                    tool.get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                }) == Some(name)
        })
    })
}
