// Minimal Chat Completions SSE adapter derived from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::BridgeError;
use super::chat::{OpenAiChatCapabilities, chat_usage, invalid_response, safe_kind, safe_message};
use super::sse::{append_utf8, parse_data_sse_block, take_sse_block};
use async_stream::stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::fmt::Display;

pub fn chat_sse_to_anthropic<S, E>(
    upstream: S,
    capabilities: OpenAiChatCapabilities,
) -> impl Stream<Item = Result<Bytes, BridgeError>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Display + Send + 'static,
{
    stream! {
        futures::pin_mut!(upstream);
        let mut state = State::new(capabilities);
        let mut buffer = String::new();
        let mut remainder = Vec::new();
        while let Some(chunk) = upstream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(_) => {
                    yield Ok(error_sse("stream_transport_error", "Chat Completions stream transport failed"));
                    return;
                }
            };
            if append_utf8(&mut buffer, &mut remainder, &chunk).is_err() {
                yield Ok(error_sse("stream_protocol_error", "Chat SSE contained invalid UTF-8"));
                return;
            }
            while let Some(block) = take_sse_block(&mut buffer) {
                let data = match parse_data_sse_block(&block) {
                    Ok(data) => data,
                    Err(message) => {
                        yield Ok(error_sse("stream_protocol_error", &message));
                        return;
                    }
                };
                if data == "[DONE]" {
                    match state.terminal_events() {
                        Ok(events) => for event in events { yield Ok(event); },
                        Err(error) => yield Ok(chat_bridge_error_sse(error)),
                    }
                    return;
                }
                let value: Value = match serde_json::from_str(data) {
                    Ok(value) => value,
                    Err(_) => {
                        yield Ok(error_sse("stream_protocol_error", "Chat SSE data must be valid JSON"));
                        return;
                    }
                };
                match state.handle(&value) {
                    Ok(events) => for event in events { yield Ok(event); },
                    Err(error) => {
                        yield Ok(chat_bridge_error_sse(error));
                        return;
                    }
                }
                if state.terminated {
                    return;
                }
            }
        }
        if !remainder.is_empty() {
            yield Ok(error_sse("stream_protocol_error", "Chat SSE ended inside a UTF-8 character"));
        } else if !buffer.is_empty() {
            yield Ok(error_sse("stream_protocol_error", "Chat SSE ended inside an event"));
        } else {
            match state.terminal_events() {
                Ok(events) => for event in events { yield Ok(event); },
                Err(error) => yield Ok(chat_bridge_error_sse(error)),
            }
        }
    }
}

struct State {
    capabilities: OpenAiChatCapabilities,
    id: Option<String>,
    model: Option<String>,
    started: bool,
    next_index: u64,
    open_block: Option<OpenBlock>,
    tools: HashMap<u64, Tool>,
    finish_reason: Option<String>,
    usage: Option<Value>,
    terminated: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Thinking,
}

struct OpenBlock {
    kind: BlockKind,
    index: u64,
}

struct Tool {
    anthropic_index: u64,
    id: String,
    name: String,
    arguments: String,
    started: bool,
    stopped: bool,
}

impl State {
    fn new(capabilities: OpenAiChatCapabilities) -> Self {
        Self {
            capabilities,
            id: None,
            model: None,
            started: false,
            next_index: 0,
            open_block: None,
            tools: HashMap::new(),
            finish_reason: None,
            usage: None,
            terminated: false,
        }
    }

    fn handle(&mut self, value: &Value) -> Result<Vec<Bytes>, BridgeError> {
        let object = value
            .as_object()
            .ok_or_else(|| invalid_response("SSE data must be an object"))?;
        if let Some(error) = object.get("error").filter(|error| !error.is_null()) {
            let error = error
                .as_object()
                .ok_or_else(|| invalid_response("error must be an object"))?;
            let kind = error
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_response("error.type must be a string"))?;
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_response("error.message must be a string"))?;
            self.terminated = true;
            return Ok(vec![error_sse(&safe_kind(kind), &safe_message(message))]);
        }
        if let Some(id) = object.get("id") {
            let id = id
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| invalid_response("SSE id must be a non-empty string"))?;
            if self.id.is_none() {
                self.id = Some(id.to_owned());
            }
        }
        if let Some(model) = object.get("model") {
            let model = model
                .as_str()
                .filter(|model| !model.is_empty())
                .ok_or_else(|| invalid_response("SSE model must be a non-empty string"))?;
            if self.model.is_none() {
                self.model = Some(model.to_owned());
            }
        }
        if let Some(usage) = object.get("usage").filter(|usage| !usage.is_null()) {
            self.usage = Some(chat_usage(Some(usage))?);
        }
        let choices = object
            .get("choices")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_response("SSE choices must be an array"))?;
        if choices.is_empty() {
            if object.get("usage").is_none_or(Value::is_null) {
                return Err(invalid_response("empty SSE choices require usage"));
            }
            return Ok(Vec::new());
        }
        if choices.len() != 1 {
            return Err(invalid_response(
                "SSE choices must contain at most one item",
            ));
        }
        let choice = choices[0]
            .as_object()
            .ok_or_else(|| invalid_response("SSE choices[0] must be an object"))?;
        if choice.get("index").and_then(Value::as_u64) != Some(0) {
            return Err(invalid_response("SSE choices[0].index must be 0"));
        }
        let delta = choice
            .get("delta")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_response("SSE choices[0].delta must be an object"))?;
        reject_delta_unknown(delta)?;
        let has_payload = delta
            .get("content")
            .is_some_and(|v| v.as_str().is_some_and(|v| !v.is_empty()))
            || delta
                .get("reasoning_content")
                .is_some_and(|v| v.as_str().is_some_and(|v| !v.is_empty()))
            || delta
                .get("tool_calls")
                .is_some_and(|v| v.as_array().is_some_and(|v| !v.is_empty()));
        if self.finish_reason.is_some() && has_payload {
            return Err(invalid_response("SSE payload arrived after finish_reason"));
        }
        let mut events = Vec::new();
        if has_payload {
            self.ensure_started(&mut events)?;
        }
        if let Some(role) = delta.get("role") {
            if role.as_str() != Some("assistant") {
                return Err(invalid_response("SSE delta.role must be assistant"));
            }
        }
        if let Some(reasoning) = delta
            .get("reasoning_content")
            .filter(|value| !value.is_null())
        {
            if !self.capabilities.reasoning_content {
                return Err(invalid_response(
                    "reasoning_content requires the provider capability",
                ));
            }
            let reasoning = reasoning
                .as_str()
                .ok_or_else(|| invalid_response("SSE reasoning_content must be a string"))?;
            if !reasoning.is_empty() {
                self.push_non_tool(BlockKind::Thinking, reasoning, &mut events);
            }
        }
        if let Some(content) = delta.get("content").filter(|value| !value.is_null()) {
            let content = content
                .as_str()
                .ok_or_else(|| invalid_response("SSE content must be a string"))?;
            if !content.is_empty() {
                self.push_non_tool(BlockKind::Text, content, &mut events);
            }
        }
        if let Some(tool_calls) = delta.get("tool_calls") {
            let tool_calls = tool_calls
                .as_array()
                .filter(|calls| !calls.is_empty())
                .ok_or_else(|| invalid_response("SSE tool_calls must be a non-empty array"))?;
            self.close_non_tool(&mut events);
            for call in tool_calls {
                self.push_tool(call, &mut events)?;
            }
        }
        if let Some(reason) = choice
            .get("finish_reason")
            .filter(|reason| !reason.is_null())
        {
            let reason = reason
                .as_str()
                .ok_or_else(|| invalid_response("SSE finish_reason must be a string"))?;
            if self.finish_reason.is_none() {
                self.finish(reason, &mut events)?;
            }
        }
        Ok(events)
    }

    fn ensure_started(&mut self, events: &mut Vec<Bytes>) -> Result<(), BridgeError> {
        if self.started {
            return Ok(());
        }
        let id = self
            .id
            .as_deref()
            .ok_or_else(|| invalid_response("first Chat payload is missing id"))?;
        let model = self
            .model
            .as_deref()
            .ok_or_else(|| invalid_response("first Chat payload is missing model"))?;
        events.push(sse(
            "message_start",
            json!({
                "type":"message_start","message":{"id":id,"type":"message","role":"assistant",
                    "model":model,"content":[],"stop_reason":null,"stop_sequence":null,
                    "usage":{"input_tokens":0,"output_tokens":0}}
            }),
        ));
        self.started = true;
        Ok(())
    }

    fn push_non_tool(&mut self, kind: BlockKind, text: &str, events: &mut Vec<Bytes>) {
        if self
            .open_block
            .as_ref()
            .is_none_or(|block| block.kind != kind)
        {
            self.close_non_tool(events);
            let index = self.take_index();
            let content = match kind {
                BlockKind::Text => json!({"type":"text","text":""}),
                BlockKind::Thinking => json!({"type":"thinking","thinking":""}),
            };
            events.push(sse(
                "content_block_start",
                json!({"type":"content_block_start","index":index,"content_block":content}),
            ));
            self.open_block = Some(OpenBlock { kind, index });
        }
        let block = self.open_block.as_ref().expect("opened above");
        let delta = match kind {
            BlockKind::Text => json!({"type":"text_delta","text":text}),
            BlockKind::Thinking => json!({"type":"thinking_delta","thinking":text}),
        };
        events.push(sse(
            "content_block_delta",
            json!({"type":"content_block_delta","index":block.index,"delta":delta}),
        ));
    }

    fn close_non_tool(&mut self, events: &mut Vec<Bytes>) {
        if let Some(block) = self.open_block.take() {
            events.push(sse(
                "content_block_stop",
                json!({"type":"content_block_stop","index":block.index}),
            ));
        }
    }

    fn push_tool(&mut self, value: &Value, events: &mut Vec<Bytes>) -> Result<(), BridgeError> {
        let call = value
            .as_object()
            .ok_or_else(|| invalid_response("SSE tool call must be an object"))?;
        let index = call
            .get("index")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid_response("SSE tool call index must be an unsigned integer"))?;
        let next_index = &mut self.next_index;
        self.tools.entry(index).or_insert_with(|| {
            let anthropic_index = *next_index;
            *next_index += 1;
            Tool {
                anthropic_index,
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
                started: false,
                stopped: false,
            }
        });
        let tool = self.tools.get_mut(&index).expect("inserted above");
        if let Some(kind) = call.get("type") {
            if kind.as_str() != Some("function") {
                return Err(invalid_response("SSE tool call type must be function"));
            }
        }
        if let Some(id) = call.get("id") {
            tool.id = id
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| invalid_response("SSE tool call id must be a non-empty string"))?
                .to_owned();
        }
        let function = call
            .get("function")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_response("SSE tool call function must be an object"))?;
        if let Some(name) = function.get("name") {
            tool.name = name
                .as_str()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| invalid_response("SSE tool name must be a non-empty string"))?
                .to_owned();
        }
        let arguments = function
            .get("arguments")
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| invalid_response("SSE tool arguments must be a string"))
            })
            .transpose()?
            .unwrap_or("");
        tool.arguments.push_str(arguments);
        if !tool.started && !tool.id.is_empty() && !tool.name.is_empty() {
            tool.started = true;
            events.push(sse("content_block_start", json!({"type":"content_block_start","index":tool.anthropic_index,"content_block":{"type":"tool_use","id":tool.id,"name":tool.name,"input":{}}})));
            if !tool.arguments.is_empty() {
                events.push(sse("content_block_delta", json!({"type":"content_block_delta","index":tool.anthropic_index,"delta":{"type":"input_json_delta","partial_json":tool.arguments}})));
            }
        } else if tool.started && !arguments.is_empty() {
            events.push(sse("content_block_delta", json!({"type":"content_block_delta","index":tool.anthropic_index,"delta":{"type":"input_json_delta","partial_json":arguments}})));
        }
        Ok(())
    }

    fn finish(&mut self, reason: &str, events: &mut Vec<Bytes>) -> Result<(), BridgeError> {
        let stop = match reason {
            "stop" | "content_filter" => "end_turn",
            "length" => "max_tokens",
            "tool_calls" | "function_call" => "tool_use",
            _ => {
                return Err(invalid_response(&format!(
                    "unsupported finish_reason: {reason}"
                )));
            }
        };
        self.close_non_tool(events);
        let mut indices: Vec<u64> = self.tools.keys().copied().collect();
        indices.sort_unstable();
        for index in indices {
            let tool = self.tools.get_mut(&index).expect("existing tool");
            if !tool.started {
                return Err(invalid_response(
                    "tool call finished before id and name arrived",
                ));
            }
            let input: Value = serde_json::from_str(&tool.arguments)
                .map_err(|_| invalid_response("completed tool arguments must be valid JSON"))?;
            if !input.is_object() {
                return Err(invalid_response(
                    "completed tool arguments must be a JSON object",
                ));
            }
            if !tool.stopped {
                tool.stopped = true;
                events.push(sse(
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":tool.anthropic_index}),
                ));
            }
        }
        self.finish_reason = Some(stop.into());
        Ok(())
    }

    fn terminal_events(&mut self) -> Result<Vec<Bytes>, BridgeError> {
        if self.terminated {
            return Ok(Vec::new());
        }
        let reason = self
            .finish_reason
            .as_deref()
            .ok_or_else(|| invalid_response("Chat SSE ended without finish_reason"))?;
        if !self.started {
            return Err(invalid_response(
                "Chat SSE finished without a response payload",
            ));
        }
        let usage = self
            .usage
            .clone()
            .unwrap_or_else(|| json!({"input_tokens":0,"output_tokens":0}));
        self.terminated = true;
        Ok(vec![
            sse(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":reason,"stop_sequence":null},"usage":usage}),
            ),
            sse("message_stop", json!({"type":"message_stop"})),
        ])
    }

    fn take_index(&mut self) -> u64 {
        let index = self.next_index;
        self.next_index += 1;
        index
    }
}

fn reject_delta_unknown(delta: &Map<String, Value>) -> Result<(), BridgeError> {
    let allowed = ["role", "content", "reasoning_content", "tool_calls"];
    if let Some(field) = delta
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(invalid_response(&format!(
            "unsupported Chat SSE delta field: {field}"
        )));
    }
    Ok(())
}

fn sse(event: &str, value: Value) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {value}\n\n"))
}

fn error_sse(kind: &str, message: &str) -> Bytes {
    sse(
        "error",
        json!({"type":"error","error":{"type":kind,"message":message}}),
    )
}

fn chat_bridge_error_sse(error: BridgeError) -> Bytes {
    error_sse("stream_protocol_error", &error.to_string())
}
