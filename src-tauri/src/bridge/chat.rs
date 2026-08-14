// Minimal Chat Completions adapter derived from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::{AnthropicSseStream, BridgeError, append_api_endpoint, chat_sse_to_anthropic, media};
use reqwest::Client;
use serde_json::{Map, Value, json};
use url::Url;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenAiChatCapabilities {
    pub reasoning_content: bool,
    pub reasoning_effort: bool,
}

pub struct OpenAiChatBridge {
    client: Client,
    endpoint: Url,
    bearer_token: Option<String>,
    capabilities: OpenAiChatCapabilities,
}

impl OpenAiChatBridge {
    pub fn new(base_url: Url, bearer_token: impl Into<String>) -> Self {
        Self::from_endpoint(
            append_api_endpoint(base_url, &["v1", "chat", "completions"]),
            bearer_token,
        )
    }

    pub fn from_endpoint(endpoint: Url, bearer_token: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            endpoint,
            bearer_token: Some(bearer_token.into()),
            capabilities: OpenAiChatCapabilities::default(),
        }
    }

    pub fn from_endpoint_without_auth(endpoint: Url) -> Self {
        Self {
            client: Client::new(),
            endpoint,
            bearer_token: None,
            capabilities: OpenAiChatCapabilities::default(),
        }
    }

    pub fn with_capabilities(mut self, capabilities: OpenAiChatCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub async fn complete(&self, request: Value) -> Result<Value, BridgeError> {
        let upstream_request = anthropic_to_chat(request, self.capabilities)?;
        if upstream_request.get("stream").and_then(Value::as_bool) == Some(true) {
            return Err(invalid_request(
                "stream=true requires the streaming bridge API",
            ));
        }
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .json(&upstream_request);
        if let Some(token) = &self.bearer_token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| BridgeError::ChatRequestFailed(error.to_string()))?;
        if !response.status().is_success() {
            return Err(BridgeError::ChatUpstream {
                status: response.status().as_u16(),
            });
        }
        let body = response
            .json::<Value>()
            .await
            .map_err(|error| invalid_response(&error.to_string()))?;
        chat_to_anthropic(body, self.capabilities)
    }

    pub async fn stream(&self, request: Value) -> Result<AnthropicSseStream, BridgeError> {
        let mut upstream_request = anthropic_to_chat(request, self.capabilities)?;
        if upstream_request.get("stream").and_then(Value::as_bool) != Some(true) {
            return Err(invalid_request(
                "the streaming bridge API requires stream=true",
            ));
        }
        upstream_request["stream_options"] = json!({"include_usage":true});
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .json(&upstream_request);
        if let Some(token) = &self.bearer_token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| BridgeError::ChatRequestFailed(error.to_string()))?;
        if !response.status().is_success() {
            return Err(BridgeError::ChatUpstream {
                status: response.status().as_u16(),
            });
        }
        Ok(Box::pin(chat_sse_to_anthropic(
            response.bytes_stream(),
            self.capabilities,
        )))
    }
}

fn anthropic_to_chat(
    body: Value,
    capabilities: OpenAiChatCapabilities,
) -> Result<Value, BridgeError> {
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
            "context_management",
            "output_config",
            "thinking",
        ],
        None,
    )?;
    let model = non_empty_string(object.get("model"), "model")?;
    let max_tokens = object
        .get("max_tokens")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_request("max_tokens must be a positive integer"))?;
    let source_messages = object
        .get("messages")
        .and_then(Value::as_array)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(|| invalid_request("messages must be a non-empty array"))?;
    let mut messages = Vec::new();
    if let Some(system) = object.get("system") {
        let system = convert_system(system)?;
        if !system.is_empty() {
            messages.push(json!({"role":"system","content":system}));
        }
    }
    for (index, message) in source_messages.iter().enumerate() {
        convert_message(message, index, capabilities, &mut messages)?;
    }
    let mut result = json!({
        "model":model,
        "max_tokens":max_tokens,
        "messages":messages
    });
    if let Some(effort) = super::request_hints::validate(
        object,
        capabilities.reasoning_effort || capabilities.reasoning_content,
        "reasoning_content or reasoning_effort",
    )? {
        if capabilities.reasoning_effort {
            result["reasoning_effort"] = json!(effort);
        }
    }
    if let Some(value) = object.get("temperature") {
        result["temperature"] = json!(finite_unit_number(value, "temperature")?);
    }
    if let Some(value) = object.get("top_p") {
        result["top_p"] = json!(finite_unit_number(value, "top_p")?);
    }
    if let Some(value) = object.get("stop_sequences") {
        let stops = value
            .as_array()
            .filter(|stops| !stops.is_empty())
            .ok_or_else(|| invalid_request("stop_sequences must be a non-empty array"))?;
        for (index, stop) in stops.iter().enumerate() {
            non_empty_string(Some(stop), &format!("stop_sequences[{index}]"))?;
        }
        result["stop"] = value.clone();
    }
    if let Some(value) = object.get("stream") {
        result["stream"] = json!(
            value
                .as_bool()
                .ok_or_else(|| invalid_request("stream must be a boolean"))?
        );
    }
    let has_tools = match object.get("tools") {
        Some(tools) => {
            let values = tools
                .as_array()
                .ok_or_else(|| invalid_request("tools must be an array"))?;
            if !values.is_empty() {
                result["tools"] = Value::Array(convert_tools(tools)?);
            }
            !values.is_empty()
        }
        None => false,
    };
    if object.contains_key("tool_choice") && !has_tools {
        return Err(invalid_request(
            "tool_choice requires a non-empty tools array",
        ));
    }
    if let Some(value) = object.get("tool_choice") {
        result["tool_choice"] = convert_tool_choice(value)?;
    }
    Ok(result)
}

fn convert_system(value: &Value) -> Result<String, BridgeError> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_owned());
    }
    let blocks = value
        .as_array()
        .filter(|blocks| !blocks.is_empty())
        .ok_or_else(|| invalid_request("system must be a string or non-empty array"))?;
    let mut parts = Vec::with_capacity(blocks.len());
    for (index, block) in blocks.iter().enumerate() {
        let block = block
            .as_object()
            .ok_or_else(|| invalid_request(&format!("system[{index}] must be an object")))?;
        reject_unknown(
            block,
            &["type", "text", "cache_control"],
            Some(&format!("system[{index}]")),
        )?;
        validate_cache_control(block.get("cache_control"), &format!("system[{index}]"))?;
        if string(block.get("type"), &format!("system[{index}].type"))? != "text" {
            return Err(invalid_request(&format!(
                "system[{index}].type must be text"
            )));
        }
        parts.push(non_empty_string(
            block.get("text"),
            &format!("system[{index}].text"),
        )?);
    }
    Ok(parts.join("\n"))
}

fn convert_message(
    value: &Value,
    index: usize,
    capabilities: OpenAiChatCapabilities,
    output: &mut Vec<Value>,
) -> Result<(), BridgeError> {
    let field = format!("messages[{index}]");
    let message = value
        .as_object()
        .ok_or_else(|| invalid_request(&format!("{field} must be an object")))?;
    reject_unknown(message, &["role", "content"], Some(&field))?;
    let role = string(message.get("role"), &format!("{field}.role"))?;
    if role != "user" && role != "assistant" && role != "system" {
        return Err(invalid_request(&format!(
            "{field}.role must be user, assistant, or system"
        )));
    }
    let content = message
        .get("content")
        .ok_or_else(|| invalid_request(&format!("{field}.content is required")))?;
    if let Some(text) = content.as_str() {
        output.push(json!({"role":role,"content":text}));
        return Ok(());
    }
    let blocks = content
        .as_array()
        .filter(|blocks| !blocks.is_empty())
        .ok_or_else(|| {
            invalid_request(&format!(
                "{field}.content must be a string or non-empty array"
            ))
        })?;
    let mut content_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();
    let mut reasoning = Vec::new();
    for (block_index, block) in blocks.iter().enumerate() {
        let block_field = format!("{field}.content[{block_index}]");
        let block = block
            .as_object()
            .ok_or_else(|| invalid_request(&format!("{block_field} must be an object")))?;
        match string(block.get("type"), &format!("{block_field}.type"))? {
            "text" => {
                reject_unknown(
                    block,
                    &["type", "text", "cache_control"],
                    Some(&block_field),
                )?;
                validate_cache_control(block.get("cache_control"), &block_field)?;
                let text = non_empty_string(block.get("text"), &format!("{block_field}.text"))?;
                content_parts.push(json!({"type":"text","text":text}));
            }
            "thinking" if capabilities.reasoning_content => {
                reject_unknown(
                    block,
                    &["type", "thinking", "signature"],
                    Some(&block_field),
                )?;
                if let Some(signature) = block.get("signature") {
                    non_empty_string(Some(signature), &format!("{block_field}.signature"))?;
                }
                reasoning.push(non_empty_string(
                    block.get("thinking"),
                    &format!("{block_field}.thinking"),
                )?);
            }
            "thinking" => {
                return Err(invalid_request(
                    "thinking blocks require the provider reasoning_content capability",
                ));
            }
            "redacted_thinking" if capabilities.reasoning_content => {
                reject_unknown(block, &["type", "data"], Some(&block_field))?;
                non_empty_string(block.get("data"), &format!("{block_field}.data"))?;
                reasoning.push("[redacted thinking]");
            }
            "redacted_thinking" => {
                return Err(invalid_request(
                    "redacted_thinking blocks require the provider reasoning_content capability",
                ));
            }
            "image" => {
                if role != "user" {
                    return Err(invalid_request("image blocks require user role"));
                }
                let image_url = media::anthropic_image_url(block, &block_field)?;
                content_parts.push(json!({"type":"image_url","image_url":{"url":image_url}}));
            }
            "document" => {
                return Err(invalid_request(
                    "document blocks cannot be represented losslessly by Chat Completions",
                ));
            }
            "tool_use" => {
                if role != "assistant" {
                    return Err(invalid_request("tool_use blocks require assistant role"));
                }
                reject_unknown(
                    block,
                    &["type", "id", "name", "input", "cache_control"],
                    Some(&block_field),
                )?;
                validate_cache_control(block.get("cache_control"), &block_field)?;
                let id = non_empty_string(block.get("id"), &format!("{block_field}.id"))?;
                let name = non_empty_string(block.get("name"), &format!("{block_field}.name"))?;
                let input = block
                    .get("input")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        invalid_request(&format!("{block_field}.input must be an object"))
                    })?;
                let arguments = serde_json::to_string(input).expect("JSON objects serialize");
                tool_calls.push(json!({
                    "id":id,"type":"function",
                    "function":{"name":name,"arguments":arguments}
                }));
            }
            "tool_result" => {
                if role != "user" {
                    return Err(invalid_request("tool_result blocks require user role"));
                }
                reject_unknown(
                    block,
                    &["type", "tool_use_id", "content", "cache_control"],
                    Some(&block_field),
                )?;
                validate_cache_control(block.get("cache_control"), &block_field)?;
                let id = non_empty_string(
                    block.get("tool_use_id"),
                    &format!("{block_field}.tool_use_id"),
                )?;
                let content =
                    tool_result_text(block.get("content"), &format!("{block_field}.content"))?;
                tool_results.push(json!({
                    "role":"tool","tool_call_id":id,"content":content
                }));
            }
            other => {
                return Err(invalid_request(&format!(
                    "{block_field}.type is unsupported: {other}"
                )));
            }
        }
    }
    if !tool_results.is_empty() {
        if !content_parts.is_empty() || !tool_calls.is_empty() || !reasoning.is_empty() {
            return Err(invalid_request(
                "tool_result cannot be mixed with other content in one message",
            ));
        }
        output.extend(tool_results);
        return Ok(());
    }
    if !reasoning.is_empty() && tool_calls.is_empty() {
        return Err(invalid_request(
            "reasoning_content is only valid on assistant tool calls",
        ));
    }
    let content = if content_parts.is_empty() {
        Value::Null
    } else if content_parts.len() == 1
        && content_parts[0].get("type").and_then(Value::as_str) == Some("text")
    {
        content_parts[0]["text"].clone()
    } else {
        Value::Array(content_parts)
    };
    let mut message = json!({"role":role,"content":content});
    let has_tool_calls = !tool_calls.is_empty();
    if has_tool_calls {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    if has_tool_calls && capabilities.reasoning_content {
        message["reasoning_content"] = json!(if reasoning.is_empty() {
            "tool call".to_owned()
        } else {
            reasoning.join("\n")
        });
    }
    output.push(message);
    Ok(())
}

fn tool_result_text(value: Option<&Value>, field: &str) -> Result<String, BridgeError> {
    let value = value.ok_or_else(|| invalid_request(&format!("{field} is required")))?;
    if let Some(text) = value.as_str() {
        return Ok(text.to_owned());
    }
    let blocks = value
        .as_array()
        .ok_or_else(|| invalid_request(&format!("{field} must be a string or array")))?;
    let mut parts = Vec::with_capacity(blocks.len());
    for (index, block) in blocks.iter().enumerate() {
        let block = block
            .as_object()
            .ok_or_else(|| invalid_request(&format!("{field}[{index}] must be an object")))?;
        match string(block.get("type"), &format!("{field}[{index}].type"))? {
            "text" => reject_unknown(block, &["type", "text"], Some(&format!("{field}[{index}]")))?,
            "image" => {
                return Err(invalid_request(
                    "tool_result images cannot be represented losslessly by Chat Completions",
                ));
            }
            "document" => {
                return Err(invalid_request(
                    "tool_result documents cannot be represented losslessly by Chat Completions",
                ));
            }
            other => {
                return Err(invalid_request(&format!(
                    "{field}[{index}].type is unsupported: {other}"
                )));
            }
        }
        parts.push(string(
            block.get("text"),
            &format!("{field}[{index}].text"),
        )?);
    }
    Ok(parts.join("\n"))
}

fn convert_tools(value: &Value) -> Result<Vec<Value>, BridgeError> {
    let tools = value
        .as_array()
        .filter(|tools| !tools.is_empty())
        .ok_or_else(|| invalid_request("tools must be a non-empty array"))?;
    let mut converted = Vec::with_capacity(tools.len());
    let mut names = std::collections::HashSet::new();
    for (index, tool) in tools.iter().enumerate() {
        let field = format!("tools[{index}]");
        let tool = tool
            .as_object()
            .ok_or_else(|| invalid_request(&format!("{field} must be an object")))?;
        reject_unknown(
            tool,
            &["name", "description", "input_schema", "cache_control"],
            Some(&field),
        )?;
        validate_cache_control(tool.get("cache_control"), &field)?;
        let name = non_empty_string(tool.get("name"), &format!("{field}.name"))?;
        if !names.insert(name) {
            return Err(invalid_request(&format!("duplicate tool name: {name}")));
        }
        let schema = tool
            .get("input_schema")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_request(&format!("{field}.input_schema must be an object")))?;
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(invalid_request(&format!(
                "{field}.input_schema.type must be object"
            )));
        }
        let mut function = json!({"name":name,"parameters":schema});
        if let Some(description) = tool.get("description") {
            function["description"] = json!(non_empty_string(
                Some(description),
                &format!("{field}.description"),
            )?);
        }
        converted.push(json!({"type":"function","function":function}));
    }
    Ok(converted)
}

fn convert_tool_choice(value: &Value) -> Result<Value, BridgeError> {
    if let Some(choice) = value.as_str() {
        return match choice {
            "auto" | "none" => Ok(json!(choice)),
            "any" => Ok(json!("required")),
            _ => Err(invalid_request(
                "tool_choice must be auto, any, none, or a named tool",
            )),
        };
    }
    let choice = value
        .as_object()
        .ok_or_else(|| invalid_request("tool_choice must be a string or object"))?;
    reject_unknown(choice, &["type", "name"], Some("tool_choice"))?;
    match string(choice.get("type"), "tool_choice.type")? {
        "auto" => Ok(json!("auto")),
        "any" => Ok(json!("required")),
        "none" => Ok(json!("none")),
        "tool" => Ok(json!({
            "type":"function",
            "function":{"name":non_empty_string(choice.get("name"), "tool_choice.name")?}
        })),
        _ => Err(invalid_request("tool_choice.type is unsupported")),
    }
}

fn chat_to_anthropic(
    body: Value,
    _capabilities: OpenAiChatCapabilities,
) -> Result<Value, BridgeError> {
    let object = body
        .as_object()
        .ok_or_else(|| invalid_response("body must be an object"))?;
    if let Some(error) = object.get("error").filter(|error| !error.is_null()) {
        return Err(chat_error(error)?);
    }
    let id = response_string(object.get("id"), "id")?;
    let model = response_string(object.get("model"), "model")?;
    let choices = object
        .get("choices")
        .and_then(Value::as_array)
        .filter(|choices| choices.len() == 1)
        .ok_or_else(|| invalid_response("choices must contain exactly one item"))?;
    let choice = choices[0]
        .as_object()
        .ok_or_else(|| invalid_response("choices[0] must be an object"))?;
    if choice.get("index").and_then(Value::as_u64) != Some(0) {
        return Err(invalid_response("choices[0].index must be 0"));
    }
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_response("choices[0].message must be an object"))?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return Err(invalid_response(
            "choices[0].message.role must be assistant",
        ));
    }
    let mut content = Vec::new();
    if let Some(reasoning) =
        super::chat_reasoning::extract_reasoning_field_text(&Value::Object(message.clone()))
    {
        content.push(json!({"type":"thinking","thinking":reasoning}));
    }
    match message.get("content") {
        Some(Value::String(text)) if !text.is_empty() => {
            content.push(json!({"type":"text","text":text}));
        }
        Some(Value::String(_)) | Some(Value::Null) | None => {}
        Some(_) => {
            return Err(invalid_response(
                "choices[0].message.content must be a string or null",
            ));
        }
    }
    if let Some(tool_calls) = message.get("tool_calls") {
        let tool_calls = tool_calls
            .as_array()
            .filter(|calls| !calls.is_empty())
            .ok_or_else(|| {
                invalid_response("choices[0].message.tool_calls must be a non-empty array")
            })?;
        for (index, call) in tool_calls.iter().enumerate() {
            let field = format!("choices[0].message.tool_calls[{index}]");
            let call = call
                .as_object()
                .ok_or_else(|| invalid_response(&format!("{field} must be an object")))?;
            if response_string(call.get("type"), &format!("{field}.type"))? != "function" {
                return Err(invalid_response(&format!("{field}.type must be function")));
            }
            let id = response_non_empty_string(call.get("id"), &format!("{field}.id"))?;
            let function = call
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(|| invalid_response(&format!("{field}.function must be an object")))?;
            let name =
                response_non_empty_string(function.get("name"), &format!("{field}.function.name"))?;
            let arguments = response_string(
                function.get("arguments"),
                &format!("{field}.function.arguments"),
            )?;
            let input: Value = serde_json::from_str(arguments).map_err(|_| {
                invalid_response(&format!("{field}.function.arguments must be valid JSON"))
            })?;
            if !input.is_object() {
                return Err(invalid_response(&format!(
                    "{field}.function.arguments must be a JSON object"
                )));
            }
            content.push(json!({"type":"tool_use","id":id,"name":name,"input":input}));
        }
    }
    let stop_reason =
        match response_string(choice.get("finish_reason"), "choices[0].finish_reason")? {
            "stop" | "content_filter" => "end_turn",
            "length" => "max_tokens",
            "tool_calls" | "function_call" => "tool_use",
            other => {
                return Err(invalid_response(&format!(
                    "unsupported finish_reason: {other}"
                )));
            }
        };
    let usage = chat_usage(object.get("usage"))?;
    Ok(json!({
        "id":id,"type":"message","role":"assistant","content":content,
        "model":model,"stop_reason":stop_reason,"stop_sequence":null,"usage":usage
    }))
}

pub(super) fn chat_usage(value: Option<&Value>) -> Result<Value, BridgeError> {
    let usage = value
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_response("usage must be an object"))?;
    let total = response_u64(usage.get("prompt_tokens"), "usage.prompt_tokens")?;
    let output = response_u64(usage.get("completion_tokens"), "usage.completion_tokens")?;
    let cached = optional_u64(
        usage.get("cache_read_input_tokens").or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(Value::as_object)
                .and_then(|details| details.get("cached_tokens"))
        }),
        "usage cache read tokens",
    )?
    .unwrap_or(0);
    let created = optional_u64(
        usage.get("cache_creation_input_tokens").or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(Value::as_object)
                .and_then(|details| details.get("cache_write_tokens"))
        }),
        "usage cache creation tokens",
    )?
    .unwrap_or(0);
    let fresh = total
        .checked_sub(
            cached
                .checked_add(created)
                .ok_or_else(|| invalid_response("usage cache tokens overflowed"))?,
        )
        .ok_or_else(|| invalid_response("usage cache tokens exceed prompt_tokens"))?;
    let mut result = json!({"input_tokens":fresh,"output_tokens":output});
    if cached > 0 {
        result["cache_read_input_tokens"] = json!(cached);
    }
    if created > 0 {
        result["cache_creation_input_tokens"] = json!(created);
    }
    Ok(result)
}

fn chat_error(value: &Value) -> Result<BridgeError, BridgeError> {
    let error = value
        .as_object()
        .ok_or_else(|| invalid_response("error must be an object"))?;
    let kind = response_string(error.get("type"), "error.type")?;
    let message = response_string(error.get("message"), "error.message")?;
    Ok(BridgeError::ChatUpstreamResponse {
        kind: safe_kind(kind),
        message: safe_message(message),
    })
}

fn reject_unknown(
    object: &Map<String, Value>,
    allowed: &[&str],
    parent: Option<&str>,
) -> Result<(), BridgeError> {
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(invalid_request(&format!(
            "unsupported field: {}{}",
            parent
                .map(|parent| format!("{parent}."))
                .unwrap_or_default(),
            field
        )));
    }
    Ok(())
}

fn string<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_request(&format!("{field} must be a string")))
}

fn non_empty_string<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, BridgeError> {
    string(value, field).and_then(|value| {
        if value.trim().is_empty() {
            Err(invalid_request(&format!("{field} must not be empty")))
        } else {
            Ok(value)
        }
    })
}

fn validate_cache_control(value: Option<&Value>, field: &str) -> Result<(), BridgeError> {
    let Some(value) = value else {
        return Ok(());
    };
    let object = value
        .as_object()
        .ok_or_else(|| invalid_request(&format!("{field}.cache_control must be an object")))?;
    reject_unknown(
        object,
        &["type", "ttl"],
        Some(&format!("{field}.cache_control")),
    )?;
    if non_empty_string(object.get("type"), &format!("{field}.cache_control.type"))? != "ephemeral"
    {
        return Err(invalid_request(&format!(
            "{field}.cache_control.type must be ephemeral"
        )));
    }
    if let Some(ttl) = object.get("ttl") {
        let ttl = non_empty_string(Some(ttl), &format!("{field}.cache_control.ttl"))?;
        if ttl != "5m" && ttl != "1h" {
            return Err(invalid_request(&format!(
                "{field}.cache_control.ttl must be 5m or 1h"
            )));
        }
    }
    Ok(())
}

fn finite_unit_number(value: &Value, field: &str) -> Result<f64, BridgeError> {
    value
        .as_f64()
        .filter(|number| number.is_finite() && (0.0..=1.0).contains(number))
        .ok_or_else(|| invalid_request(&format!("{field} must be a number from 0 to 1")))
}

fn response_string<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_response(&format!("{field} must be a string")))
}

fn response_non_empty_string<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<&'a str, BridgeError> {
    response_string(value, field).and_then(|value| {
        if value.trim().is_empty() {
            Err(invalid_response(&format!("{field} must not be empty")))
        } else {
            Ok(value)
        }
    })
}

fn response_u64(value: Option<&Value>, field: &str) -> Result<u64, BridgeError> {
    value
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_response(&format!("{field} must be an unsigned integer")))
}

fn optional_u64(value: Option<&Value>, field: &str) -> Result<Option<u64>, BridgeError> {
    value
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| invalid_response(&format!("{field} must be an unsigned integer")))
        })
        .transpose()
}

fn invalid_request(message: &str) -> BridgeError {
    BridgeError::InvalidRequest(message.into())
}

pub(super) fn invalid_response(message: &str) -> BridgeError {
    BridgeError::InvalidChatResponse(message.into())
}

pub(super) fn safe_kind(value: &str) -> String {
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

pub(super) fn safe_message(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(512)
        .collect()
}
