// Minimal Gemini Native streaming adapter derived from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::BridgeError;
use super::gemini::{anthropic_usage, map_finish_reason};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct ToolCall {
    id: String,
    name: String,
    input: Value,
}

pub(crate) fn gemini_sse_to_anthropic<S, E>(
    upstream: S,
) -> impl Stream<Item = Result<Bytes, BridgeError>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    async_stream::stream! {
        let mut buffer = String::new();
        let mut remainder = Vec::new();
        let mut started = false;
        let mut text_open = false;
        let mut accumulated_text = String::new();
        let mut message_id = String::new();
        let mut model = String::new();
        let mut finish_reason: Option<String> = None;
        let mut usage: Option<Value> = None;
        let mut tool_calls = Vec::<ToolCall>::new();
        tokio::pin!(upstream);

        while let Some(chunk) = upstream.next().await {
            let bytes = match chunk {
                Ok(bytes) => bytes,
                Err(error) => {
                    yield Err(BridgeError::GeminiRequestFailed(error.to_string()));
                    return;
                }
            };
            if let Err(error) = append_utf8(&mut buffer, &mut remainder, &bytes) {
                yield Err(error);
                return;
            }
            while let Some(block) = take_sse_block(&mut buffer) {
                let data = match parse_data_block(&block) {
                    Ok(Some(data)) if data == "[DONE]" => continue,
                    Ok(Some(data)) => data,
                    Ok(None) => continue,
                    Err(error) => {
                        yield Err(error);
                        return;
                    }
                };
                let chunk: Value = match serde_json::from_str(&data) {
                    Ok(chunk) => chunk,
                    Err(_) => {
                        yield Err(invalid_response("SSE data must be valid JSON"));
                        return;
                    }
                };
                if message_id.is_empty() {
                    message_id = chunk.get("responseId").and_then(Value::as_str).unwrap_or("").to_string();
                }
                if model.is_empty() {
                    model = chunk.get("modelVersion").and_then(Value::as_str).unwrap_or("").to_string();
                }
                if let Some(value) = chunk.get("usageMetadata") {
                    usage = Some(value.clone());
                }
                if !started {
                    yield Ok(event("message_start", json!({
                        "type":"message_start",
                        "message":{
                            "id":message_id,
                            "type":"message",
                            "role":"assistant",
                            "content":[],
                            "model":model,
                            "stop_reason":Value::Null,
                            "stop_sequence":Value::Null,
                            "usage":anthropic_usage(chunk.get("usageMetadata"))
                        }
                    })));
                    started = true;
                }
                let Some(candidate) = chunk.get("candidates").and_then(Value::as_array).and_then(|items| items.first()) else {
                    continue;
                };
                if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                    finish_reason = Some(reason.to_string());
                }
                let parts = candidate.pointer("/content/parts").and_then(Value::as_array).cloned().unwrap_or_default();
                let mut visible = String::new();
                let mut current_tool_calls = Vec::new();
                for (index, part) in parts.iter().enumerate() {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        visible.push_str(text);
                    } else if let Some(call) = part.get("functionCall") {
                        let Some(name) = call.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()) else {
                            yield Err(invalid_response(format!("SSE functionCall {index} is missing name")));
                            return;
                        };
                        let id = call
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty())
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("gemini_call_{index}"));
                        let input = call.get("args").cloned().unwrap_or_else(|| json!({}));
                        if !input.is_object() {
                            yield Err(invalid_response(format!("SSE functionCall {index} args must be an object")));
                            return;
                        }
                        current_tool_calls.push(ToolCall {
                            id,
                            name: name.to_string(),
                            input,
                        });
                    } else {
                        yield Err(invalid_response(format!("SSE candidate part {index} is unsupported")));
                        return;
                    }
                }
                if !current_tool_calls.is_empty() {
                    tool_calls = current_tool_calls;
                }
                let delta = if visible.starts_with(&accumulated_text) {
                    &visible[accumulated_text.len()..]
                } else {
                    visible.as_str()
                };
                if !delta.is_empty() {
                    if !text_open {
                        yield Ok(event("content_block_start", json!({
                            "type":"content_block_start",
                            "index":0,
                            "content_block":{"type":"text","text":""}
                        })));
                        text_open = true;
                    }
                    yield Ok(event("content_block_delta", json!({
                        "type":"content_block_delta",
                        "index":0,
                        "delta":{"type":"text_delta","text":delta}
                    })));
                }
                accumulated_text = visible;
            }
        }
        if !remainder.is_empty() {
            yield Err(invalid_response("SSE ended with incomplete UTF-8"));
            return;
        }
        if !buffer.trim().is_empty() {
            yield Err(invalid_response("SSE ended with an incomplete event"));
            return;
        }
        if !started {
            yield Err(invalid_response("SSE contained no Gemini response"));
            return;
        }
        if text_open {
            yield Ok(event("content_block_stop", json!({"type":"content_block_stop","index":0})));
        }
        let mut next_index = u64::from(text_open);
        for call in &tool_calls {
            yield Ok(event("content_block_start", json!({
                "type":"content_block_start",
                "index":next_index,
                "content_block":{"type":"tool_use","id":call.id,"name":call.name,"input":{}}
            })));
            yield Ok(event("content_block_delta", json!({
                "type":"content_block_delta",
                "index":next_index,
                "delta":{
                    "type":"input_json_delta",
                    "partial_json":serde_json::to_string(&call.input).unwrap_or_else(|_| "{}".into())
                }
            })));
            yield Ok(event("content_block_stop", json!({
                "type":"content_block_stop",
                "index":next_index
            })));
            next_index += 1;
        }
        yield Ok(event("message_delta", json!({
            "type":"message_delta",
            "delta":{
                "stop_reason":map_finish_reason(finish_reason.as_deref(), !tool_calls.is_empty()),
                "stop_sequence":Value::Null
            },
            "usage":anthropic_usage(usage.as_ref())
        })));
        yield Ok(event("message_stop", json!({"type":"message_stop"})));
    }
}

fn append_utf8(
    buffer: &mut String,
    remainder: &mut Vec<u8>,
    chunk: &[u8],
) -> Result<(), BridgeError> {
    let mut input = std::mem::take(remainder);
    input.extend_from_slice(chunk);
    match std::str::from_utf8(&input) {
        Ok(text) => buffer.push_str(text),
        Err(error) => {
            let valid = error.valid_up_to();
            buffer.push_str(std::str::from_utf8(&input[..valid]).expect("valid UTF-8 prefix"));
            if error.error_len().is_some() {
                return Err(invalid_response("SSE contained invalid UTF-8"));
            }
            remainder.extend_from_slice(&input[valid..]);
        }
    }
    Ok(())
}

fn take_sse_block(buffer: &mut String) -> Option<String> {
    let (index, length) = [("\r\n\r\n", 4), ("\n\n", 2)]
        .into_iter()
        .filter_map(|(delimiter, length)| buffer.find(delimiter).map(|index| (index, length)))
        .min_by_key(|(index, _)| *index)?;
    let block = buffer[..index].to_string();
    buffer.drain(..index + length);
    Some(block)
}

fn parse_data_block(block: &str) -> Result<Option<String>, BridgeError> {
    let mut data = Vec::new();
    for line in block.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = line
            .split_once(':')
            .ok_or_else(|| invalid_response("SSE line must contain ':'"))?;
        if field != "data" {
            return Err(invalid_response(format!(
                "SSE field is unsupported: {field}"
            )));
        }
        data.push(value.strip_prefix(' ').unwrap_or(value));
    }
    if data.is_empty() {
        Ok(None)
    } else {
        Ok(Some(data.join("\n")))
    }
}

fn event(name: &str, payload: Value) -> Bytes {
    Bytes::from(format!("event: {name}\ndata: {payload}\n\n"))
}

fn invalid_response(message: impl Into<String>) -> BridgeError {
    BridgeError::InvalidGeminiResponse(message.into())
}
