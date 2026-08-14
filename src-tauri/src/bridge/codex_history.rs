// Adapted from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::{AnthropicSseStream, BridgeError, sse};
use async_stream::stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

const MAX_RESPONSES: usize = 512;

#[derive(Clone, Default)]
struct CachedResponse {
    calls: HashMap<String, Value>,
    order: Vec<String>,
}

#[derive(Default)]
struct Inner {
    responses: HashMap<String, CachedResponse>,
    order: VecDeque<String>,
    call_index: HashMap<String, VecDeque<String>>,
}

/// Restores Responses tool-call history before a request is bridged to a
/// stateless protocol. Codex can send only `previous_response_id` plus tool
/// outputs; Chat, Anthropic and Gemini require the original assistant call.
#[derive(Default)]
pub struct CodexHistoryStore {
    inner: RwLock<Inner>,
}

impl CodexHistoryStore {
    pub async fn record_response(&self, response: &Value) -> usize {
        let Some(response_id) = response
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return 0;
        };
        let calls = response
            .get("output")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(cached_call)
            .collect();
        self.insert(response_id, calls).await
    }

    pub async fn enrich_request(&self, request: &mut Value) -> usize {
        let previous_id = request
            .get("previous_response_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let Some(input) = request.get_mut("input") else {
            return 0;
        };
        let original = std::mem::take(input);
        let was_object = original.is_object();
        let items = match original {
            Value::Array(items) => items,
            Value::Object(object) => vec![Value::Object(object)],
            other => {
                *input = other;
                return 0;
            }
        };
        let outputs = items
            .iter()
            .filter(|item| is_output(item))
            .filter_map(call_id)
            .collect::<HashSet<_>>();
        let existing = items
            .iter()
            .filter(|item| is_call(item))
            .filter_map(call_id)
            .collect::<HashSet<_>>();
        let cached = self.lookup(previous_id.as_deref(), &outputs).await;
        let mut restored = 0;
        let mut new_items = Vec::with_capacity(items.len() + outputs.len());
        for item in items {
            if is_output(&item) {
                if let Some(id) = call_id(&item) {
                    if !existing.contains(&id) {
                        if let Some(call) = cached.get(&id) {
                            new_items.push(call.clone());
                            restored += 1;
                        }
                    }
                }
            }
            new_items.push(item);
        }
        *input = if restored == 0 && was_object && new_items.len() == 1 {
            new_items.pop().unwrap_or(Value::Null)
        } else {
            Value::Array(new_items)
        };
        restored
    }

    async fn insert(&self, response_id: &str, calls: Vec<(String, Value)>) -> usize {
        if calls.is_empty() {
            return 0;
        }
        let mut inner = self.inner.write().await;
        if !inner.responses.contains_key(response_id) {
            inner.order.push_back(response_id.to_owned());
        }
        let response = inner.responses.entry(response_id.to_owned()).or_default();
        let mut ids = Vec::with_capacity(calls.len());
        for (id, item) in calls {
            if !response.calls.contains_key(&id) {
                response.order.push(id.clone());
            }
            response.calls.insert(id.clone(), item);
            ids.push(id);
        }
        let count = ids.len();
        for id in ids {
            let responses = inner.call_index.entry(id).or_default();
            if !responses.iter().any(|value| value == response_id) {
                responses.push_back(response_id.to_owned());
            }
        }
        while inner.order.len() > MAX_RESPONSES {
            if let Some(expired) = inner.order.pop_front() {
                inner.responses.remove(&expired);
                for ids in inner.call_index.values_mut() {
                    ids.retain(|value| value != &expired);
                }
                inner.call_index.retain(|_, ids| !ids.is_empty());
            }
        }
        count
    }

    async fn lookup(
        &self,
        previous_id: Option<&str>,
        wanted: &HashSet<String>,
    ) -> HashMap<String, Value> {
        let inner = self.inner.read().await;
        let mut found = HashMap::new();
        if let Some(previous) = previous_id.and_then(|id| inner.responses.get(id)) {
            for id in wanted {
                if let Some(item) = previous.calls.get(id) {
                    found.insert(id.clone(), item.clone());
                }
            }
        }
        for id in wanted {
            if found.contains_key(id) {
                continue;
            }
            let Some(response_ids) = inner.call_index.get(id) else {
                continue;
            };
            let mut candidates = response_ids
                .iter()
                .filter_map(|response_id| inner.responses.get(response_id)?.calls.get(id));
            let Some(item) = candidates.next() else {
                continue;
            };
            if candidates.next().is_none() {
                found.insert(id.clone(), item.clone());
            }
        }
        found
    }

    async fn record_item(&self, response_id: Option<&str>, item: &Value) {
        if let (Some(response_id), Some(call)) = (response_id, cached_call(item)) {
            self.insert(response_id, vec![call]).await;
        }
    }
}

pub fn record_codex_sse<S, E>(source: S, history: Arc<CodexHistoryStore>) -> AnthropicSseStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Into<BridgeError> + Send + 'static,
{
    Box::pin(stream! {
        let mut source = Box::pin(source);
        let mut buffer = String::new();
        let mut remainder = Vec::new();
        let mut response_id = None;
        while let Some(chunk) = source.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    yield Err(error.into());
                    return;
                }
            };
            if let Err(error) = sse::append_utf8(&mut buffer, &mut remainder, &chunk) {
                yield Err(error);
                return;
            }
            while let Some(block) = sse::take_sse_block(&mut buffer) {
                inspect_event(&block, &mut response_id, &history).await;
            }
            yield Ok(chunk);
        }
    })
}

async fn inspect_event(block: &str, response_id: &mut Option<String>, history: &CodexHistoryStore) {
    let Ok((_, data)) = sse::parse_sse_block(block) else {
        return;
    };
    let Ok(event) = serde_json::from_str::<Value>(data) else {
        return;
    };
    if let Some(id) = event
        .pointer("/response/id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        *response_id = Some(id.to_owned());
    }
    match event.get("type").and_then(Value::as_str) {
        Some("response.output_item.done") => {
            if let Some(item) = event.get("item") {
                history.record_item(response_id.as_deref(), item).await;
            }
        }
        Some("response.completed") => {
            if let Some(response) = event.get("response") {
                history.record_response(response).await;
            }
        }
        _ => {}
    }
}

fn cached_call(item: &Value) -> Option<(String, Value)> {
    is_call(item).then(|| call_id(item).map(|id| (id, item.clone())))?
}

fn call_id(item: &Value) -> Option<String> {
    item.get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn is_call(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call" | "custom_tool_call" | "tool_search_call")
    )
}

fn is_output(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call_output" | "custom_tool_call_output" | "tool_search_output")
    )
}
