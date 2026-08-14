// Minimal Responses SSE adapter derived from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::sse::{append_utf8, parse_sse_block, take_sse_block};
use super::{BridgeError, OpenAiResponsesCapabilities, convert_usage, reasoning};
use async_stream::stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::fmt::Display;

pub fn responses_sse_to_anthropic<S, E>(
    upstream: S,
) -> impl Stream<Item = Result<Bytes, BridgeError>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Display + Send + 'static,
{
    responses_sse_to_anthropic_with_capabilities(upstream, OpenAiResponsesCapabilities::default())
}

pub fn responses_sse_to_anthropic_with_capabilities<S, E>(
    upstream: S,
    _capabilities: OpenAiResponsesCapabilities,
) -> impl Stream<Item = Result<Bytes, BridgeError>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Display + Send + 'static,
{
    stream! {
        futures::pin_mut!(upstream);
        let mut state = State::new();
        let mut buffer = String::new();
        let mut utf8_remainder = Vec::new();

        while let Some(chunk) = upstream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(_) => {
                    yield Ok(error_sse("stream_transport_error", "Responses stream transport failed"));
                    return;
                }
            };
            if let Err(error) = append_utf8(&mut buffer, &mut utf8_remainder, &chunk) {
                yield Ok(bridge_error_sse(error));
                return;
            }
            while let Some(block) = take_sse_block(&mut buffer) {
                let (event, data) = match parse_sse_block(&block) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        yield Ok(bridge_error_sse(error));
                        return;
                    }
                };
                let value: Value = match serde_json::from_str(data) {
                    Ok(value) => value,
                    Err(_) => {
                        yield Ok(error_sse("stream_protocol_error", "Responses SSE data must be valid JSON"));
                        return;
                    }
                };
                if value.get("type").and_then(Value::as_str) != Some(event) {
                    yield Ok(error_sse("stream_protocol_error", "Responses SSE event and data type must match"));
                    return;
                }
                match state.handle(event, &value) {
                    Ok(events) => {
                        for event in events {
                            yield Ok(event);
                        }
                    }
                    Err(error) => {
                        yield Ok(bridge_error_sse(error));
                        return;
                    }
                }
                if state.terminated {
                    return;
                }
            }
        }

        if !utf8_remainder.is_empty() {
            yield Ok(error_sse("stream_protocol_error", "Responses SSE ended inside a UTF-8 character"));
        } else if !buffer.is_empty() {
            yield Ok(error_sse("stream_protocol_error", "Responses SSE ended inside an event"));
        } else if !state.terminated {
            let message = if state.tools.values().any(|tool| tool.open) {
                "Responses SSE ended before tool arguments completed"
            } else {
                "Responses SSE ended before a terminal event"
            };
            yield Ok(error_sse("stream_protocol_error", message));
        }
    }
}

struct State {
    started: bool,
    terminated: bool,
    next_index: u64,
    texts: HashMap<(u64, u64), Block>,
    tools: HashMap<String, Tool>,
    reasoning: HashMap<String, Reasoning>,
    has_tool: bool,
}

#[derive(Clone, Copy)]
struct Block {
    index: u64,
    open: bool,
}

struct Tool {
    index: u64,
    arguments: String,
    had_delta: bool,
    open: bool,
}

struct Reasoning {
    index: u64,
    summary: String,
    content: String,
    content_done: bool,
    current_part: String,
    next_summary_index: u64,
    part_open: bool,
    text_done: bool,
    block_open: bool,
}

impl State {
    fn new() -> Self {
        Self {
            started: false,
            terminated: false,
            next_index: 0,
            texts: HashMap::new(),
            tools: HashMap::new(),
            reasoning: HashMap::new(),
            has_tool: false,
        }
    }

    fn handle(&mut self, event: &str, data: &Value) -> Result<Vec<Bytes>, BridgeError> {
        let object = data
            .as_object()
            .ok_or_else(|| invalid("SSE data must be an object"))?;
        match event {
            "response.created" => self.created(object),
            "response.in_progress" => self.require_started().map(|()| Vec::new()),
            "response.content_part.added" => self.content_part_added(object),
            "response.output_text.delta" => self.text_delta(object),
            "response.content_part.done" => self.content_part_done(object),
            "response.output_text.done" => self.text_done(object),
            "response.output_item.added" => self.output_item_added(object),
            "response.reasoning_summary_part.added" => self.reasoning_part_added(object),
            "response.reasoning_summary_text.delta" => self.reasoning_text_delta(object),
            "response.reasoning_summary_text.done" => self.reasoning_text_done(object),
            "response.reasoning_summary_part.done" => self.reasoning_part_done(object),
            "response.reasoning_text.delta" => self.reasoning_content_delta(object),
            "response.reasoning_text.done" => self.reasoning_content_done(object),
            "response.function_call_arguments.delta" => self.tool_delta(object),
            "response.function_call_arguments.done" | "response.output_item.done" => {
                self.tool_done(event, object)
            }
            "response.completed" => self.completed(object),
            "response.failed" | "error" => Ok(self.failed(object)),
            _ => Err(invalid(&format!(
                "unsupported Responses SSE event: {event}"
            ))),
        }
    }

    fn created(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        if self.started {
            return Err(invalid("response.created must occur exactly once"));
        }
        let response = object(data.get("response"), "response.created.response")?;
        let id = string(response.get("id"), "response.created.response.id")?;
        let model = string(response.get("model"), "response.created.response.model")?;
        self.started = true;
        Ok(vec![anthropic_sse(
            "message_start",
            json!({
                "type":"message_start",
                "message":{"id":id,"type":"message","role":"assistant","model":model,
                    "content":[],"stop_reason":null,"stop_sequence":null,
                    "usage":{"input_tokens":0,"output_tokens":0}}
            }),
        )])
    }

    fn text_added(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let part = object(data.get("part"), "response.content_part.added.part")?;
        if string(part.get("type"), "response.content_part.added.part.type")? != "output_text" {
            return Err(invalid(
                "response.content_part.added.part.type must be output_text",
            ));
        }
        let key = indices(data)?;
        if self.texts.contains_key(&key) {
            return Err(invalid("duplicate response.content_part.added"));
        }
        let index = self.take_index();
        self.texts.insert(key, Block { index, open: true });
        Ok(vec![anthropic_sse(
            "content_block_start",
            json!({
                "type":"content_block_start","index":index,
                "content_block":{"type":"text","text":""}
            }),
        )])
    }

    fn content_part_added(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        let part = object(data.get("part"), "response.content_part.added.part")?;
        match string(part.get("type"), "response.content_part.added.part.type")? {
            "output_text" => self.text_added(data),
            "reasoning_text" => {
                self.require_started()?;
                if part.get("text").and_then(Value::as_str) != Some("") {
                    return Err(invalid(
                        "response.content_part.added reasoning_text must be empty",
                    ));
                }
                let item_id = string(data.get("item_id"), "response.content_part.added.item_id")?;
                let state = self.reasoning.get(item_id).ok_or_else(|| {
                    invalid("reasoning content part has no matching reasoning item")
                })?;
                if state.content_done {
                    return Err(invalid("reasoning content part arrived after done"));
                }
                Ok(Vec::new())
            }
            _ => Err(invalid(
                "response.content_part.added.part.type must be output_text or reasoning_text",
            )),
        }
    }

    fn text_delta(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let key = indices(data)?;
        let block = self
            .texts
            .get(&key)
            .filter(|block| block.open)
            .ok_or_else(|| invalid("response.output_text.delta has no open text block"))?;
        let delta = string(data.get("delta"), "response.output_text.delta.delta")?;
        Ok(vec![anthropic_sse(
            "content_block_delta",
            json!({
                "type":"content_block_delta","index":block.index,
                "delta":{"type":"text_delta","text":delta}
            }),
        )])
    }

    fn text_done(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let key = indices(data)?;
        let block = self
            .texts
            .get_mut(&key)
            .ok_or_else(|| invalid("text done event has no matching text block"))?;
        if !block.open {
            return Ok(Vec::new());
        }
        block.open = false;
        Ok(vec![anthropic_sse(
            "content_block_stop",
            json!({
                "type":"content_block_stop","index":block.index
            }),
        )])
    }

    fn content_part_done(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        let part = object(data.get("part"), "response.content_part.done.part")?;
        match string(part.get("type"), "response.content_part.done.part.type")? {
            "output_text" => self.text_done(data),
            "reasoning_text" => {
                self.require_started()?;
                let item_id = string(data.get("item_id"), "response.content_part.done.item_id")?;
                let text = string(part.get("text"), "response.content_part.done.part.text")?;
                let state = self.reasoning.get_mut(item_id).ok_or_else(|| {
                    invalid("reasoning content part done has no matching reasoning item")
                })?;
                if !state.content.is_empty() && state.content != text {
                    return Err(invalid(
                        "reasoning content part done must match streamed deltas",
                    ));
                }
                if state.content.is_empty() {
                    state.content.push_str(text);
                }
                state.content_done = true;
                Ok(Vec::new())
            }
            _ => Err(invalid(
                "response.content_part.done.part.type must be output_text or reasoning_text",
            )),
        }
    }

    fn output_item_added(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let item = object(data.get("item"), "response.output_item.added.item")?;
        match string(item.get("type"), "response.output_item.added.item.type")? {
            "message" => return Ok(Vec::new()),
            "function_call" => {}
            "reasoning" => return self.reasoning_added(item),
            _ => {
                return Err(invalid(
                    "response.output_item.added.item.type must be message, function_call, or reasoning",
                ));
            }
        }
        let item_id = string(item.get("id"), "response.output_item.added.item.id")?;
        let call_id = string(
            item.get("call_id"),
            "response.output_item.added.item.call_id",
        )?;
        let name = string(item.get("name"), "response.output_item.added.item.name")?;
        if self.tools.contains_key(item_id) {
            return Err(invalid("duplicate function_call item id"));
        }
        let index = self.take_index();
        self.tools.insert(
            item_id.to_owned(),
            Tool {
                index,
                arguments: String::new(),
                had_delta: false,
                open: true,
            },
        );
        self.has_tool = true;
        Ok(vec![anthropic_sse(
            "content_block_start",
            json!({
                "type":"content_block_start","index":index,
                "content_block":{"type":"tool_use","id":call_id,"name":name,"input":{}}
            }),
        )])
    }

    fn tool_delta(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let item_id = string(
            data.get("item_id"),
            "response.function_call_arguments.delta.item_id",
        )?;
        let delta = string(
            data.get("delta"),
            "response.function_call_arguments.delta.delta",
        )?;
        let tool = self
            .tools
            .get_mut(item_id)
            .filter(|tool| tool.open)
            .ok_or_else(|| invalid("tool arguments delta has no matching open function_call"))?;
        tool.arguments.push_str(delta);
        tool.had_delta = true;
        Ok(vec![anthropic_sse(
            "content_block_delta",
            json!({
                "type":"content_block_delta","index":tool.index,
                "delta":{"type":"input_json_delta","partial_json":delta}
            }),
        )])
    }

    fn reasoning_added(&mut self, item: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        let normalized = reasoning::normalize_reasoning_item(
            &Value::Object(item.clone()),
            "response.output_item.added.item",
        )
        .map_err(|message| invalid(&message))?;
        let item_id = string(normalized.get("id"), "response.output_item.added.item.id")?;
        if self.reasoning.contains_key(item_id) {
            return Err(invalid("duplicate reasoning item id"));
        }
        let index = self.take_index();
        self.reasoning.insert(
            item_id.to_owned(),
            Reasoning {
                index,
                summary: String::new(),
                content: String::new(),
                content_done: false,
                current_part: String::new(),
                next_summary_index: 0,
                part_open: false,
                text_done: false,
                block_open: false,
            },
        );
        Ok(Vec::new())
    }

    fn reasoning_content_delta(
        &mut self,
        data: &Map<String, Value>,
    ) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let item_id = string(data.get("item_id"), "response.reasoning_text.delta.item_id")?;
        let delta = string(data.get("delta"), "response.reasoning_text.delta.delta")?;
        let state = self
            .reasoning
            .get_mut(item_id)
            .ok_or_else(|| invalid("reasoning text delta has no matching reasoning item"))?;
        if state.content_done {
            return Err(invalid("reasoning text delta arrived after done"));
        }
        state.content.push_str(delta);
        Ok(Vec::new())
    }

    fn reasoning_content_done(
        &mut self,
        data: &Map<String, Value>,
    ) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let item_id = string(data.get("item_id"), "response.reasoning_text.done.item_id")?;
        let text = string(data.get("text"), "response.reasoning_text.done.text")?;
        let state = self
            .reasoning
            .get_mut(item_id)
            .ok_or_else(|| invalid("reasoning text done has no matching reasoning item"))?;
        if state.content_done || (!state.content.is_empty() && state.content != text) {
            return Err(invalid(
                "reasoning text done must match its streamed deltas",
            ));
        }
        if state.content.is_empty() {
            state.content.push_str(text);
        }
        state.content_done = true;
        Ok(Vec::new())
    }

    fn reasoning_part_added(
        &mut self,
        data: &Map<String, Value>,
    ) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let item_id = string(
            data.get("item_id"),
            "response.reasoning_summary_part.added.item_id",
        )?;
        let summary_index = number(
            data.get("summary_index"),
            "response.reasoning_summary_part.added.summary_index",
        )?;
        let part = object(
            data.get("part"),
            "response.reasoning_summary_part.added.part",
        )?;
        if string(
            part.get("type"),
            "response.reasoning_summary_part.added.part.type",
        )? != "summary_text"
        {
            return Err(invalid(
                "response.reasoning_summary_part.added.part.type must be summary_text",
            ));
        }
        if part.get("text").and_then(Value::as_str) != Some("") {
            return Err(invalid(
                "response.reasoning_summary_part.added.part.text must be empty",
            ));
        }
        let state = self
            .reasoning
            .get_mut(item_id)
            .ok_or_else(|| invalid("reasoning summary part has no matching reasoning item"))?;
        if state.part_open || summary_index != state.next_summary_index {
            return Err(invalid(
                "reasoning summary parts must be non-overlapping and ordered",
            ));
        }
        state.part_open = true;
        state.text_done = false;
        state.current_part.clear();
        Ok(Vec::new())
    }

    fn reasoning_text_delta(
        &mut self,
        data: &Map<String, Value>,
    ) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let item_id = string(
            data.get("item_id"),
            "response.reasoning_summary_text.delta.item_id",
        )?;
        let summary_index = number(
            data.get("summary_index"),
            "response.reasoning_summary_text.delta.summary_index",
        )?;
        let delta = string(
            data.get("delta"),
            "response.reasoning_summary_text.delta.delta",
        )?;
        let state = self
            .reasoning
            .get_mut(item_id)
            .ok_or_else(|| invalid("reasoning summary delta has no matching reasoning item"))?;
        if !state.part_open || state.text_done || summary_index != state.next_summary_index {
            return Err(invalid(
                "reasoning summary delta has no matching open summary part",
            ));
        }
        state.current_part.push_str(delta);
        state.summary.push_str(delta);
        let mut events = Vec::new();
        if !state.block_open {
            state.block_open = true;
            events.push(thinking_start(state.index));
        }
        events.push(anthropic_sse(
            "content_block_delta",
            json!({
                "type":"content_block_delta","index":state.index,
                "delta":{"type":"thinking_delta","thinking":delta}
            }),
        ));
        Ok(events)
    }

    fn reasoning_text_done(
        &mut self,
        data: &Map<String, Value>,
    ) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let item_id = string(
            data.get("item_id"),
            "response.reasoning_summary_text.done.item_id",
        )?;
        let summary_index = number(
            data.get("summary_index"),
            "response.reasoning_summary_text.done.summary_index",
        )?;
        let text = string(
            data.get("text"),
            "response.reasoning_summary_text.done.text",
        )?;
        let state = self
            .reasoning
            .get_mut(item_id)
            .ok_or_else(|| invalid("reasoning summary done has no matching reasoning item"))?;
        if !state.part_open || state.text_done || summary_index != state.next_summary_index {
            return Err(invalid(
                "reasoning summary done has no matching open summary part",
            ));
        }
        if text != state.current_part {
            return Err(invalid(
                "reasoning summary done text must match streamed deltas",
            ));
        }
        state.text_done = true;
        Ok(Vec::new())
    }

    fn reasoning_part_done(
        &mut self,
        data: &Map<String, Value>,
    ) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let item_id = string(
            data.get("item_id"),
            "response.reasoning_summary_part.done.item_id",
        )?;
        let summary_index = number(
            data.get("summary_index"),
            "response.reasoning_summary_part.done.summary_index",
        )?;
        let part = object(
            data.get("part"),
            "response.reasoning_summary_part.done.part",
        )?;
        if string(
            part.get("type"),
            "response.reasoning_summary_part.done.part.type",
        )? != "summary_text"
        {
            return Err(invalid(
                "response.reasoning_summary_part.done.part.type must be summary_text",
            ));
        }
        let text = string(
            part.get("text"),
            "response.reasoning_summary_part.done.part.text",
        )?;
        let state = self
            .reasoning
            .get_mut(item_id)
            .ok_or_else(|| invalid("reasoning summary part done has no matching reasoning item"))?;
        if !state.part_open
            || !state.text_done
            || summary_index != state.next_summary_index
            || text != state.current_part
        {
            return Err(invalid(
                "reasoning summary part done must match its completed text",
            ));
        }
        state.part_open = false;
        state.next_summary_index += 1;
        Ok(Vec::new())
    }

    fn tool_done(
        &mut self,
        event: &str,
        data: &Map<String, Value>,
    ) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let item = data.get("item").and_then(Value::as_object);
        if event == "response.output_item.done" {
            let item =
                item.ok_or_else(|| invalid("response.output_item.done.item must be an object"))?;
            match string(item.get("type"), "response.output_item.done.item.type")? {
                "message" => return Ok(Vec::new()),
                "function_call" => {}
                "reasoning" => return self.reasoning_done(item),
                _ => {
                    return Err(invalid(
                        "response.output_item.done.item.type must be message, function_call, or reasoning",
                    ));
                }
            }
        }
        let item_id = data
            .get("item_id")
            .and_then(Value::as_str)
            .or_else(|| item.and_then(|item| item.get("id")).and_then(Value::as_str))
            .ok_or_else(|| invalid("tool done event.item_id must be a string"))?;
        let tool = self
            .tools
            .get_mut(item_id)
            .ok_or_else(|| invalid("tool done event has no matching function_call"))?;
        if !tool.open {
            return Ok(Vec::new());
        }
        let fallback = data.get("arguments").and_then(Value::as_str).or_else(|| {
            item.and_then(|item| item.get("arguments"))
                .and_then(Value::as_str)
        });
        let mut events = Vec::new();
        if !tool.had_delta {
            let arguments =
                fallback.ok_or_else(|| invalid("tool done event is missing arguments"))?;
            tool.arguments.push_str(arguments);
            events.push(anthropic_sse(
                "content_block_delta",
                json!({
                    "type":"content_block_delta","index":tool.index,
                    "delta":{"type":"input_json_delta","partial_json":arguments}
                }),
            ));
        }
        let parsed: Value = serde_json::from_str(&tool.arguments)
            .map_err(|_| invalid("completed tool arguments must be valid JSON"))?;
        if !parsed.is_object() {
            return Err(invalid("completed tool arguments must be a JSON object"));
        }
        tool.open = false;
        events.push(anthropic_sse(
            "content_block_stop",
            json!({
                "type":"content_block_stop","index":tool.index
            }),
        ));
        Ok(events)
    }

    fn reasoning_done(&mut self, item: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        let normalized = reasoning::normalize_reasoning_item(
            &Value::Object(item.clone()),
            "response.output_item.done.item",
        )
        .map_err(|message| invalid(&message))?;
        let item_id = string(normalized.get("id"), "response.output_item.done.item.id")?;
        let state = self
            .reasoning
            .remove(item_id)
            .ok_or_else(|| invalid("reasoning item done has no matching reasoning item"))?;
        if state.part_open {
            return Err(invalid(
                "reasoning item done arrived before its summary part completed",
            ));
        }
        let final_summary = reasoning::summary_text(&normalized);
        if !state.summary.is_empty() && final_summary != state.summary {
            return Err(invalid(
                "completed reasoning summary must match streamed deltas",
            ));
        }
        let final_content = reasoning::content_text(&normalized);
        if !state.content.is_empty() && final_content != state.content {
            return Err(invalid(
                "completed reasoning content must match streamed deltas",
            ));
        }

        let mut events = Vec::new();
        let mut block_open = state.block_open;
        if !final_summary.is_empty() && !block_open {
            block_open = true;
            events.push(thinking_start(state.index));
            events.push(anthropic_sse(
                "content_block_delta",
                json!({
                    "type":"content_block_delta","index":state.index,
                    "delta":{"type":"thinking_delta","thinking":final_summary}
                }),
            ));
        }
        let has_opaque = normalized
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_some()
            || reasoning::has_content(&normalized);
        if has_opaque {
            let signature = reasoning::encode(&normalized).map_err(|message| invalid(&message))?;
            if block_open {
                events.push(anthropic_sse(
                    "content_block_delta",
                    json!({
                        "type":"content_block_delta","index":state.index,
                        "delta":{"type":"signature_delta","signature":signature}
                    }),
                ));
            } else {
                block_open = true;
                events.push(anthropic_sse(
                    "content_block_start",
                    json!({
                        "type":"content_block_start","index":state.index,
                        "content_block":{"type":"redacted_thinking","data":signature}
                    }),
                ));
            }
        }
        if !block_open {
            return Err(invalid(
                "reasoning item has neither summary nor opaque content",
            ));
        }
        events.push(anthropic_sse(
            "content_block_stop",
            json!({"type":"content_block_stop","index":state.index}),
        ));
        Ok(events)
    }

    fn completed(&mut self, data: &Map<String, Value>) -> Result<Vec<Bytes>, BridgeError> {
        self.require_started()?;
        let response = object(data.get("response"), "response.completed.response")?;
        if string(response.get("status"), "response.completed.response.status")? != "completed" {
            return Err(invalid(
                "response.completed.response.status must be completed",
            ));
        }
        if self.tools.values().any(|tool| tool.open) {
            return Err(invalid(
                "response.completed arrived before tool arguments completed",
            ));
        }
        if !self.reasoning.is_empty() {
            return Err(invalid(
                "response.completed arrived before reasoning items completed",
            ));
        }
        let mut events = Vec::new();
        for block in self.texts.values_mut().filter(|block| block.open) {
            block.open = false;
            events.push(anthropic_sse(
                "content_block_stop",
                json!({
                    "type":"content_block_stop","index":block.index
                }),
            ));
        }
        let usage = object(response.get("usage"), "response.completed.response.usage")?;
        let usage = convert_usage(usage)?;
        events.push(anthropic_sse("message_delta", json!({
            "type":"message_delta",
            "delta":{"stop_reason":if self.has_tool {"tool_use"} else {"end_turn"},"stop_sequence":null},
            "usage":usage
        })));
        events.push(anthropic_sse(
            "message_stop",
            json!({"type":"message_stop"}),
        ));
        self.terminated = true;
        Ok(events)
    }

    fn failed(&mut self, data: &Map<String, Value>) -> Vec<Bytes> {
        let error = data.get("error").and_then(Value::as_object).or_else(|| {
            data.get("response")
                .and_then(Value::as_object)
                .and_then(|response| response.get("error"))
                .and_then(Value::as_object)
        });
        let kind = error
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str)
            .map(safe_kind)
            .unwrap_or_else(|| "upstream_error".into());
        let message = error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .map(safe_message)
            .unwrap_or_else(|| "Responses upstream reported a failure".into());
        self.terminated = true;
        vec![error_sse(&kind, &message)]
    }

    fn require_started(&self) -> Result<(), BridgeError> {
        if self.started {
            Ok(())
        } else {
            Err(invalid("response.created must be the first event"))
        }
    }

    fn take_index(&mut self) -> u64 {
        let index = self.next_index;
        self.next_index += 1;
        index
    }
}

fn indices(data: &Map<String, Value>) -> Result<(u64, u64), BridgeError> {
    Ok((
        number(data.get("output_index"), "event.output_index")?,
        number(data.get("content_index"), "event.content_index")?,
    ))
}

fn thinking_start(index: u64) -> Bytes {
    anthropic_sse(
        "content_block_start",
        json!({
            "type":"content_block_start","index":index,
            "content_block":{"type":"thinking","thinking":""}
        }),
    )
}

fn object<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<&'a Map<String, Value>, BridgeError> {
    value
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(&format!("{field} must be an object")))
}

fn string<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(&format!("{field} must be a non-empty string")))
}

fn number(value: Option<&Value>, field: &str) -> Result<u64, BridgeError> {
    value
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid(&format!("{field} must be an unsigned integer")))
}

fn anthropic_sse(event: &str, data: Value) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
}

fn bridge_error_sse(error: BridgeError) -> Bytes {
    error_sse("stream_protocol_error", &error.to_string())
}

fn error_sse(kind: &str, message: &str) -> Bytes {
    anthropic_sse(
        "error",
        json!({"type":"error","error":{"type":kind,"message":message}}),
    )
}

fn invalid(message: &str) -> BridgeError {
    BridgeError::InvalidResponse(message.into())
}

fn safe_kind(value: &str) -> String {
    let result: String = value
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
    if result.is_empty() {
        "upstream_error".into()
    } else {
        result
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
