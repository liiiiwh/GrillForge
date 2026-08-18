use super::BridgeError;
use serde_json::{Map, Value};

pub(crate) fn validate(
    body: &Map<String, Value>,
    supports_reasoning_effort: bool,
    capability_name: &str,
) -> Result<Option<String>, BridgeError> {
    if let Some(metadata) = body.get("metadata") {
        validate_metadata(metadata)?;
    }
    if let Some(context) = body.get("context_management") {
        validate_context_management(context)?;
    }
    let output_effort = body
        .get("output_config")
        .map(validate_output_config)
        .transpose()?
        .flatten();
    let thinking_enabled = body
        .get("thinking")
        .map(validate_thinking)
        .transpose()?
        .unwrap_or(false);
    if (output_effort.is_some() || thinking_enabled) && !supports_reasoning_effort {
        return Err(invalid(&format!(
            "output_config/thinking require the provider {capability_name} capability"
        )));
    }
    if thinking_enabled && output_effort.is_none() {
        return Err(invalid(
            "adaptive thinking requires output_config.effort for an exact OpenAI mapping",
        ));
    }
    Ok(output_effort)
}

fn validate_metadata(value: &Value) -> Result<(), BridgeError> {
    let metadata = value
        .as_object()
        .ok_or_else(|| invalid("metadata must be an object"))?;
    reject_unknown(metadata, &["user_id"], "metadata")?;
    metadata
        .get("user_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid("metadata.user_id must be a non-empty string"))?;
    Ok(())
}

fn validate_context_management(value: &Value) -> Result<(), BridgeError> {
    let context = value
        .as_object()
        .ok_or_else(|| invalid("context_management must be an object"))?;
    reject_unknown(context, &["edits"], "context_management")?;
    let edits = context
        .get("edits")
        .and_then(Value::as_array)
        .filter(|edits| !edits.is_empty())
        .ok_or_else(|| invalid("context_management.edits must be a non-empty array"))?;
    for (index, edit) in edits.iter().enumerate() {
        let field = format!("context_management.edits[{index}]");
        let edit = edit
            .as_object()
            .ok_or_else(|| invalid(&format!("{field} must be an object")))?;
        reject_unknown(edit, &["type", "keep"], &field)?;
        if edit.get("type").and_then(Value::as_str) != Some("clear_thinking_20251015") {
            return Err(invalid(&format!(
                "{field}.type must be clear_thinking_20251015"
            )));
        }
        if edit.get("keep").and_then(Value::as_str) != Some("all") {
            return Err(invalid(&format!("{field}.keep must be all")));
        }
    }
    Ok(())
}

fn validate_output_config(value: &Value) -> Result<Option<String>, BridgeError> {
    let config = value
        .as_object()
        .ok_or_else(|| invalid("output_config must be an object"))?;
    reject_unknown(config, &["effort"], "output_config")?;
    let effort = config
        .get("effort")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("output_config.effort must be a string"))?;
    match effort {
        "low" | "medium" | "high" | "xhigh" => Ok(Some(effort.to_owned())),
        "max" => Ok(Some("xhigh".into())),
        _ => Err(invalid(
            "output_config.effort must be low, medium, high, xhigh, or max",
        )),
    }
}

fn validate_thinking(value: &Value) -> Result<bool, BridgeError> {
    let thinking = value
        .as_object()
        .ok_or_else(|| invalid("thinking must be an object"))?;
    let kind = thinking
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("thinking.type must be a string"))?;
    match kind {
        "adaptive" => {
            reject_unknown(thinking, &["type", "display"], "thinking")?;
            // An absent `display` already means omitted thinking, which is the only
            // response shape a bridged provider can produce. Claude Code sends
            // adaptive thinking without the field.
            if let Some(display) = thinking.get("display") {
                if display.as_str() != Some("omitted") {
                    return Err(invalid("thinking.display must be omitted"));
                }
            }
            Ok(true)
        }
        "disabled" => {
            reject_unknown(thinking, &["type"], "thinking")?;
            Ok(false)
        }
        _ => Err(invalid("thinking.type must be adaptive or disabled")),
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
