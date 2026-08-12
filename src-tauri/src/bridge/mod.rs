// Portions adapted from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use bytes::Bytes;
use futures::Stream;
use reqwest::Client;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::pin::Pin;
use url::Url;

mod chat;
mod codex_anthropic;
mod codex_anthropic_streaming;
mod codex_chat;
mod codex_chat_streaming;
mod gemini;
mod media;
mod reasoning;
mod request_hints;
mod sse;
mod streaming_chat;
mod streaming_gemini;
mod streaming_responses;

pub use chat::{OpenAiChatBridge, OpenAiChatCapabilities};
pub use codex_anthropic::{
    CodexAnthropicCapabilities, CodexAnthropicContext, anthropic_to_codex_response,
    anthropic_to_codex_response_with_context, codex_response_to_anthropic,
    codex_response_to_anthropic_with_context,
};
pub use codex_anthropic_streaming::{
    anthropic_sse_to_codex_responses, anthropic_sse_to_codex_responses_with_context,
};
pub use codex_chat::{chat_to_codex_response, codex_response_to_chat};
pub use codex_chat_streaming::chat_sse_to_codex_responses;
pub use gemini::GeminiNativeBridge;
pub use streaming_chat::chat_sse_to_anthropic;
pub use streaming_responses::{
    responses_sse_to_anthropic, responses_sse_to_anthropic_with_capabilities,
};

pub struct OpenAiResponsesBridge {
    client: Client,
    endpoint: Url,
    bearer_token: Option<String>,
    capabilities: OpenAiResponsesCapabilities,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenAiResponsesCapabilities {
    pub reasoning_items: bool,
}

pub type AnthropicSseStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, BridgeError>> + Send + 'static>>;

#[derive(Debug, PartialEq, Eq)]
pub enum BridgeError {
    InvalidRequest(String),
    RequestFailed(String),
    Upstream {
        status: u16,
    },
    UpstreamResponse {
        kind: String,
        message: String,
    },
    UpstreamHttpResponse {
        status: u16,
        kind: String,
        message: String,
    },
    InvalidResponse(String),
    ChatRequestFailed(String),
    ChatUpstream {
        status: u16,
    },
    ChatUpstreamResponse {
        kind: String,
        message: String,
    },
    InvalidChatResponse(String),
    InvalidCodexRequest(String),
    InvalidCodexResponse(String),
    InvalidGeminiRequest(String),
    GeminiRequestFailed(String),
    GeminiUpstream {
        status: u16,
        kind: Option<String>,
        message: Option<String>,
    },
    InvalidGeminiResponse(String),
}

impl Display for BridgeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(message) => {
                write!(formatter, "invalid Anthropic request: {message}")
            }
            Self::RequestFailed(message) => {
                write!(formatter, "Responses request failed: {message}")
            }
            Self::Upstream { status } => {
                write!(formatter, "Responses upstream returned HTTP {status}")
            }
            Self::UpstreamResponse { kind, message } => {
                write!(formatter, "Responses upstream failed ({kind}): {message}")
            }
            Self::UpstreamHttpResponse { kind, message, .. } => {
                write!(formatter, "Responses upstream failed ({kind}): {message}")
            }
            Self::InvalidResponse(message) => {
                write!(formatter, "invalid Responses response: {message}")
            }
            Self::ChatRequestFailed(message) => {
                write!(formatter, "Chat Completions request failed: {message}")
            }
            Self::ChatUpstream { status } => {
                write!(
                    formatter,
                    "Chat Completions upstream returned HTTP {status}"
                )
            }
            Self::ChatUpstreamResponse { kind, message } => {
                write!(
                    formatter,
                    "Chat Completions upstream failed ({kind}): {message}"
                )
            }
            Self::InvalidChatResponse(message) => {
                write!(formatter, "invalid Chat Completions response: {message}")
            }
            Self::InvalidCodexRequest(message) => {
                write!(formatter, "invalid Codex Responses request: {message}")
            }
            Self::InvalidCodexResponse(message) => {
                write!(
                    formatter,
                    "invalid Codex Responses bridge response: {message}"
                )
            }
            Self::InvalidGeminiRequest(message) => {
                write!(formatter, "invalid Anthropic request for Gemini: {message}")
            }
            Self::GeminiRequestFailed(message) => {
                write!(formatter, "Gemini request failed: {message}")
            }
            Self::GeminiUpstream {
                status,
                kind,
                message,
            } => match (kind, message) {
                (Some(kind), Some(message)) => {
                    write!(
                        formatter,
                        "Gemini upstream returned HTTP {status} ({kind}): {message}"
                    )
                }
                _ => write!(formatter, "Gemini upstream returned HTTP {status}"),
            },
            Self::InvalidGeminiResponse(message) => {
                write!(formatter, "invalid Gemini response: {message}")
            }
        }
    }
}

impl Error for BridgeError {}

impl BridgeError {
    pub fn upstream_http_status(&self) -> Option<u16> {
        match self {
            Self::Upstream { status }
            | Self::UpstreamHttpResponse { status, .. }
            | Self::GeminiUpstream { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl OpenAiResponsesBridge {
    pub fn new(base_url: Url, bearer_token: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            endpoint: responses_endpoint(base_url),
            bearer_token: Some(bearer_token.into()),
            capabilities: OpenAiResponsesCapabilities::default(),
        }
    }

    pub fn from_endpoint(endpoint: Url, bearer_token: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            endpoint,
            bearer_token: Some(bearer_token.into()),
            capabilities: OpenAiResponsesCapabilities::default(),
        }
    }

    pub fn from_endpoint_without_auth(endpoint: Url) -> Self {
        Self {
            client: Client::new(),
            endpoint,
            bearer_token: None,
            capabilities: OpenAiResponsesCapabilities::default(),
        }
    }

    pub fn with_capabilities(mut self, capabilities: OpenAiResponsesCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub async fn complete(&self, request: Value) -> Result<Value, BridgeError> {
        let upstream_request = anthropic_to_responses(request, self.capabilities)?;
        if upstream_request.get("stream").and_then(Value::as_bool) == Some(true) {
            return Err(BridgeError::InvalidRequest(
                "stream=true requires the streaming bridge API".into(),
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
            .map_err(|error| BridgeError::RequestFailed(error.to_string()))?;

        if !response.status().is_success() {
            return Err(responses_http_error(response).await);
        }

        let body = response
            .json::<Value>()
            .await
            .map_err(|error| BridgeError::InvalidResponse(error.to_string()))?;
        responses_to_anthropic(body, self.capabilities)
    }

    pub async fn stream(&self, request: Value) -> Result<AnthropicSseStream, BridgeError> {
        let upstream_request = anthropic_to_responses(request, self.capabilities)?;
        if upstream_request.get("stream").and_then(Value::as_bool) != Some(true) {
            return Err(BridgeError::InvalidRequest(
                "the streaming bridge API requires stream=true".into(),
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
            .map_err(|error| BridgeError::RequestFailed(error.to_string()))?;
        if !response.status().is_success() {
            return Err(responses_http_error(response).await);
        }
        Ok(Box::pin(responses_sse_to_anthropic_with_capabilities(
            response.bytes_stream(),
            self.capabilities,
        )))
    }
}

async fn responses_http_error(response: reqwest::Response) -> BridgeError {
    let status = response.status().as_u16();
    let parsed = response
        .json::<Value>()
        .await
        .ok()
        .and_then(|body| body.as_object().cloned())
        .and_then(|object| response_envelope_error(&object).ok());
    match parsed {
        Some(BridgeError::UpstreamResponse { kind, message }) => {
            BridgeError::UpstreamHttpResponse {
                status,
                kind,
                message,
            }
        }
        _ => BridgeError::Upstream { status },
    }
}

fn responses_endpoint(base_url: Url) -> Url {
    append_api_endpoint(base_url, &["v1", "responses"])
}

fn append_api_endpoint(mut base_url: Url, suffix: &[&str]) -> Url {
    let mut segments = Vec::new();
    for segment in base_url
        .path()
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        if segment == "v1" && segments.last() == Some(&"v1") {
            continue;
        }
        segments.push(segment);
    }
    if !segments.ends_with(suffix) {
        let start = usize::from(
            suffix
                .first()
                .is_some_and(|first| segments.last() == Some(first)),
        );
        for segment in &suffix[start..] {
            segments.push(segment);
        }
    }
    base_url.set_path(&format!("/{}", segments.join("/")));
    base_url
}

fn anthropic_to_responses(
    body: Value,
    capabilities: OpenAiResponsesCapabilities,
) -> Result<Value, BridgeError> {
    let object = body
        .as_object()
        .ok_or_else(|| BridgeError::InvalidRequest("body must be an object".into()))?;
    reject_unknown_fields(
        object,
        &[
            "model",
            "max_tokens",
            "system",
            "messages",
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
    let model = required_non_empty_string(object.get("model"), "model")?;
    let max_tokens = object
        .get("max_tokens")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            BridgeError::InvalidRequest("max_tokens must be a positive integer".into())
        })?;
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(|| BridgeError::InvalidRequest("messages must be a non-empty array".into()))?;

    let mut input = Vec::with_capacity(messages.len());
    for (index, message) in messages.iter().enumerate() {
        let message = message.as_object().ok_or_else(|| {
            BridgeError::InvalidRequest(format!("messages[{index}] must be an object"))
        })?;
        reject_unknown_fields(
            message,
            &["role", "content"],
            Some(&format!("messages[{index}]")),
        )?;
        let role =
            required_non_empty_string(message.get("role"), &format!("messages[{index}].role"))?;
        if role != "user" && role != "assistant" && role != "system" {
            return Err(BridgeError::InvalidRequest(format!(
                "messages[{index}].role must be user, assistant, or system"
            )));
        }
        let items = convert_message_content(
            message.get("content"),
            role,
            &format!("messages[{index}].content"),
            capabilities,
        )?;
        input.extend(items);
    }

    let mut result = json!({
        "model": model,
        "max_output_tokens": max_tokens,
        "input": input
    });
    let reasoning_effort =
        request_hints::validate(object, capabilities.reasoning_items, "reasoning_items")?;
    if capabilities.reasoning_items {
        result["store"] = json!(false);
        result["include"] = json!(["reasoning.encrypted_content"]);
    }
    if let Some(effort) = reasoning_effort {
        result["reasoning"] = json!({"effort":effort});
    }
    if let Some(stream) = object.get("stream") {
        result["stream"] =
            json!(stream.as_bool().ok_or_else(|| {
                BridgeError::InvalidRequest("stream must be a boolean".into())
            })?);
    }
    if let Some(system) = object.get("system") {
        result["instructions"] = json!(convert_system(system)?);
    }
    let has_tools = match object.get("tools") {
        Some(tools) => {
            let values = tools
                .as_array()
                .ok_or_else(|| BridgeError::InvalidRequest("tools must be an array".into()))?;
            if !values.is_empty() {
                result["tools"] = Value::Array(convert_tools(tools)?);
            }
            !values.is_empty()
        }
        None => false,
    };
    if object.contains_key("tool_choice") && !has_tools {
        return Err(BridgeError::InvalidRequest(
            "tool_choice requires a non-empty tools array".into(),
        ));
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        result["tool_choice"] = convert_tool_choice(tool_choice)?;
    }
    Ok(result)
}

fn convert_tools(tools: &Value) -> Result<Vec<Value>, BridgeError> {
    let tools = tools
        .as_array()
        .filter(|tools| !tools.is_empty())
        .ok_or_else(|| BridgeError::InvalidRequest("tools must be a non-empty array".into()))?;
    let mut converted = Vec::with_capacity(tools.len());
    let mut names = HashSet::new();
    for (index, tool) in tools.iter().enumerate() {
        let field = format!("tools[{index}]");
        let tool = tool
            .as_object()
            .ok_or_else(|| BridgeError::InvalidRequest(format!("{field} must be an object")))?;
        reject_unknown_fields(
            tool,
            &["name", "description", "input_schema", "cache_control"],
            Some(&field),
        )?;
        validate_cache_control(tool.get("cache_control"), &field)?;
        let name = required_non_empty_string(tool.get("name"), &format!("{field}.name"))?;
        if !names.insert(name) {
            return Err(BridgeError::InvalidRequest(format!(
                "duplicate tool name: {name}"
            )));
        }
        let schema = tool
            .get("input_schema")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                BridgeError::InvalidRequest(format!("{field}.input_schema must be an object"))
            })?;
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(BridgeError::InvalidRequest(format!(
                "{field}.input_schema.type must be object"
            )));
        }

        let mut response_tool = json!({
            "type": "function",
            "name": name,
            "parameters": schema
        });
        if let Some(description) = tool.get("description") {
            response_tool["description"] = json!(required_non_empty_string(
                Some(description),
                &format!("{field}.description"),
            )?);
        }
        converted.push(response_tool);
    }
    Ok(converted)
}

fn convert_tool_choice(tool_choice: &Value) -> Result<Value, BridgeError> {
    if let Some(choice) = tool_choice.as_str() {
        return match choice {
            "auto" | "none" => Ok(json!(choice)),
            "any" => Ok(json!("required")),
            _ => Err(BridgeError::InvalidRequest(
                "tool_choice must be auto, any, none, or a named tool".into(),
            )),
        };
    }

    let choice = tool_choice.as_object().ok_or_else(|| {
        BridgeError::InvalidRequest("tool_choice must be a string or an object with a type".into())
    })?;
    let choice_type = required_non_empty_string(choice.get("type"), "tool_choice.type")?;
    match choice_type {
        "auto" | "any" | "none" => {
            reject_unknown_fields(choice, &["type"], Some("tool_choice"))?;
            Ok(if choice_type == "any" {
                json!("required")
            } else {
                json!(choice_type)
            })
        }
        "tool" => {
            reject_unknown_fields(choice, &["type", "name"], Some("tool_choice"))?;
            let name = required_non_empty_string(choice.get("name"), "tool_choice.name")?;
            Ok(json!({"type": "function", "name": name}))
        }
        _ => Err(BridgeError::InvalidRequest(
            "tool_choice.type must be auto, any, none, or tool".into(),
        )),
    }
}

fn convert_system(system: &Value) -> Result<String, BridgeError> {
    if let Some(text) = system.as_str() {
        if text.trim().is_empty() {
            return Err(BridgeError::InvalidRequest(
                "system must not be empty".into(),
            ));
        }
        return Ok(text.to_string());
    }

    let blocks = system
        .as_array()
        .filter(|blocks| !blocks.is_empty())
        .ok_or_else(|| {
            BridgeError::InvalidRequest(
                "system must be a string or non-empty text block array".into(),
            )
        })?;
    let mut parts = Vec::with_capacity(blocks.len());
    for (index, block) in blocks.iter().enumerate() {
        let field = format!("system[{index}]");
        let block = block
            .as_object()
            .ok_or_else(|| BridgeError::InvalidRequest(format!("{field} must be an object")))?;
        reject_unknown_fields(block, &["type", "text", "cache_control"], Some(&field))?;
        validate_cache_control(block.get("cache_control"), &field)?;
        if required_non_empty_string(block.get("type"), &format!("{field}.type"))? != "text" {
            return Err(BridgeError::InvalidRequest(format!(
                "{field}.type must be text"
            )));
        }
        parts.push(required_non_empty_string(
            block.get("text"),
            &format!("{field}.text"),
        )?);
    }
    Ok(parts.join("\n\n"))
}

fn convert_message_content(
    content: Option<&Value>,
    role: &str,
    field: &str,
    capabilities: OpenAiResponsesCapabilities,
) -> Result<Vec<Value>, BridgeError> {
    let content_type = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    if let Some(text) = content.and_then(Value::as_str) {
        if text.trim().is_empty() {
            return Err(BridgeError::InvalidRequest(format!(
                "{field} must not be empty"
            )));
        }
        return Ok(vec![json!({
            "role": role,
            "content": [{"type": content_type, "text": text}]
        })]);
    }

    let blocks = content
        .and_then(Value::as_array)
        .filter(|blocks| !blocks.is_empty())
        .ok_or_else(|| {
            BridgeError::InvalidRequest(format!(
                "{field} must be a string or non-empty content block array"
            ))
        })?;
    let mut input = Vec::with_capacity(blocks.len());
    let mut message_content = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let block_field = format!("{field}[{index}]");
        let block = block.as_object().ok_or_else(|| {
            BridgeError::InvalidRequest(format!("{block_field} must be an object"))
        })?;
        let block_type =
            required_non_empty_string(block.get("type"), &format!("{block_field}.type"))?;
        match block_type {
            "text" => {
                reject_unknown_fields(
                    block,
                    &["type", "text", "cache_control"],
                    Some(&block_field),
                )?;
                validate_cache_control(block.get("cache_control"), &block_field)?;
                let text =
                    required_non_empty_string(block.get("text"), &format!("{block_field}.text"))?;
                message_content.push(json!({"type": content_type, "text": text}));
            }
            "tool_use" => {
                if role != "assistant" {
                    return Err(BridgeError::InvalidRequest(format!(
                        "{block_field} tool_use requires assistant role"
                    )));
                }
                reject_unknown_fields(
                    block,
                    &["type", "id", "name", "input", "cache_control"],
                    Some(&block_field),
                )?;
                validate_cache_control(block.get("cache_control"), &block_field)?;
                flush_message_content(&mut input, role, &mut message_content);
                let id = required_non_empty_string(block.get("id"), &format!("{block_field}.id"))?;
                let name =
                    required_non_empty_string(block.get("name"), &format!("{block_field}.name"))?;
                let arguments = block
                    .get("input")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        BridgeError::InvalidRequest(format!(
                            "{block_field}.input must be an object"
                        ))
                    })?;
                input.push(json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": serde_json::to_string(arguments).map_err(|error| {
                        BridgeError::InvalidRequest(format!(
                            "{block_field}.input could not be serialized: {error}"
                        ))
                    })?
                }));
            }
            "tool_result" => {
                if role != "user" {
                    return Err(BridgeError::InvalidRequest(format!(
                        "{block_field} tool_result requires user role"
                    )));
                }
                reject_unknown_fields(
                    block,
                    &[
                        "type",
                        "tool_use_id",
                        "content",
                        "is_error",
                        "cache_control",
                    ],
                    Some(&block_field),
                )?;
                validate_cache_control(block.get("cache_control"), &block_field)?;
                flush_message_content(&mut input, role, &mut message_content);
                let call_id = required_non_empty_string(
                    block.get("tool_use_id"),
                    &format!("{block_field}.tool_use_id"),
                )?;
                let output = convert_tool_result_output(block, &block_field)?;
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output
                }));
            }
            "image" => {
                if role != "user" {
                    return Err(BridgeError::InvalidRequest(format!(
                        "{block_field} image requires user role"
                    )));
                }
                let image_url = media::anthropic_image_url(block, &block_field)?;
                message_content.push(json!({"type":"input_image","image_url":image_url}));
            }
            "document" => {
                if role != "user" {
                    return Err(BridgeError::InvalidRequest(format!(
                        "{block_field} document requires user role"
                    )));
                }
                message_content.push(media::anthropic_document_part(block, &block_field)?);
            }
            "thinking" | "redacted_thinking" => {
                if role != "assistant" {
                    return Err(BridgeError::InvalidRequest(format!(
                        "{block_field} {block_type} requires assistant role"
                    )));
                }
                if !capabilities.reasoning_items {
                    return Err(BridgeError::InvalidRequest(
                        "reasoning history requires the provider reasoning_items capability".into(),
                    ));
                }
                flush_message_content(&mut input, role, &mut message_content);
                let item = reasoning::anthropic_block_to_reasoning_item(block, &block_field)
                    .map_err(BridgeError::InvalidRequest)?;
                input.push(item);
            }
            _ => {
                return Err(BridgeError::InvalidRequest(format!(
                    "{block_field}.type is unsupported: {block_type}"
                )));
            }
        }
    }
    flush_message_content(&mut input, role, &mut message_content);
    Ok(input)
}

fn flush_message_content(input: &mut Vec<Value>, role: &str, content: &mut Vec<Value>) {
    if !content.is_empty() {
        input.push(json!({
            "role": role,
            "content": std::mem::take(content)
        }));
    }
}

fn convert_tool_result_output(
    block: &Map<String, Value>,
    field: &str,
) -> Result<Value, BridgeError> {
    let is_error = match block.get("is_error") {
        Some(Value::Bool(value)) => *value,
        Some(_) => {
            return Err(BridgeError::InvalidRequest(format!(
                "{field}.is_error must be a boolean"
            )));
        }
        None => false,
    };
    let content = block
        .get("content")
        .ok_or_else(|| BridgeError::InvalidRequest(format!("{field}.content is required")))?;
    if let Some(text) = content.as_str() {
        if text.trim().is_empty() {
            return Err(BridgeError::InvalidRequest(format!(
                "{field}.content must not be empty"
            )));
        }
        if !is_error {
            return Ok(json!(text));
        }
        return Ok(json!([
            {"type": "input_text", "text": "[grillforge:tool-result-error]"},
            {"type": "input_text", "text": text}
        ]));
    }

    let blocks = content
        .as_array()
        .filter(|blocks| !blocks.is_empty())
        .ok_or_else(|| {
            BridgeError::InvalidRequest(format!(
                "{field}.content must be a string or non-empty text block array"
            ))
        })?;
    let mut output = Vec::with_capacity(blocks.len() + usize::from(is_error));
    if is_error {
        output.push(json!({"type": "input_text", "text": "[grillforge:tool-result-error]"}));
    }
    for (index, part) in blocks.iter().enumerate() {
        let part_field = format!("{field}.content[{index}]");
        let part = part.as_object().ok_or_else(|| {
            BridgeError::InvalidRequest(format!("{part_field} must be an object"))
        })?;
        match required_non_empty_string(part.get("type"), &format!("{part_field}.type"))? {
            "text" => {
                reject_unknown_fields(part, &["type", "text"], Some(&part_field))?;
                let text =
                    required_non_empty_string(part.get("text"), &format!("{part_field}.text"))?;
                output.push(json!({"type": "input_text", "text": text}));
            }
            "image" => {
                let image_url = media::anthropic_image_url(part, &part_field)?;
                output.push(json!({"type":"input_image","image_url":image_url}));
            }
            "document" => {
                output.push(media::anthropic_document_part(part, &part_field)?);
            }
            other => {
                return Err(BridgeError::InvalidRequest(format!(
                    "{part_field}.type is unsupported: {other}"
                )));
            }
        }
    }
    Ok(Value::Array(output))
}

fn reject_unknown_fields(
    object: &Map<String, Value>,
    allowed: &[&str],
    parent: Option<&str>,
) -> Result<(), BridgeError> {
    for field in object.keys() {
        if !allowed.contains(&field.as_str()) {
            let field = parent
                .map(|parent| format!("{parent}.{field}"))
                .unwrap_or_else(|| field.clone());
            return Err(BridgeError::InvalidRequest(format!(
                "unsupported field: {field}"
            )));
        }
    }
    Ok(())
}

fn responses_to_anthropic(
    body: Value,
    capabilities: OpenAiResponsesCapabilities,
) -> Result<Value, BridgeError> {
    let object = body
        .as_object()
        .ok_or_else(|| BridgeError::InvalidResponse("body must be an object".into()))?;
    let status = required_response_string(object.get("status"), "status")?;
    if status == "failed" || object.get("error").is_some_and(|error| !error.is_null()) {
        return Err(response_envelope_error(object)?);
    }
    if status != "completed" {
        return Err(BridgeError::InvalidResponse(format!(
            "status must be completed, got {status}"
        )));
    }

    let output = object
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| BridgeError::InvalidResponse("output must be an array".into()))?;
    let mut content = Vec::new();
    let mut has_tool_use = false;
    for (item_index, item) in output.iter().enumerate() {
        let item = item.as_object().ok_or_else(|| {
            BridgeError::InvalidResponse(format!("output[{item_index}] must be an object"))
        })?;
        let item_type =
            required_response_string(item.get("type"), &format!("output[{item_index}].type"))?;
        match item_type {
            "message" => append_response_message_content(item, item_index, &mut content)?,
            "function_call" => {
                let field = format!("output[{item_index}]");
                let call_id = required_non_empty_response_string(
                    item.get("call_id"),
                    &format!("{field}.call_id"),
                )?;
                let name =
                    required_non_empty_response_string(item.get("name"), &format!("{field}.name"))?;
                let arguments = required_non_empty_response_string(
                    item.get("arguments"),
                    &format!("{field}.arguments"),
                )?;
                let input: Value = serde_json::from_str(arguments).map_err(|error| {
                    BridgeError::InvalidResponse(format!(
                        "{field}.arguments must be valid JSON: {error}"
                    ))
                })?;
                if !input.is_object() {
                    return Err(BridgeError::InvalidResponse(format!(
                        "{field}.arguments must encode an object"
                    )));
                }
                content.push(json!({
                    "type": "tool_use",
                    "id": call_id,
                    "name": name,
                    "input": input
                }));
                has_tool_use = true;
            }
            "reasoning" => {
                if !capabilities.reasoning_items {
                    return Err(BridgeError::InvalidResponse(
                        "reasoning items require the provider capability".into(),
                    ));
                }
                content.push(
                    reasoning::reasoning_item_to_anthropic_block(&Value::Object(item.clone()))
                        .map_err(BridgeError::InvalidResponse)?,
                );
            }
            _ => {
                return Err(BridgeError::InvalidResponse(format!(
                    "output[{item_index}].type is unsupported: {item_type}"
                )));
            }
        }
    }

    let usage = object
        .get("usage")
        .and_then(Value::as_object)
        .ok_or_else(|| BridgeError::InvalidResponse("usage must be an object".into()))?;
    let usage = convert_usage(usage)?;

    Ok(json!({
        "id": required_response_string(object.get("id"), "id")?,
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": required_response_string(object.get("model"), "model")?,
        "stop_reason": if has_tool_use { "tool_use" } else { "end_turn" },
        "stop_sequence": null,
        "usage": usage
    }))
}

fn response_envelope_error(object: &Map<String, Value>) -> Result<BridgeError, BridgeError> {
    let error = object
        .get("error")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            BridgeError::InvalidResponse("failed response must contain an error object".into())
        })?;
    let kind = error
        .get("type")
        .or_else(|| error.get("code"))
        .and_then(Value::as_str)
        .map(safe_error_kind)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "upstream_error".into());
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .map(|message| safe_error_message(message, 512))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            BridgeError::InvalidResponse(
                "failed response error.message must be a non-empty string".into(),
            )
        })?;
    Ok(BridgeError::UpstreamResponse { kind, message })
}

fn safe_error_kind(value: &str) -> String {
    value
        .chars()
        .take(64)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn safe_error_message(value: &str, limit: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

fn convert_usage(usage: &Map<String, Value>) -> Result<Value, BridgeError> {
    let total_input = required_response_u64(usage.get("input_tokens"), "usage.input_tokens")?;
    let output_tokens = required_response_u64(usage.get("output_tokens"), "usage.output_tokens")?;
    let details = match usage.get("input_tokens_details") {
        Some(Value::Object(details)) => Some(details),
        Some(_) => {
            return Err(BridgeError::InvalidResponse(
                "usage.input_tokens_details must be an object".into(),
            ));
        }
        None => None,
    };
    let cache_read = optional_response_u64(
        usage
            .get("cache_read_input_tokens")
            .or_else(|| details.and_then(|details| details.get("cached_tokens"))),
        "usage cache read tokens",
    )?
    .unwrap_or(0);
    let cache_creation = optional_response_u64(
        usage
            .get("cache_creation_input_tokens")
            .or_else(|| details.and_then(|details| details.get("cache_write_tokens"))),
        "usage cache creation tokens",
    )?
    .unwrap_or(0);
    let cached = cache_read
        .checked_add(cache_creation)
        .ok_or_else(|| BridgeError::InvalidResponse("usage cache token total overflowed".into()))?;
    let input_tokens = total_input.checked_sub(cached).ok_or_else(|| {
        BridgeError::InvalidResponse("usage cache tokens must not exceed total input_tokens".into())
    })?;

    let mut result = json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens
    });
    if cache_read > 0 {
        result["cache_read_input_tokens"] = json!(cache_read);
    }
    if cache_creation > 0 {
        result["cache_creation_input_tokens"] = json!(cache_creation);
    }
    Ok(result)
}

fn append_response_message_content(
    item: &Map<String, Value>,
    item_index: usize,
    content: &mut Vec<Value>,
) -> Result<(), BridgeError> {
    let blocks = item
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            BridgeError::InvalidResponse(format!("output[{item_index}].content must be an array"))
        })?;
    for (block_index, block) in blocks.iter().enumerate() {
        let field = format!("output[{item_index}].content[{block_index}]");
        let block = block
            .as_object()
            .ok_or_else(|| BridgeError::InvalidResponse(format!("{field} must be an object")))?;
        if required_response_string(block.get("type"), &format!("{field}.type"))? != "output_text" {
            return Err(BridgeError::InvalidResponse(format!(
                "{field}.type must be output_text"
            )));
        }
        let text = required_response_string(block.get("text"), &format!("{field}.text"))?;
        content.push(json!({"type": "text", "text": text}));
    }
    Ok(())
}

fn required_non_empty_string<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<&'a str, BridgeError> {
    required_string(value, field).and_then(|value| {
        if value.trim().is_empty() {
            Err(BridgeError::InvalidRequest(format!(
                "{field} must not be empty"
            )))
        } else {
            Ok(value)
        }
    })
}

fn validate_cache_control(value: Option<&Value>, field: &str) -> Result<(), BridgeError> {
    let Some(value) = value else {
        return Ok(());
    };
    let object = value.as_object().ok_or_else(|| {
        BridgeError::InvalidRequest(format!("{field}.cache_control must be an object"))
    })?;
    reject_unknown_fields(
        object,
        &["type", "ttl"],
        Some(&format!("{field}.cache_control")),
    )?;
    if required_non_empty_string(object.get("type"), &format!("{field}.cache_control.type"))?
        != "ephemeral"
    {
        return Err(BridgeError::InvalidRequest(format!(
            "{field}.cache_control.type must be ephemeral"
        )));
    }
    if let Some(ttl) = object.get("ttl") {
        let ttl = required_non_empty_string(Some(ttl), &format!("{field}.cache_control.ttl"))?;
        if ttl != "5m" && ttl != "1h" {
            return Err(BridgeError::InvalidRequest(format!(
                "{field}.cache_control.ttl must be 5m or 1h"
            )));
        }
    }
    Ok(())
}

fn required_string<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .ok_or_else(|| BridgeError::InvalidRequest(format!("{field} must be a string")))
}

fn required_response_string<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<&'a str, BridgeError> {
    value
        .and_then(Value::as_str)
        .ok_or_else(|| BridgeError::InvalidResponse(format!("{field} must be a string")))
}

fn required_non_empty_response_string<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<&'a str, BridgeError> {
    let value = required_response_string(value, field)?;
    if value.trim().is_empty() {
        Err(BridgeError::InvalidResponse(format!(
            "{field} must not be empty"
        )))
    } else {
        Ok(value)
    }
}

fn required_response_u64(value: Option<&Value>, field: &str) -> Result<u64, BridgeError> {
    value
        .and_then(Value::as_u64)
        .ok_or_else(|| BridgeError::InvalidResponse(format!("{field} must be an unsigned integer")))
}

fn optional_response_u64(value: Option<&Value>, field: &str) -> Result<Option<u64>, BridgeError> {
    value
        .map(|value| {
            value.as_u64().ok_or_else(|| {
                BridgeError::InvalidResponse(format!("{field} must be an unsigned integer"))
            })
        })
        .transpose()
}
