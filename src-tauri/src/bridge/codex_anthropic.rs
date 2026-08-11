// Minimal Codex Responses ↔ Anthropic Messages bridge adapted from cc-switch,
// commit 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::BridgeError;
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use url::Url;

const DEFAULT_MAX_TOKENS: u64 = 32_768;
const THINKING_PREFIX: &str = "grillforge-anthropic-thinking-v1:";
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_DOCUMENT_BYTES: usize = 32 * 1024 * 1024;
const MAX_URL_BYTES: usize = 8 * 1024;
const MAX_FILENAME_BYTES: usize = 255;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodexAnthropicCapabilities {
    pub reasoning: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexAnthropicContext {
    pub(crate) custom_tools: BTreeSet<String>,
}

pub fn codex_response_to_anthropic(
    body: Value,
    capabilities: CodexAnthropicCapabilities,
) -> Result<Value, BridgeError> {
    codex_response_to_anthropic_with_context(body, capabilities).map(|(body, _)| body)
}

pub fn codex_response_to_anthropic_with_context(
    mut body: Value,
    capabilities: CodexAnthropicCapabilities,
) -> Result<(Value, CodexAnthropicContext), BridgeError> {
    let context = normalize_custom_tools(&mut body)?;
    codex_response_to_anthropic_inner(body, capabilities).map(|body| (body, context))
}

fn codex_response_to_anthropic_inner(
    body: Value,
    capabilities: CodexAnthropicCapabilities,
) -> Result<Value, BridgeError> {
    let body = body
        .as_object()
        .ok_or_else(|| invalid_request("body must be an object"))?;
    reject_unknown(
        body,
        &[
            "model",
            "instructions",
            "input",
            "max_output_tokens",
            "stream",
            "store",
            "tools",
            "tool_choice",
            "reasoning",
            "include",
            "parallel_tool_calls",
            "prompt_cache_key",
            "service_tier",
        ],
        "request",
    )?;
    let model = non_empty_string(body.get("model"), "model")?;
    let max_tokens = match body.get("max_output_tokens") {
        Some(value) => value
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid_request("max_output_tokens must be a positive integer"))?,
        None => DEFAULT_MAX_TOKENS,
    };
    if let Some(store) = body.get("store") {
        if store.as_bool() != Some(false) {
            return Err(invalid_request(
                "store must be false for an Anthropic route",
            ));
        }
    }
    if let Some(include) = body.get("include") {
        let include = include
            .as_array()
            .filter(|include| !include.is_empty())
            .ok_or_else(|| invalid_request("include must be a non-empty array"))?;
        if include.len() != 1 || include[0].as_str() != Some("reasoning.encrypted_content") {
            return Err(invalid_request(
                "include supports only reasoning.encrypted_content",
            ));
        }
    }
    if let Some(cache_key) = body.get("prompt_cache_key") {
        let cache_key = non_empty_string(Some(cache_key), "prompt_cache_key")?;
        if cache_key.len() > 256 || cache_key.chars().any(char::is_control) {
            return Err(invalid_request(
                "prompt_cache_key must be at most 256 bytes and contain no control characters",
            ));
        }
    }
    if let Some(service_tier) = body.get("service_tier") {
        let service_tier = non_empty_string(Some(service_tier), "service_tier")?;
        if !matches!(service_tier, "auto" | "default" | "flex" | "priority") {
            return Err(invalid_request(
                "service_tier must be auto, default, flex, or priority",
            ));
        }
    }
    let parallel_tool_calls = match body.get("parallel_tool_calls") {
        Some(value) => Some(
            value
                .as_bool()
                .ok_or_else(|| invalid_request("parallel_tool_calls must be a boolean"))?,
        ),
        None => None,
    };

    let mut messages = Vec::new();
    convert_input(
        body.get("input")
            .ok_or_else(|| invalid_request("input is required"))?,
        &mut messages,
        capabilities,
    )?;
    if messages.is_empty() {
        return Err(invalid_request("input must contain at least one message"));
    }
    if messages[0].get("role").and_then(Value::as_str) != Some("user") {
        return Err(invalid_request(
            "Anthropic message history must begin with a user message",
        ));
    }
    let mut request = json!({"model":model,"max_tokens":max_tokens,"messages":messages});
    if let Some(instructions) = body.get("instructions") {
        request["system"] = json!(non_empty_string(Some(instructions), "instructions")?);
    }
    if let Some(stream) = body.get("stream") {
        request["stream"] = json!(
            stream
                .as_bool()
                .ok_or_else(|| invalid_request("stream must be a boolean"))?
        );
    }
    let has_tools = match body.get("tools") {
        Some(tools) => {
            let values = tools
                .as_array()
                .ok_or_else(|| invalid_request("tools must be an array"))?;
            if !values.is_empty() {
                request["tools"] = Value::Array(convert_tools(tools)?);
            }
            !values.is_empty()
        }
        None => false,
    };
    if let Some(choice) = body.get("tool_choice") {
        if !has_tools {
            return Err(invalid_request("tool_choice requires tools"));
        }
        request["tool_choice"] = convert_tool_choice(choice)?;
    }
    if parallel_tool_calls == Some(false) {
        if !has_tools {
            return Err(invalid_request("parallel_tool_calls requires tools"));
        }
        if request.get("tool_choice").is_none() {
            request["tool_choice"] = json!({"type":"auto"});
        }
        request["tool_choice"]["disable_parallel_tool_use"] = json!(true);
    }
    if let Some(reasoning) = body.get("reasoning") {
        if !capabilities.reasoning {
            return Err(invalid_request(
                "reasoning requires the explicit provider capability",
            ));
        }
        let reasoning = reasoning
            .as_object()
            .ok_or_else(|| invalid_request("reasoning must be an object"))?;
        reject_unknown(reasoning, &["effort"], "reasoning")?;
        let effort = non_empty_string(reasoning.get("effort"), "reasoning.effort")?;
        let budget = reasoning_budget(effort)?;
        if budget >= max_tokens {
            return Err(invalid_request(
                "max_output_tokens must be greater than the requested reasoning budget",
            ));
        }
        request["thinking"] = json!({"type":"enabled","budget_tokens":budget});
    }
    Ok(request)
}

pub fn anthropic_to_codex_response(
    body: Value,
    capabilities: CodexAnthropicCapabilities,
) -> Result<Value, BridgeError> {
    anthropic_to_codex_response_with_context(body, capabilities, &CodexAnthropicContext::default())
}

pub fn anthropic_to_codex_response_with_context(
    body: Value,
    capabilities: CodexAnthropicCapabilities,
    context: &CodexAnthropicContext,
) -> Result<Value, BridgeError> {
    let body = body
        .as_object()
        .ok_or_else(|| invalid_response("body must be an object"))?;
    if body.get("type").and_then(Value::as_str) == Some("error")
        || body.get("error").is_some_and(|error| !error.is_null())
    {
        return Err(anthropic_error(body));
    }
    let id = response_non_empty_string(body.get("id"), "id")?;
    let model = response_non_empty_string(body.get("model"), "model")?;
    let content = body
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response("content must be an array"))?;
    let mut output = Vec::new();
    let mut text = Vec::new();
    for (index, block) in content.iter().enumerate() {
        let block = block
            .as_object()
            .ok_or_else(|| invalid_response(format!("content[{index}] must be an object")))?;
        match response_non_empty_string(block.get("type"), &format!("content[{index}].type"))? {
            "text" => {
                reject_unknown_response(block, &["type", "text"], &format!("content[{index}]"))?;
                text.push(json!({
                    "type":"output_text",
                    "text":response_non_empty_string(block.get("text"), &format!("content[{index}].text"))?,
                    "annotations":[]
                }));
            }
            "tool_use" => {
                flush_text(&mut output, &mut text, id);
                reject_unknown_response(
                    block,
                    &["type", "id", "name", "input"],
                    &format!("content[{index}]"),
                )?;
                let call_id =
                    response_non_empty_string(block.get("id"), &format!("content[{index}].id"))?;
                let name = response_non_empty_string(
                    block.get("name"),
                    &format!("content[{index}].name"),
                )?;
                let input = block
                    .get("input")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        invalid_response(format!("content[{index}].input must be an object"))
                    })?;
                let arguments = serde_json::to_string(input).map_err(|_| {
                    invalid_response(format!("content[{index}].input could not be serialized"))
                })?;
                if context.custom_tools.contains(name) {
                    if input.len() != 1 {
                        return Err(invalid_response(format!(
                            "content[{index}].input must contain only input for custom tool {name}"
                        )));
                    }
                    let input = input.get("input").and_then(Value::as_str).filter(|value| !value.is_empty())
                        .ok_or_else(|| invalid_response(format!("content[{index}].input.input must be a non-empty string for custom tool {name}")))?;
                    output.push(json!({"id":format!("ct_{id}_{index}"),"type":"custom_tool_call","status":"completed","call_id":call_id,"name":name,"input":input}));
                } else {
                    output.push(json!({
                        "id":format!("fc_{id}_{index}"),"type":"function_call","status":"completed",
                        "call_id":call_id,"name":name,"arguments":arguments
                    }));
                }
            }
            "thinking" | "redacted_thinking" => {
                if !capabilities.reasoning {
                    return Err(invalid_response(
                        "Anthropic thinking requires the explicit reasoning capability",
                    ));
                }
                flush_text(&mut output, &mut text, id);
                let normalized = normalize_anthropic_thinking(block, &format!("content[{index}]"))?;
                let encrypted = encode_thinking(&normalized)?;
                let summary = normalized
                    .get("thinking")
                    .and_then(Value::as_str)
                    .map(|thinking| vec![json!({"type":"summary_text","text":thinking})])
                    .unwrap_or_default();
                output.push(json!({
                    "id":format!("rs_{id}_{index}"),"type":"reasoning","status":"completed",
                    "summary":summary,"encrypted_content":encrypted
                }));
            }
            other => {
                return Err(invalid_response(format!(
                    "content[{index}].type is unsupported: {other}"
                )));
            }
        }
    }
    flush_text(&mut output, &mut text, id);
    if output.is_empty() {
        return Err(invalid_response("content must contain output"));
    }
    let response_id = if id.starts_with("resp_") {
        id.to_owned()
    } else {
        format!("resp_{id}")
    };
    let usage = convert_usage(body.get("usage"))?;
    let (status, incomplete_details) = match body.get("stop_reason").and_then(Value::as_str) {
        Some("end_turn" | "tool_use" | "stop_sequence") => ("completed", Value::Null),
        Some("max_tokens" | "model_context_window_exceeded") => {
            ("incomplete", json!({"reason":"max_output_tokens"}))
        }
        Some(other) => {
            return Err(invalid_response(format!(
                "stop_reason is unsupported: {other}"
            )));
        }
        None => return Err(invalid_response("stop_reason must be a string")),
    };
    Ok(json!({
        "id":response_id,"object":"response","created_at":0,"status":status,"model":model,
        "output":output,
        "error":null,"incomplete_details":incomplete_details,"usage":usage
    }))
}

fn convert_input(
    input: &Value,
    messages: &mut Vec<Value>,
    capabilities: CodexAnthropicCapabilities,
) -> Result<(), BridgeError> {
    match input {
        Value::String(text) if !text.is_empty() => {
            messages.push(json!({"role":"user","content":[{"type":"text","text":text}]}));
        }
        Value::Array(items) if !items.is_empty() => {
            for (index, item) in items.iter().enumerate() {
                let item = item
                    .as_object()
                    .ok_or_else(|| invalid_request(format!("input[{index}] must be an object")))?;
                match item.get("type").and_then(Value::as_str) {
                    Some("message") => convert_message_item(item, index, messages)?,
                    Some("function_call") => convert_function_call(item, index, messages)?,
                    Some("function_call_output") => convert_function_output(item, index, messages)?,
                    Some("reasoning") => {
                        convert_reasoning_item(item, index, messages, capabilities)?
                    }
                    Some(other) => {
                        return Err(invalid_request(format!(
                            "input[{index}].type is unsupported: {other}"
                        )));
                    }
                    None if item.contains_key("role") => {
                        convert_message_item(item, index, messages)?
                    }
                    None => {
                        return Err(invalid_request(format!(
                            "input[{index}].type must be a string"
                        )));
                    }
                }
            }
        }
        _ => return Err(invalid_request("input must be a non-empty string or array")),
    }
    Ok(())
}

fn convert_message_item(
    item: &Map<String, Value>,
    index: usize,
    messages: &mut Vec<Value>,
) -> Result<(), BridgeError> {
    reject_unknown(
        item,
        &["type", "role", "content", "status", "id"],
        &format!("input[{index}]"),
    )?;
    let role = non_empty_string(item.get("role"), &format!("input[{index}].role"))?;
    if !matches!(role, "user" | "assistant") {
        return Err(invalid_request(format!(
            "input[{index}].role must be user or assistant"
        )));
    }
    if let Some(status) = item.get("status") {
        if status.as_str() != Some("completed") {
            return Err(invalid_request(format!(
                "input[{index}].status must be completed"
            )));
        }
    }
    if let Some(text) = item.get("content").and_then(Value::as_str) {
        if text.is_empty() {
            return Err(invalid_request(format!(
                "input[{index}].content must not be empty"
            )));
        }
        push_message_blocks(messages, role, vec![json!({"type":"text","text":text})]);
        return Ok(());
    }
    let parts = item
        .get("content")
        .and_then(Value::as_array)
        .filter(|parts| !parts.is_empty())
        .ok_or_else(|| {
            invalid_request(format!(
                "input[{index}].content must be a non-empty string or array"
            ))
        })?;
    let mut content = Vec::with_capacity(parts.len());
    for (part_index, part) in parts.iter().enumerate() {
        let field = format!("input[{index}].content[{part_index}]");
        let part = part
            .as_object()
            .ok_or_else(|| invalid_request(format!("{field} must be an object")))?;
        match part.get("type").and_then(Value::as_str) {
            Some("output_text") if role == "assistant" => {
                reject_unknown(part, &["type", "text"], &field)?;
                content.push(json!({"type":"text","text":non_empty_string(part.get("text"), &format!("{field}.text"))?}));
            }
            Some("input_text") if role == "user" => {
                reject_unknown(part, &["type", "text"], &field)?;
                content.push(json!({"type":"text","text":non_empty_string(part.get("text"), &format!("{field}.text"))?}));
            }
            Some("input_image") if role == "user" => {
                content.push(convert_input_image(part, &field)?);
            }
            Some("input_file") if role == "user" => {
                content.push(convert_input_file(part, &field)?);
            }
            Some(kind) => {
                return Err(invalid_request(format!(
                    "{field}.type is unsupported for role {role}: {kind}"
                )));
            }
            None => return Err(invalid_request(format!("{field}.type must be a string"))),
        }
    }
    push_message_blocks(messages, role, content);
    Ok(())
}

fn convert_function_call(
    item: &Map<String, Value>,
    index: usize,
    messages: &mut Vec<Value>,
) -> Result<(), BridgeError> {
    reject_unknown(
        item,
        &["type", "id", "call_id", "name", "arguments", "status"],
        &format!("input[{index}]"),
    )?;
    if let Some(status) = item.get("status") {
        if status.as_str() != Some("completed") {
            return Err(invalid_request(format!(
                "input[{index}].status must be completed"
            )));
        }
    }
    let call_id = non_empty_string(
        item.get("call_id").or_else(|| item.get("id")),
        &format!("input[{index}].call_id"),
    )?;
    let name = non_empty_string(item.get("name"), &format!("input[{index}].name"))?;
    let arguments = non_empty_string(item.get("arguments"), &format!("input[{index}].arguments"))?;
    let arguments: Value = serde_json::from_str(arguments)
        .map_err(|_| invalid_request(format!("input[{index}].arguments must be valid JSON")))?;
    if !arguments.is_object() {
        return Err(invalid_request(format!(
            "input[{index}].arguments must be a JSON object"
        )));
    }
    push_message_blocks(
        messages,
        "assistant",
        vec![json!({"type":"tool_use","id":call_id,"name":name,"input":arguments})],
    );
    Ok(())
}

fn convert_function_output(
    item: &Map<String, Value>,
    index: usize,
    messages: &mut Vec<Value>,
) -> Result<(), BridgeError> {
    reject_unknown(
        item,
        &["type", "call_id", "output"],
        &format!("input[{index}]"),
    )?;
    let call_id = non_empty_string(item.get("call_id"), &format!("input[{index}].call_id"))?;
    let output_field = format!("input[{index}].output");
    let output = match item.get("output") {
        Some(Value::String(output)) if !output.is_empty() => Value::String(output.clone()),
        Some(Value::Array(parts)) if !parts.is_empty() => {
            let mut converted = Vec::with_capacity(parts.len());
            for (part_index, part) in parts.iter().enumerate() {
                let field = format!("{output_field}[{part_index}]");
                let part = part
                    .as_object()
                    .ok_or_else(|| invalid_request(format!("{field} must be an object")))?;
                match part.get("type").and_then(Value::as_str) {
                    Some("input_text") => {
                        reject_unknown(part, &["type", "text"], &field)?;
                        converted.push(json!({"type":"text","text":non_empty_string(part.get("text"), &format!("{field}.text"))?}));
                    }
                    Some("input_image") => converted.push(convert_input_image(part, &field)?),
                    Some("input_file") => converted.push(convert_input_file(part, &field)?),
                    Some(kind) => {
                        return Err(invalid_request(format!(
                            "{field}.type is unsupported: {kind}"
                        )));
                    }
                    None => return Err(invalid_request(format!("{field}.type must be a string"))),
                }
            }
            Value::Array(converted)
        }
        _ => {
            return Err(invalid_request(format!(
                "{output_field} must be a non-empty string or array"
            )));
        }
    };
    push_message_blocks(
        messages,
        "user",
        vec![json!({"type":"tool_result","tool_use_id":call_id,"content":output})],
    );
    Ok(())
}

fn convert_input_image(part: &Map<String, Value>, field: &str) -> Result<Value, BridgeError> {
    reject_unknown(part, &["type", "image_url", "detail"], field)?;
    if let Some(detail) = part.get("detail") {
        if !matches!(detail.as_str(), Some("auto" | "low" | "high")) {
            return Err(invalid_request(format!(
                "{field}.detail must be auto, low, or high"
            )));
        }
    }
    let raw = non_empty_string(part.get("image_url"), &format!("{field}.image_url"))?;
    if raw.starts_with("data:") {
        let (media_type, data) = parse_data_url(raw, field)?;
        if !matches!(
            media_type,
            "image/jpeg" | "image/png" | "image/gif" | "image/webp"
        ) {
            return Err(invalid_request(format!(
                "{field}.image_url has an unsupported image media type"
            )));
        }
        let decoded =
            decode_canonical_base64(data, &format!("{field}.image_url"), MAX_IMAGE_BYTES)?;
        if decoded.is_empty() {
            return Err(invalid_request(format!(
                "{field}.image_url must not contain empty image data"
            )));
        }
        Ok(json!({"type":"image","source":{"type":"base64","media_type":media_type,"data":data}}))
    } else {
        validate_http_url(raw, &format!("{field}.image_url"))?;
        Ok(json!({"type":"image","source":{"type":"url","url":raw}}))
    }
}

fn convert_input_file(part: &Map<String, Value>, field: &str) -> Result<Value, BridgeError> {
    reject_unknown(part, &["type", "filename", "file_data", "file_url"], field)?;
    let filename = non_empty_string(part.get("filename"), &format!("{field}.filename"))?;
    if filename.len() > MAX_FILENAME_BYTES
        || filename
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(invalid_request(format!(
            "{field}.filename must be at most {MAX_FILENAME_BYTES} bytes and contain no path or control characters"
        )));
    }
    match (part.get("file_data"), part.get("file_url")) {
        (Some(data), None) => {
            let raw = non_empty_string(Some(data), &format!("{field}.file_data"))?;
            let (media_type, encoded) = parse_data_url(raw, field)?;
            if media_type != "application/pdf" {
                return Err(invalid_request(format!(
                    "{field}.file_data must be an application/pdf data URL"
                )));
            }
            let decoded = decode_canonical_base64(
                encoded,
                &format!("{field}.file_data"),
                MAX_DOCUMENT_BYTES,
            )?;
            if !decoded.starts_with(b"%PDF-") {
                return Err(invalid_request(format!(
                    "{field}.file_data must contain a PDF document"
                )));
            }
            Ok(
                json!({"type":"document","title":filename,"source":{"type":"base64","media_type":"application/pdf","data":encoded}}),
            )
        }
        (None, Some(url)) => {
            let raw = non_empty_string(Some(url), &format!("{field}.file_url"))?;
            validate_http_url(raw, &format!("{field}.file_url"))?;
            Ok(json!({"type":"document","title":filename,"source":{"type":"url","url":raw}}))
        }
        _ => Err(invalid_request(format!(
            "{field} must contain exactly one of file_data or file_url"
        ))),
    }
}

fn parse_data_url<'a>(raw: &'a str, field: &str) -> Result<(&'a str, &'a str), BridgeError> {
    let (prefix, data) = raw
        .split_once(',')
        .ok_or_else(|| invalid_request(format!("{field} must be a base64 data URL")))?;
    let media_type = prefix
        .strip_prefix("data:")
        .and_then(|prefix| prefix.strip_suffix(";base64"))
        .ok_or_else(|| invalid_request(format!("{field} must be a base64 data URL")))?;
    if media_type.is_empty() || data.is_empty() {
        return Err(invalid_request(format!(
            "{field} must be a non-empty base64 data URL"
        )));
    }
    Ok((media_type, data))
}

fn decode_canonical_base64(
    data: &str,
    field: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, BridgeError> {
    if data.len() > ((max_bytes + 2) / 3) * 4 {
        return Err(invalid_request(format!(
            "{field} exceeds the {max_bytes}-byte limit"
        )));
    }
    let decoded = STANDARD
        .decode(data)
        .map_err(|_| invalid_request(format!("{field} must contain valid canonical base64")))?;
    if decoded.len() > max_bytes || STANDARD.encode(&decoded) != data {
        return Err(invalid_request(format!(
            "{field} must contain valid canonical base64 within the {max_bytes}-byte limit"
        )));
    }
    Ok(decoded)
}

fn validate_http_url(raw: &str, field: &str) -> Result<(), BridgeError> {
    if raw.len() > MAX_URL_BYTES {
        return Err(invalid_request(format!(
            "{field} must be at most {MAX_URL_BYTES} bytes"
        )));
    }
    let url =
        Url::parse(raw).map_err(|_| invalid_request(format!("{field} must be a valid URL")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid_request(format!("{field} must use http or https")));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid_request(format!(
            "{field} must not contain credentials"
        )));
    }
    Ok(())
}

fn convert_reasoning_item(
    item: &Map<String, Value>,
    index: usize,
    messages: &mut Vec<Value>,
    capabilities: CodexAnthropicCapabilities,
) -> Result<(), BridgeError> {
    if !capabilities.reasoning {
        return Err(invalid_request(
            "reasoning history requires the explicit provider capability",
        ));
    }
    reject_unknown(
        item,
        &["type", "id", "status", "summary", "encrypted_content"],
        &format!("input[{index}]"),
    )?;
    if let Some(status) = item.get("status") {
        if status.as_str() != Some("completed") {
            return Err(invalid_request(format!(
                "input[{index}].status must be completed"
            )));
        }
    }
    let summary = item
        .get("summary")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_request(format!("input[{index}].summary must be an array")))?;
    for (part_index, part) in summary.iter().enumerate() {
        let field = format!("input[{index}].summary[{part_index}]");
        let part = part
            .as_object()
            .ok_or_else(|| invalid_request(format!("{field} must be an object")))?;
        reject_unknown(part, &["type", "text"], &field)?;
        if part.get("type").and_then(Value::as_str) != Some("summary_text") {
            return Err(invalid_request(format!(
                "{field}.type must be summary_text"
            )));
        }
        non_empty_string(part.get("text"), &format!("{field}.text"))?;
    }
    let encrypted = non_empty_string(
        item.get("encrypted_content"),
        &format!("input[{index}].encrypted_content"),
    )?;
    let block = decode_thinking(encrypted, &format!("input[{index}].encrypted_content"))?;
    push_message_blocks(messages, "assistant", vec![block]);
    Ok(())
}

fn reasoning_budget(effort: &str) -> Result<u64, BridgeError> {
    match effort {
        "minimal" | "low" => Ok(2_048),
        "medium" => Ok(8_192),
        "high" => Ok(16_384),
        "xhigh" | "max" => Ok(24_576),
        _ => Err(invalid_request(
            "reasoning.effort must be minimal, low, medium, high, xhigh, or max",
        )),
    }
}

fn normalize_anthropic_thinking(
    block: &Map<String, Value>,
    field: &str,
) -> Result<Value, BridgeError> {
    match block.get("type").and_then(Value::as_str) {
        Some("thinking") => {
            reject_unknown_response(block, &["type", "thinking", "signature"], field)?;
            Ok(json!({
                "type":"thinking",
                "thinking":response_non_empty_string(block.get("thinking"), &format!("{field}.thinking"))?,
                "signature":response_non_empty_string(block.get("signature"), &format!("{field}.signature"))?
            }))
        }
        Some("redacted_thinking") => {
            reject_unknown_response(block, &["type", "data"], field)?;
            Ok(json!({
                "type":"redacted_thinking",
                "data":response_non_empty_string(block.get("data"), &format!("{field}.data"))?
            }))
        }
        _ => Err(invalid_response(format!(
            "{field}.type must be thinking or redacted_thinking"
        ))),
    }
}

pub(crate) fn encode_thinking(block: &Value) -> Result<String, BridgeError> {
    let bytes = serde_json::to_vec(block)
        .map_err(|_| invalid_response("Anthropic thinking could not be encoded"))?;
    Ok(format!(
        "{THINKING_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(bytes)
    ))
}

fn decode_thinking(encoded: &str, field: &str) -> Result<Value, BridgeError> {
    let payload = encoded.strip_prefix(THINKING_PREFIX).ok_or_else(|| {
        invalid_request(format!("{field} is not a GrillForge Anthropic envelope"))
    })?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| invalid_request(format!("{field} is not valid canonical base64")))?;
    if URL_SAFE_NO_PAD.encode(&bytes) != payload {
        return Err(invalid_request(format!(
            "{field} is not valid canonical base64"
        )));
    }
    let block: Value = serde_json::from_slice(&bytes)
        .map_err(|_| invalid_request(format!("{field} is not valid JSON")))?;
    let block = block
        .as_object()
        .ok_or_else(|| invalid_request(format!("{field} must contain an object")))?;
    match block.get("type").and_then(Value::as_str) {
        Some("thinking") => {
            reject_unknown(block, &["type", "thinking", "signature"], field)?;
            Ok(json!({
                "type":"thinking",
                "thinking":non_empty_string(block.get("thinking"), &format!("{field}.thinking"))?,
                "signature":non_empty_string(block.get("signature"), &format!("{field}.signature"))?
            }))
        }
        Some("redacted_thinking") => {
            reject_unknown(block, &["type", "data"], field)?;
            Ok(json!({
                "type":"redacted_thinking",
                "data":non_empty_string(block.get("data"), &format!("{field}.data"))?
            }))
        }
        _ => Err(invalid_request(format!(
            "{field} contains an unsupported Anthropic thinking block"
        ))),
    }
}

fn push_message_blocks(messages: &mut Vec<Value>, role: &str, mut blocks: Vec<Value>) {
    if let Some(content) = messages
        .last_mut()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some(role))
        .and_then(|message| message.get_mut("content"))
        .and_then(Value::as_array_mut)
    {
        content.append(&mut blocks);
    } else {
        messages.push(json!({"role":role,"content":blocks}));
    }
}

fn normalize_custom_tools(body: &mut Value) -> Result<CodexAnthropicContext, BridgeError> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| invalid_request("body must be an object"))?;
    let mut context = CodexAnthropicContext::default();
    if let Some(tools) = object.get_mut("tools") {
        let tools = tools
            .as_array_mut()
            .ok_or_else(|| invalid_request("tools must be an array"))?;
        for (index, tool) in tools.iter_mut().enumerate() {
            let field = format!("tools[{index}]");
            let definition = tool
                .as_object()
                .ok_or_else(|| invalid_request(format!("{field} must be an object")))?;
            if definition.get("type").and_then(Value::as_str) != Some("custom") {
                continue;
            }
            reject_unknown(
                definition,
                &["type", "name", "description", "format"],
                &field,
            )?;
            let name =
                non_empty_string(definition.get("name"), &format!("{field}.name"))?.to_owned();
            if !context.custom_tools.insert(name.clone()) {
                return Err(invalid_request(format!("duplicate tool name: {name}")));
            }
            if let Some(description) = definition.get("description") {
                non_empty_string(Some(description), &format!("{field}.description"))?;
            }
            let format = definition
                .get("format")
                .and_then(Value::as_object)
                .ok_or_else(|| invalid_request(format!("{field}.format must be an object")))?;
            reject_unknown(
                format,
                &["type", "syntax", "definition"],
                &format!("{field}.format"),
            )?;
            if format.get("type").and_then(Value::as_str) != Some("grammar")
                || format.get("syntax").and_then(Value::as_str) != Some("lark")
            {
                return Err(invalid_request(format!(
                    "{field}.format must be grammar with lark syntax"
                )));
            }
            non_empty_string(
                format.get("definition"),
                &format!("{field}.format.definition"),
            )?;
            let preserved = serde_json::to_string(definition)
                .map_err(|_| invalid_request(format!("{field} could not be serialized")))?;
            *tool = json!({
                "type":"function","name":name,
                "description":format!("Preserved Codex custom tool definition:\n{preserved}"),
                "parameters":{"type":"object","properties":{"input":{"type":"string"}},"required":["input"],"additionalProperties":false}
            });
        }
    }
    if let Some(choice) = object.get_mut("tool_choice") {
        if choice.get("type").and_then(Value::as_str) == Some("custom") {
            let choice_object = choice.as_object().expect("object checked by field access");
            reject_unknown(choice_object, &["type", "name"], "tool_choice")?;
            let name = non_empty_string(choice_object.get("name"), "tool_choice.name")?;
            if !context.custom_tools.contains(name) {
                return Err(invalid_request(format!(
                    "tool_choice references undeclared custom tool: {name}"
                )));
            }
            *choice = json!({"type":"function","name":name});
        }
    }
    if let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) {
        for (index, item) in input.iter_mut().enumerate() {
            let Some(item_object) = item.as_object_mut() else {
                continue;
            };
            match item_object.get("type").and_then(Value::as_str) {
                Some("custom_tool_call") => {
                    reject_unknown(
                        item_object,
                        &["type", "id", "call_id", "name", "input", "status"],
                        &format!("input[{index}]"),
                    )?;
                    let name =
                        non_empty_string(item_object.get("name"), &format!("input[{index}].name"))?;
                    if !context.custom_tools.contains(name) {
                        return Err(invalid_request(format!(
                            "input[{index}] references undeclared custom tool: {name}"
                        )));
                    }
                    let raw = non_empty_string(
                        item_object.get("input"),
                        &format!("input[{index}].input"),
                    )?
                    .to_owned();
                    item_object.insert("type".into(), json!("function_call"));
                    item_object.insert(
                        "arguments".into(),
                        json!(serde_json::to_string(&json!({"input":raw})).expect("serializable")),
                    );
                    item_object.remove("input");
                }
                Some("custom_tool_call_output") => {
                    item_object.insert("type".into(), json!("function_call_output"));
                }
                _ => {}
            }
        }
    }
    Ok(context)
}

fn convert_tools(value: &Value) -> Result<Vec<Value>, BridgeError> {
    let tools = value
        .as_array()
        .filter(|tools| !tools.is_empty())
        .ok_or_else(|| invalid_request("tools must be a non-empty array"))?;
    let mut converted = Vec::with_capacity(tools.len());
    let mut names = std::collections::HashSet::new();
    for (index, tool) in tools.iter().enumerate() {
        let field = format!("tools[{index}]");
        let tool = tool
            .as_object()
            .ok_or_else(|| invalid_request(format!("{field} must be an object")))?;
        reject_unknown(
            tool,
            &["type", "name", "description", "parameters", "strict"],
            &field,
        )?;
        if tool.get("type").and_then(Value::as_str) != Some("function") {
            return Err(invalid_request(format!("{field}.type must be function")));
        }
        let name = non_empty_string(tool.get("name"), &format!("{field}.name"))?;
        if !names.insert(name) {
            return Err(invalid_request(format!("duplicate tool name: {name}")));
        }
        let schema = tool
            .get("parameters")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_request(format!("{field}.parameters must be an object")))?;
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(invalid_request(format!(
                "{field}.parameters.type must be object"
            )));
        }
        let mut converted_tool = json!({"name":name,"input_schema":schema});
        if let Some(description) = tool.get("description") {
            converted_tool["description"] = json!(non_empty_string(
                Some(description),
                &format!("{field}.description")
            )?);
        }
        if let Some(strict) = tool.get("strict") {
            converted_tool["strict"] = json!(
                strict
                    .as_bool()
                    .ok_or_else(|| invalid_request(format!("{field}.strict must be a boolean")))?
            );
        }
        converted.push(converted_tool);
    }
    Ok(converted)
}

fn convert_tool_choice(value: &Value) -> Result<Value, BridgeError> {
    if let Some(choice) = value.as_str() {
        return match choice {
            "auto" => Ok(json!({"type":"auto"})),
            "required" => Ok(json!({"type":"any"})),
            _ => Err(invalid_request("tool_choice is unsupported")),
        };
    }
    let choice = value
        .as_object()
        .ok_or_else(|| invalid_request("tool_choice must be a string or object"))?;
    reject_unknown(choice, &["type", "name"], "tool_choice")?;
    if choice.get("type").and_then(Value::as_str) != Some("function") {
        return Err(invalid_request("tool_choice.type must be function"));
    }
    Ok(json!({
        "type":"tool",
        "name":non_empty_string(choice.get("name"), "tool_choice.name")?
    }))
}

fn flush_text(output: &mut Vec<Value>, text: &mut Vec<Value>, id: &str) {
    if !text.is_empty() {
        let index = output.len();
        output.push(json!({
            "id":format!("msg_{id}_{index}"),"type":"message","role":"assistant",
            "status":"completed","content":std::mem::take(text)
        }));
    }
}

fn convert_usage(value: Option<&Value>) -> Result<Value, BridgeError> {
    let usage = value
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_response("usage must be an object"))?;
    let fresh = response_u64(usage.get("input_tokens"), "usage.input_tokens")?;
    let output = response_u64(usage.get("output_tokens"), "usage.output_tokens")?;
    let cache_read = optional_u64(
        usage.get("cache_read_input_tokens"),
        "usage.cache_read_input_tokens",
    )?;
    let cache_write = optional_u64(
        usage.get("cache_creation_input_tokens"),
        "usage.cache_creation_input_tokens",
    )?;
    let input = fresh
        .checked_add(cache_read)
        .and_then(|value| value.checked_add(cache_write))
        .ok_or_else(|| invalid_response("usage input token count overflowed"))?;
    let total = input
        .checked_add(output)
        .ok_or_else(|| invalid_response("usage total token count overflowed"))?;
    let mut result = json!({"input_tokens":input,"output_tokens":output,"total_tokens":total});
    if cache_read > 0 || cache_write > 0 {
        result["input_tokens_details"] =
            json!({"cached_tokens":cache_read,"cache_write_tokens":cache_write});
    }
    Ok(result)
}

fn anthropic_error(body: &Map<String, Value>) -> BridgeError {
    let error = body.get("error").and_then(Value::as_object).unwrap_or(body);
    let kind = error
        .get("type")
        .and_then(Value::as_str)
        .map(safe_kind)
        .unwrap_or_else(|| "upstream_error".into());
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .map(safe_message)
        .unwrap_or_else(|| "Anthropic upstream returned an error envelope".into());
    invalid_response(format!("Anthropic upstream failed ({kind}): {message}"))
}

fn safe_kind(value: &str) -> String {
    let value: String = value
        .chars()
        .take(64)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if value.is_empty() {
        "upstream_error".into()
    } else {
        value
    }
}

fn safe_message(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(512)
        .collect()
}

fn reject_unknown(
    object: &Map<String, Value>,
    allowed: &[&str],
    field: &str,
) -> Result<(), BridgeError> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid_request(format!("unsupported field: {field}.{key}")));
    }
    Ok(())
}

fn reject_unknown_response(
    object: &Map<String, Value>,
    allowed: &[&str],
    field: &str,
) -> Result<(), BridgeError> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid_response(format!(
            "unsupported field: {field}.{key}"
        )));
    }
    Ok(())
}

fn non_empty_string<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_request(format!("{field} must be a non-empty string")))
}

fn response_non_empty_string<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_response(format!("{field} must be a non-empty string")))
}

fn response_u64(value: Option<&Value>, field: &str) -> Result<u64, BridgeError> {
    value
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_response(format!("{field} must be an unsigned integer")))
}

fn optional_u64(value: Option<&Value>, field: &str) -> Result<u64, BridgeError> {
    match value {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| invalid_response(format!("{field} must be an unsigned integer"))),
        None => Ok(0),
    }
}

fn invalid_request(message: impl Into<String>) -> BridgeError {
    BridgeError::InvalidCodexRequest(message.into())
}

fn invalid_response(message: impl Into<String>) -> BridgeError {
    BridgeError::InvalidCodexResponse(message.into())
}
