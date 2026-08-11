use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::net::IpAddr;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    AnthropicMessages,
    OpenAiResponses,
    OpenAiChatCompletions,
    GeminiNative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointMode {
    BaseUrl,
    ExactUrl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyPlacement {
    None,
    Bearer,
    XApiKey,
}

#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    None,
    ApiKey {
        placement: ApiKeyPlacement,
        secret: String,
    },
}

impl Auth {
    pub fn none() -> Self {
        Self::None
    }

    pub fn api_key(placement: ApiKeyPlacement, secret: impl Into<String>) -> Self {
        Self::ApiKey {
            placement,
            secret: secret.into(),
        }
    }
}

impl Debug for Auth {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => formatter.write_str("NoAuth"),
            Self::ApiKey { placement, .. } => formatter
                .debug_struct("ApiKey")
                .field("placement", placement)
                .field("secret", &"[REDACTED]")
                .finish(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDraft {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub protocol: Protocol,
    pub endpoint: String,
    pub endpoint_mode: EndpointMode,
    pub auth: Auth,
    pub models_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    id: String,
    name: String,
    enabled: bool,
    protocol: Protocol,
    endpoint: Url,
    endpoint_mode: EndpointMode,
    auth: Auth,
    models_url: Option<Url>,
}

impl Provider {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    pub fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    pub fn endpoint_mode(&self) -> EndpointMode {
        self.endpoint_mode
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    InvalidId(String),
    EmptyName,
    EmptyApiKey,
    InvalidApiKey,
    IncompatibleAuth(Protocol),
    InvalidEndpoint(String),
    UnsafeEndpoint,
    UnsupportedEndpointScheme(String),
    InsecureRemoteEndpoint(String),
    DuplicateProvider(String),
    BaseEndpointHasQuery,
    UnauthenticatedRemote,
    InvalidModelsUrl(String),
    UnsafeModelsUrl,
    UnsupportedModelsUrlScheme(String),
    InsecureRemoteModelsUrl(String),
}

impl Display for ProviderError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(id) => write!(formatter, "provider id must be a lowercase slug: {id}"),
            Self::EmptyName => write!(formatter, "provider name must not be empty"),
            Self::EmptyApiKey => write!(formatter, "provider API key must not be empty"),
            Self::InvalidApiKey => {
                write!(
                    formatter,
                    "provider API key contains invalid header characters"
                )
            }
            Self::IncompatibleAuth(Protocol::GeminiNative) => {
                write!(
                    formatter,
                    "Gemini Native providers require X-API-Key authentication"
                )
            }
            Self::IncompatibleAuth(Protocol::OpenAiResponses | Protocol::OpenAiChatCompletions) => {
                write!(
                    formatter,
                    "OpenAI-compatible providers require Bearer authentication"
                )
            }
            Self::IncompatibleAuth(Protocol::AnthropicMessages) => write!(
                formatter,
                "Anthropic Messages does not support the selected authentication"
            ),
            Self::InvalidEndpoint(endpoint) => {
                write!(formatter, "invalid provider endpoint: {endpoint}")
            }
            Self::UnsafeEndpoint => write!(
                formatter,
                "provider endpoint must not contain credentials or a fragment"
            ),
            Self::UnsupportedEndpointScheme(endpoint) => {
                write!(
                    formatter,
                    "provider endpoint must use HTTP or HTTPS: {endpoint}"
                )
            }
            Self::InsecureRemoteEndpoint(endpoint) => write!(
                formatter,
                "provider endpoint must use HTTPS unless it is loopback: {endpoint}"
            ),
            Self::DuplicateProvider(id) => write!(formatter, "duplicate provider id: {id}"),
            Self::BaseEndpointHasQuery => {
                write!(formatter, "provider Base URL must not contain a query")
            }
            Self::UnauthenticatedRemote => {
                write!(formatter, "no-auth providers must use a loopback endpoint")
            }
            Self::InvalidModelsUrl(url) => write!(formatter, "invalid provider models URL: {url}"),
            Self::UnsafeModelsUrl => write!(
                formatter,
                "provider models URL must not contain credentials or a fragment"
            ),
            Self::UnsupportedModelsUrlScheme(url) => {
                write!(
                    formatter,
                    "provider models URL must use HTTP or HTTPS: {url}"
                )
            }
            Self::InsecureRemoteModelsUrl(url) => write!(
                formatter,
                "provider models URL must use HTTPS unless it is loopback: {url}"
            ),
        }
    }
}

#[derive(Debug)]
pub struct ProviderRegistry {
    providers: HashMap<String, Provider>,
}

impl ProviderRegistry {
    pub fn new(providers: impl IntoIterator<Item = Provider>) -> Result<Self, ProviderError> {
        let mut by_id = HashMap::new();
        for provider in providers {
            let id = provider.id.clone();
            if by_id.insert(id.clone(), provider).is_some() {
                return Err(ProviderError::DuplicateProvider(id));
            }
        }
        Ok(Self { providers: by_id })
    }

    pub fn get(&self, id: &str) -> Option<&Provider> {
        self.providers.get(id)
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.providers.keys().map(String::as_str)
    }
}

impl Error for ProviderError {}

impl TryFrom<ProviderDraft> for Provider {
    type Error = ProviderError;

    fn try_from(draft: ProviderDraft) -> Result<Self, Self::Error> {
        if !is_slug(&draft.id) {
            return Err(ProviderError::InvalidId(draft.id));
        }
        if draft.name.trim().is_empty() {
            return Err(ProviderError::EmptyName);
        }

        match (&draft.protocol, &draft.auth) {
            (
                Protocol::GeminiNative,
                Auth::ApiKey {
                    placement: ApiKeyPlacement::XApiKey,
                    ..
                },
            ) => {}
            (Protocol::GeminiNative, _) => {
                return Err(ProviderError::IncompatibleAuth(Protocol::GeminiNative));
            }
            (_, Auth::None) => {}
            (_, Auth::ApiKey { secret, .. }) if secret.trim().is_empty() => {
                return Err(ProviderError::EmptyApiKey);
            }
            (_, Auth::ApiKey { secret, .. }) if secret.contains(['\r', '\n']) => {
                return Err(ProviderError::InvalidApiKey);
            }
            (
                Protocol::OpenAiResponses | Protocol::OpenAiChatCompletions,
                Auth::ApiKey {
                    placement: ApiKeyPlacement::XApiKey,
                    ..
                },
            ) => return Err(ProviderError::IncompatibleAuth(draft.protocol)),
            _ => {}
        }

        let endpoint = Url::parse(&draft.endpoint)
            .map_err(|_| ProviderError::InvalidEndpoint(draft.endpoint.clone()))?;

        if !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(ProviderError::UnsafeEndpoint);
        }

        if !matches!(endpoint.scheme(), "http" | "https") {
            return Err(ProviderError::UnsupportedEndpointScheme(
                endpoint.to_string(),
            ));
        }

        if draft.endpoint_mode == EndpointMode::BaseUrl && endpoint.query().is_some() {
            return Err(ProviderError::BaseEndpointHasQuery);
        }

        if endpoint.scheme() == "http" && !is_loopback(&endpoint) {
            return Err(ProviderError::InsecureRemoteEndpoint(endpoint.to_string()));
        }
        if matches!(&draft.auth, Auth::None) && !is_loopback(&endpoint) {
            return Err(ProviderError::UnauthenticatedRemote);
        }

        let models_url = draft
            .models_url
            .map(|value| {
                let url = Url::parse(&value)
                    .map_err(|_| ProviderError::InvalidModelsUrl(value.clone()))?;
                if !url.username().is_empty()
                    || url.password().is_some()
                    || url.fragment().is_some()
                {
                    return Err(ProviderError::UnsafeModelsUrl);
                }
                if !matches!(url.scheme(), "http" | "https") {
                    return Err(ProviderError::UnsupportedModelsUrlScheme(url.to_string()));
                }
                if url.scheme() == "http" && !is_loopback(&url) {
                    return Err(ProviderError::InsecureRemoteModelsUrl(url.to_string()));
                }
                Ok(url)
            })
            .transpose()?;

        Ok(Self {
            id: draft.id,
            name: draft.name,
            enabled: draft.enabled,
            protocol: draft.protocol,
            endpoint,
            endpoint_mode: draft.endpoint_mode,
            auth: draft.auth,
            models_url,
        })
    }
}

pub fn build_request_endpoint(
    base: &Url,
    mode: EndpointMode,
    standard_path: &str,
) -> Result<Url, url::ParseError> {
    if mode == EndpointMode::ExactUrl {
        return Ok(base.clone());
    }

    let mut value = format!(
        "{}/{}",
        base.as_str().trim_end_matches('/'),
        standard_path.trim_start_matches('/')
    );
    while value.contains("/v1/v1") {
        value = value.replace("/v1/v1", "/v1");
    }
    Url::parse(&value)
}

pub(crate) fn is_slug(value: &str) -> bool {
    let mut segments = value.split('-');
    segments.next().is_some_and(is_slug_segment)
        && segments.all(is_slug_segment)
        && !value.ends_with('-')
}

fn is_slug_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

fn is_loopback(endpoint: &Url) -> bool {
    match endpoint.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback()),
        None => false,
    }
}
