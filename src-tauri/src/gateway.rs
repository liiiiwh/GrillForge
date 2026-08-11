use crate::application::ControlPlaneState;
use crate::bridge::{
    BridgeError, CodexAnthropicCapabilities, GeminiNativeBridge, OpenAiChatBridge,
    OpenAiChatCapabilities, OpenAiResponsesBridge, OpenAiResponsesCapabilities,
    anthropic_sse_to_codex_responses_with_context, anthropic_to_codex_response_with_context,
    chat_sse_to_codex_responses, chat_to_codex_response, codex_response_to_anthropic_with_context,
    codex_response_to_chat,
};
use crate::configuration::{ConfigurationDocuments, ConfigurationFiles, ProviderRecord};
use crate::core::model::ProtocolCapability;
use crate::core::provider::{ApiKeyPlacement, EndpointMode, Protocol, build_request_endpoint};
use axum::Json;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{OriginalUri, Path as AxumPath, State};
use axum::http::{HeaderMap, HeaderName, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use url::Url;

pub const DEFAULT_GATEWAY_ADDRESS: &str = "127.0.0.1:15721";
const OFFICIAL_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const CLAUDE_DESKTOP_CREATED_AT: &str = "2024-01-01T00:00:00Z";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteSpec {
    pub route_id: String,
    pub model_id: String,
    pub label_override: Option<String>,
    pub supports_1m: bool,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatus {
    pub base_url: String,
    #[serde(skip)]
    native_base_url: Arc<RwLock<Url>>,
    #[serde(skip)]
    files: ConfigurationFiles,
    #[serde(skip)]
    active_routes: Arc<RwLock<Option<ActiveRoutes>>>,
    #[serde(skip)]
    active_desktop_routes: Arc<RwLock<Option<ActiveDesktopRoutes>>>,
    #[serde(skip)]
    active_pi_routes: Arc<RwLock<Option<ActivePiRoutes>>>,
    #[serde(skip)]
    active_codex_routes: Arc<RwLock<Option<ActiveCodexRoutes>>>,
    #[serde(skip)]
    active_client_routes: Arc<RwLock<HashMap<String, ActiveClientRoutes>>>,
    #[serde(skip)]
    connection_tests: Arc<Mutex<HashSet<String>>>,
}

impl GatewayStatus {
    fn new(base_url: String, gateway: &Gateway) -> Self {
        Self {
            base_url,
            native_base_url: Arc::clone(&gateway.native_base_url),
            files: gateway.files.clone(),
            active_routes: Arc::clone(&gateway.active_routes),
            active_desktop_routes: Arc::clone(&gateway.active_desktop_routes),
            active_pi_routes: Arc::clone(&gateway.active_pi_routes),
            active_codex_routes: Arc::clone(&gateway.active_codex_routes),
            active_client_routes: Arc::clone(&gateway.active_client_routes),
            connection_tests: Arc::clone(&gateway.connection_tests),
        }
    }

    pub fn activate(&self, state: &ControlPlaneState) -> Result<(), String> {
        let mut allowed_model_ids = HashSet::new();
        if let Some(main) = &state.main_model_id {
            allowed_model_ids.insert(main.clone());
        }
        allowed_model_ids.extend(state.model_slots.values().cloned());
        if state.worker_mode {
            allowed_model_ids.extend(
                state
                    .models
                    .iter()
                    .filter(|model| model.worker_enabled)
                    .map(|model| model.id.clone()),
            );
        }
        allowed_model_ids.extend(
            state
                .subagents
                .iter()
                .filter(|subagent| subagent.enabled)
                .map(|subagent| subagent.model_id.clone()),
        );
        let documents = self.files.read().map_err(|error| error.to_string())?;
        for id in &allowed_model_ids {
            if !documents.models.models.iter().any(|model| &model.id == id) {
                return Err(format!("cannot activate unknown model route: {id}"));
            }
        }
        *self
            .active_routes
            .write()
            .map_err(|_| "active route lock is poisoned".to_string())? = Some(ActiveRoutes {
            documents,
            allowed_model_ids,
        });
        Ok(())
    }

    pub fn deactivate(&self) {
        if let Ok(mut active) = self.active_routes.write() {
            *active = None;
        }
    }

    pub fn activate_claude_desktop(
        &self,
        routes: Vec<RouteSpec>,
        token: &str,
    ) -> Result<(), String> {
        if token.is_empty() || token.trim() != token {
            return Err("Claude Desktop gateway token must not be empty or padded".into());
        }
        if routes.is_empty() {
            return Err("Claude Desktop requires at least one model route".into());
        }

        let documents = self.files.read().map_err(|error| error.to_string())?;
        let mut route_ids = HashSet::new();
        for route in &routes {
            let is_safe_route = is_claude_safe_model_id(&route.route_id);
            let is_worker_route = route.route_id.starts_with("grillforge/");
            if !is_safe_route && !is_worker_route {
                return Err(format!(
                    "Claude Desktop route must use a Claude-safe model id or GrillForge worker alias: {}",
                    route.route_id
                ));
            }
            if !route_ids.insert(route.route_id.clone()) {
                return Err(format!(
                    "duplicate Claude Desktop route: {}",
                    route.route_id
                ));
            }
            if route
                .label_override
                .as_ref()
                .is_some_and(|label| label.trim().is_empty() || label.trim() != label)
            {
                return Err(format!(
                    "Claude Desktop route label must not be empty or padded: {}",
                    route.route_id
                ));
            }
            let model = documents
                .models
                .models
                .iter()
                .find(|model| model.id == route.model_id)
                .ok_or_else(|| {
                    format!(
                        "Claude Desktop route {} references unknown model {}",
                        route.route_id, route.model_id
                    )
                })?;
            if is_worker_route && route.route_id != format!("grillforge/{}", model.id) {
                return Err(format!(
                    "Claude Desktop worker alias {} does not match model {}",
                    route.route_id, model.id
                ));
            }
            let provider = documents
                .config
                .providers
                .iter()
                .find(|provider| provider.id == model.provider_id)
                .ok_or_else(|| {
                    format!(
                        "model {} references unknown provider {}",
                        model.id, model.provider_id
                    )
                })?;
            if !provider.enabled {
                return Err(format!(
                    "model {} uses disabled provider {}",
                    model.id, provider.id
                ));
            }
        }

        let next = ActiveDesktopRoutes {
            documents,
            routes,
            token: token.to_string(),
        };
        *self
            .active_desktop_routes
            .write()
            .map_err(|_| "active Claude Desktop route lock is poisoned".to_string())? = Some(next);
        Ok(())
    }

    pub fn deactivate_claude_desktop(&self) {
        if let Ok(mut active) = self.active_desktop_routes.write() {
            *active = None;
        }
    }

    pub fn activate_pi(&self, model_ids: Vec<String>, token: &str) -> Result<(), String> {
        if token.is_empty() || token.trim() != token || token.chars().any(char::is_control) {
            return Err(
                "Pi gateway token must not be empty, padded, or contain control characters".into(),
            );
        }
        if model_ids.is_empty() {
            return Err("Pi requires at least one model route".into());
        }
        let allowed_model_ids = model_ids.into_iter().collect::<HashSet<_>>();
        let documents = self.files.read().map_err(|error| error.to_string())?;
        for id in &allowed_model_ids {
            let model = documents
                .models
                .models
                .iter()
                .find(|model| &model.id == id)
                .ok_or_else(|| format!("Pi route references unknown model: {id}"))?;
            let provider = documents
                .config
                .providers
                .iter()
                .find(|provider| provider.id == model.provider_id)
                .ok_or_else(|| {
                    format!(
                        "model {} references unknown provider {}",
                        model.id, model.provider_id
                    )
                })?;
            if !provider.enabled {
                return Err(format!(
                    "model {} uses disabled provider {}",
                    model.id, provider.id
                ));
            }
        }
        *self
            .active_pi_routes
            .write()
            .map_err(|_| "active Pi route lock is poisoned".to_string())? = Some(ActivePiRoutes {
            documents,
            allowed_model_ids,
            token: token.into(),
        });
        Ok(())
    }

    pub fn deactivate_pi(&self) {
        if let Ok(mut active) = self.active_pi_routes.write() {
            *active = None;
        }
    }

    pub fn activate_codex(&self, model_ids: Vec<String>, token: &str) -> Result<(), String> {
        if token.is_empty() || token.trim() != token || token.chars().any(char::is_control) {
            return Err(
                "Codex gateway token must not be empty, padded, or contain control characters"
                    .into(),
            );
        }
        if model_ids.is_empty() {
            return Err("Codex requires at least one model route".into());
        }
        let allowed_model_ids = model_ids.into_iter().collect::<HashSet<_>>();
        let documents = self.files.read().map_err(|error| error.to_string())?;
        for id in &allowed_model_ids {
            let model = documents
                .models
                .models
                .iter()
                .find(|model| &model.id == id)
                .ok_or_else(|| format!("Codex route references unknown model: {id}"))?;
            let provider = documents
                .config
                .providers
                .iter()
                .find(|provider| provider.id == model.provider_id)
                .ok_or_else(|| {
                    format!(
                        "model {} references unknown provider {}",
                        model.id, model.provider_id
                    )
                })?;
            if !provider.enabled {
                return Err(format!(
                    "model {} uses disabled provider {}",
                    model.id, provider.id
                ));
            }
        }
        *self
            .active_codex_routes
            .write()
            .map_err(|_| "active Codex route lock is poisoned".to_string())? =
            Some(ActiveCodexRoutes {
                documents,
                allowed_model_ids,
                token: token.to_string(),
            });
        Ok(())
    }

    pub fn deactivate_codex(&self) {
        if let Ok(mut active) = self.active_codex_routes.write() {
            *active = None;
        }
    }

    pub fn activate_client(
        &self,
        client_id: &str,
        model_ids: Vec<String>,
        token: &str,
    ) -> Result<(), String> {
        if client_id.is_empty()
            || !client_id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(format!("invalid gateway client id: {client_id}"));
        }
        if token.is_empty() || token.trim() != token || token.chars().any(char::is_control) {
            return Err(format!("{client_id} gateway token is empty or invalid"));
        }
        let allowed_model_ids = model_ids.into_iter().collect::<HashSet<_>>();
        if allowed_model_ids.is_empty() {
            return Err(format!("{client_id} requires at least one model route"));
        }
        let documents = self.files.read().map_err(|error| error.to_string())?;
        for id in &allowed_model_ids {
            let model = documents
                .models
                .models
                .iter()
                .find(|model| &model.id == id)
                .ok_or_else(|| format!("{client_id} references unknown model: {id}"))?;
            let provider = documents
                .config
                .providers
                .iter()
                .find(|provider| provider.id == model.provider_id)
                .ok_or_else(|| format!("model {id} references unknown provider"))?;
            if !provider.enabled {
                return Err(format!("model {id} uses disabled provider {}", provider.id));
            }
            if provider.protocol == Protocol::GeminiNative {
                return Err(format!(
                    "{client_id} cannot route Gemini Native model through the Anthropic gateway: {id}"
                ));
            }
        }
        self.active_client_routes
            .write()
            .map_err(|_| "active client route lock is poisoned".to_string())?
            .insert(
                client_id.to_string(),
                ActiveClientRoutes {
                    documents,
                    allowed_model_ids,
                    token: token.to_string(),
                },
            );
        Ok(())
    }

    pub fn deactivate_client(&self, client_id: &str) {
        if let Ok(mut active) = self.active_client_routes.write() {
            active.remove(client_id);
        }
    }

    pub fn allow_connection_test(&self, model_id: &str) -> Result<ConnectionTestGuard, String> {
        self.connection_tests
            .lock()
            .map_err(|_| "connection test route lock is poisoned".to_string())?
            .insert(model_id.to_string());
        Ok(ConnectionTestGuard {
            model_id: model_id.to_string(),
            routes: Arc::clone(&self.connection_tests),
        })
    }

    pub fn set_native_base_url(&self, base_url: &str) -> Result<(), String> {
        let parsed = Url::parse(base_url)
            .map_err(|_| format!("invalid native Anthropic base URL: {base_url}"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(format!("invalid native Anthropic base URL: {base_url}"));
        }
        let gateway =
            Url::parse(&self.base_url).map_err(|_| "invalid GrillForge gateway URL".to_string())?;
        if same_origin_and_path(&parsed, &gateway) {
            return Err("native Anthropic base URL points back to the GrillForge gateway".into());
        }
        *self
            .native_base_url
            .write()
            .map_err(|_| "native Anthropic route lock is poisoned".to_string())? = parsed;
        Ok(())
    }

    pub fn use_official_native_base_url(&self) {
        if let Ok(mut current) = self.native_base_url.write() {
            *current =
                Url::parse(OFFICIAL_ANTHROPIC_BASE_URL).expect("official Anthropic URL is valid");
        }
    }
}

pub struct ConnectionTestGuard {
    model_id: String,
    routes: Arc<Mutex<HashSet<String>>>,
}

impl Drop for ConnectionTestGuard {
    fn drop(&mut self) {
        if let Ok(mut routes) = self.routes.lock() {
            routes.remove(&self.model_id);
        }
    }
}

#[derive(Clone)]
struct ActiveRoutes {
    documents: ConfigurationDocuments,
    allowed_model_ids: HashSet<String>,
}

#[derive(Clone)]
struct ActiveDesktopRoutes {
    documents: ConfigurationDocuments,
    routes: Vec<RouteSpec>,
    token: String,
}

#[derive(Clone)]
struct ActivePiRoutes {
    documents: ConfigurationDocuments,
    allowed_model_ids: HashSet<String>,
    token: String,
}

#[derive(Clone)]
struct ActiveCodexRoutes {
    documents: ConfigurationDocuments,
    allowed_model_ids: HashSet<String>,
    token: String,
}

#[derive(Clone)]
struct ActiveClientRoutes {
    documents: ConfigurationDocuments,
    allowed_model_ids: HashSet<String>,
    token: String,
}

#[derive(Clone)]
pub struct Gateway {
    files: ConfigurationFiles,
    client: reqwest::Client,
    native_base_url: Arc<RwLock<Url>>,
    active_routes: Arc<RwLock<Option<ActiveRoutes>>>,
    active_desktop_routes: Arc<RwLock<Option<ActiveDesktopRoutes>>>,
    active_pi_routes: Arc<RwLock<Option<ActivePiRoutes>>>,
    active_codex_routes: Arc<RwLock<Option<ActiveCodexRoutes>>>,
    active_client_routes: Arc<RwLock<HashMap<String, ActiveClientRoutes>>>,
    connection_tests: Arc<Mutex<HashSet<String>>>,
}

impl Gateway {
    pub fn new(config_root: impl Into<PathBuf>) -> Self {
        Self {
            files: ConfigurationFiles::new(config_root),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .build()
                .expect("static HTTP client configuration is valid"),
            native_base_url: Arc::new(RwLock::new(
                Url::parse(OFFICIAL_ANTHROPIC_BASE_URL).expect("official Anthropic URL is valid"),
            )),
            active_routes: Arc::new(RwLock::new(None)),
            active_desktop_routes: Arc::new(RwLock::new(None)),
            active_pi_routes: Arc::new(RwLock::new(None)),
            active_codex_routes: Arc::new(RwLock::new(None)),
            active_client_routes: Arc::new(RwLock::new(HashMap::new())),
            connection_tests: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn with_native_base_url(mut self, base_url: Url) -> Self {
        self.native_base_url = Arc::new(RwLock::new(base_url));
        self
    }

    pub fn status(&self, base_url: String) -> GatewayStatus {
        GatewayStatus::new(base_url, self)
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/v1/messages", post(messages))
            .route("/claude-desktop/v1/models", get(claude_desktop_models))
            .route("/claude-desktop/v1/messages", post(claude_desktop_messages))
            .route("/pi/v1/messages", post(pi_messages))
            .route("/codex/v1/responses", post(codex_responses))
            .route("/clients/{client}/v1/messages", post(client_messages))
            .with_state(self.clone())
    }

    async fn complete_external(
        &self,
        headers: HeaderMap,
        request: Value,
    ) -> Result<Response, GatewayError> {
        let alias = request
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| GatewayError::InvalidRequest("model must be a string".into()))?
            .to_string();
        let model_id = alias
            .strip_prefix("grillforge/")
            .ok_or_else(|| {
                GatewayError::InvalidRequest(format!("unknown GrillForge route alias: {alias}"))
            })?
            .to_string();

        let active = self
            .active_routes
            .read()
            .map_err(|_| GatewayError::Configuration("active route lock is poisoned".into()))?
            .clone();
        let testing = self
            .connection_tests
            .lock()
            .map_err(|_| {
                GatewayError::Configuration("connection test route lock is poisoned".into())
            })?
            .contains(&model_id);
        let documents = match active {
            Some(active) if active.allowed_model_ids.contains(&model_id) => active.documents,
            _ if testing => self
                .files
                .read()
                .map_err(|error| GatewayError::Configuration(error.to_string()))?,
            _ => {
                return Err(GatewayError::InvalidRequest(format!(
                    "inactive GrillForge route alias: {alias}"
                )));
            }
        };
        self.complete_configured_model(headers, request, documents, &model_id)
            .await
    }

    fn authorized_claude_desktop(
        &self,
        headers: &HeaderMap,
    ) -> Result<ActiveDesktopRoutes, GatewayError> {
        let active = self
            .active_desktop_routes
            .read()
            .map_err(|_| {
                GatewayError::Configuration("active Claude Desktop route lock is poisoned".into())
            })?
            .clone()
            .ok_or_else(|| GatewayError::Unauthorized("Claude Desktop".into()))?;
        let expected = format!("Bearer {}", active.token);
        let authorized = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == expected);
        if !authorized {
            return Err(GatewayError::Unauthorized("Claude Desktop".into()));
        }
        Ok(active)
    }

    fn authorized_pi(&self, headers: &HeaderMap) -> Result<ActivePiRoutes, GatewayError> {
        let active = self
            .active_pi_routes
            .read()
            .map_err(|_| GatewayError::Configuration("active Pi route lock is poisoned".into()))?
            .clone()
            .ok_or_else(|| GatewayError::Unauthorized("Pi".into()))?;
        let expected = format!("Bearer {}", active.token);
        let bearer_authorized = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == expected);
        let api_key_authorized = headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == active.token);
        if !bearer_authorized && !api_key_authorized {
            return Err(GatewayError::Unauthorized("Pi".into()));
        }
        Ok(active)
    }

    fn authorized_codex(&self, headers: &HeaderMap) -> Result<ActiveCodexRoutes, GatewayError> {
        let active = self
            .active_codex_routes
            .read()
            .map_err(|_| GatewayError::Configuration("active Codex route lock is poisoned".into()))?
            .clone()
            .ok_or_else(|| GatewayError::Unauthorized("Codex".into()))?;
        let expected = format!("Bearer {}", active.token);
        let authorized = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == expected);
        if !authorized {
            return Err(GatewayError::Unauthorized("Codex".into()));
        }
        Ok(active)
    }

    fn authorized_client(
        &self,
        client_id: &str,
        headers: &HeaderMap,
    ) -> Result<ActiveClientRoutes, GatewayError> {
        let active = self
            .active_client_routes
            .read()
            .map_err(|_| {
                GatewayError::Configuration("active client route lock is poisoned".into())
            })?
            .get(client_id)
            .cloned()
            .ok_or_else(|| GatewayError::Unauthorized(client_id.to_owned()))?;
        let expected = format!("Bearer {}", active.token);
        let authorized = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == expected);
        if !authorized {
            return Err(GatewayError::Unauthorized(client_id.to_owned()));
        }
        Ok(active)
    }

    async fn complete_configured_model(
        &self,
        headers: HeaderMap,
        mut request: Value,
        documents: ConfigurationDocuments,
        model_id: &str,
    ) -> Result<Response, GatewayError> {
        let model = documents
            .models
            .models
            .iter()
            .find(|model| model.id == model_id)
            .ok_or_else(|| {
                GatewayError::InvalidRequest(format!("unknown GrillForge model: {model_id}"))
            })?;
        let provider = documents
            .config
            .providers
            .iter()
            .find(|provider| provider.id == model.provider_id)
            .ok_or_else(|| {
                GatewayError::Configuration(format!(
                    "model {} references unknown provider {}",
                    model.id, model.provider_id
                ))
            })?;
        if !provider.enabled {
            return Err(GatewayError::Configuration(format!(
                "model {} uses disabled provider {}",
                model.id, provider.id
            )));
        }

        request["model"] = Value::String(model.upstream_id.clone());
        match provider.protocol {
            Protocol::OpenAiResponses => {
                let base = Url::parse(&provider.endpoint).map_err(|_| {
                    GatewayError::Configuration(format!(
                        "invalid provider endpoint: {}",
                        provider.endpoint
                    ))
                })?;
                let endpoint =
                    build_request_endpoint(&base, provider.endpoint_mode, "/v1/responses")
                        .map_err(|_| {
                            GatewayError::Configuration(format!(
                                "invalid provider endpoint: {}",
                                provider.endpoint
                            ))
                        })?;
                let bridge = match provider.api_key_placement {
                    ApiKeyPlacement::None => {
                        OpenAiResponsesBridge::from_endpoint_without_auth(endpoint)
                    }
                    ApiKeyPlacement::Bearer => {
                        OpenAiResponsesBridge::from_endpoint(endpoint, &provider.api_key)
                    }
                    ApiKeyPlacement::XApiKey => {
                        return Err(GatewayError::Configuration(format!(
                            "Responses provider {} has incompatible authentication",
                            provider.id
                        )));
                    }
                }
                .with_capabilities(OpenAiResponsesCapabilities {
                    reasoning_items: model
                        .protocol_capabilities
                        .iter()
                        .any(|capability| capability == &ProtocolCapability::ReasoningItems),
                });
                if request.get("stream").and_then(Value::as_bool) == Some(true) {
                    let stream = bridge.stream(request).await.map_err(GatewayError::Bridge)?;
                    let mut response = Response::new(Body::from_stream(stream));
                    response.headers_mut().insert(
                        HeaderName::from_static("content-type"),
                        "text/event-stream".parse().expect("static content type"),
                    );
                    Ok(response)
                } else {
                    let response = bridge
                        .complete(request)
                        .await
                        .map_err(GatewayError::Bridge)?;
                    Ok((StatusCode::OK, Json(response)).into_response())
                }
            }
            Protocol::AnthropicMessages => {
                self.forward_anthropic_provider(provider, headers, request)
                    .await
            }
            Protocol::OpenAiChatCompletions => {
                let base = Url::parse(&provider.endpoint).map_err(|_| {
                    GatewayError::Configuration(format!(
                        "invalid provider endpoint: {}",
                        provider.endpoint
                    ))
                })?;
                let endpoint =
                    build_request_endpoint(&base, provider.endpoint_mode, "/v1/chat/completions")
                        .map_err(|_| {
                        GatewayError::Configuration(format!(
                            "invalid provider endpoint: {}",
                            provider.endpoint
                        ))
                    })?;
                let bridge = match provider.api_key_placement {
                    ApiKeyPlacement::None => OpenAiChatBridge::from_endpoint_without_auth(endpoint),
                    ApiKeyPlacement::Bearer => {
                        OpenAiChatBridge::from_endpoint(endpoint, &provider.api_key)
                    }
                    ApiKeyPlacement::XApiKey => {
                        return Err(GatewayError::Configuration(format!(
                            "Chat Completions provider {} has incompatible authentication",
                            provider.id
                        )));
                    }
                }
                .with_capabilities(OpenAiChatCapabilities {
                    reasoning_content: model
                        .protocol_capabilities
                        .iter()
                        .any(|capability| capability == &ProtocolCapability::ReasoningContent),
                    reasoning_effort: model
                        .protocol_capabilities
                        .iter()
                        .any(|capability| capability == &ProtocolCapability::ReasoningEffort),
                });
                if request.get("stream").and_then(Value::as_bool) == Some(true) {
                    let stream = bridge.stream(request).await.map_err(GatewayError::Bridge)?;
                    let mut response = Response::new(Body::from_stream(stream));
                    response.headers_mut().insert(
                        HeaderName::from_static("content-type"),
                        "text/event-stream".parse().expect("static content type"),
                    );
                    Ok(response)
                } else {
                    let response = bridge
                        .complete(request)
                        .await
                        .map_err(GatewayError::Bridge)?;
                    Ok((StatusCode::OK, Json(response)).into_response())
                }
            }
            Protocol::GeminiNative => {
                if provider.endpoint_mode != EndpointMode::BaseUrl
                    || provider.api_key_placement != ApiKeyPlacement::XApiKey
                {
                    return Err(GatewayError::Configuration(format!(
                        "Gemini Native provider {} requires an API-key Base URL",
                        provider.id
                    )));
                }
                let streaming = request.get("stream").and_then(Value::as_bool) == Some(true);
                let endpoint = gemini_endpoint(&provider.endpoint, &model.upstream_id, streaming)?;
                let bridge = GeminiNativeBridge::from_endpoint(endpoint, &provider.api_key);
                if streaming {
                    let stream = bridge.stream(request).await.map_err(GatewayError::Bridge)?;
                    let mut response = Response::new(Body::from_stream(stream));
                    response.headers_mut().insert(
                        HeaderName::from_static("content-type"),
                        "text/event-stream".parse().expect("static content type"),
                    );
                    Ok(response)
                } else {
                    let response = bridge
                        .complete(request)
                        .await
                        .map_err(GatewayError::Bridge)?;
                    Ok((StatusCode::OK, Json(response)).into_response())
                }
            }
        }
    }

    async fn complete_codex_model(
        &self,
        mut request: Value,
        documents: ConfigurationDocuments,
        model_id: &str,
    ) -> Result<Response, GatewayError> {
        let model = documents
            .models
            .models
            .iter()
            .find(|model| model.id == model_id)
            .ok_or_else(|| {
                GatewayError::InvalidRequest(format!("unknown GrillForge model: {model_id}"))
            })?;
        let provider = documents
            .config
            .providers
            .iter()
            .find(|provider| provider.id == model.provider_id)
            .ok_or_else(|| {
                GatewayError::Configuration(format!(
                    "model {} references unknown provider {}",
                    model.id, model.provider_id
                ))
            })?;
        if !provider.enabled {
            return Err(GatewayError::Configuration(format!(
                "model {} uses disabled provider {}",
                model.id, provider.id
            )));
        }
        request["model"] = Value::String(model.upstream_id.clone());
        let base = Url::parse(&provider.endpoint).map_err(|_| {
            GatewayError::Configuration(format!("invalid provider endpoint: {}", provider.endpoint))
        })?;
        match provider.protocol {
            Protocol::OpenAiResponses => {
                let endpoint =
                    build_request_endpoint(&base, provider.endpoint_mode, "/v1/responses")
                        .map_err(|_| {
                            GatewayError::Configuration(format!(
                                "invalid provider endpoint: {}",
                                provider.endpoint
                            ))
                        })?;
                let mut upstream = self.client.post(endpoint).json(&request);
                upstream = match provider.api_key_placement {
                    ApiKeyPlacement::None => upstream,
                    ApiKeyPlacement::Bearer => upstream.bearer_auth(&provider.api_key),
                    ApiKeyPlacement::XApiKey => {
                        return Err(GatewayError::Configuration(format!(
                            "Responses provider {} has incompatible authentication",
                            provider.id
                        )));
                    }
                };
                let response = upstream
                    .send()
                    .await
                    .map_err(|error| GatewayError::Native(error.to_string()))?;
                Ok(response_to_axum(response))
            }
            Protocol::OpenAiChatCompletions => {
                let streaming = request.get("stream").and_then(Value::as_bool) == Some(true);
                let endpoint =
                    build_request_endpoint(&base, provider.endpoint_mode, "/v1/chat/completions")
                        .map_err(|_| {
                        GatewayError::Configuration(format!(
                            "invalid provider endpoint: {}",
                            provider.endpoint
                        ))
                    })?;
                let upstream_request =
                    codex_response_to_chat(request).map_err(GatewayError::Bridge)?;
                let mut upstream = self.client.post(endpoint).json(&upstream_request);
                upstream = match provider.api_key_placement {
                    ApiKeyPlacement::None => upstream,
                    ApiKeyPlacement::Bearer => upstream.bearer_auth(&provider.api_key),
                    ApiKeyPlacement::XApiKey => {
                        return Err(GatewayError::Configuration(format!(
                            "Chat provider {} has incompatible authentication",
                            provider.id
                        )));
                    }
                };
                let response = upstream
                    .send()
                    .await
                    .map_err(|error| GatewayError::Native(error.to_string()))?;
                if !response.status().is_success() {
                    return Ok(response_to_axum(response));
                }
                if streaming {
                    let stream = chat_sse_to_codex_responses(response.bytes_stream());
                    let mut response = Response::new(Body::from_stream(stream));
                    response.headers_mut().insert(
                        HeaderName::from_static("content-type"),
                        "text/event-stream".parse().expect("static content type"),
                    );
                    return Ok(response);
                }
                let response = response
                    .json::<Value>()
                    .await
                    .map_err(|error| GatewayError::Native(error.to_string()))?;
                let response = chat_to_codex_response(response).map_err(GatewayError::Bridge)?;
                Ok((StatusCode::OK, Json(response)).into_response())
            }
            Protocol::AnthropicMessages => {
                let streaming = request.get("stream").and_then(Value::as_bool) == Some(true);
                let capabilities = CodexAnthropicCapabilities {
                    reasoning: model
                        .protocol_capabilities
                        .iter()
                        .any(|capability| capability == &ProtocolCapability::ReasoningItems),
                };
                let (request, context) =
                    codex_response_to_anthropic_with_context(request, capabilities)
                        .map_err(GatewayError::Bridge)?;
                let endpoint =
                    build_request_endpoint(&base, provider.endpoint_mode, "/v1/messages").map_err(
                        |_| {
                            GatewayError::Configuration(format!(
                                "invalid provider endpoint: {}",
                                provider.endpoint
                            ))
                        },
                    )?;
                let mut upstream = self
                    .client
                    .post(endpoint)
                    .header("anthropic-version", "2023-06-01")
                    .json(&request);
                upstream = match provider.api_key_placement {
                    ApiKeyPlacement::None => upstream,
                    ApiKeyPlacement::Bearer => upstream.bearer_auth(&provider.api_key),
                    ApiKeyPlacement::XApiKey => upstream.header("x-api-key", &provider.api_key),
                };
                let response = upstream
                    .send()
                    .await
                    .map_err(|error| GatewayError::Native(error.to_string()))?;
                if !response.status().is_success() {
                    return Ok(response_to_axum(response));
                }
                if streaming {
                    let stream = anthropic_sse_to_codex_responses_with_context(
                        response.bytes_stream(),
                        capabilities,
                        context,
                    );
                    let mut response = Response::new(Body::from_stream(stream));
                    response.headers_mut().insert(
                        HeaderName::from_static("content-type"),
                        "text/event-stream".parse().expect("static content type"),
                    );
                    return Ok(response);
                }
                let response = response
                    .json::<Value>()
                    .await
                    .map_err(|error| GatewayError::Native(error.to_string()))?;
                let response =
                    anthropic_to_codex_response_with_context(response, capabilities, &context)
                        .map_err(GatewayError::Bridge)?;
                Ok((StatusCode::OK, Json(response)).into_response())
            }
            Protocol::GeminiNative => Err(GatewayError::Configuration(format!(
                "Codex local routing does not yet support provider protocol {:?}: {}",
                provider.protocol, provider.id
            ))),
        }
    }

    async fn forward_anthropic_provider(
        &self,
        provider: &ProviderRecord,
        headers: HeaderMap,
        request: Value,
    ) -> Result<Response, GatewayError> {
        let base = Url::parse(&provider.endpoint).map_err(|_| {
            GatewayError::Configuration(format!("invalid provider endpoint: {}", provider.endpoint))
        })?;
        let endpoint = build_request_endpoint(&base, provider.endpoint_mode, "/v1/messages")
            .map_err(|_| {
                GatewayError::Configuration(format!(
                    "invalid provider endpoint: {}",
                    provider.endpoint
                ))
            })?;
        let mut upstream = self.client.post(endpoint).json(&request);
        for name in [
            "anthropic-version",
            "anthropic-beta",
            "user-agent",
            "accept",
        ] {
            if let Some(value) = headers.get(name) {
                upstream = upstream.header(name, value);
            }
        }
        upstream = match provider.api_key_placement {
            ApiKeyPlacement::None => upstream,
            ApiKeyPlacement::Bearer => upstream.bearer_auth(&provider.api_key),
            ApiKeyPlacement::XApiKey => upstream.header("x-api-key", &provider.api_key),
        };
        let response = upstream
            .send()
            .await
            .map_err(|error| GatewayError::Native(error.to_string()))?;
        Ok(response_to_axum(response))
    }

    async fn forward_native(
        &self,
        headers: HeaderMap,
        query: Option<&str>,
        request: Value,
    ) -> Result<Response, GatewayError> {
        let native_base_url = self
            .native_base_url
            .read()
            .map_err(|_| GatewayError::Configuration("native route lock is poisoned".into()))?
            .clone();
        let mut endpoint = native_base_url
            .join("/v1/messages")
            .map_err(|_| GatewayError::Configuration("invalid native Anthropic URL".into()))?;
        endpoint.set_query(query);

        let mut upstream = self.client.post(endpoint).json(&request);
        for name in [
            "authorization",
            "x-api-key",
            "anthropic-version",
            "anthropic-beta",
            "user-agent",
            "accept",
        ] {
            if let Some(value) = headers.get(name) {
                upstream = upstream.header(name, value);
            }
        }
        let response = upstream
            .send()
            .await
            .map_err(|error| GatewayError::Native(error.to_string()))?;
        Ok(response_to_axum(response))
    }
}

fn gemini_endpoint(
    base_url: &str,
    upstream_model: &str,
    streaming: bool,
) -> Result<Url, GatewayError> {
    let model = upstream_model
        .strip_prefix("models/")
        .unwrap_or(upstream_model);
    if model.is_empty()
        || !model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(GatewayError::Configuration(format!(
            "invalid Gemini upstream model id: {upstream_model}"
        )));
    }
    let mut endpoint = Url::parse(base_url).map_err(|_| {
        GatewayError::Configuration(format!("invalid provider endpoint: {base_url}"))
    })?;
    let mut path = endpoint.path().trim_end_matches('/').to_string();
    if !path.ends_with("/v1beta") && path != "/v1beta" {
        path.push_str("/v1beta");
    }
    let operation = if streaming {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    endpoint.set_path(&format!("{path}/models/{model}:{operation}"));
    endpoint.set_query(streaming.then_some("alt=sse"));
    Ok(endpoint)
}

fn same_origin_and_path(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
        && left.path().trim_end_matches('/') == right.path().trim_end_matches('/')
}

fn is_claude_safe_model_id(value: &str) -> bool {
    if value != value.trim() || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return false;
    }
    let Some(tail) = value
        .strip_prefix("anthropic/claude-")
        .or_else(|| value.strip_prefix("claude-"))
    else {
        return false;
    };
    ["sonnet-", "opus-", "haiku-", "fable-"]
        .iter()
        .any(|prefix| {
            tail.strip_prefix(prefix)
                .is_some_and(|rest| !rest.is_empty())
        })
}

#[derive(Debug)]
enum GatewayError {
    Unauthorized(String),
    InvalidRequest(String),
    Configuration(String),
    Bridge(BridgeError),
    Native(String),
}

impl GatewayError {
    fn status(&self) -> StatusCode {
        match self {
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::InvalidRequest(_) | Self::Configuration(_) => StatusCode::BAD_REQUEST,
            Self::Bridge(error) => error
                .upstream_http_status()
                .and_then(|status| StatusCode::from_u16(status).ok())
                .unwrap_or(StatusCode::BAD_GATEWAY),
            Self::Native(_) => StatusCode::BAD_GATEWAY,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Unauthorized(client) => format!("{client} gateway authorization failed"),
            Self::InvalidRequest(message)
            | Self::Configuration(message)
            | Self::Native(message) => message.clone(),
            Self::Bridge(error) => error.to_string(),
        }
    }
}

async fn claude_desktop_models(State(gateway): State<Gateway>, headers: HeaderMap) -> Response {
    let active = match gateway.authorized_claude_desktop(&headers) {
        Ok(active) => active,
        Err(error) => return error_response(error),
    };
    let data = active
        .routes
        .iter()
        .filter(|route| is_claude_safe_model_id(&route.route_id))
        .map(|route| {
            let mut item = json!({
                "type": "model",
                "id": route.route_id,
                "created_at": CLAUDE_DESKTOP_CREATED_AT,
            });
            if route.supports_1m {
                item["supports1m"] = Value::Bool(true);
            }
            item
        })
        .collect::<Vec<_>>();
    let first_id = data
        .first()
        .and_then(|route| route.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let last_id = data
        .last()
        .and_then(|route| route.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (
        StatusCode::OK,
        Json(json!({
            "data": data,
            "has_more": false,
            "first_id": first_id,
            "last_id": last_id,
        })),
    )
        .into_response()
}

async fn claude_desktop_messages(
    State(gateway): State<Gateway>,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    let active = match gateway.authorized_claude_desktop(&parts.headers) {
        Ok(active) => active,
        Err(error) => return error_response(error),
    };
    let bytes = match to_bytes(body, 64 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(GatewayError::InvalidRequest(
                "request body could not be read".into(),
            ));
        }
    };
    let request: Value = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => {
            return error_response(GatewayError::InvalidRequest(
                "request body must be valid JSON".into(),
            ));
        }
    };
    let route_id = match request.get("model").and_then(Value::as_str) {
        Some(route_id) => route_id,
        None => {
            return error_response(GatewayError::InvalidRequest(
                "model must be a string".into(),
            ));
        }
    };
    let model_id = match active
        .routes
        .iter()
        .find(|route| route.route_id == route_id)
        .map(|route| route.model_id.clone())
    {
        Some(model_id) => model_id,
        None => {
            return error_response(GatewayError::InvalidRequest(format!(
                "unknown Claude Desktop route: {route_id}"
            )));
        }
    };

    match gateway
        .complete_configured_model(parts.headers, request, active.documents, &model_id)
        .await
    {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

async fn pi_messages(State(gateway): State<Gateway>, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    let active = match gateway.authorized_pi(&parts.headers) {
        Ok(active) => active,
        Err(error) => return error_response(error),
    };
    let bytes = match to_bytes(body, 64 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(GatewayError::InvalidRequest(
                "request body could not be read".into(),
            ));
        }
    };
    let request: Value = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => {
            return error_response(GatewayError::InvalidRequest(
                "request body must be valid JSON".into(),
            ));
        }
    };
    let alias = match request.get("model").and_then(Value::as_str) {
        Some(alias) => alias.to_string(),
        None => {
            return error_response(GatewayError::InvalidRequest(
                "model must be a string".into(),
            ));
        }
    };
    let Some(model_id) = alias.strip_prefix("grillforge/").map(str::to_string) else {
        return error_response(GatewayError::InvalidRequest(format!(
            "unknown GrillForge route alias: {alias}"
        )));
    };
    if !active.allowed_model_ids.contains(&model_id) {
        return error_response(GatewayError::InvalidRequest(format!(
            "inactive Pi route alias: {alias}"
        )));
    }
    match gateway
        .complete_configured_model(parts.headers, request, active.documents, &model_id)
        .await
    {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

async fn codex_responses(State(gateway): State<Gateway>, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    let active = match gateway.authorized_codex(&parts.headers) {
        Ok(active) => active,
        Err(error) => return error_response(error),
    };
    let bytes = match to_bytes(body, 64 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(GatewayError::InvalidRequest(
                "request body could not be read".into(),
            ));
        }
    };
    let request: Value = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => {
            return error_response(GatewayError::InvalidRequest(
                "request body must be valid JSON".into(),
            ));
        }
    };
    let alias = match request.get("model").and_then(Value::as_str) {
        Some(alias) => alias.to_string(),
        None => {
            return error_response(GatewayError::InvalidRequest(
                "model must be a string".into(),
            ));
        }
    };
    let Some(model_id) = alias.strip_prefix("grillforge/").map(str::to_string) else {
        return error_response(GatewayError::InvalidRequest(format!(
            "unknown GrillForge route alias: {alias}"
        )));
    };
    if !active.allowed_model_ids.contains(&model_id) {
        return error_response(GatewayError::InvalidRequest(format!(
            "inactive Codex route alias: {alias}"
        )));
    }
    match gateway
        .complete_codex_model(request, active.documents, &model_id)
        .await
    {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

async fn client_messages(
    State(gateway): State<Gateway>,
    AxumPath(client): AxumPath<String>,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    let active = match gateway.authorized_client(&client, &parts.headers) {
        Ok(active) => active,
        Err(error) => return error_response(error),
    };
    let bytes = match to_bytes(body, 64 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(GatewayError::InvalidRequest(
                "request body could not be read".into(),
            ));
        }
    };
    let request: Value = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => {
            return error_response(GatewayError::InvalidRequest(
                "request body must be valid JSON".into(),
            ));
        }
    };
    let alias = match request.get("model").and_then(Value::as_str) {
        Some(alias) => alias.to_string(),
        None => {
            return error_response(GatewayError::InvalidRequest(
                "model must be a string".into(),
            ));
        }
    };
    let Some(model_id) = alias.strip_prefix("grillforge/").map(str::to_string) else {
        return error_response(GatewayError::InvalidRequest(format!(
            "unknown GrillForge route alias: {alias}"
        )));
    };
    if !active.allowed_model_ids.contains(&model_id) {
        return error_response(GatewayError::InvalidRequest(format!(
            "inactive {client} route alias: {alias}"
        )));
    }
    match gateway
        .complete_configured_model(parts.headers, request, active.documents, &model_id)
        .await
    {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

async fn messages(
    State(gateway): State<Gateway>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    let managed = request
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| model.starts_with("grillforge/"));
    if !managed {
        return match gateway.forward_native(headers, uri.query(), request).await {
            Ok(response) => response,
            Err(error) => error_response(error),
        };
    }

    match gateway.complete_external(headers, request).await {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

fn response_to_axum(response: reqwest::Response) -> Response {
    let status = response.status();
    let content_type = response.headers().get("content-type").cloned();
    let mut reply = Response::new(Body::from_stream(response.bytes_stream()));
    *reply.status_mut() = status;
    if let Some(value) = content_type {
        reply
            .headers_mut()
            .insert(HeaderName::from_static("content-type"), value);
    }
    reply
}

fn error_response(error: GatewayError) -> Response {
    let status = error.status();
    let kind = match status {
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::BAD_REQUEST => "invalid_request_error",
        _ => "api_error",
    };
    (
        status,
        Json(json!({
            "type": "error",
            "error": {"type": kind, "message": error.message()}
        })),
    )
        .into_response()
}

#[tauri::command]
pub fn gateway_status(status: tauri::State<'_, GatewayStatus>) -> GatewayStatus {
    status.inner().clone()
}
