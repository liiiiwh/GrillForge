// Minimal Anthropic Messages SSE → Codex Responses SSE bridge adapted from
// cc-switch, commit 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::{
    AnthropicSseStream, BridgeError, CodexAnthropicCapabilities, CodexAnthropicContext,
    codex_anthropic, sse,
};
use async_stream::stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
struct State {
    capabilities: CodexAnthropicCapabilities,
    context: CodexAnthropicContext,
    response_id: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    started: bool,
    next_output_index: u64,
    open_blocks: BTreeMap<u64, StreamBlock>,
    output: BTreeMap<u64, Value>,
    stop_reason: Option<String>,
    terminated: bool,
}

enum StreamBlock {
    Text {
        output_index: u64,
        item_id: String,
        text: String,
    },
    Tool {
        output_index: u64,
        item_id: String,
        call_id: String,
        name: String,
        arguments: String,
        custom: bool,
    },
    Reasoning {
        output_index: u64,
        item_id: String,
        thinking: String,
        signature: String,
    },
}

pub fn anthropic_sse_to_codex_responses<S, E>(
    source: S,
    capabilities: CodexAnthropicCapabilities,
) -> AnthropicSseStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    anthropic_sse_to_codex_responses_with_context(
        source,
        capabilities,
        CodexAnthropicContext::default(),
    )
}

pub fn anthropic_sse_to_codex_responses_with_context<S, E>(
    source: S,
    capabilities: CodexAnthropicCapabilities,
    context: CodexAnthropicContext,
) -> AnthropicSseStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    Box::pin(stream! {
        let mut source = Box::pin(source);
        let mut buffer = String::new();
        let mut remainder = Vec::new();
        let mut state = State { capabilities, context, ..State::default() };
        while let Some(chunk) = source.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    yield Err(invalid(format!("Anthropic SSE transport failed: {error}")));
                    return;
                }
            };
            if let Err(error) = sse::append_utf8(&mut buffer, &mut remainder, &chunk) {
                yield Err(invalid(error.to_string()));
                return;
            }
            while let Some(block) = sse::take_sse_block(&mut buffer) {
                let (event_name, data) = match sse::parse_sse_block(&block) {
                    Ok(event) => event,
                    Err(error) => {
                        yield Err(invalid(error.to_string()));
                        return;
                    }
                };
                let value: Value = match serde_json::from_str(data) {
                    Ok(value) => value,
                    Err(_) => {
                        yield Err(invalid("Anthropic SSE data must be valid JSON"));
                        return;
                    }
                };
                if value.get("type").and_then(Value::as_str) != Some(event_name) {
                    yield Err(invalid("Anthropic SSE event and data type must match"));
                    return;
                }
                match state.consume(event_name, &value) {
                    Ok(events) => {
                        for event in events {
                            yield Ok(event);
                        }
                    }
                    Err(error) => {
                        yield Err(error);
                        return;
                    }
                }
                if state.terminated {
                    return;
                }
            }
        }
        if !remainder.is_empty() || !buffer.is_empty() {
            yield Err(invalid("Anthropic SSE ended inside an event"));
        } else if !state.terminated {
            yield Err(invalid("Anthropic SSE ended before message_stop"));
        }
    })
}

impl State {
    fn consume(&mut self, event_name: &str, value: &Value) -> Result<Vec<Bytes>, BridgeError> {
        let data = value
            .as_object()
            .ok_or_else(|| invalid("Anthropic SSE data must be an object"))?;
        match event_name {
            "message_start" => self.message_start(data),
            "content_block_start" => self.content_start(data),
            "content_block_delta" => self.content_delta(data),
            "content_block_stop" => self.content_stop(data),
            "message_delta" => self.message_delta(data),
            "message_stop" => self.message_stop(),
            "ping" => self.require_started().map(|()| Vec::new()),
            "error" => Ok(self.upstream_error(data)),
            _ => Err(invalid(format!(
                "unsupported Anthropic SSE event: {event_name}"
            ))),
        }
    }

    fn message_start(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        if self.started {
            return Err(invalid("message_start must occur exactly once"));
        }
        let message = object(data.get("message"), "message_start.message")?;
        let id = string(message.get("id"), "message_start.message.id")?;
        self.response_id = if id.starts_with("resp_") {
            id.to_owned()
        } else {
            format!("resp_{id}")
        };
        self.model = string(message.get("model"), "message_start.message.model")?.to_owned();
        let usage = object(message.get("usage"), "message_start.message.usage")?;
        self.input_tokens = number(
            usage.get("input_tokens"),
            "message_start.message.usage.input_tokens",
        )?;
        self.output_tokens = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        self.input_tokens
            .checked_add(self.output_tokens)
            .ok_or_else(|| invalid("message_start usage token count overflowed"))?;
        self.started = true;
        let response = self.response("in_progress", Value::Array(Vec::new()));
        Ok(vec![
            event(
                "response.created",
                json!({"type":"response.created","response":response}),
            ),
            event(
                "response.in_progress",
                json!({"type":"response.in_progress","response":response}),
            ),
        ])
    }

    fn content_start(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let anthropic_index = number(data.get("index"), "content_block_start.index")?;
        if self.open_blocks.contains_key(&anthropic_index) {
            return Err(invalid("duplicate content_block_start.index"));
        }
        let block = object(
            data.get("content_block"),
            "content_block_start.content_block",
        )?;
        let output_index = self.next_output_index;
        self.next_output_index += 1;
        match string(block.get("type"), "content_block_start.content_block.type")? {
            "text" => {
                if block.get("text").and_then(Value::as_str) != Some("") {
                    return Err(invalid("text content block must start with empty text"));
                }
                let item_id = format!("msg_{}_{}", self.response_id, output_index);
                self.open_blocks.insert(
                    anthropic_index,
                    StreamBlock::Text {
                        output_index,
                        item_id: item_id.clone(),
                        text: String::new(),
                    },
                );
                Ok(vec![
                    event(
                        "response.output_item.added",
                        json!({"type":"response.output_item.added","output_index":output_index,"item":{"id":item_id,"type":"message","role":"assistant","status":"in_progress","content":[]}}),
                    ),
                    event(
                        "response.content_part.added",
                        json!({"type":"response.content_part.added","item_id":item_id,"output_index":output_index,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
                    ),
                ])
            }
            "tool_use" => {
                let input = object(
                    block.get("input"),
                    "content_block_start.content_block.input",
                )?;
                if !input.is_empty() {
                    return Err(invalid("streamed tool_use must start with empty input"));
                }
                let call_id =
                    string(block.get("id"), "content_block_start.content_block.id")?.to_owned();
                let name =
                    string(block.get("name"), "content_block_start.content_block.name")?.to_owned();
                let custom = self.context.custom_tools.contains(&name);
                let item_id = format!(
                    "{}_{}_{}",
                    if custom { "ct" } else { "fc" },
                    self.response_id,
                    output_index
                );
                self.open_blocks.insert(
                    anthropic_index,
                    StreamBlock::Tool {
                        output_index,
                        item_id: item_id.clone(),
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: String::new(),
                        custom,
                    },
                );
                Ok(vec![event(
                    "response.output_item.added",
                    if custom {
                        json!({"type":"response.output_item.added","output_index":output_index,"item":{"id":item_id,"type":"custom_tool_call","status":"in_progress","call_id":call_id,"name":name,"input":""}})
                    } else {
                        json!({"type":"response.output_item.added","output_index":output_index,"item":{"id":item_id,"type":"function_call","status":"in_progress","call_id":call_id,"name":name,"arguments":""}})
                    },
                )])
            }
            "thinking" => {
                if !self.capabilities.reasoning {
                    return Err(invalid(
                        "Anthropic thinking requires the explicit reasoning capability",
                    ));
                }
                if block.get("thinking").and_then(Value::as_str) != Some("") {
                    return Err(invalid("thinking content block must start empty"));
                }
                let item_id = format!("rs_{}_{}", self.response_id, output_index);
                self.open_blocks.insert(
                    anthropic_index,
                    StreamBlock::Reasoning {
                        output_index,
                        item_id: item_id.clone(),
                        thinking: String::new(),
                        signature: String::new(),
                    },
                );
                Ok(vec![
                    event(
                        "response.output_item.added",
                        json!({"type":"response.output_item.added","output_index":output_index,"item":{"id":item_id,"type":"reasoning","status":"in_progress","summary":[]}}),
                    ),
                    event(
                        "response.reasoning_summary_part.added",
                        json!({"type":"response.reasoning_summary_part.added","item_id":item_id,"output_index":output_index,"summary_index":0,"part":{"type":"summary_text","text":""}}),
                    ),
                ])
            }
            other => Err(invalid(format!(
                "content_block_start.content_block.type is unsupported: {other}"
            ))),
        }
    }

    fn content_delta(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let anthropic_index = number(data.get("index"), "content_block_delta.index")?;
        let delta = object(data.get("delta"), "content_block_delta.delta")?;
        let block = self
            .open_blocks
            .get_mut(&anthropic_index)
            .ok_or_else(|| invalid("content_block_delta has no matching content block"))?;
        match (
            block,
            string(delta.get("type"), "content_block_delta.delta.type")?,
        ) {
            (
                StreamBlock::Text {
                    output_index,
                    item_id,
                    text: accumulated,
                },
                "text_delta",
            ) => {
                let text = string(delta.get("text"), "content_block_delta.delta.text")?;
                accumulated.push_str(text);
                Ok(vec![event(
                    "response.output_text.delta",
                    json!({"type":"response.output_text.delta","item_id":item_id,"output_index":output_index,"content_index":0,"delta":text}),
                )])
            }
            (
                StreamBlock::Tool {
                    output_index,
                    item_id,
                    arguments: accumulated,
                    custom,
                    ..
                },
                "input_json_delta",
            ) => {
                let arguments = string(
                    delta.get("partial_json"),
                    "content_block_delta.delta.partial_json",
                )?;
                accumulated.push_str(arguments);
                if *custom {
                    Ok(Vec::new())
                } else {
                    Ok(vec![event(
                        "response.function_call_arguments.delta",
                        json!({"type":"response.function_call_arguments.delta","item_id":item_id,"output_index":output_index,"delta":arguments}),
                    )])
                }
            }
            (
                StreamBlock::Reasoning {
                    output_index,
                    item_id,
                    thinking,
                    ..
                },
                "thinking_delta",
            ) => {
                let value = string(delta.get("thinking"), "content_block_delta.delta.thinking")?;
                thinking.push_str(value);
                Ok(vec![event(
                    "response.reasoning_summary_text.delta",
                    json!({"type":"response.reasoning_summary_text.delta","item_id":item_id,"output_index":output_index,"summary_index":0,"delta":value}),
                )])
            }
            (StreamBlock::Reasoning { signature, .. }, "signature_delta") => {
                let value = string(
                    delta.get("signature"),
                    "content_block_delta.delta.signature",
                )?;
                signature.push_str(value);
                Ok(Vec::new())
            }
            _ => Err(invalid(
                "content_block_delta type does not match the open content block",
            )),
        }
    }

    fn content_stop(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let anthropic_index = number(data.get("index"), "content_block_stop.index")?;
        let block = self
            .open_blocks
            .remove(&anthropic_index)
            .ok_or_else(|| invalid("content_block_stop has no matching content block"))?;
        let (output_index, item, events) = match block {
            StreamBlock::Text {
                output_index,
                item_id,
                text,
            } => {
                if text.is_empty() {
                    return Err(invalid("completed text block must not be empty"));
                }
                let item = json!({"id":item_id,"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]});
                let events = vec![
                    event(
                        "response.output_text.done",
                        json!({"type":"response.output_text.done","item_id":item_id,"output_index":output_index,"content_index":0,"text":text}),
                    ),
                    event(
                        "response.content_part.done",
                        json!({"type":"response.content_part.done","item_id":item_id,"output_index":output_index,"content_index":0,"part":{"type":"output_text","text":text,"annotations":[]}}),
                    ),
                    event(
                        "response.output_item.done",
                        json!({"type":"response.output_item.done","output_index":output_index,"item":item}),
                    ),
                ];
                (output_index, item, events)
            }
            StreamBlock::Tool {
                output_index,
                item_id,
                call_id,
                name,
                arguments,
                custom,
            } => {
                let parsed: Value = serde_json::from_str(&arguments)
                    .map_err(|_| invalid("completed tool arguments must be valid JSON"))?;
                if !parsed.is_object() {
                    return Err(invalid("completed tool arguments must be a JSON object"));
                }
                let (item, done) = if custom {
                    let object = parsed.as_object().expect("object checked");
                    let input = object
                        .get("input")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| {
                            invalid("completed custom tool arguments require non-empty input")
                        })?;
                    if object.len() != 1 {
                        return Err(invalid(
                            "completed custom tool arguments must contain only input",
                        ));
                    }
                    (
                        json!({"id":item_id,"type":"custom_tool_call","status":"completed","call_id":call_id,"name":name,"input":input}),
                        event(
                            "response.custom_tool_call_input.done",
                            json!({"type":"response.custom_tool_call_input.done","item_id":item_id,"output_index":output_index,"input":input}),
                        ),
                    )
                } else {
                    (
                        json!({"id":item_id,"type":"function_call","status":"completed","call_id":call_id,"name":name,"arguments":arguments}),
                        event(
                            "response.function_call_arguments.done",
                            json!({"type":"response.function_call_arguments.done","item_id":item_id,"output_index":output_index,"arguments":arguments}),
                        ),
                    )
                };
                let events = vec![
                    done,
                    event(
                        "response.output_item.done",
                        json!({"type":"response.output_item.done","output_index":output_index,"item":item}),
                    ),
                ];
                (output_index, item, events)
            }
            StreamBlock::Reasoning {
                output_index,
                item_id,
                thinking,
                signature,
            } => {
                if thinking.is_empty() || signature.is_empty() {
                    return Err(invalid(
                        "completed thinking block requires thinking and signature",
                    ));
                }
                let encrypted = codex_anthropic::encode_thinking(&json!({
                    "type":"thinking","thinking":thinking,"signature":signature
                }))?;
                let item = json!({"id":item_id,"type":"reasoning","status":"completed","summary":[{"type":"summary_text","text":thinking}],"encrypted_content":encrypted});
                let events = vec![
                    event(
                        "response.reasoning_summary_text.done",
                        json!({"type":"response.reasoning_summary_text.done","item_id":item_id,"output_index":output_index,"summary_index":0,"text":thinking}),
                    ),
                    event(
                        "response.reasoning_summary_part.done",
                        json!({"type":"response.reasoning_summary_part.done","item_id":item_id,"output_index":output_index,"summary_index":0,"part":{"type":"summary_text","text":thinking}}),
                    ),
                    event(
                        "response.output_item.done",
                        json!({"type":"response.output_item.done","output_index":output_index,"item":item}),
                    ),
                ];
                (output_index, item, events)
            }
        };
        self.output.insert(output_index, item);
        Ok(events)
    }

    fn message_delta(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let delta = object(data.get("delta"), "message_delta.delta")?;
        let stop_reason = string(delta.get("stop_reason"), "message_delta.delta.stop_reason")?;
        if !matches!(
            stop_reason,
            "end_turn"
                | "tool_use"
                | "stop_sequence"
                | "max_tokens"
                | "model_context_window_exceeded"
        ) {
            return Err(invalid(format!(
                "unsupported Anthropic stop_reason: {stop_reason}"
            )));
        }
        if self.stop_reason.replace(stop_reason.to_owned()).is_some() {
            return Err(invalid("message_delta stop_reason must occur exactly once"));
        }
        let usage = object(data.get("usage"), "message_delta.usage")?;
        self.output_tokens = number(
            usage.get("output_tokens"),
            "message_delta.usage.output_tokens",
        )?;
        self.input_tokens
            .checked_add(self.output_tokens)
            .ok_or_else(|| invalid("message_delta usage token count overflowed"))?;
        Ok(Vec::new())
    }

    fn message_stop(&mut self) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        if !self.open_blocks.is_empty() || self.output.is_empty() || self.stop_reason.is_none() {
            return Err(invalid(
                "message_stop arrived before content and usage completed",
            ));
        }
        let status = if matches!(
            self.stop_reason.as_deref(),
            Some("max_tokens" | "model_context_window_exceeded")
        ) {
            "incomplete"
        } else {
            "completed"
        };
        let output = Value::Array(self.output.values().cloned().collect());
        let response = self.response(status, output);
        self.terminated = true;
        Ok(vec![event(
            "response.completed",
            json!({"type":"response.completed","response":response}),
        )])
    }

    fn response(&self, status: &str, output: Value) -> Value {
        let mut response = json!({
            "id":self.response_id,"object":"response","created_at":0,"status":status,
            "model":self.model,"output":output,"error":null,
            "incomplete_details":null,
            "usage":{"input_tokens":self.input_tokens,"output_tokens":self.output_tokens,"total_tokens":self.input_tokens + self.output_tokens}
        });
        if status == "incomplete" {
            response["incomplete_details"] = json!({"reason":"max_output_tokens"});
        }
        response
    }

    fn require_started(&self) -> Result<(), BridgeError> {
        if self.started {
            Ok(())
        } else {
            Err(invalid("message_start must be the first event"))
        }
    }

    fn upstream_error(&mut self, data: &Map<String, Value>) -> Vec<Bytes> {
        let error = data.get("error").and_then(Value::as_object);
        let kind = error
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str)
            .map(safe_kind)
            .unwrap_or_else(|| "upstream_error".into());
        let message = error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .map(safe_message)
            .unwrap_or_else(|| "Anthropic upstream reported a stream error".into());
        self.terminated = true;
        vec![event(
            "error",
            json!({"type":"error","error":{"type":kind,"message":message}}),
        )]
    }
}

fn event(name: &str, data: Value) -> Bytes {
    Bytes::from(format!("event: {name}\ndata: {data}\n\n"))
}

fn object<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<&'a Map<String, Value>, BridgeError> {
    value
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(format!("{field} must be an object")))
}

fn string<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(format!("{field} must be a non-empty string")))
}

fn number(value: Option<&Value>, field: &str) -> Result<u64, BridgeError> {
    value
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid(format!("{field} must be an unsigned integer")))
}

fn invalid(message: impl Into<String>) -> BridgeError {
    BridgeError::InvalidCodexResponse(message.into())
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
