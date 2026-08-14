// Reasoning-field normalization adapted from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use serde_json::Value;

pub(super) fn extract_reasoning_field_text(value: &Value) -> Option<String> {
    for key in ["reasoning_content", "reasoning"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            if !text.is_empty() {
                return Some(text.to_owned());
            }
        }
    }

    if let Some(reasoning) = value.get("reasoning") {
        for key in ["content", "text", "summary"] {
            if let Some(text) = reasoning.get(key).and_then(Value::as_str) {
                if !text.is_empty() {
                    return Some(text.to_owned());
                }
            }
        }
    }

    value
        .get("reasoning_details")
        .and_then(extract_reasoning_details_text)
}

fn extract_reasoning_details_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => (!text.is_empty()).then(|| text.to_owned()),
        Value::Array(parts) => join_reasoning_parts(parts),
        Value::Object(_) => extract_reasoning_detail_part_text(value),
        _ => None,
    }
}

fn extract_reasoning_detail_part_text(value: &Value) -> Option<String> {
    for key in ["text", "content", "summary"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            if !text.is_empty() {
                return Some(text.to_owned());
            }
        }
    }
    value
        .get("parts")
        .and_then(Value::as_array)
        .and_then(|parts| join_reasoning_parts(parts))
}

fn join_reasoning_parts(parts: &[Value]) -> Option<String> {
    let text = parts
        .iter()
        .filter_map(extract_reasoning_detail_part_text)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}
