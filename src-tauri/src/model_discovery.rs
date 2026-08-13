//! Provider model discovery, adapted from cc-switch's bounded `/models` slice.
//! GrillForge resolves exactly one endpoint: an explicit `models_url` wins;
//! otherwise the endpoint is derived deterministically from the Provider URL.

use crate::configuration::ProviderRecord;
use crate::core::model::NativeProtocol;
use crate::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use reqwest::header::{AUTHORIZATION, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
    let endpoint = models_endpoint(provider)?;
    let client = reqwest::Client::builder()
        .connect_timeout(FETCH_TIMEOUT)
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|error| format!("could not create model discovery client: {error}"))?;
    let mut request = client.get(endpoint.clone());
    request = match (provider.protocol, provider.api_key_placement) {
        (Protocol::GeminiNative, ApiKeyPlacement::XApiKey) => request.header(
            HeaderName::from_static("x-goog-api-key"),
            HeaderValue::from_str(&provider.api_key)
                .map_err(|_| "provider API key contains invalid header characters".to_string())?,
        ),
        (_, ApiKeyPlacement::None) => request,
        (_, ApiKeyPlacement::Bearer) => request.header(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", provider.api_key))
                .map_err(|_| "provider API key contains invalid header characters".to_string())?,
        ),
        (_, ApiKeyPlacement::XApiKey) => request.header(
            HeaderName::from_static("x-api-key"),
            HeaderValue::from_str(&provider.api_key)
                .map_err(|_| "provider API key contains invalid header characters".to_string())?,
        ),
    };
    let response = request
        .send()
        .await
        .map_err(|error| format!("model discovery request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "model discovery returned HTTP {} from {endpoint}",
            status.as_u16()
        ));
    }
    let body = response
        .bytes()
        .await
        .map_err(|_| "model discovery response body could not be read".to_string())?;
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

fn models_endpoint(provider: &ProviderRecord) -> Result<Url, String> {
    if let Some(models_url) = provider.models_url.as_deref() {
        return Url::parse(models_url)
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
        return Ok(endpoint);
    }
    let models_path = match provider.endpoint_mode {
        EndpointMode::ExactUrl => {
            if let Some(index) = path.find("/v1/") {
                format!("{}/v1/models", &path[..index])
            } else {
                let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
                format!("{parent}/models")
            }
        }
        EndpointMode::BaseUrl => {
            if let Some(root) = COMPAT_SUFFIXES
                .iter()
                .find_map(|suffix| path.strip_suffix(suffix))
            {
                format!("{root}/v1/models")
            } else if is_version_path(path) {
                format!("{path}/models")
            } else {
                format!("{path}/v1/models")
            }
        }
    };
    endpoint.set_path(&models_path);
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    Ok(endpoint)
}

fn is_version_path(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|segment| {
        segment.strip_prefix('v').is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
    })
}
