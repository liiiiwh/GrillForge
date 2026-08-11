// Minimal Gemini Native adapter derived from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::{AnthropicSseStream, BridgeError, streaming_gemini::gemini_sse_to_anthropic};
use base64::Engine as _;
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
