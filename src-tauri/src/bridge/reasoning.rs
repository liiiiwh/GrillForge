// Opaque Responses reasoning transport adapted from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Map, Value, json};

pub(crate) const REASONING_PREFIX: &str = "grillforge-openai-reasoning-v1:";

pub(crate) fn reasoning_item_to_anthropic_block(item: &Value) -> Result<Value, String> {
    let item = normalize_reasoning_item(item, "reasoning item")?;
    let summary = summary_text(&item);
    let opaque = item
        .get("encrypted_content")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
        || has_content(&item);
    if opaque {
        let signature = encode(&item)?;
        if summary.is_empty() {
            return Ok(json!({"type":"redacted_thinking","data":signature}));
        }
        return Ok(json!({
            "type":"thinking","thinking":summary,"signature":signature
        }));
    }
    if summary.is_empty() {
        return Err("reasoning item has neither summary nor encrypted_content".into());
    }
    Ok(json!({"type":"thinking","thinking":summary}))
}

pub(crate) fn anthropic_block_to_reasoning_item(
    block: &Map<String, Value>,
    field: &str,
) -> Result<Value, String> {
    let kind = block
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{field}.type must be a string"))?;
    let encoded = match kind {
        "thinking" => {
            reject_unknown(block, &["type", "thinking", "signature"], field)?;
            block
                .get("thinking")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("{field}.thinking must be a non-empty string"))?;
            block
                .get("signature")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("{field}.signature is required to replay Responses reasoning")
                })?
        }
        "redacted_thinking" => {
            reject_unknown(block, &["type", "data"], field)?;
            block
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{field}.data must be a string"))?
        }
        _ => {
            return Err(format!(
                "{field}.type must be thinking or redacted_thinking"
            ));
        }
    };
    decode(encoded, field)
}

pub(crate) fn normalize_reasoning_item(item: &Value, field: &str) -> Result<Value, String> {
    let object = item
        .as_object()
        .ok_or_else(|| format!("{field} must be an object"))?;
    reject_unknown(
        object,
        &[
            "id",
            "type",
            "status",
            "summary",
            "content",
            "encrypted_content",
        ],
        field,
    )?;
    if object.get("type").and_then(Value::as_str) != Some("reasoning") {
        return Err(format!("{field}.type must be reasoning"));
    }
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{field}.id must be a non-empty string"))?;
    let summary = object
        .get("summary")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{field}.summary must be an array"))?;
    let mut normalized_summary = Vec::with_capacity(summary.len());
    for (index, part) in summary.iter().enumerate() {
        let part_field = format!("{field}.summary[{index}]");
        let part = part
            .as_object()
            .ok_or_else(|| format!("{part_field} must be an object"))?;
        reject_unknown(part, &["type", "text"], &part_field)?;
        if part.get("type").and_then(Value::as_str) != Some("summary_text") {
            return Err(format!("{part_field}.type must be summary_text"));
        }
        let text = part
            .get("text")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{part_field}.text must be a non-empty string"))?;
        normalized_summary.push(json!({"type":"summary_text","text":text}));
    }
    let mut normalized = json!({"id":id,"type":"reasoning","summary":normalized_summary});
    if let Some(status) = object.get("status") {
        let status = status
            .as_str()
            .filter(|value| matches!(*value, "in_progress" | "completed" | "incomplete"))
            .ok_or_else(|| format!("{field}.status is unsupported"))?;
        normalized["status"] = json!(status);
    }
    if let Some(encrypted) = object.get("encrypted_content") {
        let encrypted = encrypted
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{field}.encrypted_content must be a non-empty string"))?;
        normalized["encrypted_content"] = json!(encrypted);
    }
    if let Some(content) = object.get("content") {
        let content = content
            .as_array()
            .ok_or_else(|| format!("{field}.content must be an array"))?;
        let mut normalized_content = Vec::with_capacity(content.len());
        for (index, part) in content.iter().enumerate() {
            let part_field = format!("{field}.content[{index}]");
            let part = part
                .as_object()
                .ok_or_else(|| format!("{part_field} must be an object"))?;
            reject_unknown(part, &["type", "text"], &part_field)?;
            if part.get("type").and_then(Value::as_str) != Some("reasoning_text") {
                return Err(format!("{part_field}.type must be reasoning_text"));
            }
            let text = part
                .get("text")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("{part_field}.text must be a non-empty string"))?;
            normalized_content.push(json!({"type":"reasoning_text","text":text}));
        }
        normalized["content"] = json!(normalized_content);
    }
    Ok(normalized)
}

pub(crate) fn summary_text(item: &Value) -> String {
    item.get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect()
}

pub(crate) fn content_text(item: &Value) -> String {
    item.get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect()
}

pub(crate) fn has_content(item: &Value) -> bool {
    item.get("content")
        .and_then(Value::as_array)
        .is_some_and(|content| !content.is_empty())
}

pub(crate) fn encode(item: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(item).map_err(|_| "reasoning item could not be encoded")?;
    Ok(format!(
        "{REASONING_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(bytes)
    ))
}

fn decode(encoded: &str, field: &str) -> Result<Value, String> {
    let payload = encoded
        .strip_prefix(REASONING_PREFIX)
        .ok_or_else(|| format!("{field} does not contain a GrillForge Responses signature"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| format!("{field} contains an invalid Responses signature"))?;
    if URL_SAFE_NO_PAD.encode(&bytes) != payload {
        return Err(format!(
            "{field} contains a non-canonical Responses signature"
        ));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| format!("{field} contains an invalid Responses signature"))?;
    normalize_reasoning_item(&value, field)
}

fn reject_unknown(
    object: &Map<String, Value>,
    allowed: &[&str],
    field: &str,
) -> Result<(), String> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("unsupported field: {field}.{key}"));
    }
    Ok(())
}
