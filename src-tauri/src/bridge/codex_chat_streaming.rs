// Minimal Chat Completions SSE → Codex Responses SSE bridge adapted from
// cc-switch, commit 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::{AnthropicSseStream, BridgeError, sse};
use async_stream::stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
struct ToolState {
    output_index: u64,
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    added: bool,
}

#[derive(Default)]
struct State {
    response_id: String,
    model: String,
    created_at: u64,
    started: bool,
    text_added: bool,
    text: String,
    next_output_index: u64,
    tools: BTreeMap<usize, ToolState>,
    usage: Value,
    completed: bool,
}

pub fn chat_sse_to_codex_responses<S, E>(source: S) -> AnthropicSseStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    Box::pin(stream! {
        let mut source = Box::pin(source);
        let mut buffer = String::new();
        let mut remainder = Vec::new();
        let mut state = State::default();
        while let Some(chunk) = source.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    yield Err(BridgeError::InvalidCodexResponse(format!("Chat SSE transport failed: {error}")));
                    return;
                }
            };
            if let Err(error) = sse::append_utf8(&mut buffer, &mut remainder, &chunk) {
                yield Err(error);
                return;
            }
            while let Some(block) = sse::take_sse_block(&mut buffer) {
                let data = match sse::parse_data_sse_block(&block) {
                    Ok(data) => data,
                    Err(error) => {
                        yield Err(BridgeError::InvalidCodexResponse(error));
                        return;
                    }
                };
                if data == "[DONE]" {
                    if !state.completed {
                        yield Err(BridgeError::InvalidCodexResponse("Chat SSE ended before a finish_reason".into()));
                    }
                    return;
                }
                let chunk: Value = match serde_json::from_str(data) {
                    Ok(chunk) => chunk,
                    Err(_) => {
                        yield Err(BridgeError::InvalidCodexResponse("Chat SSE data must be valid JSON".into()));
                        return;
                    }
                };
                for event in state.consume(&chunk) {
                    yield Ok(event);
                }
                if state.completed {
                    return;
                }
            }
        }
        if !remainder.is_empty() || !buffer.trim().is_empty() {
            yield Err(BridgeError::InvalidCodexResponse("Chat SSE ended with an incomplete event".into()));
        } else if !state.completed {
            yield Err(BridgeError::InvalidCodexResponse("Chat SSE ended before a finish_reason".into()));
        }
    })
}

impl State {
    fn consume(&mut self, chunk: &Value) -> Vec<Bytes> {
        if let Some(id) = chunk.get("id").and_then(Value::as_str) {
            self.response_id = id.replacen("chatcmpl", "resp", 1);
        }
        if self.response_id.is_empty() {
            self.response_id = "resp_grillforge".into();
        }
        if let Some(model) = chunk.get("model").and_then(Value::as_str) {
            self.model = model.to_string();
        }
        self.created_at = chunk
            .get("created")
            .and_then(Value::as_u64)
            .unwrap_or(self.created_at);
        if let Some(usage) = chunk.get("usage").and_then(Value::as_object) {
            let input = usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let output = usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            self.usage =
                json!({"input_tokens":input,"output_tokens":output,"total_tokens":input + output});
        }
        let mut events = self.start_events();
        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
        else {
            return events;
        };
        if let Some(delta) = choice.get("delta") {
            if let Some(content) = delta
                .get("content")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                events.extend(self.text_delta(content));
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    events.extend(self.tool_delta(call));
                }
            }
        }
        if choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .is_some()
        {
            events.extend(self.finish());
        }
        events
    }

    fn start_events(&mut self) -> Vec<Bytes> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        let response = self.response("in_progress", Vec::new());
        vec![
            event(
                "response.created",
                json!({"type":"response.created","response":response}),
            ),
            event(
                "response.in_progress",
                json!({"type":"response.in_progress","response":response}),
            ),
        ]
    }

    fn text_delta(&mut self, delta: &str) -> Vec<Bytes> {
        let index = 0;
        let item_id = format!("msg_{}", self.response_id);
        let mut events = Vec::new();
        if !self.text_added {
            self.text_added = true;
            self.next_output_index = 1;
            events.push(event("response.output_item.added", json!({"type":"response.output_item.added","output_index":index,"item":{"id":item_id,"type":"message","role":"assistant","status":"in_progress","content":[]}})));
            events.push(event("response.content_part.added", json!({"type":"response.content_part.added","item_id":item_id,"output_index":index,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}})));
        }
        self.text.push_str(delta);
        events.push(event("response.output_text.delta", json!({"type":"response.output_text.delta","item_id":item_id,"output_index":index,"content_index":0,"delta":delta})));
        events
    }

    fn tool_delta(&mut self, call: &Value) -> Vec<Bytes> {
        let key = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let tool = self.tools.entry(key).or_insert_with(|| {
            let index = self.next_output_index;
            self.next_output_index += 1;
            ToolState {
                output_index: index,
                item_id: format!("fc_{}_{}", self.response_id, key),
                ..ToolState::default()
            }
        });
        if let Some(id) = call.get("id").and_then(Value::as_str) {
            tool.call_id.push_str(id);
        }
        if let Some(function) = call.get("function") {
            if let Some(name) = function.get("name").and_then(Value::as_str) {
                tool.name.push_str(name);
            }
            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                tool.arguments.push_str(arguments);
            }
        }
        let mut events = Vec::new();
        if !tool.added && !tool.name.is_empty() {
            tool.added = true;
            events.push(event("response.output_item.added", json!({"type":"response.output_item.added","output_index":tool.output_index,"item":{"id":tool.item_id,"type":"function_call","status":"in_progress","call_id":tool.call_id,"name":tool.name,"arguments":""}})));
        }
        if let Some(arguments) = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            events.push(event("response.function_call_arguments.delta", json!({"type":"response.function_call_arguments.delta","item_id":tool.item_id,"output_index":tool.output_index,"delta":arguments})));
        }
        events
    }

    fn finish(&mut self) -> Vec<Bytes> {
        let mut events = Vec::new();
        let mut output = Vec::new();
        if self.text_added {
            let item_id = format!("msg_{}", self.response_id);
            events.push(event("response.output_text.done", json!({"type":"response.output_text.done","item_id":item_id,"output_index":0,"content_index":0,"text":self.text})));
            events.push(event("response.content_part.done", json!({"type":"response.content_part.done","item_id":item_id,"output_index":0,"content_index":0,"part":{"type":"output_text","text":self.text,"annotations":[]}})));
            let item = json!({"id":item_id,"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":self.text,"annotations":[]}]});
            events.push(event(
                "response.output_item.done",
                json!({"type":"response.output_item.done","output_index":0,"item":item}),
            ));
            output.push(item);
        }
        for tool in self.tools.values() {
            if !tool.added {
                continue;
            }
            events.push(event("response.function_call_arguments.done", json!({"type":"response.function_call_arguments.done","item_id":tool.item_id,"output_index":tool.output_index,"arguments":tool.arguments})));
            let item = json!({"id":tool.item_id,"type":"function_call","status":"completed","call_id":tool.call_id,"name":tool.name,"arguments":tool.arguments});
            events.push(event("response.output_item.done", json!({"type":"response.output_item.done","output_index":tool.output_index,"item":item})));
            output.push(item);
        }
        self.completed = true;
        let response = self.response("completed", output);
        events.push(event(
            "response.completed",
            json!({"type":"response.completed","response":response}),
        ));
        events
    }

    fn response(&self, status: &str, output: Vec<Value>) -> Value {
        json!({"id":self.response_id,"object":"response","created_at":self.created_at,"status":status,"model":self.model,"output":output,"error":null,"incomplete_details":null,"usage":self.usage})
    }
}

fn event(name: &str, payload: Value) -> Bytes {
    Bytes::from(format!("event: {name}\ndata: {payload}\n\n"))
}
