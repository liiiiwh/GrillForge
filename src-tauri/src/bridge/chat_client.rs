use super::{AnthropicSseStream, BridgeError, sse};
use async_stream::stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

const DEFAULT_MAX_TOKENS: u64 = 32_768;

pub fn chat_request_to_anthropic(body: Value) -> Result<Value, BridgeError> {
    let body = body
        .as_object()
        .ok_or_else(|| invalid_request("body must be an object"))?;
    reject_unknown(
        body,
        &[
            "model",
            "messages",
            "max_tokens",
            "max_completion_tokens",
            "stream",
            "stream_options",
            "temperature",
            "top_p",
            "stop",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
        ],
        "request",
    )?;
    let model = required_string(body.get("model"), "model")?;
    let max_tokens = match (body.get("max_tokens"), body.get("max_completion_tokens")) {
        (Some(_), Some(_)) => {
            return Err(invalid_request(
                "max_tokens and max_completion_tokens cannot both be set",
            ));
        }
        (Some(value), None) | (None, Some(value)) => positive_integer(value, "max_tokens")?,
        (None, None) => DEFAULT_MAX_TOKENS,
    };
    if let Some(options) = body.get("stream_options") {
        let options = options
            .as_object()
            .ok_or_else(|| invalid_request("stream_options must be an object"))?;
        reject_unknown(options, &["include_usage"], "stream_options")?;
        if let Some(value) = options.get("include_usage") {
            value
                .as_bool()
                .ok_or_else(|| invalid_request("stream_options.include_usage must be a boolean"))?;
        }
    }

    let source = body
        .get("messages")
        .and_then(Value::as_array)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(|| invalid_request("messages must be a non-empty array"))?;
    let mut system = Vec::new();
    let mut messages = Vec::new();
    let mut conversation_started = false;
    for (index, message) in source.iter().enumerate() {
        let message = message
            .as_object()
            .ok_or_else(|| invalid_request(format!("messages[{index}] must be an object")))?;
        let role = required_string(message.get("role"), &format!("messages[{index}].role"))?;
        match role {
            "system" | "developer" => {
                if conversation_started {
                    return Err(invalid_request(format!(
                        "messages[{index}] {role} message must precede conversation messages"
                    )));
                }
                reject_unknown(
                    message,
                    &["role", "content", "name"],
                    &format!("messages[{index}]"),
                )?;
                system.push(chat_text_content(
                    message.get("content"),
                    &format!("messages[{index}].content"),
                )?);
            }
            "user" => {
                conversation_started = true;
                reject_unknown(
                    message,
                    &["role", "content", "name"],
                    &format!("messages[{index}]"),
                )?;
                push_message(
                    &mut messages,
                    "user",
                    vec![json!({
                        "type":"text",
                        "text":chat_text_content(message.get("content"), &format!("messages[{index}].content"))?
                    })],
                );
            }
            "assistant" => {
                conversation_started = true;
                reject_unknown(
                    message,
                    &[
                        "role",
                        "content",
                        "tool_calls",
                        "name",
                        "refusal",
                        "reasoning_content",
                        "reasoning_signature",
                    ],
                    &format!("messages[{index}]"),
                )?;
                let mut blocks = Vec::new();
                if let Some(reasoning) = message.get("reasoning_content") {
                    let reasoning = required_string(
                        Some(reasoning),
                        &format!("messages[{index}].reasoning_content"),
                    )?;
                    let mut thinking = json!({"type":"thinking","thinking":reasoning});
                    if let Some(signature) = message.get("reasoning_signature") {
                        thinking["signature"] = Value::String(
                            required_string(
                                Some(signature),
                                &format!("messages[{index}].reasoning_signature"),
                            )?
                            .to_string(),
                        );
                    }
                    blocks.push(thinking);
                } else if message.get("reasoning_signature").is_some() {
                    return Err(invalid_request(format!(
                        "messages[{index}].reasoning_signature requires reasoning_content"
                    )));
                }
                match message.get("content") {
                    Some(Value::String(text)) if !text.is_empty() => {
                        blocks.push(json!({"type":"text","text":text}));
                    }
                    Some(Value::String(_)) | Some(Value::Null) | None => {}
                    Some(value) => blocks.extend(chat_content_blocks(
                        value,
                        &format!("messages[{index}].content"),
                    )?),
                }
                if let Some(calls) = message.get("tool_calls") {
                    let calls = calls
                        .as_array()
                        .filter(|calls| !calls.is_empty())
                        .ok_or_else(|| {
                            invalid_request(format!(
                                "messages[{index}].tool_calls must be a non-empty array"
                            ))
                        })?;
                    for (call_index, call) in calls.iter().enumerate() {
                        blocks.push(chat_tool_call_to_anthropic(call, index, call_index)?);
                    }
                }
                if blocks.is_empty() {
                    return Err(invalid_request(format!(
                        "messages[{index}] assistant message must contain content or tool_calls"
                    )));
                }
                push_message(&mut messages, "assistant", blocks);
            }
            "tool" => {
                conversation_started = true;
                reject_unknown(
                    message,
                    &["role", "content", "tool_call_id"],
                    &format!("messages[{index}]"),
                )?;
                let id = required_string(
                    message.get("tool_call_id"),
                    &format!("messages[{index}].tool_call_id"),
                )?;
                let content = chat_text_content(
                    message.get("content"),
                    &format!("messages[{index}].content"),
                )?;
                push_message(
                    &mut messages,
                    "user",
                    vec![json!({"type":"tool_result","tool_use_id":id,"content":content})],
                );
            }
            _ => {
                return Err(invalid_request(format!(
                    "messages[{index}].role is unsupported: {role}"
                )));
            }
        }
    }
    if messages.is_empty() || messages[0]["role"] != "user" {
        return Err(invalid_request(
            "conversation must contain a user message before any assistant message",
        ));
    }

    let mut result = json!({"model":model,"max_tokens":max_tokens,"messages":messages});
    if !system.is_empty() {
        result["system"] = json!(system.join("\n"));
    }
    for (source, target) in [("temperature", "temperature"), ("top_p", "top_p")] {
        if let Some(value) = body.get(source) {
            let number = value
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or_else(|| invalid_request(format!("{source} must be a finite number")))?;
            result[target] = json!(number);
        }
    }
    if let Some(stop) = body.get("stop") {
        result["stop_sequences"] = Value::Array(chat_stop_sequences(stop)?);
    }
    if let Some(streaming) = body.get("stream") {
        result["stream"] = json!(
            streaming
                .as_bool()
                .ok_or_else(|| invalid_request("stream must be a boolean"))?
        );
    }
    let has_tools = if let Some(tools) = body.get("tools") {
        let tools = chat_tools_to_anthropic(tools)?;
        if !tools.is_empty() {
            result["tools"] = Value::Array(tools);
            true
        } else {
            false
        }
    } else {
        false
    };
    if let Some(choice) = body.get("tool_choice") {
        if !has_tools {
            return Err(invalid_request("tool_choice requires tools"));
        }
        result["tool_choice"] = chat_tool_choice_to_anthropic(choice)?;
    }
    if let Some(parallel) = body.get("parallel_tool_calls") {
        let parallel = parallel
            .as_bool()
            .ok_or_else(|| invalid_request("parallel_tool_calls must be a boolean"))?;
        if !has_tools {
            return Err(invalid_request("parallel_tool_calls requires tools"));
        }
        if !parallel {
            if result.get("tool_choice").is_none() {
                result["tool_choice"] = json!({"type":"auto"});
            }
            result["tool_choice"]["disable_parallel_tool_use"] = json!(true);
        }
    }
    Ok(result)
}

pub fn anthropic_response_to_chat(body: Value) -> Result<Value, BridgeError> {
    let body = body
        .as_object()
        .ok_or_else(|| invalid_response("body must be an object"))?;
    if body.get("type").and_then(Value::as_str) == Some("error") {
        return Err(invalid_response(
            "Anthropic upstream returned an error envelope",
        ));
    }
    let id = response_string(body.get("id"), "id")?;
    let model = response_string(body.get("model"), "model")?;
    let content = body
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response("content must be an array"))?;
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut reasoning_signature = None::<String>;
    let mut calls = Vec::new();
    for (index, block) in content.iter().enumerate() {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => text.push_str(response_string(
                block.get("text"),
                &format!("content[{index}].text"),
            )?),
            Some("thinking") => {
                reasoning.push_str(response_string(
                    block.get("thinking"),
                    &format!("content[{index}].thinking"),
                )?);
                if let Some(signature) = block.get("signature") {
                    if reasoning_signature.is_some() {
                        return Err(invalid_response(
                            "multiple thinking signatures cannot be represented by Chat Completions",
                        ));
                    }
                    reasoning_signature = Some(
                        response_string(Some(signature), &format!("content[{index}].signature"))?
                            .to_string(),
                    );
                }
            }
            Some("tool_use") => {
                let input = block
                    .get("input")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        invalid_response(format!("content[{index}].input must be an object"))
                    })?;
                calls.push(json!({
                    "id":response_string(block.get("id"), &format!("content[{index}].id"))?,
                    "type":"function",
                    "function":{
                        "name":response_string(block.get("name"), &format!("content[{index}].name"))?,
                        "arguments":serde_json::to_string(input).expect("JSON objects serialize")
                    }
                }));
            }
            Some("redacted_thinking") => {}
            Some(kind) => {
                return Err(invalid_response(format!(
                    "content[{index}].type is unsupported: {kind}"
                )));
            }
            None => {
                return Err(invalid_response(format!(
                    "content[{index}].type is required"
                )));
            }
        }
    }
    let mut message = json!({"role":"assistant","content":if text.is_empty() { Value::Null } else { Value::String(text) }});
    if !reasoning.is_empty() {
        message["reasoning_content"] = Value::String(reasoning);
    }
    if let Some(signature) = reasoning_signature {
        message["reasoning_signature"] = Value::String(signature);
    }
    if !calls.is_empty() {
        message["tool_calls"] = Value::Array(calls);
    }
    let finish_reason = match body.get("stop_reason").and_then(Value::as_str) {
        Some("tool_use") => "tool_calls",
        Some("max_tokens") => "length",
        Some("end_turn" | "stop_sequence") | None => "stop",
        Some(reason) => {
            return Err(invalid_response(format!(
                "unsupported stop_reason: {reason}"
            )));
        }
    };
    let usage = body
        .get("usage")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_response("usage must be an object"))?;
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        + usage
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Ok(json!({
        "id":id,"object":"chat.completion","model":model,
        "choices":[{"index":0,"message":message,"finish_reason":finish_reason}],
        "usage":{"prompt_tokens":input,"completion_tokens":output,"total_tokens":input + output}
    }))
}

pub fn anthropic_sse_to_chat<S, E>(source: S) -> AnthropicSseStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    Box::pin(stream! {
        let mut source = Box::pin(source);
        let mut buffer = String::new();
        let mut remainder = Vec::new();
        let mut id = String::new();
        let mut model = String::new();
        let mut input_tokens = 0_u64;
        let mut output_tokens = 0_u64;
        let mut tool_indexes = HashMap::<u64, usize>::new();
        let mut next_tool_index = 0_usize;
        let mut started = false;
        while let Some(chunk) = source.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    yield Err(invalid_response(format!("Anthropic SSE transport failed: {error}")));
                    return;
                }
            };
            if let Err(error) = sse::append_utf8(&mut buffer, &mut remainder, &chunk) {
                yield Err(error);
                return;
            }
            while let Some(block) = sse::take_sse_block(&mut buffer) {
                let (event, data) = match sse::parse_sse_block(&block) {
                    Ok(value) => value,
                    Err(error) => { yield Err(error); return; }
                };
                let payload: Value = match serde_json::from_str(data) {
                    Ok(value) => value,
                    Err(_) => { yield Err(invalid_response("Anthropic SSE data must be valid JSON")); return; }
                };
                match event {
                    "message_start" => {
                        if started { yield Err(invalid_response("duplicate message_start")); return; }
                        let message = match payload.get("message").and_then(Value::as_object) {
                            Some(value) => value,
                            None => { yield Err(invalid_response("message_start.message must be an object")); return; }
                        };
                        id = message.get("id").and_then(Value::as_str).unwrap_or("chatcmpl_grillforge").to_string();
                        model = message.get("model").and_then(Value::as_str).unwrap_or("").to_string();
                        let usage = message.get("usage").and_then(Value::as_object);
                        input_tokens = usage
                            .and_then(|usage| usage.get("input_tokens"))
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                            + usage
                                .and_then(|usage| usage.get("cache_read_input_tokens"))
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                        started = true;
                        yield Ok(chat_sse(json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]})));
                    }
                    "content_block_start" => {
                        let index = match payload.get("index").and_then(Value::as_u64) {
                            Some(value) => value,
                            None => { yield Err(invalid_response("content_block_start.index must be an integer")); return; }
                        };
                        let content = match payload.get("content_block").and_then(Value::as_object) {
                            Some(value) => value,
                            None => { yield Err(invalid_response("content_block_start.content_block must be an object")); return; }
                        };
                        if content.get("type").and_then(Value::as_str) == Some("tool_use") {
                            let tool_index = next_tool_index;
                            next_tool_index += 1;
                            tool_indexes.insert(index, tool_index);
                            let call_id = content.get("id").and_then(Value::as_str).unwrap_or("");
                            let name = content.get("name").and_then(Value::as_str).unwrap_or("");
                            yield Ok(chat_sse(json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{"tool_calls":[{"index":tool_index,"id":call_id,"type":"function","function":{"name":name,"arguments":""}}]},"finish_reason":null}]})));
                        }
                    }
                    "content_block_delta" => {
                        let index = payload.get("index").and_then(Value::as_u64).unwrap_or(0);
                        let delta = match payload.get("delta").and_then(Value::as_object) {
                            Some(value) => value,
                            None => { yield Err(invalid_response("content_block_delta.delta must be an object")); return; }
                        };
                        match delta.get("type").and_then(Value::as_str) {
                            Some("text_delta") => {
                                let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
                                if !text.is_empty() {
                                    yield Ok(chat_sse(json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]})));
                                }
                            }
                            Some("thinking_delta") => {
                                let thinking = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                                if !thinking.is_empty() {
                                    yield Ok(chat_sse(json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{"reasoning_content":thinking},"finish_reason":null}]})));
                                }
                            }
                            Some("signature_delta") => {
                                let signature = delta.get("signature").and_then(Value::as_str).unwrap_or("");
                                if !signature.is_empty() {
                                    yield Ok(chat_sse(json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{"reasoning_signature":signature},"finish_reason":null}]})));
                                }
                            }
                            Some("input_json_delta") => {
                                let Some(tool_index) = tool_indexes.get(&index).copied() else {
                                    yield Err(invalid_response("tool input delta preceded its tool_use block"));
                                    return;
                                };
                                let arguments = delta.get("partial_json").and_then(Value::as_str).unwrap_or("");
                                yield Ok(chat_sse(json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{"tool_calls":[{"index":tool_index,"function":{"arguments":arguments}}]},"finish_reason":null}]})));
                            }
                            Some(kind) => { yield Err(invalid_response(format!("unsupported content delta: {kind}"))); return; }
                            None => { yield Err(invalid_response("content_block_delta.delta.type is required")); return; }
                        }
                    }
                    "content_block_stop" => {}
                    "message_delta" => {
                        output_tokens = payload.pointer("/usage/output_tokens").and_then(Value::as_u64).unwrap_or(output_tokens);
                        let reason = payload.pointer("/delta/stop_reason").and_then(Value::as_str);
                        let finish = match reason {
                            Some("tool_use") => "tool_calls",
                            Some("max_tokens") => "length",
                            Some("end_turn" | "stop_sequence") | None => "stop",
                            Some(other) => { yield Err(invalid_response(format!("unsupported stop_reason: {other}"))); return; }
                        };
                        yield Ok(chat_sse(json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{},"finish_reason":finish}]})));
                    }
                    "message_stop" => {
                        yield Ok(chat_sse(json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[],"usage":{"prompt_tokens":input_tokens,"completion_tokens":output_tokens,"total_tokens":input_tokens + output_tokens}})));
                        yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
                        return;
                    }
                    "error" => { yield Err(invalid_response("Anthropic SSE returned an error event")); return; }
                    other => { yield Err(invalid_response(format!("unsupported Anthropic SSE event: {other}"))); return; }
                }
            }
        }
        if !remainder.is_empty() || !buffer.trim().is_empty() {
            yield Err(invalid_response("Anthropic SSE ended with an incomplete event"));
        } else {
            yield Err(invalid_response("Anthropic SSE ended before message_stop"));
        }
    })
}

fn chat_tool_call_to_anthropic(
    value: &Value,
    message_index: usize,
    call_index: usize,
) -> Result<Value, BridgeError> {
    let field = format!("messages[{message_index}].tool_calls[{call_index}]");
    let call = value
        .as_object()
        .ok_or_else(|| invalid_request(format!("{field} must be an object")))?;
    reject_unknown(call, &["id", "type", "function"], &field)?;
    if required_string(call.get("type"), &format!("{field}.type"))? != "function" {
        return Err(invalid_request(format!("{field}.type must be function")));
    }
    let function = call
        .get("function")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_request(format!("{field}.function must be an object")))?;
    reject_unknown(
        function,
        &["name", "arguments"],
        &format!("{field}.function"),
    )?;
    let arguments = required_string(
        function.get("arguments"),
        &format!("{field}.function.arguments"),
    )?;
    let arguments = serde_json::from_str::<Value>(arguments)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| {
            invalid_request(format!("{field}.function.arguments must encode an object"))
        })?;
    Ok(json!({
        "type":"tool_use",
        "id":required_string(call.get("id"), &format!("{field}.id"))?,
        "name":required_string(function.get("name"), &format!("{field}.function.name"))?,
        "input":arguments
    }))
}

fn chat_tools_to_anthropic(value: &Value) -> Result<Vec<Value>, BridgeError> {
    let tools = value
        .as_array()
        .ok_or_else(|| invalid_request("tools must be an array"))?;
    let mut names = HashSet::new();
    let mut result = Vec::with_capacity(tools.len());
    for (index, tool) in tools.iter().enumerate() {
        let field = format!("tools[{index}]");
        let tool = tool
            .as_object()
            .ok_or_else(|| invalid_request(format!("{field} must be an object")))?;
        reject_unknown(tool, &["type", "function"], &field)?;
        if required_string(tool.get("type"), &format!("{field}.type"))? != "function" {
            return Err(invalid_request(format!("{field}.type must be function")));
        }
        let function = tool
            .get("function")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_request(format!("{field}.function must be an object")))?;
        reject_unknown(
            function,
            &["name", "description", "parameters", "strict"],
            &format!("{field}.function"),
        )?;
        let name = required_string(function.get("name"), &format!("{field}.function.name"))?;
        if !names.insert(name) {
            return Err(invalid_request(format!("duplicate tool name: {name}")));
        }
        let parameters = function
            .get("parameters")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                invalid_request(format!("{field}.function.parameters must be an object"))
            })?;
        let mut converted = json!({"name":name,"input_schema":parameters});
        if let Some(description) = function.get("description") {
            converted["description"] = json!(required_string(
                Some(description),
                &format!("{field}.function.description"),
            )?);
        }
        result.push(converted);
    }
    Ok(result)
}

fn chat_tool_choice_to_anthropic(value: &Value) -> Result<Value, BridgeError> {
    if let Some(choice) = value.as_str() {
        return match choice {
            "auto" => Ok(json!({"type":"auto"})),
            "required" => Ok(json!({"type":"any"})),
            "none" => Ok(json!({"type":"none"})),
            _ => Err(invalid_request("tool_choice is unsupported")),
        };
    }
    let choice = value
        .as_object()
        .ok_or_else(|| invalid_request("tool_choice must be a string or object"))?;
    reject_unknown(choice, &["type", "function"], "tool_choice")?;
    if required_string(choice.get("type"), "tool_choice.type")? != "function" {
        return Err(invalid_request("tool_choice.type must be function"));
    }
    let function = choice
        .get("function")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_request("tool_choice.function must be an object"))?;
    reject_unknown(function, &["name"], "tool_choice.function")?;
    Ok(json!({
        "type":"tool",
        "name":required_string(function.get("name"), "tool_choice.function.name")?
    }))
}

fn chat_stop_sequences(value: &Value) -> Result<Vec<Value>, BridgeError> {
    let values = if value.is_string() {
        vec![value.clone()]
    } else {
        value
            .as_array()
            .filter(|values| !values.is_empty())
            .cloned()
            .ok_or_else(|| invalid_request("stop must be a string or non-empty array"))?
    };
    for (index, value) in values.iter().enumerate() {
        required_string(Some(value), &format!("stop[{index}]"))?;
    }
    Ok(values)
}

fn chat_text_content(value: Option<&Value>, field: &str) -> Result<String, BridgeError> {
    let value = value.ok_or_else(|| invalid_request(format!("{field} is required")))?;
    if let Some(text) = value.as_str() {
        if text.is_empty() {
            return Err(invalid_request(format!("{field} must be non-empty")));
        }
        return Ok(text.to_string());
    }
    let blocks = chat_content_blocks(value, field)?;
    Ok(blocks
        .iter()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn chat_content_blocks(value: &Value, field: &str) -> Result<Vec<Value>, BridgeError> {
    let values = value
        .as_array()
        .filter(|values| !values.is_empty())
        .ok_or_else(|| invalid_request(format!("{field} must be a string or non-empty array")))?;
    let mut result = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let block = value
            .as_object()
            .ok_or_else(|| invalid_request(format!("{field}[{index}] must be an object")))?;
        reject_unknown(block, &["type", "text"], &format!("{field}[{index}]"))?;
        let kind = required_string(block.get("type"), &format!("{field}[{index}].type"))?;
        if !matches!(kind, "text" | "input_text") {
            return Err(invalid_request(format!(
                "{field}[{index}].type is unsupported: {kind}"
            )));
        }
        result.push(json!({
            "type":"text",
            "text":required_string(block.get("text"), &format!("{field}[{index}].text"))?
        }));
    }
    Ok(result)
}

fn push_message(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if let Some(last) = messages.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some(role) {
            last["content"]
                .as_array_mut()
                .expect("projected message content is an array")
                .extend(blocks);
            return;
        }
    }
    messages.push(json!({"role":role,"content":blocks}));
}

fn required_string<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.trim() == *value)
        .ok_or_else(|| invalid_request(format!("{field} must be a non-empty unpadded string")))
}

fn response_string<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_response(format!("{field} must be a non-empty string")))
}

fn positive_integer(value: &Value, field: &str) -> Result<u64, BridgeError> {
    value
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_request(format!("{field} must be a positive integer")))
}

fn reject_unknown(
    object: &Map<String, Value>,
    allowed: &[&str],
    field: &str,
) -> Result<(), BridgeError> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid_request(format!("{field}.{key} is unsupported")));
    }
    Ok(())
}

fn chat_sse(value: Value) -> Bytes {
    Bytes::from(format!("data: {value}\n\n"))
}

fn invalid_request(message: impl Into<String>) -> BridgeError {
    BridgeError::InvalidChatRequest(message.into())
}

fn invalid_response(message: impl Into<String>) -> BridgeError {
    BridgeError::InvalidChatResponse(message.into())
}
