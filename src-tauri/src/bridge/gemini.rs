// Minimal Gemini Native adapter derived from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::{AnthropicSseStream, BridgeError, streaming_gemini::gemini_sse_to_anthropic};
use base64::Engine as _;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use url::Url;

pub struct GeminiNativeBridge {
    client: Client,
    endpoint: Url,
    api_key: String,
}

impl GeminiNativeBridge {
    pub fn from_endpoint(endpoint: Url, api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            endpoint,
            api_key: api_key.into(),
        }
    }

    pub async fn complete(&self, request: Value) -> Result<Value, BridgeError> {
        let upstream_request = anthropic_to_gemini(request, false)?;
        let response = self
            .client
            .post(self.endpoint.clone())
            .header("x-goog-api-key", &self.api_key)
            .json(&upstream_request)
            .send()
            .await
            .map_err(|error| BridgeError::GeminiRequestFailed(error.to_string()))?;
        if !response.status().is_success() {
            return Err(gemini_http_error(response, &self.api_key).await);
        }
        let body = response
            .json::<Value>()
            .await
            .map_err(|error| invalid_response(error.to_string()))?;
        gemini_to_anthropic(body)
    }

    pub async fn stream(&self, request: Value) -> Result<AnthropicSseStream, BridgeError> {
        let upstream_request = anthropic_to_gemini(request, true)?;
        let response = self
            .client
            .post(self.endpoint.clone())
            .header("x-goog-api-key", &self.api_key)
            .json(&upstream_request)
            .send()
            .await
            .map_err(|error| BridgeError::GeminiRequestFailed(error.to_string()))?;
        if !response.status().is_success() {
            return Err(gemini_http_error(response, &self.api_key).await);
        }
        Ok(Box::pin(gemini_sse_to_anthropic(response.bytes_stream())))
    }
}

pub fn gemini_request_to_anthropic(
    model: &str,
    body: Value,
    streaming: bool,
) -> Result<Value, BridgeError> {
    if model.is_empty() || model.trim() != model || model.chars().any(char::is_control) {
        return Err(invalid_request("model must be a non-empty unpadded string"));
    }
    let object = body
        .as_object()
        .ok_or_else(|| invalid_request("Gemini client body must be an object"))?;
    reject_unknown(
        object,
        &[
            "contents",
            "systemInstruction",
            "generationConfig",
            "tools",
            "toolConfig",
        ],
    )?;
    let contents = object
        .get("contents")
        .and_then(Value::as_array)
        .filter(|contents| !contents.is_empty())
        .ok_or_else(|| invalid_request("contents must be a non-empty array"))?;
    let mut tool_calls = HashMap::new();
    let mut messages = Vec::with_capacity(contents.len());
    for (message_index, content) in contents.iter().enumerate() {
        messages.push(gemini_content_to_anthropic(
            content,
            message_index,
            &mut tool_calls,
        )?);
    }
    let generation = object
        .get("generationConfig")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_request("generationConfig must be an object"))?;
    reject_unknown(
        generation,
        &[
            "maxOutputTokens",
            "temperature",
            "thinkingConfig",
            "topK",
            "topP",
            "stopSequences",
        ],
    )?;
    let max_tokens = generation
        .get("maxOutputTokens")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            invalid_request("generationConfig.maxOutputTokens must be a positive integer")
        })?;
    let mut result = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": messages,
        "stream": streaming,
    });
    let target = result
        .as_object_mut()
        .expect("Gemini request projection is an object");
    if let Some(system) = object.get("systemInstruction") {
        target.insert("system".into(), gemini_system_to_anthropic(system)?);
    }
    if let Some(value) = generation.get("temperature") {
        target.insert("temperature".into(), finite_number(value, "temperature")?);
    }
    if let Some(value) = generation.get("thinkingConfig") {
        let thinking = value
            .as_object()
            .ok_or_else(|| invalid_request("thinkingConfig must be an object"))?;
        reject_unknown(thinking, &["includeThoughts"])?;
        if let Some(include_thoughts) = thinking.get("includeThoughts") {
            include_thoughts.as_bool().ok_or_else(|| {
                invalid_request("thinkingConfig.includeThoughts must be a boolean")
            })?;
        }
    }
    if let Some(value) = generation.get("topK") {
        let top_k = value
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid_request("generationConfig.topK must be a positive integer"))?;
        target.insert("top_k".into(), Value::from(top_k));
    }
    if let Some(value) = generation.get("topP") {
        target.insert("top_p".into(), finite_number(value, "topP")?);
    }
    if let Some(value) = generation.get("stopSequences") {
        let stops = value
            .as_array()
            .filter(|stops| !stops.is_empty())
            .ok_or_else(|| invalid_request("stopSequences must be a non-empty array"))?;
        for (index, stop) in stops.iter().enumerate() {
            required_string(Some(stop), &format!("stopSequences[{index}]"))?;
        }
        target.insert("stop_sequences".into(), Value::Array(stops.clone()));
    }
    if let Some(tools) = object.get("tools") {
        target.insert("tools".into(), gemini_tools_to_anthropic(tools)?);
    }
    if let Some(config) = object.get("toolConfig") {
        if !target.contains_key("tools") {
            return Err(invalid_request("toolConfig requires tools"));
        }
        target.insert(
            "tool_choice".into(),
            gemini_tool_config_to_anthropic(config)?,
        );
    }
    Ok(result)
}

pub fn anthropic_response_to_gemini(body: Value) -> Result<Value, BridgeError> {
    let object = body
        .as_object()
        .ok_or_else(|| invalid_response("Anthropic response must be an object"))?;
    let content = object
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response("Anthropic response content must be an array"))?;
    let mut parts = Vec::with_capacity(content.len());
    for (index, block) in content.iter().enumerate() {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => parts.push(json!({
                "text": required_response_string(block.get("text"), &format!("content[{index}].text"))?
            })),
            Some("tool_use") => {
                let id = required_response_string(
                    block.get("id"),
                    &format!("content[{index}].id"),
                )?;
                let name = required_response_string(
                    block.get("name"),
                    &format!("content[{index}].name"),
                )?;
                let input = block
                    .get("input")
                    .filter(|input| input.is_object())
                    .ok_or_else(|| {
                        invalid_response(format!("content[{index}].input must be an object"))
                    })?;
                parts.push(json!({
                    "functionCall":{"id":id,"name":name,"args":input}
                }));
            }
            Some(kind) => {
                return Err(invalid_response(format!(
                    "Anthropic response content[{index}] type is unsupported: {kind}"
                )));
            }
            None => {
                return Err(invalid_response(format!(
                    "Anthropic response content[{index}].type is required"
                )));
            }
        }
    }
    let usage = object
        .get("usage")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_response("Anthropic response usage must be an object"))?;
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let finish = match object.get("stop_reason").and_then(Value::as_str) {
        Some("max_tokens") => "MAX_TOKENS",
        Some("refusal") => "SAFETY",
        Some("end_turn" | "stop_sequence" | "tool_use") | None => "STOP",
        Some(reason) => {
            return Err(invalid_response(format!(
                "unsupported Anthropic stop_reason: {reason}"
            )));
        }
    };
    Ok(json!({
        "responseId": object.get("id").and_then(Value::as_str).unwrap_or(""),
        "modelVersion": object.get("model").and_then(Value::as_str).unwrap_or(""),
        "candidates":[{
            "index":0,
            "content":{"role":"model","parts":parts},
            "finishReason":finish
        }],
        "usageMetadata":{
            "promptTokenCount":input.saturating_add(cached),
            "cachedContentTokenCount":cached,
            "candidatesTokenCount":output,
            "totalTokenCount":input.saturating_add(cached).saturating_add(output)
        }
    }))
}

pub fn anthropic_sse_to_gemini<S, E>(
    upstream: S,
) -> impl Stream<Item = Result<Bytes, BridgeError>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    #[derive(Debug)]
    enum Block {
        Text,
        Tool {
            id: String,
            name: String,
            input: String,
        },
    }

    async_stream::stream! {
        let mut buffer = String::new();
        let mut remainder = Vec::new();
        let mut blocks = HashMap::<u64, Block>::new();
        let mut message_id = String::new();
        let mut model = String::new();
        let mut input_tokens = 0_u64;
        let mut cached_tokens = 0_u64;
        let mut output_tokens = 0_u64;
        let mut finish_reason = None::<String>;
        let mut started = false;
        let mut stopped = false;
        tokio::pin!(upstream);

        while let Some(chunk) = upstream.next().await {
            let bytes = match chunk {
                Ok(bytes) => bytes,
                Err(error) => {
                    yield Err(BridgeError::GeminiRequestFailed(error.to_string()));
                    return;
                }
            };
            if let Err(error) = append_gemini_utf8(&mut buffer, &mut remainder, &bytes) {
                yield Err(error);
                return;
            }
            while let Some(block) = take_gemini_sse_block(&mut buffer) {
                let (event, data) = match parse_anthropic_sse_block(&block) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        yield Err(error);
                        return;
                    }
                };
                let payload: Value = match serde_json::from_str(data) {
                    Ok(payload) => payload,
                    Err(_) => {
                        yield Err(invalid_response("Anthropic SSE data must be valid JSON"));
                        return;
                    }
                };
                if payload.get("type").and_then(Value::as_str) != Some(event) {
                    yield Err(invalid_response("Anthropic SSE event and payload type must match"));
                    return;
                }
                match event {
                    "message_start" => {
                        if started {
                            yield Err(invalid_response("Anthropic SSE contains duplicate message_start"));
                            return;
                        }
                        let Some(message) = payload.get("message").and_then(Value::as_object) else {
                            yield Err(invalid_response("Anthropic message_start.message must be an object"));
                            return;
                        };
                        message_id = message.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                        model = message.get("model").and_then(Value::as_str).unwrap_or("").to_string();
                        let usage = message.get("usage").and_then(Value::as_object);
                        input_tokens = usage.and_then(|usage| usage.get("input_tokens")).and_then(Value::as_u64).unwrap_or(0);
                        cached_tokens = usage.and_then(|usage| usage.get("cache_read_input_tokens")).and_then(Value::as_u64).unwrap_or(0);
                        started = true;
                    }
                    "content_block_start" => {
                        if !started || stopped {
                            yield Err(invalid_response("Anthropic content block started outside a message"));
                            return;
                        }
                        let Some(index) = payload.get("index").and_then(Value::as_u64) else {
                            yield Err(invalid_response("Anthropic content_block_start.index must be an integer"));
                            return;
                        };
                        let Some(content) = payload.get("content_block").and_then(Value::as_object) else {
                            yield Err(invalid_response("Anthropic content_block_start.content_block must be an object"));
                            return;
                        };
                        let new_block = match content.get("type").and_then(Value::as_str) {
                            Some("text") => Block::Text,
                            Some("tool_use") => {
                                let Some(id) = content.get("id").and_then(Value::as_str).filter(|value| !value.is_empty()) else {
                                    yield Err(invalid_response("Anthropic tool_use.id must be a non-empty string"));
                                    return;
                                };
                                let Some(name) = content.get("name").and_then(Value::as_str).filter(|value| !value.is_empty()) else {
                                    yield Err(invalid_response("Anthropic tool_use.name must be a non-empty string"));
                                    return;
                                };
                                Block::Tool { id: id.to_string(), name: name.to_string(), input: String::new() }
                            }
                            Some(kind) => {
                                yield Err(invalid_response(format!("Anthropic streaming content type is unsupported: {kind}")));
                                return;
                            }
                            None => {
                                yield Err(invalid_response("Anthropic content_block_start type is required"));
                                return;
                            }
                        };
                        if blocks.insert(index, new_block).is_some() {
                            yield Err(invalid_response("Anthropic SSE reused an open content block index"));
                            return;
                        }
                    }
                    "content_block_delta" => {
                        let Some(index) = payload.get("index").and_then(Value::as_u64) else {
                            yield Err(invalid_response("Anthropic content_block_delta.index must be an integer"));
                            return;
                        };
                        let Some(delta) = payload.get("delta").and_then(Value::as_object) else {
                            yield Err(invalid_response("Anthropic content_block_delta.delta must be an object"));
                            return;
                        };
                        match (blocks.get_mut(&index), delta.get("type").and_then(Value::as_str)) {
                            (Some(Block::Text), Some("text_delta")) => {
                                let Some(text) = delta.get("text").and_then(Value::as_str) else {
                                    yield Err(invalid_response("Anthropic text_delta.text must be a string"));
                                    return;
                                };
                                if !text.is_empty() {
                                    yield Ok(gemini_sse_data(json!({
                                        "responseId":message_id,
                                        "modelVersion":model,
                                        "candidates":[{"index":0,"content":{"role":"model","parts":[{"text":text}]}}]
                                    })));
                                }
                            }
                            (Some(Block::Tool { input, .. }), Some("input_json_delta")) => {
                                let Some(partial) = delta.get("partial_json").and_then(Value::as_str) else {
                                    yield Err(invalid_response("Anthropic input_json_delta.partial_json must be a string"));
                                    return;
                                };
                                input.push_str(partial);
                            }
                            (Some(_), Some(kind)) => {
                                yield Err(invalid_response(format!("Anthropic delta type does not match its content block: {kind}")));
                                return;
                            }
                            (None, _) => {
                                yield Err(invalid_response("Anthropic delta references an unopened content block"));
                                return;
                            }
                            (_, None) => {
                                yield Err(invalid_response("Anthropic content_block_delta type is required"));
                                return;
                            }
                        }
                    }
                    "content_block_stop" => {
                        let Some(index) = payload.get("index").and_then(Value::as_u64) else {
                            yield Err(invalid_response("Anthropic content_block_stop.index must be an integer"));
                            return;
                        };
                        let Some(block) = blocks.remove(&index) else {
                            yield Err(invalid_response("Anthropic content_block_stop references an unopened block"));
                            return;
                        };
                        if let Block::Tool { id, name, input } = block {
                            let args: Value = match serde_json::from_str(&input) {
                                Ok(Value::Object(args)) => Value::Object(args),
                                _ => {
                                    yield Err(invalid_response("Anthropic tool input delta must form a JSON object"));
                                    return;
                                }
                            };
                            yield Ok(gemini_sse_data(json!({
                                "responseId":message_id,
                                "modelVersion":model,
                                "candidates":[{"index":0,"content":{"role":"model","parts":[{
                                    "functionCall":{"id":id,"name":name,"args":args}
                                }]}}]
                            })));
                        }
                    }
                    "message_delta" => {
                        if !blocks.is_empty() {
                            yield Err(invalid_response("Anthropic message_delta arrived before content blocks closed"));
                            return;
                        }
                        finish_reason = payload.pointer("/delta/stop_reason").and_then(Value::as_str).map(str::to_string);
                        output_tokens = payload.pointer("/usage/output_tokens").and_then(Value::as_u64).unwrap_or(0);
                    }
                    "message_stop" => {
                        if !started || stopped || !blocks.is_empty() {
                            yield Err(invalid_response("Anthropic message_stop has an invalid lifecycle"));
                            return;
                        }
                        let finish = match finish_reason.as_deref() {
                            Some("max_tokens") => "MAX_TOKENS",
                            Some("refusal") => "SAFETY",
                            Some("end_turn" | "stop_sequence" | "tool_use") | None => "STOP",
                            Some(reason) => {
                                yield Err(invalid_response(format!("unsupported Anthropic stop_reason: {reason}")));
                                return;
                            }
                        };
                        yield Ok(gemini_sse_data(json!({
                            "responseId":message_id,
                            "modelVersion":model,
                            "candidates":[{"index":0,"finishReason":finish}],
                            "usageMetadata":{
                                "promptTokenCount":input_tokens.saturating_add(cached_tokens),
                                "cachedContentTokenCount":cached_tokens,
                                "candidatesTokenCount":output_tokens,
                                "totalTokenCount":input_tokens.saturating_add(cached_tokens).saturating_add(output_tokens)
                            }
                        })));
                        stopped = true;
                    }
                    "ping" => {}
                    unsupported => {
                        yield Err(invalid_response(format!("unsupported Anthropic SSE event: {unsupported}")));
                        return;
                    }
                }
            }
        }
        if !remainder.is_empty() {
            yield Err(invalid_response("Anthropic SSE ended with incomplete UTF-8"));
            return;
        }
        if !buffer.trim().is_empty() {
            yield Err(invalid_response("Anthropic SSE ended with an incomplete event"));
            return;
        }
        if !stopped {
            yield Err(invalid_response("Anthropic SSE ended before message_stop"));
        }
    }
}

fn append_gemini_utf8(
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
                return Err(invalid_response("Anthropic SSE contained invalid UTF-8"));
            }
            remainder.extend_from_slice(&input[valid..]);
        }
    }
    Ok(())
}

fn take_gemini_sse_block(buffer: &mut String) -> Option<String> {
    let (index, length) = [("\r\n\r\n", 4), ("\n\n", 2)]
        .into_iter()
        .filter_map(|(delimiter, length)| buffer.find(delimiter).map(|index| (index, length)))
        .min_by_key(|(index, _)| *index)?;
    let block = buffer[..index].to_string();
    buffer.drain(..index + length);
    Some(block)
}

fn parse_anthropic_sse_block(block: &str) -> Result<(&str, &str), BridgeError> {
    let mut event = None;
    let mut data = None;
    for raw_line in block.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let Some((field, value)) = line.split_once(':') else {
            return Err(invalid_response("Anthropic SSE line must contain ':'"));
        };
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" if event.replace(value).is_none() => {}
            "data" if data.replace(value).is_none() => {}
            "event" | "data" => {
                return Err(invalid_response(format!(
                    "Anthropic SSE contains duplicate {field}"
                )));
            }
            unsupported => {
                return Err(invalid_response(format!(
                    "unsupported Anthropic SSE field: {unsupported}"
                )));
            }
        }
    }
    let event = event
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_response("Anthropic SSE event is required"))?;
    let data = data
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_response("Anthropic SSE data is required"))?;
    Ok((event, data))
}

fn gemini_sse_data(payload: Value) -> Bytes {
    Bytes::from(format!("data: {payload}\n\n"))
}

fn gemini_content_to_anthropic(
    content: &Value,
    message_index: usize,
    tool_calls: &mut HashMap<String, String>,
) -> Result<Value, BridgeError> {
    let object = content
        .as_object()
        .ok_or_else(|| invalid_request(format!("contents[{message_index}] must be an object")))?;
    reject_unknown(object, &["role", "parts"])?;
    let role = required_string(
        object.get("role"),
        &format!("contents[{message_index}].role"),
    )?;
    let anthropic_role = match role {
        "user" => "user",
        "model" => "assistant",
        _ => {
            return Err(invalid_request(format!(
                "contents[{message_index}].role is unsupported: {role}"
            )));
        }
    };
    let source = object
        .get("parts")
        .and_then(Value::as_array)
        .filter(|parts| !parts.is_empty())
        .ok_or_else(|| {
            invalid_request(format!(
                "contents[{message_index}].parts must be a non-empty array"
            ))
        })?;
    let mut blocks = Vec::with_capacity(source.len());
    for (part_index, part) in source.iter().enumerate() {
        let field = format!("contents[{message_index}].parts[{part_index}]");
        let part = part
            .as_object()
            .ok_or_else(|| invalid_request(format!("{field} must be an object")))?;
        if let Some(text) = part.get("text") {
            reject_unknown(part, &["text"])?;
            blocks.push(json!({"type":"text","text":required_string(Some(text), &format!("{field}.text"))?}));
        } else if let Some(inline) = part.get("inlineData") {
            reject_unknown(part, &["inlineData"])?;
            if anthropic_role != "user" {
                return Err(invalid_request(format!(
                    "{field}.inlineData requires user role"
                )));
            }
            let inline = inline
                .as_object()
                .ok_or_else(|| invalid_request(format!("{field}.inlineData must be an object")))?;
            reject_unknown(inline, &["mimeType", "data"])?;
            let mime = required_string(
                inline.get("mimeType"),
                &format!("{field}.inlineData.mimeType"),
            )?;
            let data = required_string(inline.get("data"), &format!("{field}.inlineData.data"))?;
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| {
                    invalid_request(format!("{field}.inlineData.data must be valid base64"))
                })?;
            blocks.push(json!({
                "type":"image","source":{"type":"base64","media_type":mime,"data":data}
            }));
        } else if let Some(call) = part.get("functionCall") {
            reject_unknown(part, &["functionCall"])?;
            if anthropic_role != "assistant" {
                return Err(invalid_request(format!(
                    "{field}.functionCall requires model role"
                )));
            }
            let call = call.as_object().ok_or_else(|| {
                invalid_request(format!("{field}.functionCall must be an object"))
            })?;
            reject_unknown(call, &["id", "name", "args"])?;
            let name = required_string(call.get("name"), &format!("{field}.functionCall.name"))?;
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("gemini_call_{message_index}_{part_index}"));
            let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
            if !args.is_object() {
                return Err(invalid_request(format!(
                    "{field}.functionCall.args must be an object"
                )));
            }
            tool_calls.insert(name.to_string(), id.clone());
            blocks.push(json!({"type":"tool_use","id":id,"name":name,"input":args}));
        } else if let Some(response) = part.get("functionResponse") {
            reject_unknown(part, &["functionResponse"])?;
            if anthropic_role != "user" {
                return Err(invalid_request(format!(
                    "{field}.functionResponse requires user role"
                )));
            }
            let response = response.as_object().ok_or_else(|| {
                invalid_request(format!("{field}.functionResponse must be an object"))
            })?;
            reject_unknown(response, &["id", "name", "response"])?;
            let name = required_string(
                response.get("name"),
                &format!("{field}.functionResponse.name"),
            )?;
            let id = response
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .or_else(|| tool_calls.get(name).cloned())
                .ok_or_else(|| {
                    invalid_request(format!(
                        "{field}.functionResponse has no matching functionCall"
                    ))
                })?;
            let payload = response
                .get("response")
                .cloned()
                .unwrap_or_else(|| json!({}));
            blocks.push(json!({
                "type":"tool_result","tool_use_id":id,"content":payload.to_string()
            }));
        } else {
            return Err(invalid_request(format!("{field} is unsupported")));
        }
    }
    Ok(json!({"role":anthropic_role,"content":blocks}))
}

fn gemini_system_to_anthropic(value: &Value) -> Result<Value, BridgeError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_request("systemInstruction must be an object"))?;
    reject_unknown(object, &["role", "parts"])?;
    let parts = object
        .get("parts")
        .and_then(Value::as_array)
        .filter(|parts| !parts.is_empty())
        .ok_or_else(|| invalid_request("systemInstruction.parts must be a non-empty array"))?;
    let texts = parts
        .iter()
        .enumerate()
        .map(|(index, part)| {
            let part = part.as_object().ok_or_else(|| {
                invalid_request(format!(
                    "systemInstruction.parts[{index}] must be an object"
                ))
            })?;
            reject_unknown(part, &["text"])?;
            Ok(required_string(
                part.get("text"),
                &format!("systemInstruction.parts[{index}].text"),
            )?
            .to_string())
        })
        .collect::<Result<Vec<_>, BridgeError>>()?;
    if texts.len() == 1 {
        Ok(Value::String(texts[0].clone()))
    } else {
        Ok(Value::Array(
            texts
                .into_iter()
                .map(|text| json!({"type":"text","text":text}))
                .collect(),
        ))
    }
}

fn gemini_tools_to_anthropic(value: &Value) -> Result<Value, BridgeError> {
    let groups = value
        .as_array()
        .filter(|groups| !groups.is_empty())
        .ok_or_else(|| invalid_request("tools must be a non-empty array"))?;
    let mut tools = Vec::new();
    for (group_index, group) in groups.iter().enumerate() {
        let group = group
            .as_object()
            .ok_or_else(|| invalid_request(format!("tools[{group_index}] must be an object")))?;
        reject_unknown(group, &["functionDeclarations"])?;
        let declarations = group
            .get("functionDeclarations")
            .and_then(Value::as_array)
            .filter(|declarations| !declarations.is_empty())
            .ok_or_else(|| {
                invalid_request(format!(
                    "tools[{group_index}].functionDeclarations must be a non-empty array"
                ))
            })?;
        for (index, declaration) in declarations.iter().enumerate() {
            let declaration = declaration.as_object().ok_or_else(|| {
                invalid_request(format!(
                    "tools[{group_index}].functionDeclarations[{index}] must be an object"
                ))
            })?;
            reject_unknown(
                declaration,
                &["name", "description", "parametersJsonSchema"],
            )?;
            let name = required_string(
                declaration.get("name"),
                &format!("tools[{group_index}].functionDeclarations[{index}].name"),
            )?;
            let schema = declaration
                .get("parametersJsonSchema")
                .filter(|schema| schema.is_object())
                .cloned()
                .unwrap_or_else(|| json!({"type":"object","properties":{}}));
            tools.push(json!({
                "name":name,
                "description":declaration.get("description").and_then(Value::as_str).unwrap_or(""),
                "input_schema":schema
            }));
        }
    }
    Ok(Value::Array(tools))
}

fn gemini_tool_config_to_anthropic(value: &Value) -> Result<Value, BridgeError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_request("toolConfig must be an object"))?;
    reject_unknown(object, &["functionCallingConfig"])?;
    let config = object
        .get("functionCallingConfig")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_request("toolConfig.functionCallingConfig must be an object"))?;
    reject_unknown(config, &["mode", "allowedFunctionNames"])?;
    let mode = required_string(config.get("mode"), "toolConfig.functionCallingConfig.mode")?;
    match mode {
        "AUTO" => Ok(json!({"type":"auto"})),
        "ANY" => {
            let names = config.get("allowedFunctionNames").and_then(Value::as_array);
            match names {
                Some(names) if names.len() == 1 => Ok(json!({
                    "type":"tool",
                    "name":required_string(names.first(), "allowedFunctionNames[0]")?
                })),
                Some(_) => Err(invalid_request(
                    "ANY with allowedFunctionNames must select exactly one function",
                )),
                None => Ok(json!({"type":"any"})),
            }
        }
        "NONE" => Ok(json!({"type":"none"})),
        _ => Err(invalid_request(format!(
            "unsupported function calling mode: {mode}"
        ))),
    }
}

fn finite_number(value: &Value, field: &str) -> Result<Value, BridgeError> {
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .map(|value| json!(value))
        .ok_or_else(|| invalid_request(format!("generationConfig.{field} must be finite")))
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

fn anthropic_to_gemini(body: Value, streaming: bool) -> Result<Value, BridgeError> {
    let object = body
        .as_object()
        .ok_or_else(|| invalid_request("body must be an object"))?;
    reject_unknown(
        object,
        &[
            "model",
            "max_tokens",
            "system",
            "messages",
            "temperature",
            "top_p",
            "stop_sequences",
            "tools",
            "tool_choice",
            "stream",
            "metadata",
        ],
    )?;
    required_string(object.get("model"), "model")?;
    let max_tokens = object
        .get("max_tokens")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_request("max_tokens must be a positive integer"))?;
    let requested_stream = object
        .get("stream")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| invalid_request("stream must be a boolean"))
        })
        .transpose()?
        .unwrap_or(false);
    if requested_stream != streaming {
        return Err(invalid_request(if streaming {
            "the streaming bridge API requires stream=true"
        } else {
            "stream=true requires the streaming bridge API"
        }));
    }
    let source_messages = object
        .get("messages")
        .and_then(Value::as_array)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(|| invalid_request("messages must be a non-empty array"))?;
    let mut contents = Vec::with_capacity(source_messages.len());
    let mut tool_names = HashMap::new();
    for (index, message) in source_messages.iter().enumerate() {
        contents.push(convert_message(message, index, &mut tool_names)?);
    }
    let mut result = json!({
        "contents": contents,
        "generationConfig": {"maxOutputTokens": max_tokens}
    });
    if let Some(system) = object.get("system") {
        result["systemInstruction"] = json!({"parts": convert_text_content(system, "system")?});
    }
    let generation = result["generationConfig"]
        .as_object_mut()
        .expect("generationConfig is an object");
    copy_optional_number(object, generation, "temperature", "temperature")?;
    copy_optional_number(object, generation, "top_p", "topP")?;
    if let Some(stops) = object.get("stop_sequences") {
        let stops = stops
            .as_array()
            .filter(|stops| !stops.is_empty())
            .ok_or_else(|| invalid_request("stop_sequences must be a non-empty array"))?;
        for (index, stop) in stops.iter().enumerate() {
            required_string(Some(stop), &format!("stop_sequences[{index}]"))?;
        }
        generation.insert("stopSequences".into(), Value::Array(stops.clone()));
    }
    let has_tools = match object.get("tools") {
        Some(tools) => {
            let values = tools
                .as_array()
                .ok_or_else(|| invalid_request("tools must be an array"))?;
            if !values.is_empty() {
                result["tools"] = json!([{"functionDeclarations":convert_tools(tools)?}]);
            }
            !values.is_empty()
        }
        None => false,
    };
    if object.contains_key("tool_choice") && !has_tools {
        return Err(invalid_request("tool_choice requires tools"));
    }
    if let Some(choice) = object.get("tool_choice") {
        result["toolConfig"] = convert_tool_choice(choice)?;
    }
    Ok(result)
}

fn convert_message(
    message: &Value,
    index: usize,
    tool_names: &mut HashMap<String, String>,
) -> Result<Value, BridgeError> {
    let message = message
        .as_object()
        .ok_or_else(|| invalid_request(format!("messages[{index}] must be an object")))?;
    reject_unknown(message, &["role", "content"])?;
    let role = required_string(message.get("role"), &format!("messages[{index}].role"))?;
    let (anthropic_role, role) = match role {
        "user" => ("user", "user"),
        "assistant" => ("assistant", "model"),
        _ => {
            return Err(invalid_request(format!(
                "messages[{index}].role is unsupported"
            )));
        }
    };
    let parts = convert_message_content(
        message
            .get("content")
            .ok_or_else(|| invalid_request(format!("messages[{index}].content is required")))?,
        &format!("messages[{index}].content"),
        anthropic_role,
        tool_names,
    )?;
    Ok(json!({"role":role,"parts":parts}))
}

fn convert_message_content(
    value: &Value,
    field: &str,
    role: &str,
    tool_names: &mut HashMap<String, String>,
) -> Result<Vec<Value>, BridgeError> {
    if let Some(text) = value.as_str() {
        return Ok(vec![json!({"text":text})]);
    }
    let blocks = value
        .as_array()
        .filter(|blocks| !blocks.is_empty())
        .ok_or_else(|| invalid_request(format!("{field} must be a string or non-empty array")))?;
    let mut parts = Vec::with_capacity(blocks.len());
    for (index, block) in blocks.iter().enumerate() {
        let block = block
            .as_object()
            .ok_or_else(|| invalid_request(format!("{field}[{index}] must be an object")))?;
        match block.get("type").and_then(Value::as_str) {
            Some("text") => parts.push(json!({
                "text": required_string(block.get("text"), &format!("{field}[{index}].text"))?
            })),
            Some("image") if role == "user" => {
                let source = block
                    .get("source")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        invalid_request(format!("{field}[{index}].source must be an object"))
                    })?;
                reject_unknown(source, &["type", "media_type", "data"])?;
                if source.get("type").and_then(Value::as_str) != Some("base64") {
                    return Err(invalid_request(format!(
                        "{field}[{index}].source.type must be base64"
                    )));
                }
                let mime_type = required_string(
                    source.get("media_type"),
                    &format!("{field}[{index}].source.media_type"),
                )?;
                if !mime_type.starts_with("image/") {
                    return Err(invalid_request(format!(
                        "{field}[{index}].source.media_type must be an image MIME type"
                    )));
                }
                let data =
                    required_string(source.get("data"), &format!("{field}[{index}].source.data"))?;
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| {
                        invalid_request(format!(
                            "{field}[{index}].source.data must be valid base64"
                        ))
                    })?;
                parts.push(json!({
                    "inlineData":{"mimeType":mime_type,"data":data}
                }));
            }
            Some("tool_use") if role == "assistant" => {
                let id = required_string(block.get("id"), &format!("{field}[{index}].id"))?;
                let name = required_string(block.get("name"), &format!("{field}[{index}].name"))?;
                let input = block
                    .get("input")
                    .filter(|input| input.is_object())
                    .ok_or_else(|| {
                        invalid_request(format!("{field}[{index}].input must be an object"))
                    })?;
                tool_names.insert(id.to_string(), name.to_string());
                parts.push(json!({
                    "functionCall":{"id":id,"name":name,"args":input}
                }));
            }
            Some("tool_result") if role == "user" => {
                let id = required_string(
                    block.get("tool_use_id"),
                    &format!("{field}[{index}].tool_use_id"),
                )?;
                let name = tool_names.get(id).ok_or_else(|| {
                    invalid_request(format!(
                        "{field}[{index}] references unknown tool_use_id: {id}"
                    ))
                })?;
                let response = normalize_tool_result(block.get("content"), field, index)?;
                parts.push(json!({
                    "functionResponse":{"id":id,"name":name,"response":{"content":response}}
                }));
            }
            Some(kind) => {
                return Err(invalid_request(format!(
                    "{field}[{index}] block type is unsupported: {kind}"
                )));
            }
            None => {
                return Err(invalid_request(format!(
                    "{field}[{index}].type must be a non-empty string"
                )));
            }
        }
    }
    Ok(parts)
}

fn normalize_tool_result(
    content: Option<&Value>,
    field: &str,
    index: usize,
) -> Result<Value, BridgeError> {
    match content {
        None => Ok(json!("")),
        Some(Value::String(text)) => Ok(json!(text)),
        Some(Value::Array(blocks)) => {
            let mut texts = Vec::new();
            for (block_index, block) in blocks.iter().enumerate() {
                if block.get("type").and_then(Value::as_str) != Some("text") {
                    return Err(invalid_request(format!(
                        "{field}[{index}].content[{block_index}] block type is unsupported"
                    )));
                }
                texts.push(required_string(
                    block.get("text"),
                    &format!("{field}[{index}].content[{block_index}].text"),
                )?);
            }
            Ok(json!(texts.join("\n")))
        }
        Some(_) => Err(invalid_request(format!(
            "{field}[{index}].content must be a string or text block array"
        ))),
    }
}

fn convert_tools(value: &Value) -> Result<Vec<Value>, BridgeError> {
    let tools = value
        .as_array()
        .filter(|tools| !tools.is_empty())
        .ok_or_else(|| invalid_request("tools must be a non-empty array"))?;
    tools
        .iter()
        .enumerate()
        .map(|(index, tool)| {
            let tool = tool
                .as_object()
                .ok_or_else(|| invalid_request(format!("tools[{index}] must be an object")))?;
            reject_unknown(tool, &["name", "description", "input_schema"])?;
            let name = required_string(tool.get("name"), &format!("tools[{index}].name"))?;
            let schema = tool
                .get("input_schema")
                .filter(|schema| schema.is_object())
                .ok_or_else(|| {
                    invalid_request(format!("tools[{index}].input_schema must be an object"))
                })?;
            Ok(json!({
                "name":name,
                "description":tool.get("description").and_then(Value::as_str).unwrap_or(""),
                "parametersJsonSchema":schema
            }))
        })
        .collect()
}

fn convert_tool_choice(value: &Value) -> Result<Value, BridgeError> {
    let choice = value
        .as_object()
        .ok_or_else(|| invalid_request("tool_choice must be an object"))?;
    let kind = required_string(choice.get("type"), "tool_choice.type")?;
    let config = match kind {
        "auto" => json!({"mode":"AUTO"}),
        "any" => json!({"mode":"ANY"}),
        "none" => json!({"mode":"NONE"}),
        "tool" => json!({
            "mode":"ANY",
            "allowedFunctionNames":[required_string(choice.get("name"), "tool_choice.name")?]
        }),
        _ => {
            return Err(invalid_request(format!(
                "unsupported tool_choice.type: {kind}"
            )));
        }
    };
    Ok(json!({"functionCallingConfig":config}))
}

fn convert_text_content(value: &Value, field: &str) -> Result<Vec<Value>, BridgeError> {
    if let Some(text) = value.as_str() {
        return Ok(vec![json!({"text":text})]);
    }
    let blocks = value
        .as_array()
        .filter(|blocks| !blocks.is_empty())
        .ok_or_else(|| invalid_request(format!("{field} must be a string or non-empty array")))?;
    blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let block = block
                .as_object()
                .ok_or_else(|| invalid_request(format!("{field}[{index}] must be an object")))?;
            if block.get("type").and_then(Value::as_str) != Some("text") {
                return Err(invalid_request(format!(
                    "{field}[{index}] block type is unsupported"
                )));
            }
            Ok(json!({
                "text": required_string(block.get("text"), &format!("{field}[{index}].text"))?
            }))
        })
        .collect()
}

fn gemini_to_anthropic(body: Value) -> Result<Value, BridgeError> {
    let candidate = body
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .ok_or_else(|| invalid_response("response must contain a candidate"))?;
    let parts = candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response("candidate content.parts must be an array"))?;
    let mut content = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            content.push(json!({"type":"text","text":text}));
        } else if let Some(call) = part.get("functionCall") {
            let name = call
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_response(format!("functionCall {index} is missing name")))?;
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("gemini_call_{index}"));
            let input = call.get("args").cloned().unwrap_or_else(|| json!({}));
            content.push(json!({"type":"tool_use","id":id,"name":name,"input":input}));
        } else {
            return Err(invalid_response(format!(
                "candidate part {index} is unsupported"
            )));
        }
    }
    let has_tool_use = content.iter().any(|block| block["type"] == "tool_use");
    Ok(json!({
        "id": body.get("responseId").and_then(Value::as_str).unwrap_or(""),
        "type":"message",
        "role":"assistant",
        "content":content,
        "model":body.get("modelVersion").and_then(Value::as_str).unwrap_or(""),
        "stop_reason":map_finish_reason(candidate.get("finishReason").and_then(Value::as_str), has_tool_use),
        "stop_sequence":Value::Null,
        "usage":anthropic_usage(body.get("usageMetadata"))
    }))
}

pub(crate) fn anthropic_usage(value: Option<&Value>) -> Value {
    let prompt = value
        .and_then(|value| value.get("promptTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached = value
        .and_then(|value| value.get("cachedContentTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = value
        .and_then(|value| value.get("totalTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut usage = json!({
        "input_tokens":prompt.saturating_sub(cached),
        "output_tokens":total.saturating_sub(prompt)
    });
    if cached > 0 {
        usage["cache_read_input_tokens"] = json!(cached);
    }
    usage
}

pub(crate) fn map_finish_reason(reason: Option<&str>, has_tool_use: bool) -> &'static str {
    match reason {
        Some("MAX_TOKENS") => "max_tokens",
        Some("SAFETY" | "RECITATION" | "SPII" | "BLOCKLIST" | "PROHIBITED_CONTENT") => "refusal",
        _ if has_tool_use => "tool_use",
        _ => "end_turn",
    }
}

async fn gemini_http_error(response: reqwest::Response, api_key: &str) -> BridgeError {
    let status = response.status().as_u16();
    let body = response.json::<Value>().await.ok();
    let kind = body
        .as_ref()
        .and_then(|body| body.pointer("/error/status"))
        .and_then(Value::as_str)
        .map(safe_error_text);
    let message = body
        .as_ref()
        .and_then(|body| body.pointer("/error/message"))
        .and_then(Value::as_str)
        .map(|message| safe_error_text(&message.replace(api_key, "[redacted]")));
    BridgeError::GeminiUpstream {
        status,
        kind,
        message,
    }
}

fn safe_error_text(value: &str) -> String {
    value.replace(['\r', '\n'], " ").chars().take(300).collect()
}

fn copy_optional_number(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    source_name: &str,
    target_name: &str,
) -> Result<(), BridgeError> {
    if let Some(value) = source.get(source_name) {
        let value = value
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| invalid_request(format!("{source_name} must be a finite number")))?;
        target.insert(target_name.into(), json!(value));
    }
    Ok(())
}

fn required_string<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_request(format!("{field} must be a non-empty string")))
}

fn reject_unknown(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), BridgeError> {
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(invalid_request(format!("unsupported field: {field}")));
    }
    Ok(())
}

fn invalid_request(message: impl Into<String>) -> BridgeError {
    BridgeError::InvalidGeminiRequest(message.into())
}

fn invalid_response(message: impl Into<String>) -> BridgeError {
    BridgeError::InvalidGeminiResponse(message.into())
}
