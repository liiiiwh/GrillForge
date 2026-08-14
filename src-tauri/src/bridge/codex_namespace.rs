// Adapted from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::{AnthropicSseStream, BridgeError, sse};
use async_stream::stream;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

const TOOL_NAME_MAX_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
struct NamespacedName {
    namespace: String,
    name: String,
}

#[derive(Debug, Clone, Default)]
pub struct CodexNamespaceMap(HashMap<String, NamespacedName>);

impl CodexNamespaceMap {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Converts Codex-private namespace tools into standard function tools. The
/// returned map must be used to restore the response before it reaches Codex.
pub fn flatten_codex_namespaces(body: &mut Value) -> Result<CodexNamespaceMap, BridgeError> {
    promote_tool_search_tools(body)?;
    let Some(tools) = body.get("tools").and_then(Value::as_array) else {
        return Ok(CodexNamespaceMap::default());
    };
    if !tools
        .iter()
        .any(|tool| tool.get("type").and_then(Value::as_str) == Some("namespace"))
    {
        return Ok(CodexNamespaceMap::default());
    }

    let mut occupied = HashSet::new();
    for tool in tools {
        if matches!(
            tool.get("type").and_then(Value::as_str),
            Some("function" | "custom")
        ) {
            if let Some(name) = tool
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                occupied.insert(name.to_owned());
            }
        }
    }

    let mut owners = HashMap::new();
    for tool in tools {
        if tool.get("type").and_then(Value::as_str) != Some("namespace") {
            continue;
        }
        let namespace = required_trimmed(tool.get("name"), "namespace tool name")?;
        let children = namespace_children(tool)?;
        for child in children {
            if child.get("type").and_then(Value::as_str) != Some("function") {
                return Err(invalid("namespace children must be function tools"));
            }
            let name = required_trimmed(child.get("name"), "namespace child name")?;
            let flat = flatten_name(namespace, name);
            if occupied.contains(&flat) {
                return Err(invalid(format!(
                    "namespace tool {namespace}/{name} collides with top-level tool {flat}"
                )));
            }
            let entry = NamespacedName {
                namespace: namespace.to_owned(),
                name: name.to_owned(),
            };
            if owners.get(&flat).is_some_and(|previous| previous != &entry) {
                return Err(invalid(format!(
                    "multiple namespace tools flatten to the same name: {flat}"
                )));
            }
            owners.insert(flat, entry);
        }
    }

    let original = tools.clone();
    let mut flattened = Vec::new();
    let mut emitted = HashSet::new();
    for tool in original {
        if tool.get("type").and_then(Value::as_str) != Some("namespace") {
            flattened.push(tool);
            continue;
        }
        let namespace = required_trimmed(tool.get("name"), "namespace tool name")?.to_owned();
        for child in namespace_children(&tool)? {
            let name = required_trimmed(child.get("name"), "namespace child name")?;
            let flat = flatten_name(&namespace, name);
            if emitted.insert(flat.clone()) {
                let mut child = child.clone();
                child["name"] = json!(flat);
                flattened.push(child);
            }
        }
    }
    body["tools"] = Value::Array(flattened);
    if let Some(input) = body.get_mut("input") {
        rewrite_calls(input, &owners);
    }
    if let Some(choice) = body.get_mut("tool_choice") {
        if choice.get("type").and_then(Value::as_str) == Some("namespace") {
            *choice = json!("auto");
        } else {
            rewrite_call(choice, &owners);
        }
    }
    Ok(CodexNamespaceMap(owners))
}

fn promote_tool_search_tools(body: &mut Value) -> Result<(), BridgeError> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| invalid("Codex request body must be an object"))?;
    let loaded = object
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_search_output"))
        .filter_map(|item| item.get("tools"))
        .map(|tools| {
            tools
                .as_array()
                .cloned()
                .ok_or_else(|| invalid("tool_search_output.tools must be an array"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if loaded.is_empty() {
        return Ok(());
    }
    let tools = object.entry("tools").or_insert_with(|| json!([]));
    let tools = tools
        .as_array_mut()
        .ok_or_else(|| invalid("tools must be an array"))?;
    for tool in loaded {
        if !tools.contains(&tool) {
            tools.push(tool);
        }
    }
    Ok(())
}

pub fn restore_codex_namespaces(value: &mut Value, map: &CodexNamespaceMap) -> bool {
    restore_value(value, &map.0)
}

pub fn restore_codex_namespace_sse<S, E>(source: S, map: CodexNamespaceMap) -> AnthropicSseStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    if map.is_empty() {
        return Box::pin(source.map(|item| {
            item.map_err(|error| {
                BridgeError::InvalidCodexResponse(format!(
                    "Responses SSE transport failed: {error}"
                ))
            })
        }));
    }
    Box::pin(stream! {
        let mut source = Box::pin(source);
        let mut buffer = String::new();
        let mut remainder = Vec::new();
        while let Some(chunk) = source.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    yield Err(BridgeError::InvalidCodexResponse(format!("Responses SSE transport failed: {error}")));
                    return;
                }
            };
            if let Err(error) = sse::append_utf8(&mut buffer, &mut remainder, &chunk) {
                yield Err(error);
                return;
            }
            while let Some(block) = sse::take_sse_block(&mut buffer) {
                let (event, data) = match sse::parse_sse_block(&block) {
                    Ok(fields) => fields,
                    Err(error) => {
                        yield Err(error);
                        return;
                    }
                };
                let mut value: Value = match serde_json::from_str(data) {
                    Ok(value) => value,
                    Err(_) => {
                        yield Err(invalid("Responses SSE data must be valid JSON"));
                        return;
                    }
                };
                restore_codex_namespaces(&mut value, &map);
                yield Ok(Bytes::from(format!("event: {event}\ndata: {}\n\n", value)));
            }
        }
        if !remainder.is_empty() || !buffer.trim().is_empty() {
            yield Err(invalid("Responses SSE ended with an incomplete event"));
        }
    })
}

fn namespace_children(tool: &Value) -> Result<&Vec<Value>, BridgeError> {
    tool.get("tools")
        .or_else(|| tool.get("children"))
        .and_then(Value::as_array)
        .filter(|children| !children.is_empty())
        .ok_or_else(|| invalid("namespace tools must contain a non-empty tools array"))
}

fn rewrite_calls(value: &mut Value, owners: &HashMap<String, NamespacedName>) {
    match value {
        Value::Array(items) => {
            for item in items {
                rewrite_calls(item, owners);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("function_call") {
                rewrite_call(value, owners);
            } else {
                for child in object.values_mut() {
                    rewrite_calls(child, owners);
                }
            }
        }
        _ => {}
    }
}

fn rewrite_call(value: &mut Value, owners: &HashMap<String, NamespacedName>) -> bool {
    let Some(object) = value.as_object_mut() else {
        return false;
    };
    let Some(namespace) = object
        .get("namespace")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
    else {
        return false;
    };
    let Some(name) = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
    else {
        return false;
    };
    let flat = flatten_name(&namespace, &name);
    if owners
        .get(&flat)
        .is_some_and(|entry| entry.namespace == namespace && entry.name == name)
    {
        object.insert("name".into(), json!(flat));
        object.remove("namespace");
        true
    } else {
        false
    }
}

fn restore_value(value: &mut Value, map: &HashMap<String, NamespacedName>) -> bool {
    let mut changed = false;
    match value {
        Value::Array(items) => {
            for item in items {
                changed |= restore_value(item, map);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("function_call") {
                let flat = object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(entry) = flat.as_deref().and_then(|name| map.get(name)) {
                    object.insert("name".into(), json!(entry.name));
                    object.insert("namespace".into(), json!(entry.namespace));
                    changed = true;
                }
            }
            for child in object.values_mut() {
                changed |= restore_value(child, map);
            }
        }
        _ => {}
    }
    changed
}

fn flatten_name(namespace: &str, name: &str) -> String {
    let full = format!("{namespace}__{name}");
    if full.len() <= TOOL_NAME_MAX_LEN {
        return full;
    }
    let digest = Sha256::digest(full.as_bytes());
    let hash = digest.iter().take(8).fold(String::new(), |mut hash, byte| {
        write!(hash, "{byte:02x}").expect("writing to a String cannot fail");
        hash
    });
    let suffix = format!("__{hash}");
    let limit = TOOL_NAME_MAX_LEN - suffix.len();
    let mut prefix = String::new();
    for character in full.chars() {
        if prefix.len() + character.len_utf8() > limit {
            break;
        }
        prefix.push(character);
    }
    format!("{prefix}{suffix}")
}

fn required_trimmed<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(format!("{field} must be a non-empty string")))
}

fn invalid(message: impl Into<String>) -> BridgeError {
    BridgeError::InvalidCodexRequest(message.into())
}
