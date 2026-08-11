// Image and document conversion shapes adapted from cc-switch, commit
// 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::BridgeError;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use url::Url;

const IMAGE_MIME_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/gif", "image/webp"];
const MAX_DOCUMENT_BYTES: usize = 32 * 1024 * 1024;
const MAX_DOCUMENT_BASE64_BYTES: usize = ((MAX_DOCUMENT_BYTES + 2) / 3) * 4;
const MAX_URL_BYTES: usize = 8 * 1024;
const MAX_FILENAME_BYTES: usize = 255;

pub(crate) fn anthropic_image_url(
    block: &Map<String, Value>,
    field: &str,
) -> Result<String, BridgeError> {
    reject_unknown(block, &["type", "source"], field)?;
    let source = block
        .get("source")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(&format!("{field}.source must be an object")))?;
    let source_type = source
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(&format!("{field}.source.type must be a string")))?;
    match source_type {
        "base64" => {
            reject_unknown(
                source,
                &["type", "media_type", "data"],
                &format!("{field}.source"),
            )?;
            let media_type = source
                .get("media_type")
                .and_then(Value::as_str)
                .filter(|media_type| IMAGE_MIME_TYPES.contains(media_type))
                .ok_or_else(|| invalid(&format!(
                    "{field}.source.media_type must be image/jpeg, image/png, image/gif, or image/webp"
                )))?;
            let data = source
                .get("data")
                .and_then(Value::as_str)
                .filter(|data| !data.is_empty())
                .ok_or_else(|| {
                    invalid(&format!("{field}.source.data must be a non-empty string"))
                })?;
            let decoded = STANDARD.decode(data).map_err(|_| {
                invalid(&format!(
                    "{field}.source.data must be valid canonical base64"
                ))
            })?;
            if decoded.is_empty() || STANDARD.encode(&decoded) != data {
                return Err(invalid(&format!(
                    "{field}.source.data must be valid canonical base64"
                )));
            }
            Ok(format!("data:{media_type};base64,{data}"))
        }
        "url" => {
            reject_unknown(source, &["type", "url"], &format!("{field}.source"))?;
            let raw = source
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
                .ok_or_else(|| {
                    invalid(&format!("{field}.source.url must be a non-empty string"))
                })?;
            let url = Url::parse(raw)
                .map_err(|_| invalid(&format!("{field}.source.url must be a valid URL")))?;
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err(invalid(&format!(
                    "{field}.source.url must use http or https"
                )));
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err(invalid(&format!(
                    "{field}.source.url must not contain credentials"
                )));
            }
            Ok(raw.to_owned())
        }
        _ => Err(invalid(&format!(
            "{field}.source.type must be base64 or url"
        ))),
    }
}

pub(crate) fn anthropic_document_part(
    block: &Map<String, Value>,
    field: &str,
) -> Result<Value, BridgeError> {
    reject_unknown(block, &["type", "source", "title", "filename"], field)?;
    if block.contains_key("title") && block.contains_key("filename") {
        return Err(invalid(&format!(
            "{field} must not contain both title and filename"
        )));
    }
    let filename = block
        .get("title")
        .or_else(|| block.get("filename"))
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| invalid(&format!("{field} filename must be a non-empty string")))
        })
        .transpose()?
        .unwrap_or("document.pdf");
    if filename.len() > MAX_FILENAME_BYTES
        || filename
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(invalid(&format!(
            "{field} filename must be at most {MAX_FILENAME_BYTES} bytes and contain no path or control characters"
        )));
    }

    let source = block
        .get("source")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(&format!("{field}.source must be an object")))?;
    match source.get("type").and_then(Value::as_str) {
        Some("base64") => {
            reject_unknown(
                source,
                &["type", "media_type", "data"],
                &format!("{field}.source"),
            )?;
            if source.get("media_type").and_then(Value::as_str) != Some("application/pdf") {
                return Err(invalid(&format!(
                    "{field}.source.media_type must be application/pdf"
                )));
            }
            let data = source
                .get("data")
                .and_then(Value::as_str)
                .filter(|data| !data.is_empty())
                .ok_or_else(|| {
                    invalid(&format!("{field}.source.data must be a non-empty string"))
                })?;
            if data.len() > MAX_DOCUMENT_BASE64_BYTES {
                return Err(invalid(&format!(
                    "{field}.source.data exceeds the {MAX_DOCUMENT_BYTES}-byte document limit"
                )));
            }
            let decoded = STANDARD.decode(data).map_err(|_| {
                invalid(&format!(
                    "{field}.source.data must be valid canonical base64"
                ))
            })?;
            if decoded.is_empty() || STANDARD.encode(&decoded) != data {
                return Err(invalid(&format!(
                    "{field}.source.data must be valid canonical base64"
                )));
            }
            if decoded.len() > MAX_DOCUMENT_BYTES {
                return Err(invalid(&format!(
                    "{field}.source.data exceeds the {MAX_DOCUMENT_BYTES}-byte document limit"
                )));
            }
            if !decoded.starts_with(b"%PDF-") {
                return Err(invalid(&format!(
                    "{field}.source.data must contain a PDF document"
                )));
            }
            Ok(json!({
                "type":"input_file",
                "file_data":format!("data:application/pdf;base64,{data}"),
                "filename":filename
            }))
        }
        Some("url") => {
            reject_unknown(source, &["type", "url"], &format!("{field}.source"))?;
            let raw = source
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
                .ok_or_else(|| {
                    invalid(&format!("{field}.source.url must be a non-empty string"))
                })?;
            if raw.len() > MAX_URL_BYTES {
                return Err(invalid(&format!(
                    "{field}.source.url must be at most {MAX_URL_BYTES} bytes"
                )));
            }
            let url = Url::parse(raw)
                .map_err(|_| invalid(&format!("{field}.source.url must be a valid URL")))?;
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err(invalid(&format!(
                    "{field}.source.url must use http or https"
                )));
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err(invalid(&format!(
                    "{field}.source.url must not contain credentials"
                )));
            }
            Ok(json!({"type":"input_file","file_url":raw,"filename":filename}))
        }
        Some(_) | None => Err(invalid(&format!(
            "{field}.source.type must be base64 or url"
        ))),
    }
}

fn reject_unknown(
    object: &Map<String, Value>,
    allowed: &[&str],
    field: &str,
) -> Result<(), BridgeError> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid(&format!("unsupported field: {field}.{key}")));
    }
    Ok(())
}

fn invalid(message: &str) -> BridgeError {
    BridgeError::InvalidRequest(message.into())
}
