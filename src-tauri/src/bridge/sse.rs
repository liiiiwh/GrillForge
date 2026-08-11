// Adapted from cc-switch, commit 413c09e0790c304506888ae24b9be72820aca126.
// Copyright (c) 2025 Jason Young. Licensed under the MIT License.

use super::BridgeError;

pub(crate) fn take_sse_block(buffer: &mut String) -> Option<String> {
    let delimiter = [("\r\n\r\n", 4), ("\n\n", 2)]
        .into_iter()
        .filter_map(|(delimiter, length)| buffer.find(delimiter).map(|index| (index, length)))
        .min_by_key(|(index, _)| *index)?;
    let block = buffer[..delimiter.0].to_owned();
    buffer.drain(..delimiter.0 + delimiter.1);
    Some(block)
}

pub(crate) fn append_utf8(
    buffer: &mut String,
    remainder: &mut Vec<u8>,
    chunk: &[u8],
) -> Result<(), BridgeError> {
    let mut input = std::mem::take(remainder);
    input.extend_from_slice(chunk);
    match std::str::from_utf8(&input) {
        Ok(text) => buffer.push_str(text),
        Err(error) => {
            let valid = error.valid_up_to();
            buffer.push_str(std::str::from_utf8(&input[..valid]).expect("validated UTF-8 prefix"));
            if error.error_len().is_some() {
                return Err(BridgeError::InvalidResponse(
                    "Responses SSE contained invalid UTF-8".into(),
                ));
            }
            remainder.extend_from_slice(&input[valid..]);
        }
    }
    Ok(())
}

pub(crate) fn parse_sse_block(block: &str) -> Result<(&str, &str), BridgeError> {
    let mut event = None;
    let mut data = None;
    for raw_line in block.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let (field, value) = line.split_once(':').ok_or_else(|| {
            BridgeError::InvalidResponse("Responses SSE line must contain ':'".into())
        })?;
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" if event.replace(value).is_none() => {}
            "data" if data.replace(value).is_none() => {}
            "event" | "data" => {
                return Err(BridgeError::InvalidResponse(format!(
                    "Responses SSE block contains duplicate {field} field"
                )));
            }
            _ => {
                return Err(BridgeError::InvalidResponse(format!(
                    "Responses SSE field {field} is unsupported"
                )));
            }
        }
    }
    let event = event.filter(|value| !value.is_empty()).ok_or_else(|| {
        BridgeError::InvalidResponse("Responses SSE block is missing event".into())
    })?;
    let data = data.filter(|value| !value.is_empty()).ok_or_else(|| {
        BridgeError::InvalidResponse("Responses SSE block is missing data".into())
    })?;
    Ok((event, data))
}

pub(crate) fn parse_data_sse_block(block: &str) -> Result<&str, String> {
    let mut data = None;
    for raw_line in block.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let (field, value) = line
            .split_once(':')
            .ok_or_else(|| "Chat SSE line must contain ':'".to_string())?;
        let value = value.strip_prefix(' ').unwrap_or(value);
        if field != "data" {
            return Err(format!("Chat SSE field {field} is unsupported"));
        }
        if data.replace(value).is_some() {
            return Err("Chat SSE block contains duplicate data field".into());
        }
    }
    data.filter(|value| !value.is_empty())
        .ok_or_else(|| "Chat SSE block is missing data".into())
}
