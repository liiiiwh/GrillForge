// Minimal Codex Responses ↔ Chat Completions bridge adapted from cc-switch,
// commit 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::BridgeError;
use serde_json::{Value, json};

pub fn codex_response_to_chat(body: Value) -> Result<Value, BridgeError> {
    let object = body
        .as_object()
        .ok_or_else(|| invalid_request("body must be an object"))?;
    let model = required_string(object.get("model"), "model")?;
    let input = object
        .get("input")
        .ok_or_else(|| invalid_request("input is required"))?;
    let mut messages = Vec::new();
    if let Some(instructions) = object.get("instructions") {
        let text = text_value(instructions, "instructions")?;
        if !text.is_empty() {
            messages.push(json!({"role":"system","content":text}));
        }
    }
    append_input(input, &mut messages)?;
    if messages.is_empty() {
        return Err(invalid_request("input must contain at least one message"));
    }
    let mut result = json!({"model":model,"messages":messages});
    for key in ["stream", "temperature", "top_p", "parallel_tool_calls"] {
        if let Some(value) = object.get(key) {
            result[key] = value.clone();
        }
    }
    if let Some(value) = object.get("max_output_tokens") {
        result["max_tokens"] = value.clone();
    }
    if let Some(reasoning) = object
        .get("reasoning")
        .and_then(Value::as_object)
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(Value::as_str)
    {
        result["reasoning_effort"] = json!(reasoning);
    }
    if let Some(tools) = object.get("tools") {
        let tools = tools
            .as_array()
            .ok_or_else(|| invalid_request("tools must be an array"))?;
        let mut converted = Vec::with_capacity(tools.len());
        for (index, tool) in tools.iter().enumerate() {
            let tool = tool
                .as_object()
                .ok_or_else(|| invalid_request(format!("tools[{index}] must be an object")))?;
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(invalid_request(format!(
                    "tools[{index}].type must be function"
                )));
            }
            let name = required_string(tool.get("name"), &format!("tools[{index}].name"))?;
            let parameters = tool
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type":"object","properties":{}}));
            let mut function = json!({"name":name,"parameters":parameters});
            if let Some(description) = tool.get("description") {
                function["description"] = description.clone();
            }
            if let Some(strict) = tool.get("strict") {
                function["strict"] = strict.clone();
            }
            converted.push(json!({"type":"function","function":function}));
        }
        if !converted.is_empty() {
            result["tools"] = json!(converted);
        }
    }
    if let Some(choice) = object.get("tool_choice") {
        result["tool_choice"] = match choice {
            Value::String(value) => json!(value),
            Value::Object(choice)
                if choice.get("type").and_then(Value::as_str) == Some("function") =>
            {
                let name = required_string(choice.get("name"), "tool_choice.name")?;
                json!({"type":"function","function":{"name":name}})
            }
            _ => return Err(invalid_request("tool_choice is unsupported")),
        };
    }
    if object.get("stream").and_then(Value::as_bool) == Some(true) {
        result["stream_options"] = json!({"include_usage":true});
    }
    Ok(result)
}

pub fn chat_to_codex_response(body: Value) -> Result<Value, BridgeError> {
    let object = body
        .as_object()
        .ok_or_else(|| invalid_response("body must be an object"))?;
    if let Some(error) = object.get("error") {
        return Err(invalid_response(format!(
            "upstream error: {}",
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        )));
    }
    let choice = object
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_response("choices[0] must be an object"))?;
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_response("choices[0].message must be an object"))?;
    let response_id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("chatcmpl_grillforge");
    let model = required_response_string(object.get("model"), "model")?;
    let mut output = Vec::new();
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        if !content.is_empty() {
            output.push(json!({
                "id": format!("msg_{response_id}"),
                "type":"message",
                "role":"assistant",
                "status":"completed",
                "content":[{"type":"output_text","text":content,"annotations":[]}]
            }));
        }
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for (index, call) in tool_calls.iter().enumerate() {
            let call = call.as_object().ok_or_else(|| {
                invalid_response(format!("tool_calls[{index}] must be an object"))
            })?;
            let function = call
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    invalid_response(format!("tool_calls[{index}].function must be an object"))
                })?;
            let call_id =
                required_response_string(call.get("id"), &format!("tool_calls[{index}].id"))?;
            let name = required_response_string(
                function.get("name"),
                &format!("tool_calls[{index}].function.name"),
            )?;
            let arguments = required_response_string(
                function.get("arguments"),
                &format!("tool_calls[{index}].function.arguments"),
            )?;
            serde_json::from_str::<Value>(arguments).map_err(|_| {
                invalid_response(format!("tool_calls[{index}] arguments must be valid JSON"))
            })?;
            output.push(json!({
                "id": format!("fc_{response_id}_{index}"),
                "type":"function_call",
                "status":"completed",
                "call_id":call_id,
                "name":name,
                "arguments":arguments
            }));
        }
    }
    if output.is_empty() {
        return Err(invalid_response(
            "assistant response contains no text or tool calls",
        ));
    }
    let usage = object.get("usage").and_then(Value::as_object);
    let input_tokens = usage
        .and_then(|usage| usage.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .and_then(|usage| usage.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Ok(json!({
        "id": response_id.replace("chatcmpl", "resp"),
        "object":"response",
        "created_at":object.get("created").and_then(Value::as_u64).unwrap_or(0),
        "status":"completed",
        "model":model,
        "output":output,
        "error":null,
        "incomplete_details":null,
        "usage":{"input_tokens":input_tokens,"output_tokens":output_tokens,"total_tokens":input_tokens + output_tokens}
    }))
}

fn append_input(input: &Value, messages: &mut Vec<Value>) -> Result<(), BridgeError> {
    match input {
        Value::String(text) => messages.push(json!({"role":"user","content":text})),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                let item = item
                    .as_object()
                    .ok_or_else(|| invalid_request(format!("input[{index}] must be an object")))?;
                match item.get("type").and_then(Value::as_str) {
                    Some("message") => {
                        let role =
                            required_string(item.get("role"), &format!("input[{index}].role"))?;
                        let role = match role {
                            "developer" | "system" => "system",
                            "user" => "user",
                            "assistant" => "assistant",
                            _ => {
                                return Err(invalid_request(format!(
                                    "input[{index}].role is unsupported"
                                )));
                            }
                        };
                        let content = chat_content(item.get("content"), index)?;
                        messages.push(json!({"role":role,"content":content}));
                    }
                    Some("function_call") => {
                        let call_id = required_string(
                            item.get("call_id"),
                            &format!("input[{index}].call_id"),
                        )?;
                        let name =
                            required_string(item.get("name"), &format!("input[{index}].name"))?;
                        let arguments = required_string(
                            item.get("arguments"),
                            &format!("input[{index}].arguments"),
                        )?;
                        messages.push(json!({"role":"assistant","content":null,"tool_calls":[{"id":call_id,"type":"function","function":{"name":name,"arguments":arguments}}]}));
                    }
                    Some("function_call_output") => {
                        let call_id = required_string(
                            item.get("call_id"),
                            &format!("input[{index}].call_id"),
                        )?;
                        let output = text_value(
                            item.get("output").unwrap_or(&Value::Null),
                            &format!("input[{index}].output"),
                        )?;
                        messages
                            .push(json!({"role":"tool","tool_call_id":call_id,"content":output}));
                    }
                    Some("reasoning") => {
                        let summary = item
                            .get("summary")
                            .and_then(Value::as_array)
                            .map(|parts| {
                                parts
                                    .iter()
                                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                            .unwrap_or_default();
                        if !summary.is_empty() {
                            messages.push(json!({"role":"assistant","content":summary}));
                        }
                    }
                    Some(kind) => {
                        return Err(invalid_request(format!(
                            "input[{index}].type is unsupported: {kind}"
                        )));
                    }
                    None => {
                        return Err(invalid_request(format!(
                            "input[{index}].type must be a string"
                        )));
                    }
                }
            }
        }
        _ => return Err(invalid_request("input must be a string or array")),
    }
    Ok(())
}

fn chat_content(value: Option<&Value>, index: usize) -> Result<Value, BridgeError> {
    match value {
        Some(Value::String(text)) => Ok(json!(text)),
        Some(Value::Array(parts)) => {
            let mut text = Vec::new();
            for (part_index, part) in parts.iter().enumerate() {
                let kind = part.get("type").and_then(Value::as_str).ok_or_else(|| {
                    invalid_request(format!(
                        "input[{index}].content[{part_index}].type must be a string"
                    ))
                })?;
                match kind {
                    "input_text" | "output_text" => text.push(required_string(
                        part.get("text"),
                        &format!("input[{index}].content[{part_index}].text"),
                    )?),
                    _ => {
                        return Err(invalid_request(format!(
                            "input[{index}].content[{part_index}].type is unsupported: {kind}"
                        )));
                    }
                }
            }
            Ok(json!(text.join("\n")))
        }
        _ => Err(invalid_request(format!(
            "input[{index}].content is required"
        ))),
    }
}

fn text_value(value: &Value, field: &str) -> Result<String, BridgeError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Array(parts) => Ok(parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| part.as_str())
            })
            .collect::<Vec<_>>()
            .join("\n")),
        _ => Err(invalid_request(format!("{field} must be text"))),
    }
}

fn required_string<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_request(format!("{field} must be a non-empty string")))
}

fn required_response_string<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_response(format!("{field} must be a non-empty string")))
}

fn invalid_request(message: impl Into<String>) -> BridgeError {
    BridgeError::InvalidCodexRequest(message.into())
}

fn invalid_response(message: impl Into<String>) -> BridgeError {
    BridgeError::InvalidCodexResponse(message.into())
}
