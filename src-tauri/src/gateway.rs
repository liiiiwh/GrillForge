use crate::application::ControlPlaneState;
use crate::bridge::{
    BridgeError, CodexAnthropicCapabilities, CodexHistoryStore, GeminiNativeBridge,
    GeminiThoughtStore, OpenAiChatBridge, OpenAiChatCapabilities, OpenAiResponsesBridge,
    OpenAiResponsesCapabilities, anthropic_response_to_chat, anthropic_response_to_gemini,
    anthropic_sse_to_chat, anthropic_sse_to_codex_responses_with_context, anthropic_sse_to_gemini,
    anthropic_to_codex_response_with_context, chat_request_to_anthropic,
    chat_sse_to_codex_responses_with_context, chat_to_codex_response_with_context,
    codex_response_to_anthropic_with_context, codex_response_to_chat_with_context,
    flatten_codex_namespaces, record_codex_sse, restore_codex_namespace_sse,
    restore_codex_namespaces, sanitize_xai_responses_request,
};
use crate::configuration::{
    ConfigurationDocuments, ConfigurationFiles, ProviderProtocolEndpoint, ProviderRecord,
};
use crate::core::model::{NativeProtocol, ProtocolCapability};
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
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::mpsc;
use url::Url;

pub const DEFAULT_GATEWAY_ADDRESS: &str = "127.0.0.1:15721";
const OFFICIAL_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const CLAUDE_DESKTOP_CREATED_AT: &str = "2024-01-01T00:00:00Z";
const AGENT_RUNTIME_TIMEOUT_SECONDS: u64 = 3 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteSpec {
    pub route_id: String,
    pub model_id: String,
    pub label_override: Option<String>,
    pub supports_1m: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRuntimeRoute {
    pub extension_id: String,
    pub source_client_id: String,
    pub source_agent_id: String,
    pub model_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSourceRuntime {
    pub source_client_id: String,
    pub runtime: PathBuf,
    pub config_root: PathBuf,
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
    active_response_client_routes: Arc<RwLock<HashMap<String, ActiveCodexRoutes>>>,
    #[serde(skip)]
    active_client_routes: Arc<RwLock<HashMap<String, ActiveClientRoutes>>>,
    #[serde(skip)]
    active_agent_brokers: Arc<RwLock<HashMap<String, ActiveAgentBroker>>>,
    #[serde(skip)]
    active_agent_runtime_routes: Arc<Mutex<HashMap<String, ActiveAgentRuntimeRoute>>>,
    #[serde(skip)]
    active_agent_runs: Arc<Mutex<HashMap<String, AgentRun>>>,
    #[serde(skip)]
    active_agent_permissions: Arc<Mutex<HashMap<String, PermissionRequest>>>,
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
            active_response_client_routes: Arc::clone(&gateway.active_response_client_routes),
            active_client_routes: Arc::clone(&gateway.active_client_routes),
            active_agent_brokers: Arc::clone(&gateway.active_agent_brokers),
            active_agent_runtime_routes: Arc::clone(&gateway.active_agent_runtime_routes),
            active_agent_runs: Arc::clone(&gateway.active_agent_runs),
            active_agent_permissions: Arc::clone(&gateway.active_agent_permissions),
            connection_tests: Arc::clone(&gateway.connection_tests),
        }
    }

    pub fn activate(&self, state: &ControlPlaneState) -> Result<(), String> {
        let mut allowed_model_ids = HashSet::new();
        if let Some(main) = &state.main_model_id {
            allowed_model_ids.insert(main.clone());
        }
        allowed_model_ids.extend(state.model_slots.values().cloned());
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

    pub fn activate_client_agent_broker(
        &self,
        client_id: &str,
        state: &ControlPlaneState,
        token: &str,
        runtime: &Path,
        runtime_config_root: &Path,
        routes: Vec<AgentRuntimeRoute>,
    ) -> Result<(), String> {
        self.activate_client_agent_broker_with_sources(
            client_id,
            state,
            token,
            vec![AgentSourceRuntime {
                source_client_id: "claude_code".into(),
                runtime: runtime.to_path_buf(),
                config_root: runtime_config_root.to_path_buf(),
            }],
            routes,
        )
    }

    pub fn activate_client_agent_broker_with_sources(
        &self,
        client_id: &str,
        state: &ControlPlaneState,
        token: &str,
        source_runtimes: Vec<AgentSourceRuntime>,
        routes: Vec<AgentRuntimeRoute>,
    ) -> Result<(), String> {
        validate_agent_client_id(client_id)?;
        if token.is_empty() || token.trim() != token || token.chars().any(char::is_control) {
            return Err(
                "Agent broker token must not be empty, padded, or contain control characters"
                    .into(),
            );
        }
        let mut runtimes = HashMap::new();
        for source in source_runtimes {
            if !matches!(
                source.source_client_id.as_str(),
                "claude_code" | "codex" | "gemini" | "pi" | "opencode" | "kimi_code" | "grok_build"
            ) {
                return Err(format!(
                    "unsupported Agent source client: {}",
                    source.source_client_id
                ));
            }
            if !source.runtime.is_absolute() || !source.runtime.is_file() {
                return Err(format!(
                    "{} runtime does not exist: {}",
                    source.source_client_id,
                    source.runtime.display()
                ));
            }
            if !source.config_root.is_absolute()
                || (!source.config_root.is_dir()
                    && !matches!(source.source_client_id.as_str(), "opencode" | "kimi_code"))
            {
                return Err(format!(
                    "{} configuration root does not exist: {}",
                    source.source_client_id,
                    source.config_root.display()
                ));
            }
            if runtimes
                .insert(source.source_client_id.clone(), source)
                .is_some()
            {
                return Err("duplicate Agent source runtime".into());
            }
        }
        let documents = self.files.read().map_err(|error| error.to_string())?;
        let mut extension_ids = HashSet::new();
        for route in &routes {
            validate_extension_id(&route.extension_id)?;
            if !extension_ids.insert(route.extension_id.clone()) {
                return Err(format!(
                    "duplicate Agent broker extension: {}",
                    route.extension_id
                ));
            }
            if !runtimes.contains_key(&route.source_client_id) {
                return Err(format!(
                    "Agent source runtime is unavailable: {}",
                    route.source_client_id
                ));
            }
            validate_source_agent_id(&route.source_agent_id)?;
            if let Some(model_id) = &route.model_id {
                let model = documents
                    .models
                    .models
                    .iter()
                    .find(|model| &model.id == model_id)
                    .ok_or_else(|| {
                        format!(
                            "extension {} references unknown model {}",
                            route.extension_id, model_id
                        )
                    })?;
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
                if !state
                    .models
                    .iter()
                    .any(|candidate| candidate.id == model.id)
                {
                    return Err(format!(
                        "extension {} references an inactive model {}",
                        route.extension_id, model.id
                    ));
                }
            }
        }
        self.active_agent_brokers
            .write()
            .map_err(|_| "active Agent broker lock is poisoned".to_string())?
            .insert(
                client_id.to_string(),
                ActiveAgentBroker {
                    target_client_id: client_id.to_string(),
                    documents,
                    routes,
                    token: token.to_string(),
                    source_runtimes: runtimes,
                    base_url: self.base_url.clone(),
                    runtime_routes: Arc::clone(&self.active_agent_runtime_routes),
                    runs: Arc::clone(&self.active_agent_runs),
                    permissions: Arc::clone(&self.active_agent_permissions),
                },
            );
        Ok(())
    }

    pub fn deactivate_client_agent_broker(&self, client_id: &str) {
        if let Ok(mut active) = self.active_agent_brokers.write() {
            active.remove(client_id);
        }
        if let Ok(mut routes) = self.active_agent_runtime_routes.lock() {
            routes.retain(|_, route| route.target_client_id != client_id);
        }
        // An unmounted client must not leave a child running.
        if let Ok(mut runs) = self.active_agent_runs.lock() {
            runs.retain(|_, run| {
                if run.client_id == client_id {
                    if let Some(task) = run.task.take() {
                        task.abort();
                    }
                    return false;
                }
                true
            });
        }
    }

    pub fn agent_broker_routes_for_client(
        &self,
        client_id: &str,
    ) -> Result<Vec<AgentRuntimeRoute>, String> {
        self.active_agent_brokers
            .read()
            .map_err(|_| "active Agent broker lock is poisoned".to_string())?
            .get(client_id)
            .map(|active| active.routes.clone())
            .ok_or_else(|| format!("Agent broker is not active for {client_id}"))
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
            let is_managed_route = route.route_id.starts_with("grillforge/");
            if !is_safe_route && !is_managed_route {
                return Err(format!(
                    "Claude Desktop route must use a Claude-safe model id or GrillForge model alias: {}",
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
            if is_managed_route && route.route_id != format!("grillforge/{}", model.id) {
                return Err(format!(
                    "Claude Desktop model alias {} does not match model {}",
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
        let routes = self.validated_codex_routes(model_ids, token, "Codex")?;
        *self
            .active_codex_routes
            .write()
            .map_err(|_| "active Codex route lock is poisoned".to_string())? = Some(routes);
        Ok(())
    }

    fn validated_codex_routes(
        &self,
        model_ids: Vec<String>,
        token: &str,
        client_name: &str,
    ) -> Result<ActiveCodexRoutes, String> {
        if token.is_empty() || token.trim() != token || token.chars().any(char::is_control) {
            return Err(format!(
                "{client_name} gateway token must not be empty, padded, or contain control characters"
            ));
        }
        if model_ids.is_empty() {
            return Err(format!("{client_name} requires at least one model route"));
        }
        let allowed_model_ids = model_ids.into_iter().collect::<HashSet<_>>();
        let documents = self.files.read().map_err(|error| error.to_string())?;
        for id in &allowed_model_ids {
            let model = documents
                .models
                .models
                .iter()
                .find(|model| &model.id == id)
                .ok_or_else(|| format!("{client_name} route references unknown model: {id}"))?;
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
        Ok(ActiveCodexRoutes {
            documents,
            allowed_model_ids,
            token: token.to_string(),
        })
    }

    pub fn deactivate_codex(&self) {
        if let Ok(mut active) = self.active_codex_routes.write() {
            *active = None;
        }
    }

    pub fn activate_response_client(
        &self,
        client_id: &str,
        model_ids: Vec<String>,
        token: &str,
    ) -> Result<(), String> {
        if client_id != "grok-build" {
            return Err(format!("unsupported Responses gateway client: {client_id}"));
        }
        let routes = self.validated_codex_routes(model_ids, token, client_id)?;
        self.active_response_client_routes
            .write()
            .map_err(|_| "active Responses client route lock is poisoned".to_string())?
            .insert(client_id.to_string(), routes);
        Ok(())
    }

    pub fn deactivate_response_client(&self, client_id: &str) {
        if let Ok(mut active) = self.active_response_client_routes.write() {
            active.remove(client_id);
        }
    }

    pub fn activate_client(
        &self,
        client_id: &str,
        model_ids: Vec<String>,
        token: &str,
    ) -> Result<(), String> {
        if !matches!(
            client_id,
            "gemini" | "opencode" | "hermes" | "kimi-code" | "dsh"
        ) {
            return Err(format!("unsupported gateway client: {client_id}"));
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
struct ActiveAgentBroker {
    target_client_id: String,
    documents: ConfigurationDocuments,
    routes: Vec<AgentRuntimeRoute>,
    token: String,
    source_runtimes: HashMap<String, AgentSourceRuntime>,
    base_url: String,
    runtime_routes: Arc<Mutex<HashMap<String, ActiveAgentRuntimeRoute>>>,
    runs: Arc<Mutex<HashMap<String, AgentRun>>>,
    permissions: Arc<Mutex<HashMap<String, PermissionRequest>>>,
}

/// A tool call the child is holding on, waiting for the parent to decide.
struct PermissionRequest {
    run_id: String,
    tool_name: String,
    input: Value,
    decision: Option<Value>,
    notify: Arc<tokio::sync::Notify>,
}

/// A child that is never answered must not hang forever.
const AGENT_PERMISSION_TIMEOUT: Duration = Duration::from_secs(600);

/// One delegated run. It outlives the request that started it, so the child is
/// owned by a task rather than by the request future.
struct AgentRun {
    client_id: String,
    permission_secret: String,
    outcome: Option<Result<String, String>>,
    progress: Option<String>,
    finished_at: Option<Instant>,
    task: Option<tokio::task::JoinHandle<()>>,
    notify: Arc<tokio::sync::Notify>,
}

/// A result nobody collects is dropped rather than retained forever.
const AGENT_RUN_RETENTION: Duration = Duration::from_secs(3600);
const AGENT_RUN_MAX_WAIT_SECONDS: u64 = 300;

/// How long a collect waits when the caller names no interval. Collecting is
/// almost always the caller's next move after starting a run, so without a wait
/// it reports a run that has barely begun as `running` to a caller that then
/// treats its turn as finished.
///
/// It has to hold for every mounted client, so it stays under the shortest
/// tool-call budget any of them applies: 60 seconds, the MCP default that a
/// client aborts a silent call at. A caller that knows its own client tolerates
/// more passes `waitSeconds`, up to [`AGENT_RUN_MAX_WAIT_SECONDS`].
const AGENT_RUN_DEFAULT_WAIT_SECONDS: u64 = 45;

/// What the caller still owes on an unfinished run. It travels with the payload
/// because that is where the caller decides whether the turn is done.
const AGENT_RUN_COLLECT_OBLIGATION: &str =
    "call get_agent_result with this runId until it returns completed; the result cannot reach you after you answer the user";

/// Every tool the broker offers. A client that is only allowed to call some of
/// these cannot honour the run contract: `run_agent` hands back a handle, so a
/// client without `get_agent_result` starts runs it can never collect.
pub const AGENT_MCP_TOOLS: [&str; 5] = [
    "list_agents",
    "run_agent",
    "get_agent_result",
    "answer_agent_permission",
    "stop_agent",
];

#[derive(Clone)]
struct ActiveAgentRuntimeRoute {
    documents: ConfigurationDocuments,
    model_id: String,
    target_client_id: String,
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
    active_response_client_routes: Arc<RwLock<HashMap<String, ActiveCodexRoutes>>>,
    active_client_routes: Arc<RwLock<HashMap<String, ActiveClientRoutes>>>,
    active_agent_brokers: Arc<RwLock<HashMap<String, ActiveAgentBroker>>>,
    active_agent_runtime_routes: Arc<Mutex<HashMap<String, ActiveAgentRuntimeRoute>>>,
    active_agent_runs: Arc<Mutex<HashMap<String, AgentRun>>>,
    active_agent_permissions: Arc<Mutex<HashMap<String, PermissionRequest>>>,
    connection_tests: Arc<Mutex<HashSet<String>>>,
    codex_history: Arc<CodexHistoryStore>,
    gemini_thoughts: Arc<GeminiThoughtStore>,
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
            active_response_client_routes: Arc::new(RwLock::new(HashMap::new())),
            active_client_routes: Arc::new(RwLock::new(HashMap::new())),
            active_agent_brokers: Arc::new(RwLock::new(HashMap::new())),
            active_agent_runtime_routes: Arc::new(Mutex::new(HashMap::new())),
            active_agent_runs: Arc::new(Mutex::new(HashMap::new())),
            active_agent_permissions: Arc::new(Mutex::new(HashMap::new())),
            connection_tests: Arc::new(Mutex::new(HashSet::new())),
            codex_history: Arc::new(CodexHistoryStore::default()),
            gemini_thoughts: Arc::new(GeminiThoughtStore::default()),
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
            .route(
                "/responses/{client}/v1/responses",
                post(response_client_responses),
            )
            .route(
                "/chat/{client}/v1/chat/completions",
                post(chat_client_completions),
            )
            .route(
                "/gemini/v1beta/models/{*operation}",
                post(gemini_model_operation),
            )
            .route(
                "/agent-runtime/gemini/v1beta/models/{*operation}",
                post(gemini_agent_model_operation),
            )
            .route("/clients/{client}/v1/messages", post(client_messages))
            .route("/mcp/{client}", post(agent_broker_mcp))
            .route("/mcp/agent-permission/{run}", post(agent_permission_mcp))
            .route("/agent-runtime/v1/messages", post(agent_runtime_messages))
            .route("/agent-runtime/v1/responses", post(agent_runtime_responses))
            .route(
                "/agent-runtime/v1/chat/completions",
                post(agent_runtime_chat_completions),
            )
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
        let bearer_authorized = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == expected);
        let api_key_authorized = client_id == "gemini"
            && headers
                .get("x-goog-api-key")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value == active.token);
        if !bearer_authorized && !api_key_authorized {
            return Err(GatewayError::Unauthorized(client_id.to_owned()));
        }
        Ok(active)
    }

    fn authorized_agent_broker(
        &self,
        client_id: &str,
        headers: &HeaderMap,
    ) -> Result<ActiveAgentBroker, GatewayError> {
        let active = self
            .active_agent_brokers
            .read()
            .map_err(|_| {
                GatewayError::Configuration("active Agent broker lock is poisoned".into())
            })?
            .get(client_id)
            .cloned()
            .ok_or_else(|| GatewayError::Unauthorized(format!("{client_id} Agent broker")))?;
        let expected = format!("Bearer {}", active.token);
        let authorized = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == expected);
        if !authorized {
            return Err(GatewayError::Unauthorized(format!(
                "{client_id} Agent broker"
            )));
        }
        Ok(active)
    }

    async fn complete_configured_model(
        &self,
        headers: HeaderMap,
        request: Value,
        documents: ConfigurationDocuments,
        model_id: &str,
    ) -> Result<Response, GatewayError> {
        self.complete_configured_model_for_inbound(
            headers,
            request,
            documents,
            model_id,
            NativeProtocol::AnthropicMessages,
        )
        .await
    }

    async fn complete_configured_model_for_inbound(
        &self,
        headers: HeaderMap,
        mut request: Value,
        documents: ConfigurationDocuments,
        model_id: &str,
        inbound: NativeProtocol,
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
        let protocol = select_model_protocol(&documents, model_id, inbound)?;
        let surface = provider_protocol_endpoint(provider, protocol)?;
        match protocol {
            Protocol::OpenAiResponses => {
                let base = Url::parse(&surface.endpoint).map_err(|_| {
                    GatewayError::Configuration(format!(
                        "invalid provider endpoint: {}",
                        surface.endpoint
                    ))
                })?;
                let endpoint =
                    build_request_endpoint(&base, surface.endpoint_mode, "/v1/responses").map_err(
                        |_| {
                            GatewayError::Configuration(format!(
                                "invalid provider endpoint: {}",
                                surface.endpoint
                            ))
                        },
                    )?;
                let bridge = match surface.api_key_placement {
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
                self.forward_anthropic_provider(provider, surface, headers, request)
                    .await
            }
            Protocol::OpenAiChatCompletions => {
                let base = Url::parse(&surface.endpoint).map_err(|_| {
                    GatewayError::Configuration(format!(
                        "invalid provider endpoint: {}",
                        surface.endpoint
                    ))
                })?;
                let endpoint =
                    build_request_endpoint(&base, surface.endpoint_mode, "/v1/chat/completions")
                        .map_err(|_| {
                            GatewayError::Configuration(format!(
                                "invalid provider endpoint: {}",
                                surface.endpoint
                            ))
                        })?;
                let bridge = match surface.api_key_placement {
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
                if surface.endpoint_mode != EndpointMode::BaseUrl
                    || surface.api_key_placement != ApiKeyPlacement::XApiKey
                {
                    return Err(GatewayError::Configuration(format!(
                        "Gemini Native provider {} requires an API-key Base URL",
                        provider.id
                    )));
                }
                let streaming = request.get("stream").and_then(Value::as_bool) == Some(true);
                let endpoint = gemini_endpoint(&surface.endpoint, &model.upstream_id, streaming)?;
                let bridge = GeminiNativeBridge::from_endpoint(endpoint, &provider.api_key)
                    .with_thought_store(
                        Arc::clone(&self.gemini_thoughts),
                        format!("{}:{}", provider.id, model.id),
                    );
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

    async fn complete_chat_model(
        &self,
        headers: HeaderMap,
        mut request: Value,
        documents: ConfigurationDocuments,
        model_id: &str,
    ) -> Result<Response, GatewayError> {
        if select_model_protocol(&documents, model_id, NativeProtocol::OpenAiChat)?
            == Protocol::OpenAiChatCompletions
        {
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
            let surface = provider_protocol_endpoint(provider, Protocol::OpenAiChatCompletions)?;
            let base = Url::parse(&surface.endpoint).map_err(|_| {
                GatewayError::Configuration(format!(
                    "invalid provider endpoint: {}",
                    surface.endpoint
                ))
            })?;
            let endpoint =
                build_request_endpoint(&base, surface.endpoint_mode, "/v1/chat/completions")
                    .map_err(|_| {
                        GatewayError::Configuration(format!(
                            "invalid provider endpoint: {}",
                            surface.endpoint
                        ))
                    })?;
            let mut upstream = self.client.post(endpoint).json(&request);
            upstream = match surface.api_key_placement {
                ApiKeyPlacement::None => upstream,
                ApiKeyPlacement::Bearer => upstream.bearer_auth(&provider.api_key),
                ApiKeyPlacement::XApiKey => {
                    return Err(GatewayError::Configuration(format!(
                        "Chat Completions provider {} has incompatible authentication",
                        provider.id
                    )));
                }
            };
            let response = upstream
                .send()
                .await
                .map_err(|error| GatewayError::Native(error.to_string()))?;
            return Ok(response_to_axum(response));
        }
        let streaming = request.get("stream").and_then(Value::as_bool) == Some(true);
        let request = chat_request_to_anthropic(request).map_err(GatewayError::Bridge)?;
        let response = self
            .complete_configured_model_for_inbound(
                headers,
                request,
                documents,
                model_id,
                NativeProtocol::OpenAiChat,
            )
            .await?;
        if !response.status().is_success() {
            return Ok(response);
        }
        if streaming {
            let stream = anthropic_sse_to_chat(response.into_body().into_data_stream());
            let mut response = Response::new(Body::from_stream(stream));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                "text/event-stream".parse().expect("static content type"),
            );
            return Ok(response);
        }
        let bytes = to_bytes(response.into_body(), 64 * 1024 * 1024)
            .await
            .map_err(|_| {
                GatewayError::Bridge(BridgeError::InvalidChatResponse(
                    "Anthropic response body could not be read".into(),
                ))
            })?;
        let anthropic = serde_json::from_slice::<Value>(&bytes).map_err(|_| {
            GatewayError::Bridge(BridgeError::InvalidChatResponse(
                "Anthropic response body must be valid JSON".into(),
            ))
        })?;
        let response = anthropic_response_to_chat(anthropic).map_err(GatewayError::Bridge)?;
        Ok((StatusCode::OK, Json(response)).into_response())
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
        let protocol =
            select_model_protocol(&documents, model_id, NativeProtocol::OpenAiResponses)?;
        if protocol != Protocol::OpenAiResponses {
            self.codex_history.enrich_request(&mut request).await;
        }
        let namespace_map = flatten_codex_namespaces(&mut request).map_err(GatewayError::Bridge)?;
        let surface = provider_protocol_endpoint(provider, protocol)?;
        let base = Url::parse(&surface.endpoint).map_err(|_| {
            GatewayError::Configuration(format!("invalid provider endpoint: {}", surface.endpoint))
        })?;
        match protocol {
            Protocol::OpenAiResponses => {
                if base
                    .host_str()
                    .is_some_and(|host| host.eq_ignore_ascii_case("api.x.ai"))
                {
                    sanitize_xai_responses_request(&mut request);
                }
                let streaming = request.get("stream").and_then(Value::as_bool) == Some(true);
                let endpoint =
                    build_request_endpoint(&base, surface.endpoint_mode, "/v1/responses").map_err(
                        |_| {
                            GatewayError::Configuration(format!(
                                "invalid provider endpoint: {}",
                                surface.endpoint
                            ))
                        },
                    )?;
                let mut upstream = self.client.post(endpoint).json(&request);
                upstream = match surface.api_key_placement {
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
                if !response.status().is_success() || namespace_map.is_empty() {
                    return Ok(response_to_axum(response));
                }
                if streaming {
                    let stream = record_codex_sse(
                        restore_codex_namespace_sse(response.bytes_stream(), namespace_map),
                        Arc::clone(&self.codex_history),
                    );
                    let mut response = Response::new(Body::from_stream(stream));
                    response.headers_mut().insert(
                        HeaderName::from_static("content-type"),
                        "text/event-stream".parse().expect("static content type"),
                    );
                    return Ok(response);
                }
                let mut response = response
                    .json::<Value>()
                    .await
                    .map_err(|error| GatewayError::Native(error.to_string()))?;
                restore_codex_namespaces(&mut response, &namespace_map);
                self.codex_history.record_response(&response).await;
                Ok((StatusCode::OK, Json(response)).into_response())
            }
            Protocol::OpenAiChatCompletions => {
                let streaming = request.get("stream").and_then(Value::as_bool) == Some(true);
                let endpoint =
                    build_request_endpoint(&base, surface.endpoint_mode, "/v1/chat/completions")
                        .map_err(|_| {
                            GatewayError::Configuration(format!(
                                "invalid provider endpoint: {}",
                                surface.endpoint
                            ))
                        })?;
                let (upstream_request, tool_context) =
                    codex_response_to_chat_with_context(request).map_err(GatewayError::Bridge)?;
                let mut upstream = self.client.post(endpoint).json(&upstream_request);
                upstream = match surface.api_key_placement {
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
                    let stream = record_codex_sse(
                        restore_codex_namespace_sse(
                            chat_sse_to_codex_responses_with_context(
                                response.bytes_stream(),
                                tool_context,
                            ),
                            namespace_map,
                        ),
                        Arc::clone(&self.codex_history),
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
                let mut response = chat_to_codex_response_with_context(response, &tool_context)
                    .map_err(GatewayError::Bridge)?;
                restore_codex_namespaces(&mut response, &namespace_map);
                self.codex_history.record_response(&response).await;
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
                let endpoint = build_request_endpoint(&base, surface.endpoint_mode, "/v1/messages")
                    .map_err(|_| {
                        GatewayError::Configuration(format!(
                            "invalid provider endpoint: {}",
                            surface.endpoint
                        ))
                    })?;
                let mut upstream = self
                    .client
                    .post(endpoint)
                    .header("anthropic-version", "2023-06-01")
                    .json(&request);
                upstream = match surface.api_key_placement {
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
                    let stream = record_codex_sse(
                        restore_codex_namespace_sse(
                            anthropic_sse_to_codex_responses_with_context(
                                response.bytes_stream(),
                                capabilities,
                                context,
                            ),
                            namespace_map,
                        ),
                        Arc::clone(&self.codex_history),
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
                let mut response =
                    anthropic_to_codex_response_with_context(response, capabilities, &context)
                        .map_err(GatewayError::Bridge)?;
                restore_codex_namespaces(&mut response, &namespace_map);
                self.codex_history.record_response(&response).await;
                Ok((StatusCode::OK, Json(response)).into_response())
            }
            Protocol::GeminiNative => {
                if surface.endpoint_mode != EndpointMode::BaseUrl
                    || surface.api_key_placement != ApiKeyPlacement::XApiKey
                {
                    return Err(GatewayError::Configuration(format!(
                        "Gemini Native provider {} requires an API-key Base URL",
                        provider.id
                    )));
                }
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
                let endpoint = gemini_endpoint(&surface.endpoint, &model.upstream_id, streaming)?;
                let bridge = GeminiNativeBridge::from_endpoint(endpoint, &provider.api_key)
                    .with_thought_store(
                        Arc::clone(&self.gemini_thoughts),
                        format!("{}:{}", provider.id, model.id),
                    );
                if streaming {
                    let stream = bridge.stream(request).await.map_err(GatewayError::Bridge)?;
                    let stream = record_codex_sse(
                        restore_codex_namespace_sse(
                            anthropic_sse_to_codex_responses_with_context(
                                stream,
                                capabilities,
                                context,
                            ),
                            namespace_map,
                        ),
                        Arc::clone(&self.codex_history),
                    );
                    let mut response = Response::new(Body::from_stream(stream));
                    response.headers_mut().insert(
                        HeaderName::from_static("content-type"),
                        "text/event-stream".parse().expect("static content type"),
                    );
                    return Ok(response);
                }
                let response = bridge
                    .complete(request)
                    .await
                    .map_err(GatewayError::Bridge)?;
                let mut response =
                    anthropic_to_codex_response_with_context(response, capabilities, &context)
                        .map_err(GatewayError::Bridge)?;
                restore_codex_namespaces(&mut response, &namespace_map);
                self.codex_history.record_response(&response).await;
                Ok((StatusCode::OK, Json(response)).into_response())
            }
        }
    }

    async fn forward_anthropic_provider(
        &self,
        provider: &ProviderRecord,
        surface: &ProviderProtocolEndpoint,
        headers: HeaderMap,
        request: Value,
    ) -> Result<Response, GatewayError> {
        let base = Url::parse(&surface.endpoint).map_err(|_| {
            GatewayError::Configuration(format!("invalid provider endpoint: {}", surface.endpoint))
        })?;
        let endpoint = build_request_endpoint(&base, surface.endpoint_mode, "/v1/messages")
            .map_err(|_| {
                GatewayError::Configuration(format!(
                    "invalid provider endpoint: {}",
                    surface.endpoint
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
        upstream = match surface.api_key_placement {
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

fn validate_agent_client_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(format!("invalid Agent broker client id: {value}"));
    }
    Ok(())
}

fn validate_extension_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.trim() != value
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(format!("invalid extension SubAgent id: {value}"));
    }
    Ok(())
}

fn validate_source_agent_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || !value.split(['/', ':']).all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
    {
        return Err(format!("invalid source Agent id: {value}"));
    }
    Ok(())
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

async fn response_client_responses(
    State(gateway): State<Gateway>,
    AxumPath(client): AxumPath<String>,
    request: Request<Body>,
) -> Response {
    let active = match gateway
        .active_response_client_routes
        .read()
        .map_err(|_| GatewayError::Configuration("Responses client route lock is poisoned".into()))
        .and_then(|routes| {
            routes
                .get(&client)
                .cloned()
                .ok_or_else(|| GatewayError::Unauthorized(client.clone()))
        }) {
        Ok(active) => active,
        Err(error) => return error_response(error),
    };
    let (parts, body) = request.into_parts();
    let expected = format!("Bearer {}", active.token);
    if parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return error_response(GatewayError::Unauthorized(client));
    }
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
        .complete_codex_model(request, active.documents, &model_id)
        .await
    {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

async fn chat_client_completions(
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
        .complete_chat_model(parts.headers, request, active.documents, &model_id)
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

async fn gemini_model_operation(
    State(gateway): State<Gateway>,
    AxumPath(operation): AxumPath<String>,
    request: Request<Body>,
) -> Response {
    if let Some(model) = operation.strip_suffix(":generateContent") {
        return gemini_client_request(gateway, model.to_string(), request, false).await;
    }
    if let Some(model) = operation.strip_suffix(":streamGenerateContent") {
        return gemini_client_request(gateway, model.to_string(), request, true).await;
    }
    error_response(GatewayError::InvalidRequest(format!(
        "unsupported Gemini model operation: {operation}"
    )))
}

async fn gemini_client_request(
    gateway: Gateway,
    alias: String,
    request: Request<Body>,
    streaming: bool,
) -> Response {
    let active = match gateway.authorized_client("gemini", request.headers()) {
        Ok(active) => active,
        Err(error) => return error_response(error),
    };
    let Some(model_id) = alias.strip_prefix("grillforge--").map(str::to_string) else {
        return error_response(GatewayError::InvalidRequest(format!(
            "unknown GrillForge Gemini route alias: {alias}"
        )));
    };
    if !active.allowed_model_ids.contains(&model_id) {
        return error_response(GatewayError::InvalidRequest(format!(
            "inactive Gemini route alias: {alias}"
        )));
    }
    gemini_configured_request(gateway, model_id, request, streaming, active.documents).await
}

async fn gemini_agent_model_operation(
    State(gateway): State<Gateway>,
    AxumPath(operation): AxumPath<String>,
    request: Request<Body>,
) -> Response {
    let (alias, streaming) = if let Some(model) = operation.strip_suffix(":generateContent") {
        (model, false)
    } else if let Some(model) = operation.strip_suffix(":streamGenerateContent") {
        (model, true)
    } else {
        return error_response(GatewayError::InvalidRequest(format!(
            "unsupported Gemini Agent model operation: {operation}"
        )));
    };
    let Some(token) = local_runtime_token(request.headers()) else {
        return error_response(GatewayError::Unauthorized("Gemini Agent runtime".into()));
    };
    let active = match gateway.active_agent_runtime_routes.lock() {
        Ok(routes) => routes.get(token).cloned(),
        Err(_) => {
            return error_response(GatewayError::Configuration(
                "active Agent runtime route lock is poisoned".into(),
            ));
        }
    };
    let Some(active) = active else {
        return error_response(GatewayError::Unauthorized("Gemini Agent runtime".into()));
    };
    let Some(model_id) = alias.strip_prefix("grillforge--").map(str::to_string) else {
        return error_response(GatewayError::InvalidRequest(format!(
            "unknown GrillForge Gemini Agent route alias: {alias}"
        )));
    };
    if active.model_id != model_id {
        return error_response(GatewayError::InvalidRequest(format!(
            "inactive Gemini Agent runtime route alias: {alias}"
        )));
    }
    gemini_configured_request(gateway, model_id, request, streaming, active.documents).await
}

async fn gemini_configured_request(
    gateway: Gateway,
    model_id: String,
    request: Request<Body>,
    streaming: bool,
    documents: ConfigurationDocuments,
) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, 64 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(GatewayError::InvalidRequest(
                "request body could not be read".into(),
            ));
        }
    };
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => {
            return error_response(GatewayError::InvalidRequest(
                "request body must be valid JSON".into(),
            ));
        }
    };
    let direct_gemini =
        match select_model_protocol(&documents, &model_id, NativeProtocol::GeminiNative) {
            Ok(protocol) => protocol == Protocol::GeminiNative,
            Err(error) => return error_response(error),
        };
    if direct_gemini {
        let Some(model) = documents
            .models
            .models
            .iter()
            .find(|model| model.id == model_id)
        else {
            return error_response(GatewayError::InvalidRequest(format!(
                "unknown GrillForge model: {model_id}"
            )));
        };
        let Some(provider) = documents
            .config
            .providers
            .iter()
            .find(|provider| provider.id == model.provider_id)
        else {
            return error_response(GatewayError::Configuration(format!(
                "model {} references unknown provider {}",
                model.id, model.provider_id
            )));
        };
        if !provider.enabled {
            return error_response(GatewayError::Configuration(format!(
                "model {} uses disabled provider {}",
                model.id, provider.id
            )));
        }
        let surface = match provider_protocol_endpoint(provider, Protocol::GeminiNative) {
            Ok(surface) => surface,
            Err(error) => return error_response(error),
        };
        if surface.endpoint_mode != EndpointMode::BaseUrl
            || surface.api_key_placement != ApiKeyPlacement::XApiKey
        {
            return error_response(GatewayError::Configuration(format!(
                "Gemini Native provider {} requires an API-key Base URL",
                provider.id
            )));
        }
        let endpoint = match gemini_endpoint(&surface.endpoint, &model.upstream_id, streaming) {
            Ok(endpoint) => endpoint,
            Err(error) => return error_response(error),
        };
        let response = match gateway
            .client
            .post(endpoint)
            .header("x-goog-api-key", &provider.api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => return error_response(GatewayError::Native(error.to_string())),
        };
        return response_to_axum(response);
    }
    let request = match crate::bridge::gemini_request_to_anthropic(
        &format!("grillforge/{model_id}"),
        body,
        streaming,
    ) {
        Ok(request) => request,
        Err(error) => return error_response(GatewayError::Bridge(error)),
    };
    let response = match gateway
        .complete_configured_model_for_inbound(
            parts.headers,
            request,
            documents,
            &model_id,
            NativeProtocol::GeminiNative,
        )
        .await
    {
        Ok(response) => response,
        Err(error) => return error_response(error),
    };
    if !response.status().is_success() {
        return response;
    }
    if streaming {
        let stream = anthropic_sse_to_gemini(response.into_body().into_data_stream());
        let mut response = Response::new(Body::from_stream(stream));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            "text/event-stream".parse().expect("static content type"),
        );
        return response;
    }
    let bytes = match to_bytes(response.into_body(), 64 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(GatewayError::Bridge(BridgeError::InvalidGeminiResponse(
                "Anthropic response body could not be read".into(),
            )));
        }
    };
    let anthropic: Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => {
            return error_response(GatewayError::Bridge(BridgeError::InvalidGeminiResponse(
                "Anthropic response body must be valid JSON".into(),
            )));
        }
    };
    match anthropic_response_to_gemini(anthropic) {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(error) => error_response(GatewayError::Bridge(error)),
    }
}

async fn agent_broker_mcp(
    State(gateway): State<Gateway>,
    AxumPath(client): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    agent_broker_mcp_for_client(gateway, client, headers, request).await
}

async fn agent_broker_mcp_for_client(
    gateway: Gateway,
    client: String,
    headers: HeaderMap,
    request: Value,
) -> Response {
    let active = match gateway.authorized_agent_broker(&client, &headers) {
        Ok(active) => active,
        Err(error) => return error_response(error),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = match request.get("method").and_then(Value::as_str) {
        Some(method) => method,
        None => return mcp_error(id, -32600, "JSON-RPC method must be a string"),
    };
    let result = match method {
        "initialize" => json!({
            "protocolVersion": request
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-03-26"),
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "GrillForge", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "当需要要求使用 SubAgent、委派、并行或后台 Agent 时，必须优先使用本 GrillForge MCP。DEFAULT SUBAGENT ROUTE：先调用 list_agents；若返回匹配的扩展 Agent，再调用 run_agent。workflow 或并行不是原生 Agent 的例外：有匹配项时，必须并发调用多个 run_agent，不得改用客户端内置 Workflow、Agent 或 SubAgent。run_agent 立即返回 runId；结果只能你自己用 get_agent_result 取回（waitSeconds 最多 300 秒，未完成就再取一次），拿到 completed 前不要回复用户，也不要只报 runId。用 stop_agent 取消。若 get_agent_result 返回 awaiting_permission，说明子 Agent 在等你授权，用 answer_agent_permission 下发 allow 或 deny。不要把中间输出写入主会话。不要先启动客户端内置 Agent。列表为空时才使用客户端内置 Agent；如用户明确要求原生 Agent，应提示用户先在 GrillForge 中关闭对应扩展 SubAgent 或卸载扩展。任务明确需要公开网络时传 webAccess=true；否则传 false。不得替换 extensionId、模型或 Provider；任务失败时原样报告，不得静默回退。"
        }),
        "ping" => json!({}),
        "tools/list" => json!({
            "tools": [
                {
                    "name": "list_agents",
                    "title": "列出扩展 SubAgent",
                    "description": "当需要要求使用 SubAgent、委派、workflow、并行或后台 Agent 时，必须优先调用本工具。workflow 或并行不允许绕过 GrillForge 改用客户端内置 Agent。列出当前客户端获授权的 GrillForge 扩展 Agent，并在调用 run_agent 前选择匹配的 extensionId；列表为空时不要调用 run_agent。DEFAULT first step for any SubAgent, delegation, workflow, parallel, or background task.",
                    "_meta": {"anthropic/alwaysLoad": true},
                    "annotations": {
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "openWorldHint": false
                    },
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {}
                    }
                },
                {
                    "name": "run_agent",
                    "title": "运行扩展 SubAgent",
                    "description": "Starts one delegated task and returns a runId immediately; it does not block your turn. For workflow or parallel requests, invoke multiple run_agent calls concurrently. Do not use the client's native Workflow, Agent, or SubAgent when list_agents returned a matching extension. The local source Coding Agent owns the Agent loop and tools. The result reaches you only through get_agent_result: nothing can deliver it after you answer the user, so collect every runId you start, repeating the call while it reports running, and never report a runId to the user in place of a result. Pass waitSeconds to wait inside the call. Provide cwd and a complete prompt; webAccess defaults to true; set it to false only to keep an Agent offline. Choose permissionMode from the Agent's published list when the task needs less or more than its default. Never submit runtime, model, Provider, or native CLI arguments or silently switch Agent. 使用 extensionId 委派任务，立即拿到 runId。",
                    "_meta": {"anthropic/alwaysLoad": true},
                    "annotations": {
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "openWorldHint": true
                    },
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["extensionId", "cwd", "prompt"],
                        "properties": {
                            "extensionId": {"type": "string"},
                            "cwd": {"type": "string"},
                            "prompt": {"type": "string"},
                            "description": {"type": "string"},
                            "webAccess": {"type": "boolean", "default": true},
                            "permissionMode": {
                                "type": "string",
                                "description": "One of the Agent's permissionModes from list_agents. Omit to use its defaultPermissionMode."
                            }
                        }
                    }
                },
                {
                    "name": "get_agent_result",
                    "title": "收取扩展 SubAgent 结果",
                    "description": "Collects a run started by run_agent. Returns status \"running\" with the latest coarse progress, or \"completed\" with the Agent's final result. It waits for the run by default, so one call is usually enough; pass waitSeconds to choose the interval, up to 300 seconds, or 0 to look without waiting. Call it again whenever it reports running. 用 runId 收取结果。",
                    "_meta": {"anthropic/alwaysLoad": true},
                    "annotations": {"readOnlyHint": true, "destructiveHint": false},
                    "inputSchema": {
                        "type": "object",
                        "required": ["runId"],
                        "properties": {
                            "runId": {"type": "string"},
                            "waitSeconds": {"type": "integer", "minimum": 0, "maximum": 300, "default": 0}
                        }
                    }
                },
                {
                    "name": "answer_agent_permission",
                    "title": "答复扩展 SubAgent 的授权请求",
                    "description": "Answers a permission request raised by a delegated Agent, reported as pendingPermissions by get_agent_result. behavior is allow or deny; deny may carry a message explaining why. 由你决定子 Agent 能否执行该工具调用。",
                    "_meta": {"anthropic/alwaysLoad": true},
                    "annotations": {"readOnlyHint": false, "destructiveHint": false},
                    "inputSchema": {
                        "type": "object",
                        "required": ["requestId", "behavior"],
                        "properties": {
                            "requestId": {"type": "string"},
                            "behavior": {"type": "string", "enum": ["allow", "deny"]},
                            "message": {"type": "string"},
                            "updatedInput": {"type": "object"}
                        }
                    }
                },
                {
                    "name": "stop_agent",
                    "title": "停止扩展 SubAgent",
                    "description": "Cancels a run started by run_agent and terminates its Agent process. 用 runId 取消。",
                    "_meta": {"anthropic/alwaysLoad": true},
                    "annotations": {"readOnlyHint": false, "destructiveHint": true},
                    "inputSchema": {
                        "type": "object",
                        "required": ["runId"],
                        "properties": {"runId": {"type": "string"}}
                    }
                }
            ]
        }),
        "tools/call" => match request.pointer("/params/name").and_then(Value::as_str) {
            Some("list_agents") => match list_agents(&active, request.pointer("/params/arguments"))
            {
                Ok(text) => mcp_tool_result(text, false),
                Err(message) => mcp_tool_result(message, true),
            },
            Some("run_agent") => {
                let arguments = match request.pointer("/params/arguments") {
                    Some(arguments) => arguments.clone(),
                    None => return mcp_error(id, -32602, "run_agent arguments are required"),
                };
                match start_agent(active, arguments).await {
                    Ok(text) => mcp_tool_result(text, false),
                    Err(message) => mcp_tool_result(message, true),
                }
            }
            Some("get_agent_result") => {
                let arguments = request
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match get_agent_result(&active, arguments).await {
                    Ok(text) => mcp_tool_result(text, false),
                    Err(message) => mcp_tool_result(message, true),
                }
            }
            Some("answer_agent_permission") => {
                let arguments = request
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match answer_agent_permission(&active, arguments) {
                    Ok(text) => mcp_tool_result(text, false),
                    Err(message) => mcp_tool_result(message, true),
                }
            }
            Some("stop_agent") => {
                let arguments = request
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match stop_agent(&active, arguments) {
                    Ok(text) => mcp_tool_result(text, false),
                    Err(message) => mcp_tool_result(message, true),
                }
            }
            _ => return mcp_error(id, -32602, "unknown MCP tool"),
        },
        "notifications/initialized" => return StatusCode::ACCEPTED.into_response(),
        _ => return mcp_error(id, -32601, "JSON-RPC method is not supported"),
    };
    (
        StatusCode::OK,
        Json(json!({"jsonrpc": "2.0", "id": id, "result": result})),
    )
        .into_response()
}

fn mcp_tool_result(text: String, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error
    })
}

fn list_agents(active: &ActiveAgentBroker, arguments: Option<&Value>) -> Result<String, String> {
    if let Some(arguments) = arguments {
        if arguments
            .as_object()
            .is_none_or(|object| !object.is_empty())
        {
            return Err("list_agents does not accept arguments".into());
        }
    }
    serde_json::to_string(
        &active
            .routes
            .iter()
            .map(|route| {
                json!({
                    "extensionId": route.extension_id,
                    "sourceClientId": route.source_client_id,
                    "sourceAgentId": route.source_agent_id,
                    "modelId": route.model_id,
                    "webAccessSupported": matches!(route.source_client_id.as_str(), "claude_code" | "codex" | "grok_build"),
                    "permissionModes": crate::agent_permissions::permissions(&route.source_client_id)
                        .modes
                        .iter()
                        .map(|mode| mode.id)
                        .collect::<Vec<_>>(),
                    "defaultPermissionMode": crate::agent_permissions::permissions(&route.source_client_id)
                        .default_mode,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|_| "could not serialize extension SubAgent list".to_string())
}

/// The permission prompt a child raises. It is held here until the parent
/// decides, so GrillForge relays a decision rather than making one.
async fn agent_permission_mcp(
    State(gateway): State<Gateway>,
    AxumPath(run_id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    let Some(token) = local_runtime_token(&headers) else {
        return error_response(GatewayError::Unauthorized("Agent permission".into()));
    };
    let authorized = gateway
        .active_agent_runs
        .lock()
        .ok()
        .and_then(|runs| runs.get(&run_id).map(|run| run.permission_secret == token))
        .unwrap_or(false);
    if !authorized {
        return error_response(GatewayError::Unauthorized("Agent permission".into()));
    }
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "GrillForge Agent Permission", "version": env!("CARGO_PKG_VERSION")}
        }),
        "tools/list" => json!({
            "tools": [{
                "name": "approve",
                "description": "Asks the delegating Agent whether this tool call may proceed.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tool_name": {"type": "string"},
                        "input": {"type": "object"}
                    }
                }
            }]
        }),
        "tools/call" => {
            if request.pointer("/params/name").and_then(Value::as_str) != Some("approve") {
                return mcp_error(id, -32602, "unknown MCP tool");
            }
            let arguments = request
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let decision = await_permission_decision(&gateway, &run_id, arguments).await;
            json!({"content": [{"type": "text", "text": decision.to_string()}]})
        }
        "notifications/initialized" => return StatusCode::ACCEPTED.into_response(),
        _ => return mcp_error(id, -32601, "JSON-RPC method is not supported"),
    };
    (
        StatusCode::OK,
        Json(json!({"jsonrpc": "2.0", "id": id, "result": result})),
    )
        .into_response()
}

/// Registers the request, waits for the parent, and denies on timeout rather
/// than leaving the child stuck.
async fn await_permission_decision(gateway: &Gateway, run_id: &str, arguments: Value) -> Value {
    let request_id = uuid::Uuid::new_v4().to_string();
    let notify = Arc::new(tokio::sync::Notify::new());
    {
        let Ok(mut pending) = gateway.active_agent_permissions.lock() else {
            return json!({"behavior": "deny", "message": "GrillForge permission registry is unavailable"});
        };
        pending.insert(
            request_id.clone(),
            PermissionRequest {
                run_id: run_id.to_string(),
                tool_name: arguments
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: arguments.get("input").cloned().unwrap_or(Value::Null),
                decision: None,
                notify: Arc::clone(&notify),
            },
        );
    }
    // The parent may be inside a collect; the request is what it has to answer.
    if let Ok(runs) = gateway.active_agent_runs.lock() {
        if let Some(run) = runs.get(run_id) {
            run.notify.notify_waiters();
        }
    }
    let deadline = Instant::now() + AGENT_PERMISSION_TIMEOUT;
    loop {
        {
            let Ok(mut pending) = gateway.active_agent_permissions.lock() else {
                return json!({"behavior": "deny", "message": "GrillForge permission registry is unavailable"});
            };
            let Some(entry) = pending.get(&request_id) else {
                return json!({"behavior": "deny", "message": "the delegating Agent withdrew this request"});
            };
            if entry.decision.is_some() {
                let entry = pending.remove(&request_id).expect("checked above");
                return entry.decision.expect("checked above");
            }
            if Instant::now() >= deadline {
                pending.remove(&request_id);
                return json!({
                    "behavior": "deny",
                    "message": "the delegating Agent did not answer this permission request in time"
                });
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let _ = tokio::time::timeout(remaining, notify.notified()).await;
    }
}

fn mcp_error(id: Value, code: i64, message: &str) -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message}
        })),
    )
        .into_response()
}

struct AgentInvocation {
    cwd: PathBuf,
    prompt: String,
    web_access: bool,
    permission_args: &'static [&'static str],
    route: AgentRuntimeRoute,
    source_runtime: AgentSourceRuntime,
}

/// Everything a child needs to reach one managed model, kept together so a
/// runtime takes a single route rather than a widening argument list.
struct ManagedRoute {
    alias: String,
    runtime_token: String,
    model_id: String,
    context_window: Option<u64>,
}

#[derive(Clone)]
struct AgentRunOptions {
    web_access: bool,
    permission_args: &'static [&'static str],
    /// Where the child sends a permission prompt, when its runtime can route one.
    permission_endpoint: Option<PermissionEndpoint>,
    progress: Option<mpsc::UnboundedSender<String>>,
}

#[derive(Clone)]
struct PermissionEndpoint {
    url: String,
    token: String,
}

fn prepare_agent_invocation(
    active: &ActiveAgentBroker,
    arguments: Value,
) -> Result<AgentInvocation, String> {
    let object = arguments
        .as_object()
        .ok_or_else(|| "run_agent arguments must be an object".to_string())?;
    let allowed = [
        "extensionId",
        "cwd",
        "prompt",
        "description",
        "webAccess",
        "permissionMode",
        "waitSeconds",
    ];
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("run_agent does not accept {key}"));
    }
    let extension_id = required_mcp_string(object, "extensionId")?;
    let cwd = PathBuf::from(required_mcp_string(object, "cwd")?);
    let prompt = required_mcp_string(object, "prompt")?;
    // A delegated Agent reaches the network like any other Agent unless the call
    // deliberately withholds it.
    let requested_web = optional_mcp_opt_bool(object, "webAccess")?;
    let web_access = requested_web.unwrap_or(true);
    let permission_mode = object
        .get("permissionMode")
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "run_agent permissionMode must be a non-empty string".to_string())
        })
        .transpose()?;
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err(format!(
            "Agent working directory does not exist: {}",
            cwd.display()
        ));
    }
    let route = active
        .routes
        .iter()
        .find(|route| route.extension_id == extension_id)
        .cloned()
        .ok_or_else(|| format!("unknown configured extension SubAgent: {extension_id}"))?;
    let source_runtime = active
        .source_runtimes
        .get(&route.source_client_id)
        .cloned()
        .ok_or_else(|| {
            format!(
                "Agent source runtime is unavailable: {}",
                route.source_client_id
            )
        })?;
    // Only a deliberate refusal can fail: a runtime with no scoped web switch
    // cannot honour it, and silently granting the network would hide that.
    if requested_web == Some(false)
        && !matches!(
            route.source_client_id.as_str(),
            "claude_code" | "codex" | "grok_build"
        )
    {
        return Err(format!(
            "{} cannot withhold native web access for this runtime",
            route.source_client_id
        ));
    }
    let permission_args =
        crate::agent_permissions::resolve(&route.source_client_id, permission_mode)?;
    Ok(AgentInvocation {
        cwd,
        prompt,
        web_access,
        permission_args,
        route,
        source_runtime,
    })
}

/// Starts the run and hands back a handle, so the caller keeps its turn instead
/// of waiting on a child that may take minutes.
async fn start_agent(active: ActiveAgentBroker, arguments: Value) -> Result<String, String> {
    let wait = match arguments.get("waitSeconds") {
        None => 0,
        Some(value) => value
            .as_u64()
            .filter(|seconds| *seconds <= AGENT_RUN_MAX_WAIT_SECONDS)
            .ok_or_else(|| {
                format!("run_agent waitSeconds must be 0..={AGENT_RUN_MAX_WAIT_SECONDS}")
            })?,
    };
    let invocation = prepare_agent_invocation(&active, arguments)?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let permission_secret = uuid::Uuid::new_v4().to_string();
    let client_id = active.target_client_id.clone();
    let notify = Arc::new(tokio::sync::Notify::new());
    let runs = Arc::clone(&active.runs);
    evict_finished_runs(&runs)?;

    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
    let progress_runs = Arc::clone(&runs);
    let progress_id = run_id.clone();
    tokio::spawn(async move {
        while let Some(message) = progress_rx.recv().await {
            let Ok(mut runs) = progress_runs.lock() else {
                return;
            };
            let Some(run) = runs.get_mut(&progress_id) else {
                return;
            };
            run.progress = Some(message);
        }
    });

    // The child reaches this endpoint to ask; the parent answers through the broker.
    let endpoint = Some(PermissionEndpoint {
        url: format!(
            "{}/mcp/agent-permission/{run_id}",
            active.base_url.trim_end_matches('/')
        ),
        token: permission_secret.clone(),
    });
    let active_for_wait = active.clone();
    let finish_runs = Arc::clone(&runs);
    let finish_notify = Arc::clone(&notify);
    let finish_id = run_id.clone();
    let task = tokio::spawn(async move {
        let outcome = execute_agent(active, invocation, endpoint, Some(progress_tx)).await;
        if let Ok(mut runs) = finish_runs.lock() {
            if let Some(run) = runs.get_mut(&finish_id) {
                run.outcome = Some(outcome);
                run.finished_at = Some(Instant::now());
            }
        }
        finish_notify.notify_waiters();
    });

    runs.lock()
        .map_err(|_| "Agent run registry lock is poisoned".to_string())?
        .insert(
            run_id.clone(),
            AgentRun {
                client_id,
                permission_secret: permission_secret.clone(),
                outcome: None,
                progress: None,
                finished_at: None,
                task: Some(task),
                notify,
            },
        );
    if wait == 0 {
        return serde_json::to_string(&json!({
            "runId": run_id,
            "status": "running",
            "next": AGENT_RUN_COLLECT_OBLIGATION,
        }))
        .map_err(|_| "could not serialize the Agent run handle".to_string());
    }
    collect_run(
        &active_for_wait,
        &run_id,
        Instant::now() + Duration::from_secs(wait),
    )
    .await
}

/// Collects a run, optionally waiting. The wait is bounded so a caller never
/// blocks longer than it asked to.
async fn get_agent_result(active: &ActiveAgentBroker, arguments: Value) -> Result<String, String> {
    let object = arguments
        .as_object()
        .ok_or_else(|| "get_agent_result arguments must be an object".to_string())?;
    let allowed = ["runId", "waitSeconds"];
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("get_agent_result does not accept {key}"));
    }
    let run_id = required_mcp_string(object, "runId")?;
    let wait = match object.get("waitSeconds") {
        None => AGENT_RUN_DEFAULT_WAIT_SECONDS,
        Some(value) => value
            .as_u64()
            .filter(|seconds| *seconds <= AGENT_RUN_MAX_WAIT_SECONDS)
            .ok_or_else(|| {
                format!("get_agent_result waitSeconds must be 0..={AGENT_RUN_MAX_WAIT_SECONDS}")
            })?,
    };
    collect_run(active, &run_id, Instant::now() + Duration::from_secs(wait)).await
}

/// Waits for a run until the deadline, then reports whatever is true at that
/// moment. A caller never blocks longer than it asked to.
async fn collect_run(
    active: &ActiveAgentBroker,
    run_id: &str,
    deadline: Instant,
) -> Result<String, String> {
    loop {
        let notify = {
            let mut runs = active
                .runs
                .lock()
                .map_err(|_| "Agent run registry lock is poisoned".to_string())?;
            let run = runs
                .get_mut(run_id)
                .ok_or_else(|| format!("unknown Agent run: {run_id}"))?;
            if run.outcome.is_some() {
                let run = runs.remove(run_id).expect("run was present");
                return match run.outcome.expect("outcome was present") {
                    Ok(result) => serde_json::to_string(
                        &json!({"runId": run_id, "status": "completed", "result": result}),
                    )
                    .map_err(|_| "could not serialize the Agent result".to_string()),
                    Err(message) => Err(message),
                };
            }
            let pending = active
                .permissions
                .lock()
                .map_err(|_| "Agent permission registry lock is poisoned".to_string())?
                .iter()
                .filter(|(_, entry)| entry.run_id == run_id && entry.decision.is_none())
                .map(|(request_id, entry)| {
                    json!({
                        "requestId": request_id,
                        "toolName": entry.tool_name,
                        "input": entry.input,
                    })
                })
                .collect::<Vec<_>>();
            // A permission request is the caller's to answer, so it ends the
            // wait: the child is blocked until the answer comes back.
            if !pending.is_empty() || Instant::now() >= deadline {
                let progress = run.progress.clone();
                return serde_json::to_string(&json!({
                    "runId": run_id,
                    "status": if pending.is_empty() { "running" } else { "awaiting_permission" },
                    "progress": progress,
                    "next": if pending.is_empty() {
                        AGENT_RUN_COLLECT_OBLIGATION
                    } else {
                        "answer every pendingPermissions entry with answer_agent_permission, then call get_agent_result again; the child is blocked until you do"
                    },
                    "pendingPermissions": pending,
                }))
                .map_err(|_| "could not serialize the Agent run status".to_string());
            }
            Arc::clone(&run.notify)
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        let _ = tokio::time::timeout(remaining, notify.notified()).await;
    }
}

/// Ends a run the caller no longer wants; dropping the task kills the child.
fn stop_agent(active: &ActiveAgentBroker, arguments: Value) -> Result<String, String> {
    let object = arguments
        .as_object()
        .ok_or_else(|| "stop_agent arguments must be an object".to_string())?;
    if let Some(key) = object.keys().find(|key| key.as_str() != "runId") {
        return Err(format!("stop_agent does not accept {key}"));
    }
    let run_id = required_mcp_string(object, "runId")?;
    let mut runs = active
        .runs
        .lock()
        .map_err(|_| "Agent run registry lock is poisoned".to_string())?;
    let mut run = runs
        .remove(&run_id)
        .ok_or_else(|| format!("unknown Agent run: {run_id}"))?;
    if let Some(task) = run.task.take() {
        task.abort();
    }
    Ok(format!("stopped {run_id}"))
}

/// Relays the delegating Agent's decision to the child that is waiting on it.
fn answer_agent_permission(active: &ActiveAgentBroker, arguments: Value) -> Result<String, String> {
    let object = arguments
        .as_object()
        .ok_or_else(|| "answer_agent_permission arguments must be an object".to_string())?;
    let allowed = ["requestId", "behavior", "message", "updatedInput"];
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("answer_agent_permission does not accept {key}"));
    }
    let request_id = required_mcp_string(object, "requestId")?;
    let behavior = required_mcp_string(object, "behavior")?;
    if behavior != "allow" && behavior != "deny" {
        return Err("answer_agent_permission behavior must be allow or deny".into());
    }
    let mut pending = active
        .permissions
        .lock()
        .map_err(|_| "Agent permission registry lock is poisoned".to_string())?;
    let entry = pending
        .get_mut(&request_id)
        .ok_or_else(|| format!("unknown permission request: {request_id}"))?;
    if entry.decision.is_some() {
        return Err(format!(
            "permission request was already answered: {request_id}"
        ));
    }
    entry.decision = Some(if behavior == "allow" {
        json!({
            "behavior": "allow",
            "updatedInput": object.get("updatedInput").cloned().unwrap_or(entry.input.clone()),
        })
    } else {
        json!({
            "behavior": "deny",
            "message": object
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the delegating Agent denied this tool call"),
        })
    });
    entry.notify.notify_waiters();
    Ok(format!("answered {request_id} with {behavior}"))
}

fn evict_finished_runs(runs: &Arc<Mutex<HashMap<String, AgentRun>>>) -> Result<(), String> {
    runs.lock()
        .map_err(|_| "Agent run registry lock is poisoned".to_string())?
        .retain(|_, run| {
            run.finished_at
                .is_none_or(|finished| finished.elapsed() < AGENT_RUN_RETENTION)
        });
    Ok(())
}

async fn execute_agent(
    active: ActiveAgentBroker,
    invocation: AgentInvocation,
    permission_endpoint: Option<PermissionEndpoint>,
    progress: Option<mpsc::UnboundedSender<String>>,
) -> Result<String, String> {
    let AgentInvocation {
        cwd,
        prompt,
        web_access,
        permission_args,
        route,
        source_runtime,
    } = invocation;
    let managed_route = route.model_id.as_ref().map(|model_id| ManagedRoute {
        alias: format!("grillforge/{model_id}"),
        runtime_token: uuid::Uuid::new_v4().to_string(),
        model_id: model_id.clone(),
        // A client that is not told the real window falls back to its own default
        // and rejects a prompt the upstream would have accepted.
        context_window: active
            .documents
            .models
            .models
            .iter()
            .find(|model| model.id == *model_id)
            .and_then(|model| model.context_window),
    });
    if let Some(ManagedRoute {
        runtime_token,
        model_id,
        ..
    }) = &managed_route
    {
        active
            .runtime_routes
            .lock()
            .map_err(|_| "active Agent runtime route lock is poisoned".to_string())?
            .insert(
                runtime_token.clone(),
                ActiveAgentRuntimeRoute {
                    documents: active.documents.clone(),
                    model_id: model_id.clone(),
                    target_client_id: active.target_client_id.clone(),
                },
            );
    }
    let runtime_routes = Arc::clone(&active.runtime_routes);
    let cleanup_token = managed_route
        .as_ref()
        .map(|route| route.runtime_token.clone());
    let options = AgentRunOptions {
        web_access,
        permission_args,
        permission_endpoint,
        progress,
    };
    let output = match route.source_client_id.as_str() {
        "claude_code" => {
            run_claude_agent_runtime(
                &source_runtime,
                &cwd,
                &route.source_agent_id,
                &prompt,
                managed_route.as_ref(),
                &active.base_url,
                &options,
            )
            .await
        }
        "codex" => {
            run_codex_agent_runtime(
                &source_runtime,
                &cwd,
                &route.source_agent_id,
                &prompt,
                managed_route.as_ref(),
                &active.base_url,
                &options,
            )
            .await
        }
        "gemini" => {
            run_gemini_agent_runtime(
                &source_runtime,
                &cwd,
                &route.source_agent_id,
                &prompt,
                managed_route.as_ref(),
                &active.base_url,
                &options,
            )
            .await
        }
        "pi" => {
            run_pi_agent_runtime(
                &source_runtime,
                &cwd,
                &route.source_agent_id,
                &prompt,
                managed_route.as_ref(),
                &active.base_url,
                &options,
            )
            .await
        }
        "opencode" => {
            run_opencode_agent_runtime(
                &source_runtime,
                &cwd,
                &route.source_agent_id,
                &prompt,
                managed_route.as_ref(),
                &active.base_url,
                &options,
            )
            .await
        }
        "kimi_code" => {
            run_kimi_agent_runtime(
                &source_runtime,
                &cwd,
                &route.source_agent_id,
                &prompt,
                managed_route.as_ref(),
                &active.base_url,
                &options,
            )
            .await
        }
        "grok_build" => {
            run_grok_build_agent_runtime(
                &source_runtime,
                &cwd,
                &route.source_agent_id,
                &prompt,
                managed_route.as_ref(),
                &active.base_url,
                &options,
            )
            .await
        }
        source => Err(format!("unsupported Agent source client: {source}")),
    };
    if let (Some(cleanup_token), Ok(mut routes)) = (cleanup_token, runtime_routes.lock()) {
        routes.remove(&cleanup_token);
    }
    let output = output?;
    if !output.status.success() {
        return Err(agent_runtime_failure(&route.source_client_id, &output));
    }
    match route.source_client_id.as_str() {
        "codex" => return codex_last_agent_message(&output.stdout),
        "gemini" => return gemini_agent_message(&output.stdout),
        "pi" => return pi_last_agent_message(&output.stdout),
        "opencode" => return opencode_last_agent_message(&output.stdout),
        "kimi_code" => return kimi_last_agent_message(&output.stdout),
        "grok_build" => return grok_build_agent_message(&output.stdout),
        _ => {}
    }
    claude_last_agent_message(&output.stdout)
}

async fn run_gemini_agent_runtime(
    source: &AgentSourceRuntime,
    cwd: &Path,
    agent_id: &str,
    prompt: &str,
    managed_route: Option<&ManagedRoute>,
    base_url: &str,
    options: &AgentRunOptions,
) -> Result<std::process::Output, String> {
    let discovered =
        crate::local_agents::discover_gemini_agents_for_project(&source.config_root, cwd)?;
    if !discovered.iter().any(|agent| agent.agent_id == agent_id) {
        return Err(format!(
            "Gemini Agent does not exist in the user or project configuration: {agent_id}"
        ));
    }
    let home = source
        .config_root
        .parent()
        .ok_or_else(|| "Gemini configuration root has no parent home directory".to_string())?;
    let managed_config = managed_route
        .map(|route| {
            GeminiManagedConfigScratch::new(agent_id, &route.model_id, &route.runtime_token)
        })
        .transpose()?;
    let mut command = tokio::process::Command::new(&source.runtime);
    // Applied before any positional argument so the prompt stays last.
    command.args(options.permission_args);
    command
        .current_dir(cwd)
        .env("GEMINI_CLI_HOME", home)
        .args(["--skip-trust", "--output-format", "json", "-p"])
        .arg(format!("@{agent_id} {prompt}"));
    if let (Some(route), Some(config)) = (managed_route, &managed_config) {
        let model_route = format!("grillforge--{}", route.model_id);
        command
            .env("GEMINI_API_KEY", &route.runtime_token)
            .env("GEMINI_MODEL", &model_route)
            .env("GEMINI_CLI_SYSTEM_SETTINGS_PATH", config.path())
            .env(
                "GOOGLE_GEMINI_BASE_URL",
                format!("{}/agent-runtime/gemini", base_url.trim_end_matches('/')),
            )
            .env("GRILLFORGE_AGENT_CHILD", "1");
    }
    let output = run_agent_command(command, "Gemini CLI", options.progress.clone()).await;
    drop(managed_config);
    output
}

async fn run_claude_agent_runtime(
    source: &AgentSourceRuntime,
    cwd: &Path,
    agent_id: &str,
    prompt: &str,
    managed_route: Option<&ManagedRoute>,
    base_url: &str,
    options: &AgentRunOptions,
) -> Result<std::process::Output, String> {
    let prompt = format!("Working directory: {}\n\n{prompt}", cwd.display());
    let mut command = tokio::process::Command::new(&source.runtime);
    // Applied before any positional argument so the prompt stays last.
    command.args(options.permission_args);
    command.current_dir(cwd).args(["--agent", agent_id]);
    if options.web_access {
        command.args(["--allowedTools", "WebSearch,WebFetch"]);
    } else {
        // The permission mode approves every tool, so a call that refused the web
        // must have the web tools withheld explicitly.
        command.args(["--disallowedTools", "WebSearch,WebFetch"]);
    }
    if let Some(route) = managed_route {
        command.args(["--model", &route.alias]);
    }
    command.args([
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
        &prompt,
    ]);
    for key in [
        "CLAUDE_CODE_OAUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
    ] {
        command.env_remove(key);
    }
    command.env("CLAUDE_CONFIG_DIR", &source.config_root);
    if let Some(route) = managed_route {
        command
            .env(
                "ANTHROPIC_BASE_URL",
                format!("{}/agent-runtime", base_url.trim_end_matches('/')),
            )
            .env("ANTHROPIC_API_KEY", &route.runtime_token)
            .env("ANTHROPIC_MODEL", &route.alias)
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env("GRILLFORGE_AGENT_CHILD", "1");
    }
    // A prompt the child raises is relayed to the delegating Agent instead of
    // being answered here; GrillForge only carries the question and the answer.
    if let Some(permission) = &options.permission_endpoint {
        let executable = std::env::current_exe().map_err(|error| {
            format!("could not resolve the GrillForge executable for the permission relay: {error}")
        })?;
        let config = json!({
            "mcpServers": {
                "grillforge_permission": {
                    "type": "stdio",
                    "command": executable,
                    "args": ["mcp-stdio"],
                    "env": {
                        "GRILLFORGE_MCP_URL": permission.url,
                        "GRILLFORGE_MCP_TOKEN": permission.token
                    }
                }
            }
        });
        command.args(["--mcp-config", &config.to_string()]).args([
            "--permission-prompt-tool",
            "mcp__grillforge_permission__approve",
        ]);
    }
    // Claude Code assumes a fixed window for a model it does not recognize.
    if let Some(context_window) = managed_route.and_then(|route| route.context_window) {
        command.env("CLAUDE_CODE_MAX_CONTEXT_TOKENS", context_window.to_string());
    }
    run_agent_command(command, "Claude Code", options.progress.clone()).await
}

async fn run_codex_agent_runtime(
    source: &AgentSourceRuntime,
    cwd: &Path,
    agent_id: &str,
    prompt: &str,
    managed_route: Option<&ManagedRoute>,
    base_url: &str,
    options: &AgentRunOptions,
) -> Result<std::process::Output, String> {
    let custom_agent_file =
        crate::local_agents::resolve_codex_custom_agent_file(&source.config_root, cwd, agent_id)?;
    if custom_agent_file.is_none() && !crate::local_agents::is_codex_builtin_agent(agent_id) {
        return Err(format!(
            "Codex Agent does not exist in the user or project configuration: {agent_id}"
        ));
    }
    let developer_instructions = custom_agent_file
        .as_deref()
        .map(codex_agent_developer_instructions)
        .transpose()?;
    if managed_route.is_none() && (custom_agent_file.is_some() || agent_id != "default") {
        return run_native_codex_subagent_runtime(
            source,
            cwd,
            agent_id,
            prompt,
            custom_agent_file.as_deref(),
            options,
        )
        .await;
    }
    if managed_route.is_some() && custom_agent_file.is_none() && agent_id != "default" {
        return Err(format!(
            "Codex built-in Agent {agent_id} cannot use an external model because Codex validates its native SubAgent model catalog before sending a request; use the default or a custom Codex Agent"
        ));
    }
    let mut command = tokio::process::Command::new(&source.runtime);
    // Applied before any positional argument so the prompt stays last.
    command.args(options.permission_args);
    if options.web_access {
        command.arg("--search");
    }
    command
        .current_dir(cwd)
        .env("CODEX_HOME", &source.config_root)
        .args([
            "exec",
            "--json",
            "--ephemeral",
            "--skip-git-repo-check",
            "-C",
        ])
        .arg(cwd);
    if let Some(route) = managed_route {
        command
            .args(["-c", &format!("model={}", route.alias)])
            .args(["-c", "model_provider=grillforge_agent"])
            .args([
                "-c",
                &format!(
                    "model_providers.grillforge_agent.base_url={}/agent-runtime/v1",
                    base_url.trim_end_matches('/')
                ),
            ])
            .args([
                "-c",
                "model_providers.grillforge_agent.env_key=GRILLFORGE_AGENT_TOKEN",
            ])
            .args(["-c", "model_providers.grillforge_agent.wire_api=responses"])
            .args(["-c", "model_providers.grillforge_agent.name=GrillForge"])
            .env("GRILLFORGE_AGENT_TOKEN", &route.runtime_token)
            .env("GRILLFORGE_AGENT_CHILD", "1");
    }
    command.arg(match developer_instructions {
        Some(instructions) => format!("{instructions}\n\nTask:\n{prompt}"),
        None => prompt.to_string(),
    });
    run_agent_command(command, "Codex", options.progress.clone()).await
}

async fn run_native_codex_subagent_runtime(
    source: &AgentSourceRuntime,
    cwd: &Path,
    agent_id: &str,
    prompt: &str,
    custom_agent_file: Option<&Path>,
    options: &AgentRunOptions,
) -> Result<std::process::Output, String> {
    let mut command = tokio::process::Command::new(&source.runtime);
    // Applied before any positional argument so the prompt stays last.
    command.args(options.permission_args);
    if options.web_access {
        command.arg("--search");
    }
    command
        .current_dir(cwd)
        .env("CODEX_HOME", &source.config_root)
        .args([
            "exec",
            "--json",
            "--ephemeral",
            "--skip-git-repo-check",
            "-C",
        ])
        .arg(cwd)
        .args(["--enable", "multi_agent"]);
    if let Some(path) = custom_agent_file {
        let description = codex_agent_description(path)?;
        command
            .args([
                "-c",
                &format!(
                    "agents.{agent_id}.description={}",
                    toml_edit::Value::from(description)
                ),
            ])
            .args([
                "-c",
                &format!("agents.{agent_id}.config_file=\"{}\"", path.display()),
            ]);
    }
    command.arg(format!(
        "Use the Codex collaboration spawn_agent tool exactly once with agent_type {agent_id} and fork_turns none. Do not perform the task in the parent. Send the child this complete task:\n\n{prompt}\n\nWait for that child and return its final answer verbatim. If that exact agent_type cannot be selected, return an error instead of spawning a generic Agent."
    ));
    run_agent_command(command, "Codex", options.progress.clone()).await
}

async fn run_pi_agent_runtime(
    source: &AgentSourceRuntime,
    cwd: &Path,
    agent_id: &str,
    prompt: &str,
    managed_route: Option<&ManagedRoute>,
    base_url: &str,
    options: &AgentRunOptions,
) -> Result<std::process::Output, String> {
    let agent_file =
        crate::local_agents::resolve_pi_agent_file(&source.config_root, cwd, agent_id)?
            .ok_or_else(|| {
                format!("Pi Agent does not exist in the user or project configuration: {agent_id}")
            })?;
    let agent = crate::local_agents::read_pi_agent_definition(&agent_file)?;
    let scratch = PiAgentScratch::new(&agent.system_prompt)?;
    let managed_config = managed_route
        .map(|route| {
            PiManagedConfigScratch::new(
                base_url,
                &route.runtime_token,
                &route.alias,
                route.context_window,
            )
        })
        .transpose()?;
    let mut command = tokio::process::Command::new(&source.runtime);
    // Applied before any positional argument so the prompt stays last.
    command.args(options.permission_args);
    command
        .current_dir(cwd)
        .env(
            "PI_CODING_AGENT_DIR",
            managed_config
                .as_ref()
                .map(PiManagedConfigScratch::root)
                .unwrap_or(&source.config_root),
        )
        .args(["--mode", "json", "-p", "--no-session"]);
    if let Some(route) = managed_route {
        command.args(["--model", &format!("grillforge_agent/{}", route.alias)]);
        command.env("GRILLFORGE_AGENT_CHILD", "1");
    } else if let Some(model) = &agent.model {
        command.args(["--model", model]);
    }
    if !agent.tools.is_empty() {
        command.args(["--tools", &agent.tools.join(",")]);
    }
    if let Some(system_prompt) = scratch.system_prompt_path() {
        command.arg("--append-system-prompt").arg(system_prompt);
    }
    command.arg(format!("Task: {prompt}"));
    let output = run_agent_command(command, "Pi", options.progress.clone()).await;
    drop(scratch);
    drop(managed_config);
    output
}

async fn run_opencode_agent_runtime(
    source: &AgentSourceRuntime,
    cwd: &Path,
    agent_id: &str,
    prompt: &str,
    managed_route: Option<&ManagedRoute>,
    base_url: &str,
    options: &AgentRunOptions,
) -> Result<std::process::Output, String> {
    let (mode, _) =
        crate::local_agents::resolve_opencode_agent(&source.config_root, cwd, agent_id)?
            .ok_or_else(|| {
                format!(
                    "OpenCode Agent does not exist in the user or project configuration: {agent_id}"
                )
            })?;
    if mode == crate::local_agents::OpenCodeAgentMode::Primary {
        return Err(format!(
            "OpenCode primary Agent cannot be used as an extension SubAgent: {agent_id}"
        ));
    }
    let mut command = tokio::process::Command::new(&source.runtime);
    // Applied before any positional argument so the prompt stays last.
    command.args(options.permission_args);
    command
        .current_dir(cwd)
        .env("OPENCODE_CONFIG_DIR", &source.config_root)
        .args(["run", "--format", "json"]);
    let promote_subagent = mode == crate::local_agents::OpenCodeAgentMode::Subagent;
    if let Some(route) = managed_route {
        let provider_model = format!("grillforge_agent/{}", route.alias);
        let mut agent = json!({ "model": provider_model });
        if promote_subagent {
            agent["mode"] = Value::String("primary".into());
        }
        let config = json!({
            "provider": {
                "grillforge_agent": {
                    "id": "grillforge_agent",
                    "name": "GrillForge",
                    "npm": "@ai-sdk/anthropic",
                    "env": [],
                    "options": {
                        "apiKey": route.runtime_token,
                        "baseURL": format!("{}/agent-runtime/v1", base_url.trim_end_matches('/'))
                    },
                    "models": {
                        (&route.alias): {
                            "id": route.alias,
                            "name": format!("GrillForge {}", route.alias.trim_start_matches("grillforge/"))
                        }
                    }
                }
            },
            "agent": {
                (agent_id): agent
            }
        });
        command
            .args(["--model", &provider_model])
            .env("OPENCODE_CONFIG_CONTENT", config.to_string())
            .env("GRILLFORGE_AGENT_CHILD", "1");
    } else if promote_subagent {
        command.env(
            "OPENCODE_CONFIG_CONTENT",
            json!({ "agent": { (agent_id): { "mode": "primary" } } }).to_string(),
        );
    }
    command.args(["--agent", agent_id]).arg(prompt);
    run_agent_command(command, "OpenCode", options.progress.clone()).await
}

async fn run_kimi_agent_runtime(
    source: &AgentSourceRuntime,
    cwd: &Path,
    agent_id: &str,
    prompt: &str,
    managed_route: Option<&ManagedRoute>,
    base_url: &str,
    options: &AgentRunOptions,
) -> Result<std::process::Output, String> {
    let home = crate::local_agents::kimi_user_home(&source.config_root)?;
    let custom_agent_file =
        crate::local_agents::resolve_kimi_agent_file(&source.config_root, &home, cwd, agent_id)?;
    if custom_agent_file.is_none() && !crate::local_agents::is_kimi_builtin_agent(agent_id) {
        return Err(format!(
            "Kimi Code Agent does not exist in the user, project, plugin, or extra Agent configuration: {agent_id}"
        ));
    }
    let managed_config = managed_route
        .map(|route| {
            KimiManagedConfigScratch::new(
                &source.config_root,
                base_url,
                &route.runtime_token,
                &route.alias,
            )
        })
        .transpose()?;
    let mut command = tokio::process::Command::new(&source.runtime);
    // Applied before any positional argument so the prompt stays last.
    command.args(options.permission_args);
    command
        .current_dir(cwd)
        .env("KIMI_CODE_HOME", &source.config_root)
        .env("KIMI_CODE_NO_AUTO_UPDATE", "1")
        .env("KIMI_DISABLE_TELEMETRY", "1");
    if let Some(path) = &custom_agent_file {
        command.arg("--agent-file").arg(path);
    } else {
        command.args(["--agent", agent_id]);
    }
    if let (Some(route), Some(config)) = (managed_route, &managed_config) {
        command
            .args(["--model", &route.alias])
            .env("KIMI_CODE_HOME", config.root())
            .env("KIMI_CODE_EXPERIMENTAL_SECONDARY_MODEL", "1")
            .env("GRILLFORGE_AGENT_CHILD", "1");
    }
    command.args(["--prompt", prompt, "--output-format", "stream-json"]);
    let output = run_agent_command(command, "Kimi Code", options.progress.clone()).await;
    drop(managed_config);
    output
}

async fn run_grok_build_agent_runtime(
    source: &AgentSourceRuntime,
    cwd: &Path,
    agent_id: &str,
    prompt: &str,
    managed_route: Option<&ManagedRoute>,
    base_url: &str,
    options: &AgentRunOptions,
) -> Result<std::process::Output, String> {
    // Discovery waits on a child process, so it runs off the async worker threads
    // rather than blocking one for as long as that child takes.
    let discovered = {
        let runtime = source.runtime.clone();
        let cwd = cwd.to_path_buf();
        tokio::task::spawn_blocking(move || {
            crate::local_agents::discover_grok_build_agents(&runtime, &cwd)
        })
        .await
        .map_err(|error| format!("Grok Build Agent discovery did not finish: {error}"))??
    };
    if !discovered.iter().any(|agent| agent.agent_id == agent_id) {
        return Err(format!(
            "Grok Build Agent is not available for this project: {agent_id}"
        ));
    }
    let managed_config = managed_route
        .map(|route| {
            GrokBuildManagedConfigScratch::new(
                base_url,
                &route.runtime_token,
                &route.alias,
                route.context_window,
            )
        })
        .transpose()?;
    let mut command = tokio::process::Command::new(&source.runtime);
    // Applied before any positional argument so the prompt stays last.
    command.args(options.permission_args);
    command.current_dir(cwd).args(["--agent", agent_id]);
    if !options.web_access {
        command.arg("--disable-web-search");
    }
    command
        .arg("-p")
        .arg(prompt)
        .args(["--output-format", "json"]);
    if let (Some(_), Some(config)) = (managed_route, &managed_config) {
        command
            .args(["--model", "grillforge"])
            .env("GROK_HOME", config.root())
            .env("GRILLFORGE_GROK_BUILD_API_KEY", config.runtime_token())
            .env("GRILLFORGE_AGENT_CHILD", "1");
    } else {
        command.env("GROK_HOME", &source.config_root);
    }
    let output = run_agent_command(command, "Grok Build", options.progress.clone()).await;
    drop(managed_config);
    output
}

async fn run_agent_command(
    mut command: tokio::process::Command,
    runtime_name: &str,
    progress: Option<mpsc::UnboundedSender<String>>,
) -> Result<std::process::Output, String> {
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let timeout_name = runtime_name.to_string();
    let runtime_name = timeout_name.clone();
    tokio::time::timeout(
        Duration::from_secs(AGENT_RUNTIME_TIMEOUT_SECONDS),
        async move {
            let mut child = command.spawn().map_err(|error| {
                format!("could not start {runtime_name} Agent runtime: {error}")
            })?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| format!("could not capture {runtime_name} Agent stdout"))?;
            let mut stderr = child
                .stderr
                .take()
                .ok_or_else(|| format!("could not capture {runtime_name} Agent stderr"))?;
            let stderr_task = tokio::spawn(async move {
                let mut bytes = Vec::new();
                stderr.read_to_end(&mut bytes).await.map(|_| bytes)
            });
            let mut reader = BufReader::new(stdout);
            let mut stdout = Vec::new();
            let mut line = Vec::new();
            let mut last_message = None;
            let mut last_sent = tokio::time::Instant::now() - Duration::from_secs(1);
            loop {
                line.clear();
                let read = reader.read_until(b'\n', &mut line).await.map_err(|error| {
                    format!("could not read {runtime_name} Agent stdout: {error}")
                })?;
                if read == 0 {
                    break;
                }
                stdout.extend_from_slice(&line);
                let Some(sender) = progress.as_ref() else {
                    continue;
                };
                let Some(message) = agent_progress_message(&runtime_name, &line) else {
                    continue;
                };
                if last_message.as_deref() == Some(message.as_str())
                    || last_sent.elapsed() < Duration::from_millis(500)
                {
                    continue;
                }
                let _ = sender.send(message.clone());
                last_message = Some(message);
                last_sent = tokio::time::Instant::now();
            }
            let status = child.wait().await.map_err(|error| {
                format!("could not wait for {runtime_name} Agent runtime: {error}")
            })?;
            let stderr = stderr_task
                .await
                .map_err(|error| format!("could not join {runtime_name} stderr reader: {error}"))?
                .map_err(|error| format!("could not read {runtime_name} Agent stderr: {error}"))?;
            Ok(Output {
                status,
                stdout,
                stderr,
            })
        },
    )
    .await
    .map_err(|_| format!("{timeout_name} Agent runtime exceeded three hours"))?
}

fn agent_progress_message(runtime_name: &str, line: &[u8]) -> Option<String> {
    let event: Value = serde_json::from_slice(line).ok()?;
    if let Some(content) = event.pointer("/message/content").and_then(Value::as_array) {
        if let Some(tool) = content.iter().find(|block| {
            matches!(
                block.get("type").and_then(Value::as_str),
                Some("tool_use" | "toolCall")
            )
        }) {
            let name = tool
                .get("name")
                .or_else(|| tool.get("toolName"))
                .and_then(Value::as_str)?;
            return Some(format!("{runtime_name} 正在调用 {name}"));
        }
        if let Some(text) = content
            .iter()
            .find_map(|block| block.get("text").and_then(Value::as_str))
        {
            return progress_excerpt(runtime_name, text);
        }
    }
    if let Some(item) = event.get("item") {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if item_type == "agent_message" {
            return item
                .get("text")
                .and_then(Value::as_str)
                .and_then(|text| progress_excerpt(runtime_name, text));
        }
        if matches!(
            item_type,
            "command_execution" | "mcp_tool_call" | "web_search"
        ) {
            let detail = item
                .get("command")
                .or_else(|| item.get("tool"))
                .or_else(|| item.get("name"))
                .or_else(|| item.get("query"))
                .and_then(Value::as_str)
                .unwrap_or(item_type);
            return progress_excerpt(runtime_name, &format!("正在执行 {detail}"));
        }
    }
    if event.get("role").and_then(Value::as_str) == Some("assistant") {
        if let Some(text) = event.get("content").and_then(Value::as_str) {
            return progress_excerpt(runtime_name, text);
        }
    }
    None
}

fn progress_excerpt(runtime_name: &str, text: &str) -> Option<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    let mut excerpt = text.chars().take(280).collect::<String>();
    if text.chars().count() > 280 {
        excerpt.push('…');
    }
    Some(format!("{runtime_name}: {excerpt}"))
}

fn claude_last_agent_message(stdout: &[u8]) -> Result<String, String> {
    let stdout = std::str::from_utf8(stdout)
        .map_err(|_| "Claude Code Agent runtime returned non-UTF-8 output".to_string())?;
    let mut result = None;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let event: Value = serde_json::from_str(line)
            .map_err(|_| "Claude Code Agent runtime returned invalid JSONL".to_string())?;
        if let Some(value) = event
            .get("result")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            result = Some(value.to_string());
        }
    }
    result.ok_or_else(|| "Claude Code Agent runtime returned no result".to_string())
}

fn codex_agent_developer_instructions(path: &Path) -> Result<String, String> {
    codex_agent_string(path, "developer_instructions")
}

fn codex_agent_description(path: &Path) -> Result<String, String> {
    codex_agent_string(path, "description")
}

fn codex_agent_string(path: &Path, field: &str) -> Result<String, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    contents
        .parse::<toml_edit::DocumentMut>()
        .ok()
        .and_then(|document| {
            document
                .get(field)
                .and_then(toml_edit::Item::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| format!("Codex Agent does not define {field}: {}", path.display()))
}

fn codex_last_agent_message(stdout: &[u8]) -> Result<String, String> {
    let stdout = std::str::from_utf8(stdout)
        .map_err(|_| "Codex Agent runtime returned non-UTF-8 output".to_string())?;
    let mut last_message = None;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let event: Value = serde_json::from_str(line)
            .map_err(|_| "Codex Agent runtime returned invalid JSONL".to_string())?;
        if event.get("type").and_then(Value::as_str) != Some("item.completed") {
            continue;
        }
        let Some(item) = event.get("item") else {
            return Err("Codex Agent runtime returned an item without content".into());
        };
        if item.get("type").and_then(Value::as_str) == Some("agent_message") {
            last_message = Some(
                item.get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        "Codex Agent runtime returned an Agent message without text".to_string()
                    })?
                    .to_string(),
            );
        }
    }
    last_message.ok_or_else(|| "Codex Agent runtime returned no final Agent message".to_string())
}

fn gemini_agent_message(stdout: &[u8]) -> Result<String, String> {
    let response: Value = serde_json::from_slice(stdout)
        .map_err(|_| "Gemini CLI Agent runtime returned invalid JSON".to_string())?;
    if let Some(error) = response.get("error") {
        return Err(format!(
            "Gemini CLI Agent runtime failed: {}",
            safe_single_line(&error.to_string())
        ));
    }
    response
        .get("response")
        .and_then(Value::as_str)
        .filter(|response| !response.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Gemini CLI Agent runtime returned no response".to_string())
}

fn pi_last_agent_message(stdout: &[u8]) -> Result<String, String> {
    let stdout = std::str::from_utf8(stdout)
        .map_err(|_| "Pi Agent runtime returned non-UTF-8 output".to_string())?;
    let mut last_message = None;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let event: Value = serde_json::from_str(line)
            .map_err(|_| "Pi Agent runtime returned invalid JSONL".to_string())?;
        if event.get("type").and_then(Value::as_str) != Some("message_end") {
            continue;
        }
        let message = event
            .get("message")
            .ok_or_else(|| "Pi Agent runtime returned message_end without a message".to_string())?;
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if matches!(
            message.get("stopReason").and_then(Value::as_str),
            Some("error" | "aborted")
        ) {
            return Err(message
                .get("errorMessage")
                .and_then(Value::as_str)
                .map(|message| format!("Pi Agent runtime failed: {}", safe_single_line(message)))
                .unwrap_or_else(|| "Pi Agent runtime stopped before completion".into()));
        }
        let content = message
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                "Pi Agent runtime returned an assistant message without content".to_string()
            })?;
        if let Some(text) = content.iter().find_map(|part| {
            (part.get("type").and_then(Value::as_str) == Some("text"))
                .then(|| part.get("text").and_then(Value::as_str))
                .flatten()
        }) {
            last_message = Some(text.to_string());
        }
    }
    last_message.ok_or_else(|| "Pi Agent runtime returned no final Agent message".to_string())
}

fn opencode_last_agent_message(stdout: &[u8]) -> Result<String, String> {
    let stdout = std::str::from_utf8(stdout)
        .map_err(|_| "OpenCode Agent runtime returned non-UTF-8 output".to_string())?;
    let mut last_message = None;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let event: Value = serde_json::from_str(line)
            .map_err(|_| "OpenCode Agent runtime returned invalid JSONL".to_string())?;
        if event.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let Some(text) = event
            .get("part")
            .and_then(|part| part.get("text"))
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
        else {
            continue;
        };
        last_message = Some(text.to_string());
    }
    last_message.ok_or_else(|| "OpenCode Agent runtime returned no final Agent message".to_string())
}

fn kimi_last_agent_message(stdout: &[u8]) -> Result<String, String> {
    let stdout = std::str::from_utf8(stdout)
        .map_err(|_| "Kimi Code Agent runtime returned non-UTF-8 output".to_string())?;
    let mut last_message = None;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let event: Value = serde_json::from_str(line)
            .map_err(|_| "Kimi Code Agent runtime returned invalid JSONL".to_string())?;
        if event.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(text) = event
            .get("content")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
        else {
            continue;
        };
        last_message = Some(text.to_string());
    }
    last_message
        .ok_or_else(|| "Kimi Code Agent runtime returned no final Agent message".to_string())
}

fn grok_build_agent_message(stdout: &[u8]) -> Result<String, String> {
    let response: Value = serde_json::from_slice(stdout)
        .map_err(|_| "Grok Build Agent runtime returned invalid JSON".to_string())?;
    if response.get("type").and_then(Value::as_str) == Some("error") {
        return Err(response
            .get("message")
            .and_then(Value::as_str)
            .map(|message| {
                format!(
                    "Grok Build Agent runtime failed: {}",
                    safe_single_line(message)
                )
            })
            .unwrap_or_else(|| "Grok Build Agent runtime failed without a message".into()));
    }
    response
        .get("result")
        .or_else(|| response.get("text"))
        .or_else(|| response.get("message"))
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Grok Build Agent runtime returned no final Agent message".to_string())
}

/// Grok Build requires the field, so an unknown model keeps the previous default.
const DEFAULT_GROK_BUILD_CONTEXT_WINDOW: u64 = 500_000;

struct GrokBuildManagedConfigScratch {
    root: PathBuf,
    runtime_token: String,
}

impl GrokBuildManagedConfigScratch {
    fn new(
        base_url: &str,
        runtime_token: &str,
        model_route: &str,
        context_window: Option<u64>,
    ) -> Result<Self, String> {
        let root = std::env::temp_dir().join(format!(
            "grillforge-grok-build-agent-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root)
            .map_err(|error| format!("could not create Grok Build Agent configuration: {error}"))?;
        let config = format!(
            "[models]\ndefault = \"grillforge\"\nsession_summary = \"grillforge\"\n\n[model.grillforge]\nmodel = {}\nbase_url = {}\nname = \"GrillForge Agent\"\nenv_key = \"GRILLFORGE_GROK_BUILD_API_KEY\"\napi_backend = \"responses\"\ncontext_window = {}\n",
            toml_edit::Value::from(model_route),
            toml_edit::Value::from(format!(
                "{}/agent-runtime/v1",
                base_url.trim_end_matches('/')
            )),
            toml_edit::Value::from(
                i64::try_from(context_window.unwrap_or(DEFAULT_GROK_BUILD_CONTEXT_WINDOW))
                    .unwrap_or(i64::MAX),
            ),
        );
        if let Err(error) =
            crate::storage::atomic_replace(&root.join("config.toml"), config.as_bytes())
        {
            let _ = std::fs::remove_dir_all(&root);
            return Err(format!(
                "could not write Grok Build Agent configuration: {error}"
            ));
        }
        Ok(Self {
            root,
            runtime_token: runtime_token.to_string(),
        })
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn runtime_token(&self) -> &str {
        &self.runtime_token
    }
}

impl Drop for GrokBuildManagedConfigScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct PiAgentScratch {
    root: Option<PathBuf>,
    system_prompt: Option<PathBuf>,
}

impl PiAgentScratch {
    fn new(system_prompt: &str) -> Result<Self, String> {
        if system_prompt.trim().is_empty() {
            return Ok(Self {
                root: None,
                system_prompt: None,
            });
        }
        let root =
            std::env::temp_dir().join(format!("grillforge-pi-agent-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root)
            .map_err(|error| format!("could not create Pi Agent scratch directory: {error}"))?;
        let path = root.join("system-prompt.md");
        if let Err(error) = crate::storage::atomic_replace(&path, system_prompt.as_bytes()) {
            let _ = std::fs::remove_dir_all(&root);
            return Err(format!("could not write Pi Agent system prompt: {error}"));
        }
        Ok(Self {
            root: Some(root),
            system_prompt: Some(path),
        })
    }

    fn system_prompt_path(&self) -> Option<&Path> {
        self.system_prompt.as_deref()
    }
}

impl Drop for PiAgentScratch {
    fn drop(&mut self) {
        if let Some(root) = &self.root {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

struct GeminiManagedConfigScratch {
    root: PathBuf,
    path: PathBuf,
}

impl GeminiManagedConfigScratch {
    fn new(agent_id: &str, model_id: &str, runtime_token: &str) -> Result<Self, String> {
        if runtime_token.is_empty() {
            return Err("Gemini Agent runtime token must not be empty".into());
        }
        let model_route = format!("grillforge--{model_id}");
        let root = std::env::temp_dir().join(format!(
            "grillforge-gemini-managed-agent-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root)
            .map_err(|error| format!("could not create Gemini Agent configuration: {error}"))?;
        let path = root.join("settings.json");
        let settings = json!({
            "general": {"maxAttempts": 1, "retryFetchErrors": false},
            "security": {"auth": {"selectedType": "gemini-api-key"}},
            "model": {"name": model_route},
            "modelConfigs": {"customOverrides": [{
                "match": {"model": model_route},
                "modelConfig": {"generateContentConfig": {"maxOutputTokens": 8192}}
            }]},
            "agents": {"overrides": {
                (agent_id): {"modelConfig": {
                    "model": model_route,
                    "generateContentConfig": {"maxOutputTokens": 8192}
                }}
            }}
        });
        let bytes = serde_json::to_vec_pretty(&settings)
            .map_err(|error| format!("could not encode Gemini Agent configuration: {error}"))?;
        if let Err(error) = crate::storage::atomic_replace(&path, &bytes) {
            let _ = std::fs::remove_dir_all(&root);
            return Err(format!(
                "could not write Gemini Agent configuration: {error}"
            ));
        }
        Ok(Self { root, path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for GeminiManagedConfigScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct PiManagedConfigScratch {
    root: PathBuf,
}

impl PiManagedConfigScratch {
    fn new(
        base_url: &str,
        runtime_token: &str,
        model_route: &str,
        context_window: Option<u64>,
    ) -> Result<Self, String> {
        let root = std::env::temp_dir().join(format!(
            "grillforge-pi-managed-agent-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root)
            .map_err(|error| format!("could not create Pi Agent configuration: {error}"))?;
        let models = json!({
            "providers": {
                "grillforge_agent": {
                    "baseUrl": format!("{}/agent-runtime", base_url.trim_end_matches('/')),
                    "api": "anthropic-messages",
                    "apiKey": runtime_token,
                    "models": [{
                        "id": model_route,
                        "name": "GrillForge Agent",
                        "reasoning": true,
                        "input": ["text", "image"],
                        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
                        // Pi requires both fields, so an unknown model keeps the
                        // previous default rather than blocking the run.
                        "contextWindow": context_window.unwrap_or(200_000),
                        "maxTokens": 64000
                    }]
                }
            }
        });
        let bytes = serde_json::to_vec_pretty(&models)
            .map_err(|error| format!("could not encode Pi Agent configuration: {error}"))?;
        if let Err(error) = crate::storage::atomic_replace(&root.join("models.json"), &bytes) {
            let _ = std::fs::remove_dir_all(&root);
            return Err(format!("could not write Pi Agent configuration: {error}"));
        }
        Ok(Self { root })
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for PiManagedConfigScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct KimiManagedConfigScratch {
    root: PathBuf,
}

impl KimiManagedConfigScratch {
    fn new(
        source_root: &Path,
        base_url: &str,
        runtime_token: &str,
        model_route: &str,
    ) -> Result<Self, String> {
        let source = source_root.join("config.toml");
        let contents = if source.exists() {
            std::fs::read_to_string(&source)
                .map_err(|error| format!("could not read {}: {error}", source.display()))?
        } else {
            String::new()
        };
        let mut document = contents
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| format!("invalid Kimi Code TOML {}: {error}", source.display()))?;
        document["default_model"] = toml_edit::value(model_route);
        document["telemetry"] = toml_edit::value(false);
        document["experimental"]["secondary-model"] = toml_edit::value(true);

        if document.get("providers").is_none() {
            document["providers"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let providers = document["providers"]
            .as_table_mut()
            .ok_or_else(|| format!("invalid Kimi Code providers table: {}", source.display()))?;
        let mut provider = toml_edit::Table::new();
        provider["type"] = toml_edit::value("anthropic");
        provider["base_url"] = toml_edit::value(format!(
            "{}/agent-runtime/v1",
            base_url.trim_end_matches('/')
        ));
        provider["api_key"] = toml_edit::value(runtime_token);
        providers.insert("grillforge_agent", toml_edit::Item::Table(provider));

        if document.get("models").is_none() {
            document["models"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let models = document["models"]
            .as_table_mut()
            .ok_or_else(|| format!("invalid Kimi Code models table: {}", source.display()))?;
        let mut model = toml_edit::Table::new();
        model["provider"] = toml_edit::value("grillforge_agent");
        model["model"] = toml_edit::value(model_route);
        model["display_name"] = toml_edit::value(format!(
            "GrillForge {}",
            model_route.trim_start_matches("grillforge/")
        ));
        model["max_context_size"] = toml_edit::value(200_000);
        let mut capabilities = toml_edit::Array::new();
        capabilities.push("tool_use");
        model["capabilities"] = toml_edit::value(capabilities);
        models.insert(model_route, toml_edit::Item::Table(model));

        let mut secondary = toml_edit::Table::new();
        secondary["default_model"] = toml_edit::value(model_route);
        secondary["force"] = toml_edit::value(true);
        document["secondary_model"] = toml_edit::Item::Table(secondary);

        let root = std::env::temp_dir().join(format!(
            "grillforge-kimi-managed-agent-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root)
            .map_err(|error| format!("could not create Kimi Code Agent configuration: {error}"))?;
        let path = root.join("config.toml");
        if let Err(error) = crate::storage::atomic_replace(&path, document.to_string().as_bytes()) {
            let _ = std::fs::remove_dir_all(&root);
            return Err(format!(
                "could not write Kimi Code Agent configuration: {error}"
            ));
        }
        Ok(Self { root })
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for KimiManagedConfigScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn required_mcp_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.trim() == *value)
        .map(str::to_string)
        .ok_or_else(|| format!("run_agent {key} must be a non-empty string"))
}

/// Distinguishes "not asked for" from "explicitly refused", which decide
/// different things for a runtime that cannot scope the setting.
fn optional_mcp_opt_bool(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("run_agent {key} must be a boolean")),
    }
}

fn agent_runtime_failure(runtime_name: &str, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = agent_stdout_failure(&output.stdout).unwrap_or_else(|| stderr.into_owned());
    format!(
        "{runtime_name} Agent runtime exited with {}: {}",
        output.status,
        safe_single_line(&detail)
    )
}

/// A failed Agent runtime reports its cause on stdout, not stderr, because every
/// supported runtime is launched with a machine-readable event stream.
fn agent_stdout_failure(stdout: &[u8]) -> Option<String> {
    let stdout = std::str::from_utf8(stdout).ok()?;
    let mut detail = None;
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if event.get("is_error").and_then(Value::as_bool) != Some(true)
            && event.get("type").and_then(Value::as_str) != Some("error")
        {
            continue;
        }
        if let Some(found) = ["/result", "/message", "/error/message"]
            .iter()
            .find_map(|pointer| event.pointer(pointer).and_then(Value::as_str))
            .filter(|value| !value.trim().is_empty())
        {
            detail = Some(found.to_string());
        }
    }
    detail
}

fn safe_single_line(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(500)
        .collect()
}

async fn agent_runtime_messages(
    State(gateway): State<Gateway>,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    let token = match local_runtime_token(&parts.headers) {
        Some(token) => token,
        None => return error_response(GatewayError::Unauthorized("Agent broker".into())),
    };
    let active = match gateway.active_agent_runtime_routes.lock() {
        Ok(routes) => routes.get(token).cloned(),
        Err(_) => {
            return error_response(GatewayError::Configuration(
                "active Agent runtime route lock is poisoned".into(),
            ));
        }
    };
    let active = match active {
        Some(active) => active,
        None => return error_response(GatewayError::Unauthorized("Agent broker".into())),
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
    if active.model_id != model_id {
        return error_response(GatewayError::InvalidRequest(format!(
            "inactive Agent runtime route alias: {alias}"
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

async fn agent_runtime_responses(
    State(gateway): State<Gateway>,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    let token = match local_runtime_token(&parts.headers) {
        Some(token) => token,
        None => return error_response(GatewayError::Unauthorized("Agent broker".into())),
    };
    let active = match gateway.active_agent_runtime_routes.lock() {
        Ok(routes) => routes.get(token).cloned(),
        Err(_) => {
            return error_response(GatewayError::Configuration(
                "active Agent runtime route lock is poisoned".into(),
            ));
        }
    };
    let Some(active) = active else {
        return error_response(GatewayError::Unauthorized("Agent broker".into()));
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
    let Some(model_id) = alias.strip_prefix("grillforge/") else {
        return error_response(GatewayError::InvalidRequest(format!(
            "unknown GrillForge route alias: {alias}"
        )));
    };
    if active.model_id != model_id {
        return error_response(GatewayError::InvalidRequest(format!(
            "inactive Agent runtime route alias: {alias}"
        )));
    }
    match gateway
        .complete_codex_model(request, active.documents, model_id)
        .await
    {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

async fn agent_runtime_chat_completions(
    State(gateway): State<Gateway>,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    let token = match local_runtime_token(&parts.headers) {
        Some(token) => token,
        None => return error_response(GatewayError::Unauthorized("Agent broker".into())),
    };
    let active = match gateway.active_agent_runtime_routes.lock() {
        Ok(routes) => routes.get(token).cloned(),
        Err(_) => {
            return error_response(GatewayError::Configuration(
                "active Agent runtime route lock is poisoned".into(),
            ));
        }
    };
    let Some(active) = active else {
        return error_response(GatewayError::Unauthorized("Agent broker".into()));
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
    if active.model_id != model_id {
        return error_response(GatewayError::InvalidRequest(format!(
            "inactive Agent runtime route alias: {alias}"
        )));
    }
    match gateway
        .complete_chat_model(parts.headers, request, active.documents, &model_id)
        .await
    {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

fn local_runtime_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .or_else(|| {
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            headers
                .get("x-goog-api-key")
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
        })
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

fn select_model_protocol(
    documents: &ConfigurationDocuments,
    model_id: &str,
    inbound: NativeProtocol,
) -> Result<Protocol, GatewayError> {
    let model = documents
        .models
        .models
        .iter()
        .find(|model| model.id == model_id);
    let protocols = model.and_then(|model| model.native_protocols.as_ref());
    let Some(protocols) = protocols else {
        return Err(GatewayError::Configuration(format!(
            "model {model_id} has not been protocol-tested"
        )));
    };
    if protocols.is_empty() {
        return Err(GatewayError::Configuration(format!(
            "model {model_id} has no verified native protocol"
        )));
    }
    let selected = if protocols.contains(&inbound) {
        inbound
    } else {
        match inbound {
            NativeProtocol::AnthropicMessages => [
                NativeProtocol::OpenAiResponses,
                NativeProtocol::OpenAiChat,
                NativeProtocol::GeminiNative,
                NativeProtocol::AnthropicMessages,
            ],
            NativeProtocol::OpenAiResponses => [
                NativeProtocol::OpenAiChat,
                NativeProtocol::AnthropicMessages,
                NativeProtocol::GeminiNative,
                NativeProtocol::OpenAiResponses,
            ],
            NativeProtocol::OpenAiChat => [
                NativeProtocol::OpenAiResponses,
                NativeProtocol::AnthropicMessages,
                NativeProtocol::GeminiNative,
                NativeProtocol::OpenAiChat,
            ],
            NativeProtocol::GeminiNative => [
                NativeProtocol::OpenAiResponses,
                NativeProtocol::OpenAiChat,
                NativeProtocol::AnthropicMessages,
                NativeProtocol::GeminiNative,
            ],
        }
        .into_iter()
        .find(|candidate| protocols.contains(candidate))
        .ok_or_else(|| {
            GatewayError::Configuration(format!(
                "model {model_id} has no usable verified native protocol"
            ))
        })?
    };
    Ok(match selected {
        NativeProtocol::AnthropicMessages => Protocol::AnthropicMessages,
        NativeProtocol::OpenAiResponses => Protocol::OpenAiResponses,
        NativeProtocol::OpenAiChat => Protocol::OpenAiChatCompletions,
        NativeProtocol::GeminiNative => Protocol::GeminiNative,
    })
}

fn provider_protocol_endpoint(
    provider: &ProviderRecord,
    protocol: Protocol,
) -> Result<&ProviderProtocolEndpoint, GatewayError> {
    let native = match protocol {
        Protocol::AnthropicMessages => NativeProtocol::AnthropicMessages,
        Protocol::OpenAiResponses => NativeProtocol::OpenAiResponses,
        Protocol::OpenAiChatCompletions => NativeProtocol::OpenAiChat,
        Protocol::GeminiNative => NativeProtocol::GeminiNative,
    };
    provider
        .protocol_endpoints
        .iter()
        .find(|entry| entry.protocol == native)
        .ok_or_else(|| {
            GatewayError::Configuration(format!(
                "provider {} has no verified {:?} endpoint",
                provider.id, native
            ))
        })
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
