//! Provider model discovery, adapted from cc-switch's bounded `/models` slice.
//! An explicit `models_url` wins. Otherwise GrillForge tries cc-switch's small,
//! deterministic candidate set and falls back to a matching preset's pinned
//! model IDs only when every listing endpoint is absent.

use crate::configuration::{ProviderProtocolEndpoint, ProviderRecord};
use crate::core::model::{NativeProtocol, ProtocolCapability};
use crate::core::provider::{ApiKeyPlacement, EndpointMode, Protocol, build_request_endpoint};
use reqwest::header::{AUTHORIZATION, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;
use url::Url;

const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const COMPAT_SUFFIXES: &[&str] = &[
    "/api/claudecode",
    "/api/anthropic",
    "/apps/anthropic",
    "/api/coding",
    "/claudecode",
    "/anthropic",
    "/step_plan",
    "/coding",
    "/claude",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    pub id: String,
    pub owned_by: Option<String>,
    #[serde(default)]
    pub native_protocols: Vec<NativeProtocol>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelProtocolProbe {
    pub supported: Vec<NativeProtocol>,
    pub protocol_capabilities: Vec<ProtocolCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolProbeSummary {
    pub provider_endpoints: Vec<ProviderProtocolEndpoint>,
    pub models: BTreeMap<String, ModelProtocolProbe>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    owned_by: Option<String>,
    #[serde(default, alias = "protocols")]
    supported_protocols: Vec<NativeProtocol>,
}

#[derive(Deserialize)]
struct GeminiModelsResponse {
    models: Vec<GeminiModelEntry>,
}

#[derive(Deserialize)]
struct GeminiModelEntry {
    name: String,
}

pub async fn discover(provider: &ProviderRecord) -> Result<Vec<DiscoveredModel>, String> {
    if !provider.enabled {
        return Err(format!("provider {} is disabled", provider.id));
    }
    let endpoints = models_endpoints(provider)?;
    let client = reqwest::Client::builder()
        .connect_timeout(FETCH_TIMEOUT)
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|error| format!("could not create model discovery client: {error}"))?;
    let mut body = None;
    let mut last_missing = None;
    for endpoint in endpoints {
        let mut request = client.get(endpoint.clone());
        request = match (provider.protocol, provider.api_key_placement) {
            (Protocol::GeminiNative, ApiKeyPlacement::XApiKey) => request.header(
                HeaderName::from_static("x-goog-api-key"),
                HeaderValue::from_str(&provider.api_key).map_err(|_| {
                    "provider API key contains invalid header characters".to_string()
                })?,
            ),
            (_, ApiKeyPlacement::None) => request,
            (_, ApiKeyPlacement::Bearer) => request.header(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", provider.api_key)).map_err(|_| {
                    "provider API key contains invalid header characters".to_string()
                })?,
            ),
            (_, ApiKeyPlacement::XApiKey) => request.header(
                HeaderName::from_static("x-api-key"),
                HeaderValue::from_str(&provider.api_key).map_err(|_| {
                    "provider API key contains invalid header characters".to_string()
                })?,
            ),
        };
        let response = request
            .send()
            .await
            .map_err(|error| format!("model discovery request failed: {error}"))?;
        let status = response.status();
        if status.is_success() {
            body = Some(
                response
                    .bytes()
                    .await
                    .map_err(|_| "model discovery response body could not be read".to_string())?,
            );
            break;
        }
        if matches!(status.as_u16(), 404 | 405) {
            last_missing = Some(format!(
                "model discovery returned HTTP {} from {endpoint}",
                status.as_u16()
            ));
            continue;
        }
        return Err(format!(
            "model discovery returned HTTP {} from {endpoint}",
            status.as_u16()
        ));
    }
    let Some(body) = body else {
        let suggested = matching_preset(provider)?
            .map(|preset| preset.suggested_models)
            .unwrap_or_default();
        if suggested.is_empty() {
            return Err(last_missing
                .unwrap_or_else(|| "provider has no usable model discovery endpoint".into()));
        }
        return discovered_from_suggestions(suggested);
    };
    let entries = if provider.protocol == Protocol::GeminiNative {
        serde_json::from_slice::<GeminiModelsResponse>(&body)
            .map_err(|_| "Gemini model discovery returned invalid JSON data".to_string())?
            .models
            .into_iter()
            .map(|entry| ModelEntry {
                id: entry
                    .name
                    .strip_prefix("models/")
                    .unwrap_or(&entry.name)
                    .to_string(),
                owned_by: Some("google".into()),
                supported_protocols: vec![NativeProtocol::GeminiNative],
            })
            .collect()
    } else {
        serde_json::from_slice::<ModelsResponse>(&body)
            .map_err(|_| "model discovery returned invalid JSON data".to_string())?
            .data
    };
    let mut seen = HashSet::new();
    let mut models = Vec::with_capacity(entries.len());
    for entry in entries {
        let id = entry.id.trim();
        if id.is_empty() || id.chars().any(char::is_control) {
            return Err("model discovery returned an invalid model ID".to_string());
        }
        if seen.insert(id.to_string()) {
            models.push(DiscoveredModel {
                id: id.to_string(),
                owned_by: entry.owned_by,
                native_protocols: entry.supported_protocols,
            });
        }
    }
    models.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(models)
}

pub(crate) fn matching_preset(
    provider: &ProviderRecord,
) -> Result<Option<crate::presets::ProviderPreset>, String> {
    let catalog = crate::presets::catalog()
        .map_err(|_| "built-in Provider catalog is invalid".to_string())?;
    if let Some(preset) = catalog
        .presets
        .iter()
        .find(|preset| preset.id == provider.id)
    {
        return Ok(Some(preset.clone()));
    }
    let endpoint = provider.endpoint.trim_end_matches('/');
    Ok(catalog.presets.into_iter().find(|preset| {
        let protocol_matches = matches!(
            (preset.protocol, provider.protocol),
            (
                crate::presets::PresetProtocol::AnthropicMessages,
                Protocol::AnthropicMessages
            ) | (
                crate::presets::PresetProtocol::OpenAiResponses,
                Protocol::OpenAiResponses
            ) | (
                crate::presets::PresetProtocol::OpenAiChatCompletions,
                Protocol::OpenAiChatCompletions
            ) | (
                crate::presets::PresetProtocol::GeminiNative,
                Protocol::GeminiNative
            )
        );
        protocol_matches
            && matches!(
                &preset.endpoint,
                crate::presets::PresetEndpoint::Literal { url }
                    if url.trim_end_matches('/') == endpoint
            )
    }))
}

fn discovered_from_suggestions(models: Vec<String>) -> Result<Vec<DiscoveredModel>, String> {
    let mut seen = HashSet::new();
    let mut discovered = Vec::new();
    for model in models {
        let id = model.trim();
        if id.is_empty() || id.chars().any(char::is_control) {
            return Err("built-in Provider preset contains an invalid model ID".into());
        }
        if seen.insert(id.to_string()) {
            discovered.push(DiscoveredModel {
                id: id.into(),
                owned_by: None,
                native_protocols: Vec::new(),
            });
        }
    }
    discovered.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(discovered)
}

pub async fn probe_protocols(
    provider: &ProviderRecord,
    models: &[DiscoveredModel],
) -> Result<ProtocolProbeSummary, String> {
    if !provider.enabled {
        return Err(format!("provider {} is disabled", provider.id));
    }
    let endpoints = protocol_probe_endpoints(provider)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("could not create protocol probe client: {error}"))?;
    let mut supported_by_model = BTreeMap::<String, Vec<NativeProtocol>>::new();
    let mut capabilities_by_model = BTreeMap::<String, Vec<ProtocolCapability>>::new();
    let mut provider_supported = HashSet::new();

    for endpoint in &endpoints {
        for model in models {
            if let Some(capabilities) =
                probe_model_protocol(&client, provider, endpoint, &model.id).await?
            {
                provider_supported.insert(endpoint.protocol);
                supported_by_model
                    .entry(model.id.clone())
                    .or_default()
                    .push(endpoint.protocol);
                let observed = capabilities_by_model.entry(model.id.clone()).or_default();
                for capability in capabilities {
                    if !observed.contains(&capability) {
                        observed.push(capability);
                    }
                }
            }
        }
    }

    let provider_endpoints = endpoints
        .into_iter()
        .filter(|entry| provider_supported.contains(&entry.protocol))
        .collect::<Vec<_>>();
    let models = models
        .iter()
        .map(|model| {
            (
                model.id.clone(),
                ModelProtocolProbe {
                    supported: supported_by_model.remove(&model.id).unwrap_or_default(),
                    protocol_capabilities: capabilities_by_model
                        .remove(&model.id)
                        .unwrap_or_default(),
                },
            )
        })
        .collect();
    Ok(ProtocolProbeSummary {
        provider_endpoints,
        models,
    })
}

fn protocol_probe_endpoints(
    provider: &ProviderRecord,
) -> Result<Vec<ProviderProtocolEndpoint>, String> {
    let base = Url::parse(&provider.endpoint)
        .map_err(|_| format!("provider {} has an invalid endpoint", provider.id))?;
    let mut endpoints = [
        NativeProtocol::AnthropicMessages,
        NativeProtocol::OpenAiResponses,
        NativeProtocol::OpenAiChat,
        NativeProtocol::GeminiNative,
    ]
    .into_iter()
    .map(|protocol| ProviderProtocolEndpoint {
        protocol,
        endpoint: provider.endpoint.clone(),
        endpoint_mode: provider.endpoint_mode,
        api_key_placement: provider.api_key_placement,
    })
    .collect::<Vec<_>>();
    for configured in &provider.protocol_endpoints {
        if let Some(entry) = endpoints
            .iter_mut()
            .find(|entry| entry.protocol == configured.protocol)
        {
            *entry = configured.clone();
        }
    }

    // cc-switch keeps protocol-specific variants as sibling presets. When the
    // Provider still uses the exact preset endpoint, reuse those verified
    // surfaces for probing instead of guessing that every protocol shares one
    // URL. A user-edited endpoint remains authoritative and is never replaced.
    if let Some(selected) = matching_preset(provider)? {
        if preset_literal_endpoint(&selected)
            .is_some_and(|endpoint| same_endpoint(endpoint, &provider.endpoint))
        {
            let family = selected.name.split(" · ").next().unwrap_or(&selected.name);
            let catalog = crate::presets::catalog()
                .map_err(|_| "built-in Provider catalog is invalid".to_string())?;
            for preset in catalog
                .presets
                .iter()
                .filter(|preset| preset.name.split(" · ").next() == Some(family))
            {
                let Some(endpoint) = preset_literal_endpoint(preset) else {
                    continue;
                };
                let protocol = preset_native_protocol(preset.protocol);
                if let Some(surface) = endpoints
                    .iter_mut()
                    .find(|surface| surface.protocol == protocol)
                {
                    surface.endpoint = endpoint.to_string();
                    surface.endpoint_mode = EndpointMode::BaseUrl;
                    surface.api_key_placement = preset_api_key_placement(preset.auth);
                }
            }
        }
    }

    // DeepSeek exposes its Anthropic-compatible surface under a distinct base
    // path. Keep this one verified upstream fact next to the probe rather than
    // teaching the generic router provider-specific URL heuristics.
    if base.host_str() == Some("api.deepseek.com") {
        let anthropic = endpoints
            .iter_mut()
            .find(|entry| entry.protocol == NativeProtocol::AnthropicMessages)
            .expect("static protocol endpoint");
        if anthropic.endpoint == provider.endpoint {
            let mut endpoint = base.clone();
            endpoint.set_path("/anthropic");
            endpoint.set_query(None);
            endpoint.set_fragment(None);
            anthropic.endpoint = endpoint.to_string().trim_end_matches('/').to_string();
            anthropic.endpoint_mode = EndpointMode::BaseUrl;
        }
    }
    Ok(endpoints)
}

fn preset_literal_endpoint(preset: &crate::presets::ProviderPreset) -> Option<&str> {
    match &preset.endpoint {
        crate::presets::PresetEndpoint::Literal { url } => Some(url),
        crate::presets::PresetEndpoint::Parameterized { .. } => None,
    }
}

fn same_endpoint(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}

fn preset_native_protocol(protocol: crate::presets::PresetProtocol) -> NativeProtocol {
    match protocol {
        crate::presets::PresetProtocol::AnthropicMessages => NativeProtocol::AnthropicMessages,
        crate::presets::PresetProtocol::OpenAiResponses => NativeProtocol::OpenAiResponses,
        crate::presets::PresetProtocol::OpenAiChatCompletions => NativeProtocol::OpenAiChat,
        crate::presets::PresetProtocol::GeminiNative => NativeProtocol::GeminiNative,
    }
}

fn preset_api_key_placement(auth: crate::presets::PresetAuth) -> ApiKeyPlacement {
    match auth {
        crate::presets::PresetAuth::Bearer => ApiKeyPlacement::Bearer,
        crate::presets::PresetAuth::XApiKey => ApiKeyPlacement::XApiKey,
    }
}

async fn probe_model_protocol(
    client: &reqwest::Client,
    provider: &ProviderRecord,
    surface: &ProviderProtocolEndpoint,
    model: &str,
) -> Result<Option<Vec<ProtocolCapability>>, String> {
    let base = Url::parse(&surface.endpoint).map_err(|_| {
        format!(
            "provider {} has an invalid {:?} endpoint",
            provider.id, surface.protocol
        )
    })?;
    let (endpoint, body) = match surface.protocol {
        NativeProtocol::AnthropicMessages => (
            build_request_endpoint(&base, surface.endpoint_mode, "/v1/messages").map_err(|_| {
                format!("provider {} has an invalid Anthropic endpoint", provider.id)
            })?,
            json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "Reply with OK."}]
            }),
        ),
        NativeProtocol::OpenAiResponses => (
            build_request_endpoint(&base, surface.endpoint_mode, "/v1/responses").map_err(
                |_| format!("provider {} has an invalid Responses endpoint", provider.id),
            )?,
            json!({
                "model": model,
                "max_output_tokens": 1,
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "Reply with OK."}]}]
            }),
        ),
        NativeProtocol::OpenAiChat => (
            build_request_endpoint(&base, surface.endpoint_mode, "/v1/chat/completions")
                .map_err(|_| format!("provider {} has an invalid Chat endpoint", provider.id))?,
            json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "Reply with OK."}]
            }),
        ),
        NativeProtocol::GeminiNative => (
            gemini_probe_endpoint(&base, surface.endpoint_mode, model)?,
            json!({
                "contents": [{"role": "user", "parts": [{"text": "Reply with OK."}]}],
                "generationConfig": {"maxOutputTokens": 1}
            }),
        ),
    };
    let mut request = client.post(endpoint).json(&body);
    if surface.protocol == NativeProtocol::AnthropicMessages {
        request = request.header("anthropic-version", "2023-06-01");
    }
    request = apply_probe_auth(request, provider, surface)?;
    let response = match request.send().await {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    let status = response.status();
    let primary_protocol = match provider.protocol {
        Protocol::AnthropicMessages => NativeProtocol::AnthropicMessages,
        Protocol::OpenAiResponses => NativeProtocol::OpenAiResponses,
        Protocol::OpenAiChatCompletions => NativeProtocol::OpenAiChat,
        Protocol::GeminiNative => NativeProtocol::GeminiNative,
    };
    if matches!(status.as_u16(), 401 | 403) && surface.protocol == primary_protocol {
        return Err(format!(
            "provider {} authentication failed while probing {:?} (HTTP {})",
            provider.id,
            surface.protocol,
            status.as_u16()
        ));
    }
    if status.as_u16() == 429 && surface.protocol == primary_protocol {
        return Err(format!(
            "provider {} quota blocked the {:?} protocol probe (HTTP 429)",
            provider.id, surface.protocol
        ));
    }
    if !status.is_success() {
        return Ok(None);
    }
    let body = match response.json::<Value>().await {
        Ok(body) => body,
        Err(_) => return Ok(None),
    };
    if !valid_protocol_response(surface.protocol, &body) {
        return Ok(None);
    }
    Ok(Some(observed_protocol_capabilities(
        surface.protocol,
        &body,
    )))
}

fn apply_probe_auth(
    request: reqwest::RequestBuilder,
    provider: &ProviderRecord,
    surface: &ProviderProtocolEndpoint,
) -> Result<reqwest::RequestBuilder, String> {
    match surface.api_key_placement {
        ApiKeyPlacement::None => Ok(request),
        ApiKeyPlacement::Bearer => {
            let value = HeaderValue::from_str(&format!("Bearer {}", provider.api_key))
                .map_err(|_| "provider API key contains invalid header characters".to_string())?;
            Ok(request.header(AUTHORIZATION, value))
        }
        ApiKeyPlacement::XApiKey => {
            let name = if surface.protocol == NativeProtocol::GeminiNative {
                HeaderName::from_static("x-goog-api-key")
            } else {
                HeaderName::from_static("x-api-key")
            };
            let value = HeaderValue::from_str(&provider.api_key)
                .map_err(|_| "provider API key contains invalid header characters".to_string())?;
            Ok(request.header(name, value))
        }
    }
}

fn gemini_probe_endpoint(base: &Url, mode: EndpointMode, model: &str) -> Result<Url, String> {
    if mode == EndpointMode::ExactUrl {
        return Ok(base.clone());
    }
    if model.is_empty() || model.chars().any(char::is_control) {
        return Err("model discovery returned an invalid model ID".into());
    }
    let model = model.strip_prefix("models/").unwrap_or(model);
    let mut endpoint = base.clone();
    let path = endpoint.path().trim_end_matches('/');
    let prefix = if path.ends_with("/v1beta") {
        path.to_string()
    } else {
        format!("{path}/v1beta")
    };
    endpoint.set_path(&format!("{prefix}/models/{model}:generateContent"));
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    Ok(endpoint)
}

fn valid_protocol_response(protocol: NativeProtocol, body: &Value) -> bool {
    match protocol {
        NativeProtocol::AnthropicMessages => {
            body.get("type").and_then(Value::as_str) == Some("message")
                && body.get("content").is_some_and(Value::is_array)
        }
        NativeProtocol::OpenAiResponses => {
            body.get("object").and_then(Value::as_str) == Some("response")
                && matches!(
                    body.get("status").and_then(Value::as_str),
                    Some("completed" | "incomplete")
                )
        }
        NativeProtocol::OpenAiChat => body.get("choices").is_some_and(Value::is_array),
        NativeProtocol::GeminiNative => body.get("candidates").is_some_and(Value::is_array),
    }
}

fn observed_protocol_capabilities(
    protocol: NativeProtocol,
    body: &Value,
) -> Vec<ProtocolCapability> {
    match protocol {
        NativeProtocol::OpenAiChat
            if body
                .pointer("/choices/0/message/reasoning_content")
                .is_some_and(|value| !value.is_null()) =>
        {
            vec![ProtocolCapability::ReasoningContent]
        }
        NativeProtocol::OpenAiResponses
            if body
                .get("output")
                .and_then(Value::as_array)
                .is_some_and(|output| {
                    output.iter().any(|item| {
                        matches!(
                            item.get("type").and_then(Value::as_str),
                            Some("reasoning" | "reasoning_item")
                        )
                    })
                }) =>
        {
            vec![ProtocolCapability::ReasoningItems]
        }
        _ => Vec::new(),
    }
}

fn models_endpoints(provider: &ProviderRecord) -> Result<Vec<Url>, String> {
    if let Some(models_url) = provider.models_url.as_deref() {
        return Url::parse(models_url)
            .map(|url| vec![url])
            .map_err(|_| format!("provider {} has an invalid models URL", provider.id));
    }
    let mut endpoint = Url::parse(&provider.endpoint)
        .map_err(|_| format!("provider {} has an invalid endpoint", provider.id))?;
    let path = endpoint.path().trim_end_matches('/');
    if provider.protocol == Protocol::GeminiNative {
        let models_path = if path.ends_with("/v1beta") {
            format!("{path}/models")
        } else {
            format!("{path}/v1beta/models")
        };
        endpoint.set_path(&models_path);
        return Ok(vec![endpoint]);
    }
    let paths = match provider.endpoint_mode {
        EndpointMode::ExactUrl => {
            if let Some(index) = path.find("/v1/") {
                vec![format!("{}/v1/models", &path[..index])]
            } else {
                let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
                vec![format!("{parent}/v1/models")]
            }
        }
        EndpointMode::BaseUrl => {
            if let Some(root) = COMPAT_SUFFIXES
                .iter()
                .find_map(|suffix| path.strip_suffix(suffix))
            {
                vec![
                    format!("{path}/v1/models"),
                    format!("{root}/v1/models"),
                    format!("{root}/models"),
                ]
            } else if is_version_path(path) {
                let mut paths = vec![format!("{path}/models")];
                if !path.ends_with("/v1") {
                    paths.push(format!("{path}/v1/models"));
                }
                paths
            } else {
                vec![format!("{path}/v1/models")]
            }
        }
    };
    let mut endpoints = Vec::new();
    for path in paths {
        let mut candidate = endpoint.clone();
        candidate.set_path(&path);
        candidate.set_query(None);
        candidate.set_fragment(None);
        if !endpoints.contains(&candidate) {
            endpoints.push(candidate);
        }
    }
    Ok(endpoints)
}

fn is_version_path(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|segment| {
        segment.strip_prefix('v').is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(endpoint: &str, mode: EndpointMode) -> ProviderRecord {
        ProviderRecord {
            id: "test".into(),
            name: "Test".into(),
            enabled: true,
            protocol: Protocol::AnthropicMessages,
            endpoint: endpoint.into(),
            endpoint_mode: mode,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: "secret".into(),
            models_url: None,
            protocol_endpoints: Vec::new(),
        }
    }

    fn urls(provider: &ProviderRecord) -> Vec<String> {
        models_endpoints(provider)
            .unwrap()
            .into_iter()
            .map(|url| url.to_string())
            .collect()
    }

    #[test]
    fn compatible_subpaths_use_the_same_bounded_candidates_as_cc_switch() {
        assert_eq!(
            urls(&provider(
                "https://api.kimi.com/coding/",
                EndpointMode::BaseUrl
            )),
            vec![
                "https://api.kimi.com/coding/v1/models",
                "https://api.kimi.com/v1/models",
                "https://api.kimi.com/models",
            ]
        );
        assert_eq!(
            urls(&provider(
                "https://open.bigmodel.cn/api/anthropic",
                EndpointMode::BaseUrl
            )),
            vec![
                "https://open.bigmodel.cn/api/anthropic/v1/models",
                "https://open.bigmodel.cn/v1/models",
                "https://open.bigmodel.cn/models",
            ]
        );
    }

    #[test]
    fn versioned_and_exact_endpoints_match_cc_switch_derivation() {
        assert_eq!(
            urls(&provider(
                "https://open.bigmodel.cn/api/coding/paas/v4",
                EndpointMode::BaseUrl
            )),
            vec![
                "https://open.bigmodel.cn/api/coding/paas/v4/models",
                "https://open.bigmodel.cn/api/coding/paas/v4/v1/models",
            ]
        );
        assert_eq!(
            urls(&provider(
                "https://proxy.example.com/chat/completions",
                EndpointMode::ExactUrl
            )),
            vec!["https://proxy.example.com/chat/v1/models"]
        );
        assert_eq!(
            urls(&provider(
                "https://proxy.example.com/v1/chat/completions",
                EndpointMode::ExactUrl
            )),
            vec!["https://proxy.example.com/v1/models"]
        );
    }

    #[test]
    fn explicit_models_url_is_the_only_candidate() {
        let mut provider = provider("https://api.example.com/anthropic", EndpointMode::BaseUrl);
        provider.models_url = Some("https://catalog.example.com/models".into());
        assert_eq!(urls(&provider), vec!["https://catalog.example.com/models"]);
    }

    #[test]
    fn preset_siblings_supply_real_protocol_specific_probe_endpoints() {
        let mut kimi = provider("https://api.kimi.com/coding/", EndpointMode::BaseUrl);
        kimi.id = "kimi-for-coding".into();
        let surfaces = protocol_probe_endpoints(&kimi).unwrap();
        assert_eq!(
            surfaces
                .iter()
                .find(|surface| surface.protocol == NativeProtocol::AnthropicMessages)
                .unwrap()
                .endpoint,
            "https://api.kimi.com/coding/"
        );
        assert_eq!(
            surfaces
                .iter()
                .find(|surface| surface.protocol == NativeProtocol::OpenAiChat)
                .unwrap()
                .endpoint,
            "https://api.kimi.com/coding/v1"
        );

        let mut deepseek = provider("https://api.deepseek.com", EndpointMode::BaseUrl);
        deepseek.id = "deepseek".into();
        deepseek.protocol = Protocol::OpenAiResponses;
        let surfaces = protocol_probe_endpoints(&deepseek).unwrap();
        assert_eq!(
            surfaces
                .iter()
                .find(|surface| surface.protocol == NativeProtocol::AnthropicMessages)
                .unwrap()
                .endpoint,
            "https://api.deepseek.com/anthropic"
        );
    }

    #[test]
    fn an_edited_preset_endpoint_is_not_replaced_by_catalog_siblings() {
        let mut provider = provider("https://proxy.example.com", EndpointMode::BaseUrl);
        provider.id = "kimi-for-coding".into();
        let surfaces = protocol_probe_endpoints(&provider).unwrap();
        assert!(
            surfaces
                .iter()
                .all(|surface| surface.endpoint == "https://proxy.example.com")
        );
    }
}
