// Minimal Codex Responses ↔ Chat Completions bridge adapted from cc-switch,
// commit 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::BridgeError;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

const CUSTOM_INPUT_FIELD: &str = "input";
const TOOL_SEARCH_NAME: &str = "tool_search";

#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolKind {
    Function,
    Custom,
    ToolSearch,
    Namespace { namespace: String, name: String },
}

#[derive(Debug, Clone, Default)]
pub struct CodexChatContext {
    tools: BTreeMap<String, ToolKind>,
    chat_tools: Vec<Value>,
}

impl CodexChatContext {
    fn kind(&self, name: &str) -> Option<&ToolKind> {
        self.tools.get(name)
    }

    pub(crate) fn is_custom(&self, name: &str) -> bool {
        matches!(self.kind(name), Some(ToolKind::Custom))
    }

    pub(crate) fn response_item(
        &self,
        item_id: String,
        call_id: &str,
        name: &str,
        arguments: &str,
        status: &str,
    ) -> Result<Value, BridgeError> {
        match self.kind(name) {
            Some(ToolKind::Custom) => Ok(json!({
                "id": item_id,
                "type":"custom_tool_call",
                "status":status,
                "call_id":call_id,
                "name":name,
                "input":custom_input(arguments)
            })),
            Some(ToolKind::ToolSearch) => Ok(json!({
                "type":"tool_search_call",
                "status":status,
                "execution":"client",
                "call_id":call_id,
                "arguments":parse_object(arguments)
            })),
            Some(ToolKind::Namespace { namespace, name }) => Ok(json!({
                "id":item_id,
                "type":"function_call",
                "status":status,
                "call_id":call_id,
                "namespace":namespace,
                "name":name,
                "arguments":canonical_arguments(arguments)?
            })),
            _ => Ok(json!({
                "id":item_id,
                "type":"function_call",
                "status":status,
                "call_id":call_id,
                "name":name,
                "arguments":canonical_arguments(arguments)?
            })),
        }
    }
}

pub fn codex_response_to_chat(body: Value) -> Result<Value, BridgeError> {
    codex_response_to_chat_with_context(body).map(|(request, _)| request)
}

pub fn codex_response_to_chat_with_context(
    body: Value,
) -> Result<(Value, CodexChatContext), BridgeError> {
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
    let mut context = build_context(&body)?;
    append_input(input, &mut messages, &mut context)?;
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
        let _tools = tools
            .as_array()
            .ok_or_else(|| invalid_request("tools must be an array"))?;
        let converted = context.chat_tools.clone();
        if !converted.is_empty() {
            result["tools"] = json!(converted);
        }
    }
    if let Some(choice) = object.get("tool_choice") {
        result["tool_choice"] = match choice {
            Value::String(value) => json!(value),
            Value::Object(choice)
                if matches!(
                    choice.get("type").and_then(Value::as_str),
                    Some("function" | "custom")
                ) =>
            {
                let name = required_string(choice.get("name"), "tool_choice.name")?;
                json!({"type":"function","function":{"name":name}})
            }
            Value::Object(choice)
                if choice.get("type").and_then(Value::as_str) == Some("tool_search") =>
            {
                json!({"type":"function","function":{"name":TOOL_SEARCH_NAME}})
            }
            _ => return Err(invalid_request("tool_choice is unsupported")),
        };
    }
    if object.get("stream").and_then(Value::as_bool) == Some(true) {
        result["stream_options"] = json!({"include_usage":true});
    }
    Ok((result, context))
}

pub fn chat_to_codex_response(body: Value) -> Result<Value, BridgeError> {
    chat_to_codex_response_with_context(body, &CodexChatContext::default())
}

pub fn chat_to_codex_response_with_context(
    body: Value,
    context: &CodexChatContext,
) -> Result<Value, BridgeError> {
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
            output.push(context.response_item(
                format!(
                    "{}_{}_{}",
                    if context.is_custom(name) { "ctc" } else { "fc" },
                    response_id,
                    index
                ),
                call_id,
                name,
                arguments,
                "completed",
            )?);
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

fn append_input(
    input: &Value,
    messages: &mut Vec<Value>,
    context: &mut CodexChatContext,
) -> Result<(), BridgeError> {
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
                    Some("custom_tool_call") => {
                        let call_id = required_string(
                            item.get("call_id"),
                            &format!("input[{index}].call_id"),
                        )?;
                        let name =
                            required_string(item.get("name"), &format!("input[{index}].name"))?;
                        let input = item.get("input").cloned().unwrap_or_else(|| json!(""));
                        context.tools.insert(name.to_string(), ToolKind::Custom);
                        messages.push(json!({"role":"assistant","content":null,"tool_calls":[{"id":call_id,"type":"function","function":{"name":name,"arguments":json!({"input":input}).to_string()}}]}));
                    }
                    Some("tool_search_call") => {
                        let call_id = required_string(
                            item.get("call_id"),
                            &format!("input[{index}].call_id"),
                        )?;
                        let arguments = item.get("arguments").cloned().unwrap_or_else(|| json!({}));
                        context
                            .tools
                            .insert(TOOL_SEARCH_NAME.to_string(), ToolKind::ToolSearch);
                        messages.push(json!({"role":"assistant","content":null,"tool_calls":[{"id":call_id,"type":"function","function":{"name":TOOL_SEARCH_NAME,"arguments":arguments.to_string()}}]}));
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
                    Some("custom_tool_call_output") | Some("tool_search_output") => {
                        let call_id = required_string(
                            item.get("call_id"),
                            &format!("input[{index}].call_id"),
                        )?;
                        let output = item.get("output").cloned().unwrap_or(Value::Null);
                        messages.push(json!({"role":"tool","tool_call_id":call_id,"content":text_or_json(&output)}));
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

fn build_context(body: &Value) -> Result<CodexChatContext, BridgeError> {
    let mut context = CodexChatContext::default();
    let mut seen = BTreeSet::new();
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        for (index, tool) in tools.iter().enumerate() {
            add_tool(
                tool,
                None,
                &mut context,
                &mut seen,
                &format!("tools[{index}]"),
            )?;
        }
    }
    if let Some(input) = body.get("input") {
        collect_loaded_tools(input, &mut context, &mut seen)?;
    }
    Ok(context)
}

fn collect_loaded_tools(
    value: &Value,
    context: &mut CodexChatContext,
    seen: &mut BTreeSet<String>,
) -> Result<(), BridgeError> {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_loaded_tools(item, context, seen)?;
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("tool_search_output") {
                if let Some(tools) = object.get("tools").and_then(Value::as_array) {
                    for (index, tool) in tools.iter().enumerate() {
                        add_tool(
                            tool,
                            None,
                            context,
                            seen,
                            &format!("tool_search_output.tools[{index}]"),
                        )?;
                    }
                }
            }
            for child in object.values() {
                collect_loaded_tools(child, context, seen)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn add_tool(
    tool: &Value,
    namespace: Option<&str>,
    context: &mut CodexChatContext,
    seen: &mut BTreeSet<String>,
    field: &str,
) -> Result<(), BridgeError> {
    if let Some(name) = tool.as_str() {
        return add_custom_tool(name, &json!({"type":"custom","name":name}), context, seen);
    }
    let object = tool
        .as_object()
        .ok_or_else(|| invalid_request(format!("{field} must be an object")))?;
    match object.get("type").and_then(Value::as_str) {
        Some("function") => {
            let name = required_string(object.get("name"), &format!("{field}.name"))?;
            let chat_name = namespace
                .map(|ns| flatten_name(ns, name))
                .unwrap_or_else(|| name.to_string());
            if !seen.insert(chat_name.clone()) {
                return Ok(());
            }
            let parameters = object
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type":"object","properties":{}}));
            let mut function = json!({"name":chat_name,"parameters":parameters});
            if let Some(description) = object.get("description") {
                function["description"] = description.clone();
            }
            if let Some(strict) = object.get("strict") {
                function["strict"] = strict.clone();
            }
            context
                .chat_tools
                .push(json!({"type":"function","function":function}));
            context.tools.insert(
                chat_name,
                match namespace {
                    Some(namespace) => ToolKind::Namespace {
                        namespace: namespace.to_string(),
                        name: name.to_string(),
                    },
                    None => ToolKind::Function,
                },
            );
        }
        Some("custom") => {
            let name = required_string(object.get("name"), &format!("{field}.name"))?;
            add_custom_tool(name, tool, context, seen)?;
        }
        Some("tool_search") => {
            if seen.insert(TOOL_SEARCH_NAME.to_string()) {
                context.chat_tools.push(json!({
                    "type":"function",
                    "function":{
                        "name":TOOL_SEARCH_NAME,
                        "description":"Search and load tools, plugins, connectors, and MCP namespaces for the current task.",
                        "parameters":{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}},"required":["query"]}
                    }
                }));
                context
                    .tools
                    .insert(TOOL_SEARCH_NAME.to_string(), ToolKind::ToolSearch);
            }
        }
        Some("namespace") => {
            let namespace = required_string(object.get("name"), &format!("{field}.name"))?;
            let children = object
                .get("tools")
                .or_else(|| object.get("children"))
                .and_then(Value::as_array)
                .ok_or_else(|| invalid_request(format!("{field} must contain tools")))?;
            for (index, child) in children.iter().enumerate() {
                add_tool(
                    child,
                    Some(namespace),
                    context,
                    seen,
                    &format!("{field}.tools[{index}]"),
                )?;
            }
        }
        Some(kind) => {
            return Err(invalid_request(format!(
                "{field}.type is unsupported: {kind}"
            )));
        }
        None => return Err(invalid_request(format!("{field}.type must be a string"))),
    }
    Ok(())
}

fn add_custom_tool(
    name: &str,
    original: &Value,
    context: &mut CodexChatContext,
    seen: &mut BTreeSet<String>,
) -> Result<(), BridgeError> {
    if !seen.insert(name.to_string()) {
        return Ok(());
    }
    context.chat_tools.push(json!({
        "type":"function",
        "function":{
            "name":name,
            "description":format!("Original custom tool definition:\n```json\n{}\n```", original),
            "parameters":{"type":"object","properties":{"input":{"type":"string"}},"required":[CUSTOM_INPUT_FIELD]}
        }
    }));
    context.tools.insert(name.to_string(), ToolKind::Custom);
    Ok(())
}

fn custom_input(arguments: &str) -> String {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get(CUSTOM_INPUT_FIELD)
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| arguments.to_string())
}

fn parse_object(arguments: &str) -> Value {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({"query":arguments}))
}

fn canonical_arguments(arguments: &str) -> Result<String, BridgeError> {
    if arguments.is_empty() {
        return Ok(String::new());
    }
    serde_json::from_str::<Value>(arguments)
        .map(|value| value.to_string())
        .map_err(|_| invalid_response("tool call arguments must be valid JSON"))
}

fn text_or_json(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn flatten_name(namespace: &str, name: &str) -> String {
    let full = format!("{namespace}__{name}");
    if full.len() <= 64 {
        return full;
    }
    let digest = Sha256::digest(full.as_bytes());
    let hash = digest.iter().take(8).fold(String::new(), |mut hash, byte| {
        write!(hash, "{byte:02x}").expect("writing to a String cannot fail");
        hash
    });
    let suffix = format!("__{hash}");
    let limit = 64 - suffix.len();
    let prefix = full
        .chars()
        .scan(0usize, |len, ch| {
            if *len + ch.len_utf8() > limit {
                None
            } else {
                *len += ch.len_utf8();
                Some(ch)
            }
        })
        .collect::<String>();
    format!("{prefix}{suffix}")
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
