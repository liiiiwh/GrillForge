use crate::adapters::claude_code::MODEL_SLOT_IDS;
use crate::adapters::codex::{CodexModelSelection, CodexProviderRequest, CodexRequest};
use crate::configuration::{
    AgentRecord, ConfigurationDocuments, ConfigurationFiles, MainRecord, ModelRecord,
    ProviderRecord, SubAgentRecord,
};
use crate::core::model::ProtocolCapability;
use crate::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use crate::gateway::GatewayStatus;
use crate::model_discovery::{self, DiscoveredModel};
use crate::usage_query::{UsageQueryCredentials, UsageQueryPreset, UsageSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;
use tauri::State;

const CLAUDE_CODE_AGENT: &str = "claude_code";
const CLAUDE_DESKTOP_AGENT: &str = "claude_desktop";
const PI_AGENT: &str = "pi";
const CODEX_AGENT: &str = "codex";
const CODEX_MAIN_NATIVE_SLOT: &str = "main";
const CODEX_DEFAULT_SUBAGENT_SLOT: &str = "default_subagent";
const CODEX_DEFAULT_SUBAGENT_SLOT_MODEL: &str = "default_subagent";
const CODEX_AGENT_NATIVE_PREFIX: &str = "agent_";
const GEMINI_AGENT: &str = "gemini";
const GROK_BUILD_AGENT: &str = "grok_build";
const OPENCODE_AGENT: &str = "opencode";
const OPENCLAW_AGENT: &str = "openclaw";
const HERMES_AGENT: &str = "hermes";
const KIMI_CODE_AGENT: &str = "kimi_code";
const GENERIC_CLIENTS: &[&str] = &[
    GEMINI_AGENT,
    GROK_BUILD_AGENT,
    OPENCODE_AGENT,
    OPENCLAW_AGENT,
    HERMES_AGENT,
    KIMI_CODE_AGENT,
];
const CLAUDE_DESKTOP_MODEL_SLOT_IDS: &[&str] = &["sonnet", "opus", "fable", "haiku"];

pub struct ControlPlaneService {
    files: ConfigurationFiles,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ControlPlaneState {
    pub providers: Vec<PublicProvider>,
    pub models: Vec<PublicModel>,
    pub agent_enabled: bool,
    pub main_model_id: Option<String>,
    pub model_slots: BTreeMap<String, String>,
    pub claude_desktop_model_slots: BTreeMap<String, String>,
    pub pi_enabled: bool,
    pub pi_main_model_id: Option<String>,
    pub pi_enabled_model_ids: Vec<String>,
    pub codex_main_model_id: Option<String>,
    pub codex_native_model_slots: BTreeMap<String, String>,
    pub codex_agent_model_ids: BTreeMap<String, String>,
    pub client_configurations: BTreeMap<String, PublicClientConfiguration>,
    pub worker_mode: bool,
    pub native_subagent_enabled: bool,
    pub subagents: Vec<PublicSubAgent>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicClientConfiguration {
    pub main_model_id: Option<String>,
    pub secondary_model_id: Option<String>,
    pub enabled_model_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ClientSelection {
    pub provider: ProviderRecord,
    pub main_model: ModelRecord,
    pub enabled_models: Vec<ModelRecord>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicProvider {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub endpoint: String,
    pub endpoint_mode: EndpointMode,
    pub api_key_placement: ApiKeyPlacement,
    pub enabled: bool,
    pub credential_set: bool,
    pub models_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicModel {
    pub id: String,
    pub name: String,
    pub upstream_id: String,
    pub provider_id: String,
    pub capabilities: Vec<String>,
    pub protocol_capabilities: Vec<ProtocolCapability>,
    pub worker_enabled: bool,
    pub route_alias: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicSubAgent {
    pub id: String,
    pub name: String,
    pub model_id: String,
    pub capabilities: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub endpoint: String,
    pub endpoint_mode: EndpointMode,
    pub api_key_placement: ApiKeyPlacement,
    pub api_key: Option<String>,
    pub enabled: bool,
    pub models_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInput {
    pub id: String,
    pub name: String,
    pub upstream_id: String,
    pub provider_id: String,
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub protocol_capabilities: Vec<ProtocolCapability>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentInput {
    pub id: String,
    pub name: String,
    pub model_id: String,
    pub capabilities: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionResult {
    pub model_id: String,
    pub provider_id: String,
    pub upstream_id: String,
}

impl ControlPlaneService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            files: ConfigurationFiles::new(root),
        }
    }

    pub fn state(&self) -> Result<ControlPlaneState, String> {
        let documents = self.documents()?;
        public_state(&documents)
    }

    pub fn client_integration_enabled(&self, client_id: &str) -> Result<bool, String> {
        validate_known_client(client_id)?;
        Ok(self
            .documents()?
            .agents
            .agents
            .iter()
            .find(|agent| agent.id == client_id)
            .is_some_and(|agent| agent.enabled))
    }

    pub fn set_client_integration_enabled(
        &self,
        client_id: &str,
        enabled: bool,
    ) -> Result<(), String> {
        validate_known_client(client_id)?;
        let mut documents = self.documents()?;
        client_agent_mut(&mut documents, client_id).enabled = enabled;
        self.files
            .save(&documents.config, &documents.models, &documents.agents)
            .map_err(|error| error.to_string())
    }

    pub fn client_has_managed_configuration(&self, client_id: &str) -> Result<bool, String> {
        validate_known_client(client_id)?;
        let documents = self.documents()?;
        let Some(agent) = documents
            .agents
            .agents
            .iter()
            .find(|agent| agent.id == client_id)
        else {
            return Ok(false);
        };
        Ok(matches!(agent.main, MainRecord::Managed(_))
            || !agent.model_slots.is_empty()
            || !agent.native_model_slots.is_empty()
            || !agent.enabled_workers.is_empty()
            || !agent.subagents.is_empty())
    }

    fn provider_usage_query(
        &self,
        provider_id: &str,
    ) -> Result<(UsageQueryPreset, UsageQueryCredentials), String> {
        let documents = self.documents()?;
        let provider = documents
            .config
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .ok_or_else(|| format!("供应商不存在：{provider_id}"))?;
        if !provider.enabled {
            return Err(format!("供应商已停用：{provider_id}"));
        }
        let endpoint = url::Url::parse(&provider.endpoint)
            .map_err(|error| format!("供应商 Endpoint 无效：{error}"))?;
        let host = endpoint
            .host_str()
            .ok_or_else(|| "供应商 Endpoint 缺少主机名".to_string())?;
        let preset = match host {
            "api.deepseek.com" => UsageQueryPreset::DeepSeekBalance,
            "api.stepfun.com" => UsageQueryPreset::StepFunBalance,
            "api.siliconflow.cn" => UsageQueryPreset::SiliconFlowCnBalance,
            "api.siliconflow.com" => UsageQueryPreset::SiliconFlowGlobalBalance,
            "openrouter.ai" => UsageQueryPreset::OpenRouterBalance,
            "api.novita.ai" => UsageQueryPreset::NovitaBalance,
            "api.kimi.com" => UsageQueryPreset::KimiCodingPlan,
            "open.bigmodel.cn" => UsageQueryPreset::ZhipuCnCodingPlan,
            "api.z.ai" => UsageQueryPreset::ZhipuGlobalCodingPlan,
            "api.minimaxi.com" => UsageQueryPreset::MiniMaxCnCodingPlan,
            "api.minimax.io" => UsageQueryPreset::MiniMaxGlobalCodingPlan,
            _ => {
                return Err(format!(
                    "{} 暂无可用的官方余额或套餐查询接口",
                    provider.name
                ));
            }
        };
        let credentials = UsageQueryCredentials::new(provider.api_key.clone())
            .map_err(|error| error.to_string())?;
        Ok((preset, credentials))
    }

    pub fn save_provider(&self, input: ProviderInput) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let existing = documents
            .config
            .providers
            .iter()
            .position(|provider| provider.id == input.id);
        if existing.is_some() && input.api_key.is_some() {
            return Err(format!("duplicate provider id: {}", input.id));
        }
        let no_auth = input.api_key_placement == ApiKeyPlacement::None;
        let api_key = match (input.api_key, existing, no_auth) {
            (Some(value), _, false) => value,
            (None, Some(index), false) => documents.config.providers[index].api_key.clone(),
            (None, None, false) => return Err("provider API key must not be empty".to_string()),
            (_, _, true) => String::new(),
        };
        let record = ProviderRecord {
            id: input.id,
            name: input.name,
            enabled: input.enabled,
            protocol: input.protocol,
            endpoint: input.endpoint,
            endpoint_mode: input.endpoint_mode,
            api_key_placement: input.api_key_placement,
            api_key,
            models_url: input.models_url,
        };
        match existing {
            Some(index) => documents.config.providers[index] = record,
            None => documents.config.providers.push(record),
        }
        self.save_and_return(documents)
    }

    pub fn delete_provider(&self, id: &str) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let blocking: Vec<_> = documents
            .models
            .models
            .iter()
            .filter(|model| model.provider_id == id)
            .map(|model| model.display_name.as_str())
            .collect();
        if !blocking.is_empty() {
            return Err(format!(
                "provider {id} is referenced by models: {}",
                blocking.join(", ")
            ));
        }
        let before = documents.config.providers.len();
        documents
            .config
            .providers
            .retain(|provider| provider.id != id);
        if documents.config.providers.len() == before {
            return Err(format!("unknown provider: {id}"));
        }
        self.save_and_return(documents)
    }

    pub fn update_provider(&self, input: ProviderInput) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let index = documents
            .config
            .providers
            .iter()
            .position(|provider| provider.id == input.id)
            .ok_or_else(|| format!("unknown provider: {}", input.id))?;
        let no_auth = input.api_key_placement == ApiKeyPlacement::None;
        let api_key = match (input.api_key, no_auth) {
            (_, true) => String::new(),
            (Some(value), false) => value,
            (None, false) => documents.config.providers[index].api_key.clone(),
        };
        documents.config.providers[index] = ProviderRecord {
            id: input.id,
            name: input.name,
            enabled: input.enabled,
            protocol: input.protocol,
            endpoint: input.endpoint,
            endpoint_mode: input.endpoint_mode,
            api_key_placement: input.api_key_placement,
            api_key,
            models_url: input.models_url,
        };
        self.save_and_return(documents)
    }

    pub async fn discover_provider_models(
        &self,
        provider_id: &str,
    ) -> Result<Vec<DiscoveredModel>, String> {
        let documents = self.documents()?;
        let provider = documents
            .config
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .ok_or_else(|| format!("unknown provider: {provider_id}"))?;
        model_discovery::discover(provider).await
    }

    pub fn import_provider_models(
        &self,
        provider_id: &str,
        discovered: Vec<DiscoveredModel>,
    ) -> Result<ControlPlaneState, String> {
        if discovered.is_empty() {
            return Err("select at least one model to import".to_string());
        }
        let mut documents = self.documents()?;
        if !documents
            .config
            .providers
            .iter()
            .any(|provider| provider.id == provider_id)
        {
            return Err(format!("unknown provider: {provider_id}"));
        }
        for model in discovered {
            if documents.models.models.iter().any(|existing| {
                existing.provider_id == provider_id && existing.upstream_id == model.id
            }) {
                continue;
            }
            let id = model_slug(&model.id);
            if id.is_empty() {
                return Err(format!(
                    "model ID cannot produce a stable slug: {}",
                    model.id
                ));
            }
            if documents
                .models
                .models
                .iter()
                .any(|existing| existing.id == id)
            {
                return Err(format!("model slug collision: {id}"));
            }
            let protocol_capabilities = crate::presets::catalog()
                .map_err(|_| "built-in Provider catalog is invalid".to_string())?
                .presets
                .into_iter()
                .find(|preset| preset.id == provider_id)
                .and_then(|preset| preset.model_protocol_capabilities.get(&model.id).cloned())
                .unwrap_or_default();
            documents.models.models.push(ModelRecord {
                id,
                provider_id: provider_id.to_string(),
                upstream_id: model.id.clone(),
                display_name: model.id,
                capabilities: Vec::new(),
                protocol_capabilities,
            });
        }
        documents
            .models
            .models
            .sort_by(|left, right| left.id.cmp(&right.id));
        self.save_and_return(documents)
    }

    pub fn save_model(&self, input: ModelInput) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let record = ModelRecord {
            id: input.id,
            provider_id: input.provider_id,
            upstream_id: input.upstream_id,
            display_name: input.name,
            capabilities: input.capabilities,
            protocol_capabilities: input.protocol_capabilities,
        };
        if documents
            .models
            .models
            .iter()
            .any(|model| model.id == record.id)
        {
            return Err(format!("duplicate model id: {}", record.id));
        }
        documents.models.models.push(record);
        self.save_and_return(documents)
    }

    pub fn update_model(&self, input: ModelInput) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let index = documents
            .models
            .models
            .iter()
            .position(|model| model.id == input.id)
            .ok_or_else(|| format!("unknown model: {}", input.id))?;
        documents.models.models[index] = ModelRecord {
            id: input.id,
            provider_id: input.provider_id,
            upstream_id: input.upstream_id,
            display_name: input.name,
            capabilities: input.capabilities,
            protocol_capabilities: input.protocol_capabilities,
        };
        self.save_and_return(documents)
    }

    pub fn delete_model(&self, id: &str) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let selected_by = documents.agents.agents.iter().find(|agent| {
            matches!(&agent.main, MainRecord::Managed(model) if model == id)
                || agent.enabled_workers.iter().any(|model| model == id)
                || agent.model_slots.values().any(|model| model == id)
                || agent
                    .subagents
                    .iter()
                    .any(|subagent| subagent.model_id == id)
        });
        if let Some(agent) = selected_by {
            return Err(format!("model {id} is selected by {}", agent.id));
        }
        let before = documents.models.models.len();
        documents.models.models.retain(|model| model.id != id);
        if documents.models.models.len() == before {
            return Err(format!("unknown model: {id}"));
        }
        self.save_and_return(documents)
    }

    pub fn set_main_model(&self, id: Option<String>) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        claude_agent_mut(&mut documents)?.main = match id {
            Some(id) => MainRecord::Managed(id),
            None => MainRecord::Native,
        };
        self.save_and_return(documents)
    }

    pub fn set_model_slot(
        &self,
        slot: String,
        id: Option<String>,
    ) -> Result<ControlPlaneState, String> {
        if !MODEL_SLOT_IDS.contains(&slot.as_str()) {
            return Err(format!("unsupported Claude Code model slot: {slot}"));
        }
        let mut documents = self.documents()?;
        let agent = claude_agent_mut(&mut documents)?;
        match id {
            Some(id) => {
                agent.model_slots.insert(slot, id);
            }
            None => {
                agent.model_slots.remove(&slot);
            }
        }
        self.save_and_return(documents)
    }

    pub fn set_claude_desktop_model_slot(
        &self,
        slot: String,
        id: Option<String>,
    ) -> Result<ControlPlaneState, String> {
        if !CLAUDE_DESKTOP_MODEL_SLOT_IDS.contains(&slot.as_str()) {
            return Err(format!("unsupported Claude Client model slot: {slot}"));
        }
        let mut documents = self.documents()?;
        let agent = match documents
            .agents
            .agents
            .iter_mut()
            .find(|agent| agent.id == CLAUDE_DESKTOP_AGENT)
        {
            Some(agent) => agent,
            None if id.is_none() => return public_state(&documents),
            None => {
                documents.agents.agents.push(AgentRecord {
                    id: CLAUDE_DESKTOP_AGENT.into(),
                    adapter: CLAUDE_DESKTOP_AGENT.into(),
                    enabled: true,
                    main: MainRecord::Native,
                    model_slots: BTreeMap::new(),
                    native_model_slots: BTreeMap::new(),
                    worker_mode: false,
                    enabled_workers: Vec::new(),
                    native_subagent_enabled: true,
                    subagents: Vec::new(),
                });
                documents
                    .agents
                    .agents
                    .last_mut()
                    .expect("just inserted Claude Client agent")
            }
        };
        match id {
            Some(id) => {
                agent.model_slots.insert(slot, id);
            }
            None => {
                agent.model_slots.remove(&slot);
            }
        }
        self.save_and_return(documents)
    }

    pub fn set_pi_main_model(&self, id: Option<String>) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let agent = pi_agent_mut(&mut documents);
        agent.main = match id {
            Some(id) => {
                if !agent.enabled_workers.contains(&id) {
                    agent.enabled_workers.push(id.clone());
                    agent.enabled_workers.sort();
                }
                agent.worker_mode = true;
                MainRecord::Managed(id)
            }
            None => MainRecord::Native,
        };
        self.save_and_return(documents)
    }

    pub fn set_pi_model_enabled(
        &self,
        id: String,
        enabled: bool,
    ) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let agent = pi_agent_mut(&mut documents);
        let is_default = matches!(&agent.main, MainRecord::Managed(model) if model == &id);
        if !enabled && is_default {
            return Err(format!("Pi default model cannot be disabled: {id}"));
        }
        let exists = agent.enabled_workers.contains(&id);
        match (enabled, exists) {
            (true, false) => {
                agent.enabled_workers.push(id);
                agent.enabled_workers.sort();
            }
            (false, true) => agent.enabled_workers.retain(|model| model != &id),
            _ => {}
        }
        agent.worker_mode = !agent.enabled_workers.is_empty();
        self.save_and_return(documents)
    }

    pub fn set_codex_main_model(&self, id: Option<String>) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        if let Some(id) = &id {
            validate_codex_registry_model(&documents, id)?;
        }
        let agent = client_agent_mut(&mut documents, CODEX_AGENT);
        agent.main = id.map_or(MainRecord::Native, MainRecord::Managed);
        agent.native_model_slots.remove(CODEX_MAIN_NATIVE_SLOT);
        self.save_and_return(documents)
    }

    pub fn set_codex_native_main_model(
        &self,
        model: Option<String>,
    ) -> Result<ControlPlaneState, String> {
        if let Some(model) = &model {
            validate_codex_native_model(model)?;
        }
        let mut documents = self.documents()?;
        let agent = client_agent_mut(&mut documents, CODEX_AGENT);
        agent.main = MainRecord::Native;
        match model {
            Some(model) => {
                agent
                    .native_model_slots
                    .insert(CODEX_MAIN_NATIVE_SLOT.into(), model);
            }
            None => {
                agent.native_model_slots.remove(CODEX_MAIN_NATIVE_SLOT);
            }
        }
        self.save_and_return(documents)
    }

    pub fn set_codex_default_subagent_model(
        &self,
        id: Option<String>,
    ) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        if let Some(id) = &id {
            validate_codex_registry_model(&documents, id)?;
        }
        let agent = client_agent_mut(&mut documents, CODEX_AGENT);
        match id {
            Some(id) => {
                agent
                    .model_slots
                    .insert(CODEX_DEFAULT_SUBAGENT_SLOT_MODEL.into(), id);
            }
            None => {
                agent.model_slots.remove(CODEX_DEFAULT_SUBAGENT_SLOT_MODEL);
            }
        }
        agent.native_model_slots.remove(CODEX_DEFAULT_SUBAGENT_SLOT);
        self.save_and_return(documents)
    }

    pub fn set_codex_native_default_subagent_model(
        &self,
        model: Option<String>,
    ) -> Result<ControlPlaneState, String> {
        if let Some(model) = &model {
            validate_codex_native_model(model)?;
        }
        let mut documents = self.documents()?;
        let agent = client_agent_mut(&mut documents, CODEX_AGENT);
        agent.model_slots.remove(CODEX_DEFAULT_SUBAGENT_SLOT_MODEL);
        match model {
            Some(model) => {
                agent
                    .native_model_slots
                    .insert(CODEX_DEFAULT_SUBAGENT_SLOT.into(), model);
            }
            None => {
                agent.native_model_slots.remove(CODEX_DEFAULT_SUBAGENT_SLOT);
            }
        }
        self.save_and_return(documents)
    }

    pub fn set_codex_custom_agent_model(
        &self,
        name: String,
        id: Option<String>,
    ) -> Result<ControlPlaneState, String> {
        validate_codex_agent_name(&name)?;
        let mut documents = self.documents()?;
        if let Some(id) = &id {
            validate_codex_registry_model(&documents, id)?;
        }
        let agent = client_agent_mut(&mut documents, CODEX_AGENT);
        agent.subagents.retain(|record| record.id != name);
        if let Some(id) = id {
            agent.subagents.push(SubAgentRecord {
                id: name.clone(),
                name: name.clone(),
                model_id: id,
                capabilities: Vec::new(),
                enabled: true,
            });
            agent
                .subagents
                .sort_by(|left, right| left.id.cmp(&right.id));
        }
        agent
            .native_model_slots
            .remove(&codex_agent_native_slot(&name));
        self.save_and_return(documents)
    }

    pub fn set_codex_native_custom_agent_model(
        &self,
        name: String,
        model: Option<String>,
    ) -> Result<ControlPlaneState, String> {
        validate_codex_agent_name(&name)?;
        if let Some(model) = &model {
            validate_codex_native_model(model)?;
        }
        let mut documents = self.documents()?;
        let agent = client_agent_mut(&mut documents, CODEX_AGENT);
        agent.subagents.retain(|record| record.id != name);
        let slot = codex_agent_native_slot(&name);
        match model {
            Some(model) => {
                agent.native_model_slots.insert(slot, model);
            }
            None => {
                agent.native_model_slots.remove(&slot);
            }
        }
        self.save_and_return(documents)
    }

    pub fn codex_request(
        &self,
        gateway_base_url: &str,
        token: &str,
    ) -> Result<CodexRequest, String> {
        let documents = self.documents()?;
        let routed_provider = CodexProviderRequest::new(
            "grillforge",
            "GrillForge 本地路由",
            format!("{}/codex/v1", gateway_base_url.trim_end_matches('/')),
            token,
        )
        .map_err(|error| error.to_string())?;
        let agent = documents
            .agents
            .agents
            .iter()
            .find(|agent| agent.id == CODEX_AGENT)
            .ok_or_else(|| "Codex has no configured model".to_string())?;
        let main = match &agent.main {
            MainRecord::Managed(model_id) => {
                codex_routed_selection(&documents, model_id, &routed_provider)?
            }
            MainRecord::Native => agent
                .native_model_slots
                .get(CODEX_MAIN_NATIVE_SLOT)
                .ok_or_else(|| "Codex has no configured model".to_string())
                .and_then(|model| {
                    CodexModelSelection::native(model).map_err(|error| error.to_string())
                })?,
        };
        let default_subagent =
            if let Some(model) = agent.native_model_slots.get(CODEX_DEFAULT_SUBAGENT_SLOT) {
                Some(CodexModelSelection::native(model).map_err(|error| error.to_string())?)
            } else {
                agent
                    .model_slots
                    .get(CODEX_DEFAULT_SUBAGENT_SLOT_MODEL)
                    .map(|id| codex_routed_selection(&documents, id, &routed_provider))
                    .transpose()?
            };
        let mut custom_agents = BTreeMap::new();
        for record in &agent.subagents {
            if record.enabled {
                custom_agents.insert(
                    record.id.clone(),
                    codex_routed_selection(&documents, &record.model_id, &routed_provider)?,
                );
            }
        }
        for (slot, model) in &agent.native_model_slots {
            let Some(name) = slot.strip_prefix(CODEX_AGENT_NATIVE_PREFIX) else {
                continue;
            };
            custom_agents.insert(
                name.to_string(),
                CodexModelSelection::native(model).map_err(|error| error.to_string())?,
            );
        }
        CodexRequest::from_selections(main, default_subagent, custom_agents)
            .map_err(|error| error.to_string())
    }

    pub fn codex_route_model_ids(&self) -> Result<Vec<String>, String> {
        let documents = self.documents()?;
        let agent = documents
            .agents
            .agents
            .iter()
            .find(|agent| agent.id == CODEX_AGENT)
            .ok_or_else(|| "Codex has no configured model".to_string())?;
        let mut ids = BTreeMap::new();
        if let MainRecord::Managed(id) = &agent.main {
            validate_codex_registry_model(&documents, id)?;
            ids.insert(id.clone(), ());
        }
        if let Some(id) = agent.model_slots.get(CODEX_DEFAULT_SUBAGENT_SLOT_MODEL) {
            validate_codex_registry_model(&documents, id)?;
            ids.insert(id.clone(), ());
        }
        for record in &agent.subagents {
            if record.enabled {
                validate_codex_registry_model(&documents, &record.model_id)?;
                ids.insert(record.model_id.clone(), ());
            }
        }
        Ok(ids.into_keys().collect())
    }

    pub fn set_client_main_model(
        &self,
        client_id: String,
        id: Option<String>,
    ) -> Result<ControlPlaneState, String> {
        validate_generic_client(&client_id)?;
        let mut documents = self.documents()?;
        if let Some(model_id) = &id {
            validate_client_model(&documents, &client_id, model_id)?;
        }
        let agent = client_agent_mut(&mut documents, &client_id);
        if let Some(model_id) = &id {
            if matches!(
                client_id.as_str(),
                OPENCODE_AGENT | OPENCLAW_AGENT | HERMES_AGENT | KIMI_CODE_AGENT
            ) && !agent.enabled_workers.contains(model_id)
            {
                agent.enabled_workers.push(model_id.clone());
                agent.enabled_workers.sort();
            }
        }
        agent.main = id.map_or(MainRecord::Native, MainRecord::Managed);
        agent.worker_mode = !agent.enabled_workers.is_empty();
        self.save_and_return(documents)
    }

    pub fn set_client_model_enabled(
        &self,
        client_id: String,
        id: String,
        enabled: bool,
    ) -> Result<ControlPlaneState, String> {
        if !matches!(
            client_id.as_str(),
            OPENCODE_AGENT | OPENCLAW_AGENT | HERMES_AGENT | KIMI_CODE_AGENT
        ) {
            return Err(format!("{client_id} does not expose a managed model pool"));
        }
        let mut documents = self.documents()?;
        validate_client_model(&documents, &client_id, &id)?;
        let agent = client_agent_mut(&mut documents, &client_id);
        if !enabled && matches!(&agent.main, MainRecord::Managed(main) if main == &id) {
            return Err(format!("{client_id} main model cannot be disabled: {id}"));
        }
        if !enabled && agent.model_slots.get("secondary") == Some(&id) {
            return Err(format!(
                "{client_id} secondary model cannot be disabled: {id}"
            ));
        }
        let exists = agent.enabled_workers.contains(&id);
        match (enabled, exists) {
            (true, false) => {
                agent.enabled_workers.push(id);
                agent.enabled_workers.sort();
            }
            (false, true) => agent.enabled_workers.retain(|model| model != &id),
            _ => {}
        }
        agent.worker_mode = !agent.enabled_workers.is_empty();
        self.save_and_return(documents)
    }

    pub fn set_client_secondary_model(
        &self,
        client_id: String,
        id: Option<String>,
    ) -> Result<ControlPlaneState, String> {
        if client_id != KIMI_CODE_AGENT {
            return Err(format!("{client_id} does not expose a secondary model"));
        }
        let mut documents = self.documents()?;
        if let Some(model_id) = &id {
            validate_client_model(&documents, &client_id, model_id)?;
        }
        let agent = client_agent_mut(&mut documents, &client_id);
        match id {
            Some(model_id) => {
                if !agent.enabled_workers.contains(&model_id) {
                    agent.enabled_workers.push(model_id.clone());
                    agent.enabled_workers.sort();
                }
                agent.model_slots.insert("secondary".into(), model_id);
            }
            None => {
                agent.model_slots.remove("secondary");
            }
        }
        agent.worker_mode = !agent.enabled_workers.is_empty();
        self.save_and_return(documents)
    }

    pub fn client_selection(&self, client_id: &str) -> Result<ClientSelection, String> {
        validate_generic_client(client_id)?;
        let documents = self.documents()?;
        let agent = documents
            .agents
            .agents
            .iter()
            .find(|agent| agent.id == client_id)
            .ok_or_else(|| format!("{client_id} has no configured model"))?;
        let MainRecord::Managed(main_model_id) = &agent.main else {
            return Err(format!("{client_id} has no configured main model"));
        };
        validate_client_model(&documents, client_id, main_model_id)?;
        let main_model = documents
            .models
            .models
            .iter()
            .find(|model| &model.id == main_model_id)
            .expect("validated model")
            .clone();
        let provider = documents
            .config
            .providers
            .iter()
            .find(|provider| provider.id == main_model.provider_id)
            .expect("validated provider")
            .clone();
        let enabled_ids = if matches!(
            client_id,
            OPENCODE_AGENT | OPENCLAW_AGENT | HERMES_AGENT | KIMI_CODE_AGENT
        ) {
            &agent.enabled_workers
        } else {
            std::slice::from_ref(main_model_id)
        };
        let enabled_models = enabled_ids
            .iter()
            .map(|id| {
                validate_client_model(&documents, client_id, id)?;
                documents
                    .models
                    .models
                    .iter()
                    .find(|model| &model.id == id)
                    .cloned()
                    .ok_or_else(|| format!("{client_id} references unknown model: {id}"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(ClientSelection {
            provider,
            main_model,
            enabled_models,
        })
    }

    pub fn set_worker(&self, id: String, enabled: bool) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let agent = claude_agent_mut(&mut documents)?;
        let exists = agent.enabled_workers.iter().any(|model| model == &id);
        match (enabled, exists) {
            (true, false) => agent.enabled_workers.push(id),
            (false, true) => agent.enabled_workers.retain(|model| model != &id),
            _ => {}
        }
        self.save_and_return(documents)
    }

    pub fn set_worker_mode(&self, enabled: bool) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        claude_agent_mut(&mut documents)?.worker_mode = enabled;
        self.save_and_return(documents)
    }

    pub fn set_native_subagent_enabled(&self, enabled: bool) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        claude_agent_mut(&mut documents)?.native_subagent_enabled = enabled;
        self.save_and_return(documents)
    }

    pub fn save_subagent(&self, input: SubAgentInput) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let agent = claude_agent_mut(&mut documents)?;
        if agent
            .subagents
            .iter()
            .any(|subagent| subagent.id == input.id)
        {
            return Err(format!("duplicate SubAgent id: {}", input.id));
        }
        agent.subagents.push(subagent_record(input));
        self.save_and_return(documents)
    }

    pub fn update_subagent(&self, input: SubAgentInput) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let agent = claude_agent_mut(&mut documents)?;
        let index = agent
            .subagents
            .iter()
            .position(|subagent| subagent.id == input.id)
            .ok_or_else(|| format!("unknown SubAgent: {}", input.id))?;
        agent.subagents[index] = subagent_record(input);
        self.save_and_return(documents)
    }

    pub fn delete_subagent(&self, id: &str) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let agent = claude_agent_mut(&mut documents)?;
        let before = agent.subagents.len();
        agent.subagents.retain(|subagent| subagent.id != id);
        if agent.subagents.len() == before {
            return Err(format!("unknown SubAgent: {id}"));
        }
        self.save_and_return(documents)
    }

    pub async fn test_model_connection(
        &self,
        gateway_base_url: &str,
        id: &str,
    ) -> Result<ConnectionResult, String> {
        let documents = self.documents()?;
        let private_model = documents
            .models
            .models
            .iter()
            .find(|model| model.id == id)
            .ok_or_else(|| format!("unknown model: {id}"))?;
        let private_provider = documents
            .config
            .providers
            .iter()
            .find(|provider| provider.id == private_model.provider_id)
            .ok_or_else(|| format!("model {id} references unknown provider"))?;
        if private_provider.protocol == Protocol::GeminiNative {
            return test_gemini_connection(private_provider, private_model).await;
        }
        let state = self.state()?;
        let model = state
            .models
            .iter()
            .find(|model| model.id == id)
            .ok_or_else(|| format!("unknown model: {id}"))?;
        let provider = state
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

        let endpoint = format!("{}/v1/messages", gateway_base_url.trim_end_matches('/'));
        let response = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| format!("could not create connection test client: {error}"))?
            .post(endpoint)
            .header("anthropic-version", "2023-06-01")
            .json(&json!({
                "model": model.route_alias,
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "Reply with OK."}]
            }))
            .send()
            .await
            .map_err(|error| format!("model connection failed: {error}"))?;
        let status = response.status();
        let body = response
            .json::<Value>()
            .await
            .map_err(|_| "model connection returned invalid JSON".to_string())?;
        if !status.is_success() {
            let message = body
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("upstream request failed")
                .replace(['\r', '\n'], " ");
            return Err(format!(
                "model connection returned HTTP {}: {}",
                status.as_u16(),
                message.chars().take(300).collect::<String>()
            ));
        }
        let valid_message = body.get("type").and_then(Value::as_str) == Some("message")
            && body
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|content| !content.is_empty());
        if !valid_message {
            return Err("model connection returned an invalid Anthropic message".to_string());
        }
        Ok(ConnectionResult {
            model_id: model.id.clone(),
            provider_id: provider.id.clone(),
            upstream_id: model.upstream_id.clone(),
        })
    }

    fn documents(&self) -> Result<ConfigurationDocuments, String> {
        self.files
            .open_or_initialize()
            .map_err(|error| error.to_string())
    }

    fn save_and_return(
        &self,
        documents: ConfigurationDocuments,
    ) -> Result<ControlPlaneState, String> {
        self.files
            .save(&documents.config, &documents.models, &documents.agents)
            .map_err(|error| error.to_string())?;
        public_state(&documents)
    }
}

async fn test_gemini_connection(
    provider: &ProviderRecord,
    model: &ModelRecord,
) -> Result<ConnectionResult, String> {
    if !provider.enabled {
        return Err(format!(
            "model {} uses disabled provider {}",
            model.id, provider.id
        ));
    }
    if provider.endpoint_mode != EndpointMode::BaseUrl
        || provider.api_key_placement != ApiKeyPlacement::XApiKey
    {
        return Err(format!(
            "Gemini Native connection requires an API-key Base URL provider: {}",
            provider.id
        ));
    }
    let base = provider.endpoint.trim_end_matches('/');
    let prefix = if base.ends_with("/v1beta") {
        base.to_string()
    } else {
        format!("{base}/v1beta")
    };
    let endpoint = format!("{prefix}/models/{}:generateContent", model.upstream_id);
    let response = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("could not create Gemini connection test client: {error}"))?
        .post(&endpoint)
        .header("x-goog-api-key", &provider.api_key)
        .json(&json!({
            "contents": [{"role": "user", "parts": [{"text": "Reply with OK."}]}],
            "generationConfig": {"maxOutputTokens": 16}
        }))
        .send()
        .await
        .map_err(|error| format!("Gemini model connection failed: {error}"))?;
    let status = response.status();
    let body = response
        .json::<Value>()
        .await
        .map_err(|_| "Gemini model connection returned invalid JSON".to_string())?;
    if !status.is_success() {
        let message = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("upstream request failed")
            .replace(['\r', '\n'], " ");
        return Err(format!(
            "Gemini model connection returned HTTP {}: {}",
            status.as_u16(),
            message.chars().take(300).collect::<String>()
        ));
    }
    if body
        .get("candidates")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        return Err("Gemini model connection returned no candidates".into());
    }
    Ok(ConnectionResult {
        model_id: model.id.clone(),
        provider_id: provider.id.clone(),
        upstream_id: model.upstream_id.clone(),
    })
}

fn model_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut needs_separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if needs_separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character);
            needs_separator = false;
        } else if !slug.is_empty() {
            needs_separator = true;
        }
    }
    slug
}

fn claude_agent(documents: &ConfigurationDocuments) -> Result<&AgentRecord, String> {
    documents
        .agents
        .agents
        .iter()
        .find(|agent| agent.id == CLAUDE_CODE_AGENT)
        .ok_or_else(|| "agents.yaml is missing claude_code".to_string())
}

fn subagent_record(input: SubAgentInput) -> SubAgentRecord {
    SubAgentRecord {
        id: input.id,
        name: input.name,
        model_id: input.model_id,
        capabilities: input.capabilities,
        enabled: input.enabled,
    }
}

fn claude_agent_mut(documents: &mut ConfigurationDocuments) -> Result<&mut AgentRecord, String> {
    documents
        .agents
        .agents
        .iter_mut()
        .find(|agent| agent.id == CLAUDE_CODE_AGENT)
        .ok_or_else(|| "agents.yaml is missing claude_code".to_string())
}

fn pi_agent_mut(documents: &mut ConfigurationDocuments) -> &mut AgentRecord {
    client_agent_mut(documents, PI_AGENT)
}

fn client_agent_mut<'a>(
    documents: &'a mut ConfigurationDocuments,
    client_id: &str,
) -> &'a mut AgentRecord {
    if let Some(index) = documents
        .agents
        .agents
        .iter()
        .position(|agent| agent.id == client_id)
    {
        return &mut documents.agents.agents[index];
    }
    documents.agents.agents.push(AgentRecord {
        id: client_id.into(),
        adapter: client_id.into(),
        enabled: false,
        main: MainRecord::Native,
        model_slots: BTreeMap::new(),
        native_model_slots: BTreeMap::new(),
        worker_mode: false,
        enabled_workers: Vec::new(),
        native_subagent_enabled: true,
        subagents: Vec::new(),
    });
    documents
        .agents
        .agents
        .last_mut()
        .expect("client agent was just inserted")
}

fn validate_codex_native_model(model: &str) -> Result<(), String> {
    if model.trim().is_empty() || model.trim() != model || model.chars().any(char::is_control) {
        return Err(
            "Codex native model must not be empty, padded, or contain control characters".into(),
        );
    }
    Ok(())
}

fn validate_codex_agent_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.starts_with(['-', '_'])
        || name.ends_with(['-', '_'])
        || name.contains("--")
        || name.contains("__")
        || !name.bytes().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, b'-' | b'_')
        })
    {
        return Err(format!(
            "Codex custom Agent name must be a lowercase slug: {name}"
        ));
    }
    Ok(())
}

fn codex_agent_native_slot(name: &str) -> String {
    format!("{CODEX_AGENT_NATIVE_PREFIX}{name}")
}

fn validate_codex_registry_model(
    documents: &ConfigurationDocuments,
    id: &str,
) -> Result<(), String> {
    let model = documents
        .models
        .models
        .iter()
        .find(|model| model.id == id)
        .ok_or_else(|| format!("Codex references unknown model: {id}"))?;
    let provider = documents
        .config
        .providers
        .iter()
        .find(|provider| provider.id == model.provider_id)
        .ok_or_else(|| format!("model {id} references unknown provider"))?;
    if !provider.enabled {
        return Err(format!(
            "Codex model uses disabled provider: {}",
            provider.id
        ));
    }
    if !matches!(
        provider.protocol,
        Protocol::AnthropicMessages | Protocol::OpenAiResponses | Protocol::OpenAiChatCompletions
    ) {
        return Err(format!(
            "Codex local routing does not support provider protocol {:?}: {}",
            provider.protocol, provider.id
        ));
    }
    Ok(())
}

fn codex_routed_selection(
    documents: &ConfigurationDocuments,
    model_id: &str,
    provider: &CodexProviderRequest,
) -> Result<CodexModelSelection, String> {
    let model = documents
        .models
        .models
        .iter()
        .find(|model| model.id == model_id)
        .ok_or_else(|| format!("Codex references unknown model: {model_id}"))?;
    let upstream = documents
        .config
        .providers
        .iter()
        .find(|candidate| candidate.id == model.provider_id)
        .ok_or_else(|| format!("model {model_id} references unknown provider"))?;
    if !upstream.enabled {
        return Err(format!(
            "Codex model uses disabled provider: {}",
            upstream.id
        ));
    }
    CodexModelSelection::managed(provider.clone(), format!("grillforge/{model_id}"))
        .map_err(|error| error.to_string())
}

fn validate_generic_client(client_id: &str) -> Result<(), String> {
    if GENERIC_CLIENTS.contains(&client_id) {
        Ok(())
    } else {
        Err(format!("unsupported client adapter: {client_id}"))
    }
}

fn validate_known_client(client_id: &str) -> Result<(), String> {
    if matches!(
        client_id,
        CLAUDE_CODE_AGENT | CLAUDE_DESKTOP_AGENT | PI_AGENT | CODEX_AGENT
    ) || GENERIC_CLIENTS.contains(&client_id)
    {
        Ok(())
    } else {
        Err(format!("unsupported client adapter: {client_id}"))
    }
}

fn validate_client_model(
    documents: &ConfigurationDocuments,
    client_id: &str,
    model_id: &str,
) -> Result<(), String> {
    let model = documents
        .models
        .models
        .iter()
        .find(|model| model.id == model_id)
        .ok_or_else(|| format!("{client_id} references unknown model: {model_id}"))?;
    let provider = documents
        .config
        .providers
        .iter()
        .find(|provider| provider.id == model.provider_id)
        .ok_or_else(|| format!("model {model_id} references unknown provider"))?;
    if !provider.enabled {
        return Err(format!(
            "model {model_id} uses disabled provider {}",
            provider.id
        ));
    }
    let compatible = match client_id {
        GEMINI_AGENT => {
            provider.protocol == Protocol::GeminiNative
                && provider.endpoint_mode == EndpointMode::BaseUrl
                && provider.api_key_placement == ApiKeyPlacement::XApiKey
        }
        GROK_BUILD_AGENT => {
            provider.protocol == Protocol::OpenAiResponses
                && provider.endpoint_mode == EndpointMode::BaseUrl
                && provider.api_key_placement == ApiKeyPlacement::Bearer
        }
        OPENCODE_AGENT | OPENCLAW_AGENT | HERMES_AGENT | KIMI_CODE_AGENT => {
            provider.protocol != Protocol::GeminiNative
        }
        _ => false,
    };
    if compatible {
        Ok(())
    } else {
        Err(format!(
            "provider {} is incompatible with {client_id}",
            provider.id
        ))
    }
}

fn public_state(documents: &ConfigurationDocuments) -> Result<ControlPlaneState, String> {
    let agent = claude_agent(documents)?;
    let claude_desktop_model_slots = documents
        .agents
        .agents
        .iter()
        .find(|agent| agent.id == CLAUDE_DESKTOP_AGENT)
        .map(|agent| agent.model_slots.clone())
        .unwrap_or_default();
    let workers = &agent.enabled_workers;
    let pi = documents
        .agents
        .agents
        .iter()
        .find(|agent| agent.id == PI_AGENT);
    let codex = documents
        .agents
        .agents
        .iter()
        .find(|agent| agent.id == CODEX_AGENT);
    let client_configurations = GENERIC_CLIENTS
        .iter()
        .map(|client_id| {
            let agent = documents
                .agents
                .agents
                .iter()
                .find(|agent| agent.id == *client_id);
            (
                (*client_id).to_string(),
                PublicClientConfiguration {
                    main_model_id: agent.and_then(|agent| match &agent.main {
                        MainRecord::Native => None,
                        MainRecord::Managed(id) => Some(id.clone()),
                    }),
                    secondary_model_id: agent
                        .and_then(|agent| agent.model_slots.get("secondary").cloned()),
                    enabled_model_ids: agent
                        .map(|agent| agent.enabled_workers.clone())
                        .unwrap_or_default(),
                },
            )
        })
        .collect();
    Ok(ControlPlaneState {
        providers: documents
            .config
            .providers
            .iter()
            .map(|provider| PublicProvider {
                id: provider.id.clone(),
                name: provider.name.clone(),
                protocol: provider.protocol,
                endpoint: provider.endpoint.clone(),
                endpoint_mode: provider.endpoint_mode,
                api_key_placement: provider.api_key_placement,
                enabled: provider.enabled,
                credential_set: !provider.api_key.trim().is_empty(),
                models_url: provider.models_url.clone(),
            })
            .collect(),
        models: documents
            .models
            .models
            .iter()
            .map(|model| PublicModel {
                id: model.id.clone(),
                name: model.display_name.clone(),
                upstream_id: model.upstream_id.clone(),
                provider_id: model.provider_id.clone(),
                capabilities: model.capabilities.clone(),
                protocol_capabilities: model.protocol_capabilities.clone(),
                worker_enabled: workers.contains(&model.id),
                route_alias: format!("grillforge/{}", model.id),
            })
            .collect(),
        agent_enabled: agent.enabled,
        main_model_id: match &agent.main {
            MainRecord::Native => None,
            MainRecord::Managed(id) => Some(id.clone()),
        },
        model_slots: agent.model_slots.clone(),
        claude_desktop_model_slots,
        pi_enabled: pi.is_some_and(|agent| agent.enabled && agent.worker_mode),
        pi_main_model_id: pi.and_then(|agent| match &agent.main {
            MainRecord::Native => None,
            MainRecord::Managed(id) => Some(id.clone()),
        }),
        pi_enabled_model_ids: pi
            .map(|agent| agent.enabled_workers.clone())
            .unwrap_or_default(),
        codex_main_model_id: codex.and_then(|agent| match &agent.main {
            MainRecord::Native => None,
            MainRecord::Managed(id) => Some(id.clone()),
        }),
        codex_native_model_slots: codex
            .map(|agent| agent.native_model_slots.clone())
            .unwrap_or_default(),
        codex_agent_model_ids: codex
            .map(|agent| {
                let mut selections = agent
                    .subagents
                    .iter()
                    .filter(|record| record.enabled)
                    .map(|record| (record.id.clone(), record.model_id.clone()))
                    .collect::<BTreeMap<_, _>>();
                if let Some(model) = agent.model_slots.get(CODEX_DEFAULT_SUBAGENT_SLOT_MODEL) {
                    selections.insert(CODEX_DEFAULT_SUBAGENT_SLOT_MODEL.into(), model.clone());
                }
                selections
            })
            .unwrap_or_default(),
        client_configurations,
        worker_mode: agent.worker_mode,
        native_subagent_enabled: agent.native_subagent_enabled,
        subagents: agent
            .subagents
            .iter()
            .map(|subagent| PublicSubAgent {
                id: subagent.id.clone(),
                name: subagent.name.clone(),
                model_id: subagent.model_id.clone(),
                capabilities: subagent.capabilities.clone(),
                enabled: subagent.enabled,
            })
            .collect(),
    })
}

#[tauri::command]
pub fn load_state(service: State<'_, ControlPlaneService>) -> Result<ControlPlaneState, String> {
    service.state()
}

#[tauri::command]
pub fn save_provider(
    service: State<'_, ControlPlaneService>,
    input: ProviderInput,
) -> Result<ControlPlaneState, String> {
    service.save_provider(input)
}

#[tauri::command]
pub fn delete_provider(
    service: State<'_, ControlPlaneService>,
    id: String,
) -> Result<ControlPlaneState, String> {
    service.delete_provider(&id)
}

#[tauri::command]
pub fn update_provider(
    service: State<'_, ControlPlaneService>,
    input: ProviderInput,
) -> Result<ControlPlaneState, String> {
    service.update_provider(input)
}

#[tauri::command]
pub async fn discover_provider_models(
    service: State<'_, ControlPlaneService>,
    provider_id: String,
) -> Result<Vec<DiscoveredModel>, String> {
    service.discover_provider_models(&provider_id).await
}

#[tauri::command]
pub fn import_provider_models(
    service: State<'_, ControlPlaneService>,
    provider_id: String,
    models: Vec<DiscoveredModel>,
) -> Result<ControlPlaneState, String> {
    service.import_provider_models(&provider_id, models)
}

#[tauri::command]
pub fn save_model(
    service: State<'_, ControlPlaneService>,
    input: ModelInput,
) -> Result<ControlPlaneState, String> {
    service.save_model(input)
}

#[tauri::command]
pub fn update_model(
    service: State<'_, ControlPlaneService>,
    input: ModelInput,
) -> Result<ControlPlaneState, String> {
    service.update_model(input)
}

#[tauri::command]
pub fn delete_model(
    service: State<'_, ControlPlaneService>,
    id: String,
) -> Result<ControlPlaneState, String> {
    service.delete_model(&id)
}

#[tauri::command]
pub fn set_main_model(
    service: State<'_, ControlPlaneService>,
    id: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_main_model(id)
}

#[tauri::command]
pub fn set_model_slot(
    service: State<'_, ControlPlaneService>,
    slot: String,
    id: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_model_slot(slot, id)
}

#[tauri::command]
pub fn set_pi_main_model(
    service: State<'_, ControlPlaneService>,
    id: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_pi_main_model(id)
}

#[tauri::command]
pub fn set_pi_model_enabled(
    service: State<'_, ControlPlaneService>,
    id: String,
    enabled: bool,
) -> Result<ControlPlaneState, String> {
    service.set_pi_model_enabled(id, enabled)
}

#[tauri::command]
pub fn set_codex_main_model(
    service: State<'_, ControlPlaneService>,
    id: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_codex_main_model(id)
}

#[tauri::command]
pub fn set_codex_native_main_model(
    service: State<'_, ControlPlaneService>,
    model: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_codex_native_main_model(model)
}

#[tauri::command]
pub fn set_codex_default_subagent_model(
    service: State<'_, ControlPlaneService>,
    id: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_codex_default_subagent_model(id)
}

#[tauri::command]
pub fn set_codex_native_default_subagent_model(
    service: State<'_, ControlPlaneService>,
    model: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_codex_native_default_subagent_model(model)
}

#[tauri::command]
pub fn set_codex_custom_agent_model(
    service: State<'_, ControlPlaneService>,
    name: String,
    id: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_codex_custom_agent_model(name, id)
}

#[tauri::command]
pub fn set_codex_native_custom_agent_model(
    service: State<'_, ControlPlaneService>,
    name: String,
    model: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_codex_native_custom_agent_model(name, model)
}

#[tauri::command]
pub fn set_client_main_model(
    service: State<'_, ControlPlaneService>,
    client_id: String,
    id: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_client_main_model(client_id, id)
}

#[tauri::command]
pub fn set_client_model_enabled(
    service: State<'_, ControlPlaneService>,
    client_id: String,
    id: String,
    enabled: bool,
) -> Result<ControlPlaneState, String> {
    service.set_client_model_enabled(client_id, id, enabled)
}

#[tauri::command]
pub fn set_client_secondary_model(
    service: State<'_, ControlPlaneService>,
    client_id: String,
    id: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_client_secondary_model(client_id, id)
}

#[tauri::command]
pub fn set_claude_desktop_model_slot(
    service: State<'_, ControlPlaneService>,
    slot: String,
    id: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_claude_desktop_model_slot(slot, id)
}

#[tauri::command]
pub fn set_worker(
    service: State<'_, ControlPlaneService>,
    id: String,
    enabled: bool,
) -> Result<ControlPlaneState, String> {
    service.set_worker(id, enabled)
}

#[tauri::command]
pub fn set_worker_mode(
    service: State<'_, ControlPlaneService>,
    enabled: bool,
) -> Result<ControlPlaneState, String> {
    service.set_worker_mode(enabled)
}

#[tauri::command]
pub fn set_native_subagent_enabled(
    service: State<'_, ControlPlaneService>,
    enabled: bool,
) -> Result<ControlPlaneState, String> {
    service.set_native_subagent_enabled(enabled)
}

#[tauri::command]
pub fn save_subagent(
    service: State<'_, ControlPlaneService>,
    input: SubAgentInput,
) -> Result<ControlPlaneState, String> {
    service.save_subagent(input)
}

#[tauri::command]
pub fn update_subagent(
    service: State<'_, ControlPlaneService>,
    input: SubAgentInput,
) -> Result<ControlPlaneState, String> {
    service.update_subagent(input)
}

#[tauri::command]
pub fn delete_subagent(
    service: State<'_, ControlPlaneService>,
    id: String,
) -> Result<ControlPlaneState, String> {
    service.delete_subagent(&id)
}

#[tauri::command]
pub async fn test_model_connection(
    service: State<'_, ControlPlaneService>,
    gateway: State<'_, GatewayStatus>,
    id: String,
) -> Result<ConnectionResult, String> {
    let _route = gateway.allow_connection_test(&id)?;
    service.test_model_connection(&gateway.base_url, &id).await
}

#[tauri::command]
pub async fn query_provider_usage(
    service: State<'_, ControlPlaneService>,
    id: String,
) -> Result<UsageSnapshot, String> {
    let (preset, credentials) = service.provider_usage_query(&id)?;
    crate::usage_query::query_usage(preset, &credentials)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod usage_query_tests {
    use super::*;

    fn provider(id: &str, endpoint: &str) -> ProviderInput {
        ProviderInput {
            id: id.into(),
            name: id.into(),
            protocol: Protocol::OpenAiResponses,
            endpoint: endpoint.into(),
            endpoint_mode: EndpointMode::BaseUrl,
            api_key_placement: ApiKeyPlacement::Bearer,
            api_key: Some("test-secret".into()),
            enabled: true,
            models_url: None,
        }
    }

    #[test]
    fn provider_usage_is_selected_only_by_a_vetted_official_host() {
        let directory = tempfile::tempdir().expect("temporary configuration");
        let service = ControlPlaneService::new(directory.path());
        service
            .save_provider(provider("deepseek", "https://api.deepseek.com/v1"))
            .expect("DeepSeek provider");
        let (preset, credentials) = service
            .provider_usage_query("deepseek")
            .expect("DeepSeek balance query");
        assert_eq!(preset, UsageQueryPreset::DeepSeekBalance);
        assert_eq!(
            format!("{credentials:?}"),
            "UsageQueryCredentials([REDACTED])"
        );

        service
            .save_provider(provider("custom", "https://example.com/v1"))
            .expect("custom provider");
        assert!(
            service
                .provider_usage_query("custom")
                .unwrap_err()
                .contains("暂无可用")
        );
    }
}
