use crate::adapters::claude_code::{MODEL_SLOT_IDS, is_claude_native_model};
use crate::adapters::codex::{
    CodexConfiguredModel, CodexModelSelection, CodexProviderRequest, CodexRequest,
};
use crate::configuration::{
    AgentRecord, CodexAgentModelRecord, ConfigurationDocuments, ConfigurationFiles,
    ExtensionSubAgentRecord, MainRecord, ModelRecord, ProviderProtocolEndpoint, ProviderRecord,
};
use crate::core::model::{NativeProtocol, ProtocolCapability};
use crate::core::provider::{ApiKeyPlacement, EndpointMode, Protocol};
use crate::gateway::GatewayStatus;
use crate::model_discovery;
use crate::usage_query::{UsageQueryCredentials, UsageQueryPreset, UsageSnapshot};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;
use tauri::State;

const CLAUDE_CODE_AGENT: &str = "claude_code";
const CLAUDE_CODE_MAIN_NATIVE_SLOT: &str = "main";
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
const HERMES_AGENT: &str = "hermes";
const KIMI_CODE_AGENT: &str = "kimi_code";
const GENERIC_CLIENTS: &[&str] = &[
    GEMINI_AGENT,
    GROK_BUILD_AGENT,
    OPENCODE_AGENT,
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
    pub claude_native_model_slots: BTreeMap<String, String>,
    pub claude_desktop_model_slots: BTreeMap<String, String>,
    pub pi_enabled: bool,
    pub pi_main_model_id: Option<String>,
    pub pi_enabled_model_ids: Vec<String>,
    pub codex_main_model_id: Option<String>,
    pub codex_native_model_slots: BTreeMap<String, String>,
    pub codex_agent_model_ids: BTreeMap<String, String>,
    pub client_configurations: BTreeMap<String, PublicClientConfiguration>,
    pub extension_subagents: Vec<PublicExtensionSubAgent>,
    pub client_extension_subagent_ids: BTreeMap<String, Vec<String>>,
    pub mcp_mounted_client_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicClientConfiguration {
    pub main_model_id: Option<String>,
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
    pub protocol_endpoints: Vec<PublicProviderProtocolEndpoint>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicProviderProtocolEndpoint {
    pub protocol: NativeProtocol,
    pub endpoint: String,
    pub endpoint_mode: EndpointMode,
    pub api_key_placement: ApiKeyPlacement,
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
    pub native_protocols: Vec<NativeProtocol>,
    pub unsupported_native_protocols: Vec<NativeProtocol>,
    pub route_alias: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicExtensionSubAgent {
    pub id: String,
    pub name: String,
    pub source_client_id: String,
    pub source_agent_id: String,
    pub model_id: Option<String>,
    pub capabilities: Vec<String>,
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
pub struct ModelWithNativeProtocolsInput {
    #[serde(flatten)]
    pub model: ModelInput,
    pub native_protocols: Vec<NativeProtocol>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionSubAgentInput {
    #[serde(default, deserialize_with = "deserialize_optional_id")]
    pub id: String,
    pub name: String,
    pub source_client_id: String,
    pub source_agent_id: String,
    pub model_id: Option<String>,
    pub capabilities: Vec<String>,
}

fn deserialize_optional_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
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
            || !agent.model_pool.is_empty()
            || !agent.codex_agent_models.is_empty())
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
            protocol_endpoints: Vec::new(),
        };
        match existing {
            Some(index) => documents.config.providers[index] = record,
            None => documents.config.providers.push(record),
        }
        self.save_and_return(documents)
    }

    pub async fn save_provider_with_model_check(
        &self,
        input: ProviderInput,
    ) -> Result<ControlPlaneState, String> {
        let before = self.documents()?;
        if before
            .config
            .providers
            .iter()
            .any(|provider| provider.id == input.id)
        {
            return Err(format!("duplicate provider id: {}", input.id));
        }
        let no_auth = input.api_key_placement == ApiKeyPlacement::None;
        let api_key = match (input.api_key, no_auth) {
            (_, true) => String::new(),
            (Some(value), false) if !value.is_empty() => value,
            _ => return Err("provider API key must not be empty".to_string()),
        };
        let mut provider = ProviderRecord {
            id: input.id,
            name: input.name,
            enabled: input.enabled,
            protocol: input.protocol,
            endpoint: input.endpoint,
            endpoint_mode: input.endpoint_mode,
            api_key_placement: input.api_key_placement,
            api_key,
            models_url: input.models_url,
            protocol_endpoints: Vec::new(),
        };
        let discovered = model_discovery::discover(&provider).await?;
        if discovered.is_empty() {
            return Err("provider model discovery returned no models".to_string());
        }
        let probes = model_discovery::probe_protocols(&provider, &discovered).await?;
        if probes.provider_endpoints.is_empty() {
            return Err("provider model checks found no supported API protocol".to_string());
        }
        provider.protocol_endpoints = probes.provider_endpoints.clone();

        let mut documents = self.documents()?;
        if documents
            .config
            .providers
            .iter()
            .any(|current| current.id == provider.id)
        {
            return Err(format!("duplicate provider id: {}", provider.id));
        }
        let provider_id = provider.id.clone();
        documents.config.providers.push(provider);
        apply_discovered_models(&mut documents, &provider_id, discovered, &probes)?;
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
        let provider_id = input.id.clone();
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
            protocol_endpoints: Vec::new(),
        };
        for model in documents
            .models
            .models
            .iter_mut()
            .filter(|model| model.provider_id == provider_id)
        {
            model.native_protocols = None;
            model.unsupported_native_protocols.clear();
        }
        self.save_and_return(documents)
    }

    pub async fn sync_provider_models(
        &self,
        provider_id: &str,
    ) -> Result<ControlPlaneState, String> {
        let before = self.documents()?;
        let provider = before
            .config
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .cloned()
            .ok_or_else(|| format!("unknown provider: {provider_id}"))?;
        let discovered = model_discovery::discover(&provider).await?;
        if discovered.is_empty() {
            return Err("provider model discovery returned no models".to_string());
        }
        let probes = model_discovery::probe_protocols(&provider, &discovered).await?;

        // Network work happens outside the atomic file write. Refuse to apply
        // stale probe facts if the Provider changed while synchronization ran.
        let mut documents = self.documents()?;
        let current_provider = documents
            .config
            .providers
            .iter_mut()
            .find(|item| item.id == provider_id)
            .ok_or_else(|| format!("provider {provider_id} was deleted during model sync"))?;
        if *current_provider != provider {
            return Err(format!(
                "provider {provider_id} changed during model sync; run synchronization again"
            ));
        }
        current_provider.protocol_endpoints = probes.provider_endpoints.clone();
        apply_discovered_models(&mut documents, provider_id, discovered, &probes)?;
        self.save_and_return(documents)
    }

    pub fn save_model(&self, input: ModelInput) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let default_protocol = documents
            .config
            .providers
            .iter()
            .find(|provider| provider.id == input.provider_id)
            .map(|provider| native_protocol(provider.protocol))
            .ok_or_else(|| format!("unknown provider: {}", input.provider_id))?;
        ensure_provider_protocol_endpoints(
            &mut documents,
            &input.provider_id,
            &[default_protocol],
        )?;
        let record = ModelRecord {
            id: input.id,
            provider_id: input.provider_id,
            upstream_id: input.upstream_id,
            display_name: input.name,
            capabilities: input.capabilities,
            protocol_capabilities: input.protocol_capabilities,
            native_protocols: Some(vec![default_protocol]),
            unsupported_native_protocols: Vec::new(),
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

    pub fn save_model_with_native_protocols(
        &self,
        input: ModelWithNativeProtocolsInput,
    ) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        validate_native_protocols(&input.model.id, &input.native_protocols)?;
        ensure_provider_protocol_endpoints(
            &mut documents,
            &input.model.provider_id,
            &input.native_protocols,
        )?;
        let record = ModelRecord {
            id: input.model.id,
            provider_id: input.model.provider_id,
            upstream_id: input.model.upstream_id,
            display_name: input.model.name,
            capabilities: input.model.capabilities,
            protocol_capabilities: input.model.protocol_capabilities,
            native_protocols: Some(input.native_protocols),
            unsupported_native_protocols: Vec::new(),
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
        let native_protocols = documents.models.models[index].native_protocols.clone();
        let unsupported_native_protocols = documents.models.models[index]
            .unsupported_native_protocols
            .clone();
        documents.models.models[index] = ModelRecord {
            id: input.id,
            provider_id: input.provider_id,
            upstream_id: input.upstream_id,
            display_name: input.name,
            capabilities: input.capabilities,
            protocol_capabilities: input.protocol_capabilities,
            native_protocols,
            unsupported_native_protocols,
        };
        self.save_and_return(documents)
    }

    pub fn update_model_with_native_protocols(
        &self,
        input: ModelWithNativeProtocolsInput,
    ) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        validate_native_protocols(&input.model.id, &input.native_protocols)?;
        ensure_provider_protocol_endpoints(
            &mut documents,
            &input.model.provider_id,
            &input.native_protocols,
        )?;
        let index = documents
            .models
            .models
            .iter()
            .position(|model| model.id == input.model.id)
            .ok_or_else(|| format!("unknown model: {}", input.model.id))?;
        documents.models.models[index] = ModelRecord {
            id: input.model.id,
            provider_id: input.model.provider_id,
            upstream_id: input.model.upstream_id,
            display_name: input.model.name,
            capabilities: input.model.capabilities,
            protocol_capabilities: input.model.protocol_capabilities,
            native_protocols: Some(input.native_protocols),
            unsupported_native_protocols: Vec::new(),
        };
        self.save_and_return(documents)
    }

    pub fn set_model_native_protocols(
        &self,
        id: &str,
        mut protocols: Vec<NativeProtocol>,
    ) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        if !documents.models.models.iter().any(|model| model.id == id) {
            return Err(format!("unknown model: {id}"));
        }
        protocols.sort();
        validate_native_protocols(id, &protocols)?;
        let provider_id = documents
            .models
            .models
            .iter()
            .find(|model| model.id == id)
            .expect("validated model")
            .provider_id
            .clone();
        ensure_provider_protocol_endpoints(&mut documents, &provider_id, &protocols)?;
        documents
            .models
            .models
            .iter_mut()
            .find(|model| model.id == id)
            .expect("validated model")
            .native_protocols = Some(protocols);
        self.save_and_return(documents)
    }

    pub fn delete_model(&self, id: &str) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        let selected_by = documents.agents.agents.iter().find(|agent| {
            matches!(&agent.main, MainRecord::Managed(model) if model == id)
                || agent.model_pool.iter().any(|model| model == id)
                || agent.model_slots.values().any(|model| model == id)
                || agent
                    .codex_agent_models
                    .iter()
                    .any(|agent_model| agent_model.model_id == id)
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
        let agent = claude_agent_mut(&mut documents)?;
        agent.main = match id {
            Some(id) => MainRecord::Managed(id),
            None => MainRecord::Native,
        };
        if matches!(agent.main, MainRecord::Managed(_)) {
            agent
                .native_model_slots
                .remove(CLAUDE_CODE_MAIN_NATIVE_SLOT);
        }
        self.save_and_return(documents)
    }

    pub fn set_claude_native_model(
        &self,
        slot: String,
        model: Option<String>,
    ) -> Result<ControlPlaneState, String> {
        if slot != CLAUDE_CODE_MAIN_NATIVE_SLOT && !MODEL_SLOT_IDS.contains(&slot.as_str()) {
            return Err(format!("unsupported Claude Code native model slot: {slot}"));
        }
        if let Some(model) = model.as_deref() {
            if !is_claude_native_model(model) {
                return Err(format!("unsupported Claude Code native model: {model}"));
            }
        }
        let mut documents = self.documents()?;
        let agent = claude_agent_mut(&mut documents)?;
        if slot == CLAUDE_CODE_MAIN_NATIVE_SLOT {
            agent.main = MainRecord::Native;
        } else if model.is_some() {
            agent.model_slots.remove(&slot);
        }
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
                agent.model_slots.insert(slot.clone(), id);
                agent.native_model_slots.remove(&slot);
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
                    model_pool: Vec::new(),
                    codex_agent_models: Vec::new(),
                    extension_subagent_ids: Vec::new(),
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
                if !agent.model_pool.contains(&id) {
                    agent.model_pool.push(id.clone());
                    agent.model_pool.sort();
                }
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
        let exists = agent.model_pool.contains(&id);
        match (enabled, exists) {
            (true, false) => {
                agent.model_pool.push(id);
                agent.model_pool.sort();
            }
            (false, true) => agent.model_pool.retain(|model| model != &id),
            _ => {}
        }
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
        agent.codex_agent_models.retain(|record| record.id != name);
        if let Some(id) = id {
            agent.codex_agent_models.push(CodexAgentModelRecord {
                id: name.clone(),
                name: name.clone(),
                model_id: id,
                capabilities: Vec::new(),
                enabled: true,
            });
            agent
                .codex_agent_models
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
        agent.codex_agent_models.retain(|record| record.id != name);
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
        current_config: Option<&CodexConfiguredModel>,
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
                .map(CodexModelSelection::native)
                .or_else(|| {
                    current_config.map(|configured| {
                        CodexModelSelection::existing(
                            &configured.model,
                            configured.provider.as_deref(),
                        )
                    })
                })
                .ok_or_else(|| "Codex 当前配置没有可用的主模型；请先选择一个模型".to_string())?
                .map_err(|error| error.to_string())?,
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
        for record in &agent.codex_agent_models {
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
        for record in &agent.codex_agent_models {
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
                OPENCODE_AGENT | HERMES_AGENT | KIMI_CODE_AGENT
            ) && !agent.model_pool.contains(model_id)
            {
                agent.model_pool.push(model_id.clone());
                agent.model_pool.sort();
            }
        }
        agent.main = id.map_or(MainRecord::Native, MainRecord::Managed);
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
            OPENCODE_AGENT | HERMES_AGENT | KIMI_CODE_AGENT
        ) {
            return Err(format!("{client_id} does not expose a managed model pool"));
        }
        let mut documents = self.documents()?;
        validate_client_model(&documents, &client_id, &id)?;
        let agent = client_agent_mut(&mut documents, &client_id);
        if !enabled && matches!(&agent.main, MainRecord::Managed(main) if main == &id) {
            return Err(format!("{client_id} main model cannot be disabled: {id}"));
        }
        let exists = agent.model_pool.contains(&id);
        match (enabled, exists) {
            (true, false) => {
                agent.model_pool.push(id);
                agent.model_pool.sort();
            }
            (false, true) => agent.model_pool.retain(|model| model != &id),
            _ => {}
        }
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
        let enabled_ids = if matches!(client_id, OPENCODE_AGENT | HERMES_AGENT | KIMI_CODE_AGENT) {
            &agent.model_pool
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

    pub fn save_extension_subagent(
        &self,
        mut input: ExtensionSubAgentInput,
    ) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        if input.id.is_empty() {
            input.id = generated_extension_subagent_id(&input);
        }
        validate_extension_subagent_input(&documents, &input)?;
        if documents
            .agents
            .extension_subagents
            .iter()
            .any(|extension| extension.id == input.id)
        {
            return Err(format!("duplicate extension SubAgent id: {}", input.id));
        }
        documents
            .agents
            .extension_subagents
            .push(extension_subagent_record(input));
        documents
            .agents
            .extension_subagents
            .sort_by(|left, right| left.id.cmp(&right.id));
        self.save_and_return(documents)
    }

    pub fn update_extension_subagent(
        &self,
        input: ExtensionSubAgentInput,
    ) -> Result<ControlPlaneState, String> {
        if input.id.is_empty() {
            return Err("extension SubAgent update requires an id".into());
        }
        let mut documents = self.documents()?;
        validate_extension_subagent_input(&documents, &input)?;
        let record = documents
            .agents
            .extension_subagents
            .iter_mut()
            .find(|extension| extension.id == input.id)
            .ok_or_else(|| format!("unknown extension SubAgent: {}", input.id))?;
        *record = extension_subagent_record(input);
        self.save_and_return(documents)
    }

    pub fn delete_extension_subagent(&self, id: &str) -> Result<ControlPlaneState, String> {
        let mut documents = self.documents()?;
        if !documents
            .agents
            .extension_subagents
            .iter()
            .any(|extension| extension.id == id)
        {
            return Err(format!("unknown extension SubAgent: {id}"));
        }
        let mut bound_clients = documents
            .agents
            .agents
            .iter()
            .filter(|agent| {
                agent
                    .extension_subagent_ids
                    .iter()
                    .any(|binding| binding == id)
            })
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();
        if !bound_clients.is_empty() {
            bound_clients.sort();
            return Err(format!(
                "extension SubAgent {id} is still bound to clients: {}",
                bound_clients.join(", ")
            ));
        }
        documents
            .agents
            .extension_subagents
            .retain(|extension| extension.id != id);
        self.save_and_return(documents)
    }

    pub fn set_client_extension_subagent_enabled(
        &self,
        client_id: &str,
        extension_subagent_id: &str,
        enabled: bool,
    ) -> Result<ControlPlaneState, String> {
        validate_known_client(client_id)?;
        let mut documents = self.documents()?;
        if !documents
            .agents
            .extension_subagents
            .iter()
            .any(|extension| extension.id == extension_subagent_id)
        {
            return Err(format!(
                "unknown extension SubAgent: {extension_subagent_id}"
            ));
        }
        let bindings = &mut client_agent_mut(&mut documents, client_id).extension_subagent_ids;
        let exists = bindings.iter().any(|id| id == extension_subagent_id);
        match (enabled, exists) {
            (true, false) => {
                bindings.push(extension_subagent_id.into());
                bindings.sort();
            }
            (false, true) => bindings.retain(|id| id != extension_subagent_id),
            _ => {}
        }
        self.save_and_return(documents)
    }

    pub fn set_client_mcp_mounted(
        &self,
        client_id: &str,
        mounted: bool,
    ) -> Result<ControlPlaneState, String> {
        validate_known_client(client_id)?;
        let mut documents = self.documents()?;
        client_agent_mut(&mut documents, client_id);
        let ids = &mut documents.agents.mcp_mounted_client_ids;
        let exists = ids.iter().any(|id| id == client_id);
        match (mounted, exists) {
            (true, false) => {
                ids.push(client_id.into());
                ids.sort();
            }
            (false, true) => ids.retain(|id| id != client_id),
            _ => {}
        }
        self.save_and_return(documents)
    }

    pub async fn test_model_connection(
        &self,
        gateway_base_url: &str,
        id: &str,
    ) -> Result<ConnectionResult, String> {
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

fn apply_discovered_models(
    documents: &mut ConfigurationDocuments,
    provider_id: &str,
    discovered: Vec<model_discovery::DiscoveredModel>,
    probes: &model_discovery::ProtocolProbeSummary,
) -> Result<(), String> {
    let provider = documents
        .config
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| format!("unknown provider: {provider_id}"))?;
    let preset = model_discovery::matching_preset(provider)?;
    for model in discovered {
        let supported = probes
            .models
            .get(&model.id)
            .map(|probe| probe.supported.clone())
            .unwrap_or_default();
        let unsupported = [
            NativeProtocol::AnthropicMessages,
            NativeProtocol::OpenAiResponses,
            NativeProtocol::OpenAiChat,
            NativeProtocol::GeminiNative,
        ]
        .into_iter()
        .filter(|protocol| !supported.contains(protocol))
        .collect::<Vec<_>>();
        if let Some(existing) = documents.models.models.iter_mut().find(|existing| {
            existing.provider_id == provider_id && existing.upstream_id == model.id
        }) {
            existing.native_protocols = Some(supported);
            existing.unsupported_native_protocols = unsupported;
            continue;
        }
        let upstream_slug = model_slug(&model.id);
        if upstream_slug.is_empty() {
            return Err(format!(
                "model ID cannot produce a stable slug: {}",
                model.id
            ));
        }
        let id = if documents
            .models
            .models
            .iter()
            .any(|item| item.id == upstream_slug)
        {
            let provider_slug = model_slug(provider_id);
            if provider_slug.is_empty() {
                return Err(format!(
                    "provider ID cannot produce a stable model namespace: {provider_id}"
                ));
            }
            format!("{provider_slug}-{upstream_slug}")
        } else {
            upstream_slug
        };
        if documents.models.models.iter().any(|item| item.id == id) {
            return Err(format!("model route collision: {id}"));
        }
        let protocol_capabilities = preset
            .as_ref()
            .and_then(|preset| preset.model_protocol_capabilities.get(&model.id).cloned())
            .unwrap_or_default();
        documents.models.models.push(ModelRecord {
            id,
            provider_id: provider_id.to_string(),
            upstream_id: model.id.clone(),
            display_name: model.id,
            capabilities: Vec::new(),
            protocol_capabilities,
            native_protocols: Some(supported),
            unsupported_native_protocols: unsupported,
        });
    }
    documents
        .models
        .models
        .sort_by(|left, right| left.id.cmp(&right.id));
    Ok(())
}

fn native_protocol(protocol: Protocol) -> NativeProtocol {
    match protocol {
        Protocol::AnthropicMessages => NativeProtocol::AnthropicMessages,
        Protocol::OpenAiResponses => NativeProtocol::OpenAiResponses,
        Protocol::OpenAiChatCompletions => NativeProtocol::OpenAiChat,
        Protocol::GeminiNative => NativeProtocol::GeminiNative,
    }
}

fn ensure_provider_protocol_endpoints(
    documents: &mut ConfigurationDocuments,
    provider_id: &str,
    protocols: &[NativeProtocol],
) -> Result<(), String> {
    let provider = documents
        .config
        .providers
        .iter_mut()
        .find(|provider| provider.id == provider_id)
        .ok_or_else(|| format!("unknown provider: {provider_id}"))?;
    for protocol in protocols {
        if provider
            .protocol_endpoints
            .iter()
            .any(|entry| entry.protocol == *protocol)
        {
            continue;
        }
        let mut endpoint = provider.endpoint.clone();
        if provider_id == "deepseek" && *protocol == NativeProtocol::AnthropicMessages {
            let mut url = url::Url::parse(&endpoint)
                .map_err(|_| format!("provider {provider_id} has an invalid endpoint"))?;
            if url.host_str() == Some("api.deepseek.com") {
                url.set_path("/anthropic");
                endpoint = url.to_string().trim_end_matches('/').to_string();
            }
        }
        provider.protocol_endpoints.push(ProviderProtocolEndpoint {
            protocol: *protocol,
            endpoint,
            endpoint_mode: provider.endpoint_mode,
            api_key_placement: provider.api_key_placement,
        });
    }
    provider
        .protocol_endpoints
        .sort_by_key(|entry| entry.protocol);
    Ok(())
}

fn validate_native_protocols(id: &str, protocols: &[NativeProtocol]) -> Result<(), String> {
    if protocols.is_empty() {
        return Err(format!(
            "model {id} requires at least one verified native protocol"
        ));
    }
    let mut sorted = protocols.to_vec();
    sorted.sort();
    if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(format!("duplicate native protocol for model: {id}"));
    }
    Ok(())
}

fn claude_agent(documents: &ConfigurationDocuments) -> Result<&AgentRecord, String> {
    documents
        .agents
        .agents
        .iter()
        .find(|agent| agent.id == CLAUDE_CODE_AGENT)
        .ok_or_else(|| "agents.yaml is missing claude_code".to_string())
}

fn extension_subagent_record(input: ExtensionSubAgentInput) -> ExtensionSubAgentRecord {
    ExtensionSubAgentRecord {
        id: input.id,
        name: input.name,
        source_client_id: input.source_client_id,
        source_agent_id: input.source_agent_id,
        model_id: input.model_id,
        capabilities: input.capabilities,
    }
}

fn generated_extension_subagent_id(input: &ExtensionSubAgentInput) -> String {
    let source_client = extension_id_segment(&input.source_client_id);
    let source_agent = extension_id_segment(&input.source_agent_id);
    let name = extension_id_segment(&input.name);
    let mut prefix = [source_client, source_agent, name]
        .into_iter()
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    prefix.truncate(prefix.len().min(48));
    while prefix.ends_with('-') {
        prefix.pop();
    }
    if prefix.is_empty() {
        prefix.push_str("extension-subagent");
    }
    let identity = format!(
        "{}\0{}\0{}",
        input.name, input.source_client_id, input.source_agent_id
    );
    format!("{prefix}-{:016x}", stable_hash(identity.as_bytes()))
}

fn extension_id_segment(value: &str) -> String {
    let mut segment = String::new();
    let mut separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !segment.is_empty() {
                segment.push('-');
            }
            segment.push(character.to_ascii_lowercase());
            separator = false;
        } else if !segment.is_empty() {
            separator = true;
        }
    }
    segment
}

fn stable_hash(value: &[u8]) -> u64 {
    value.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn validate_extension_subagent_input(
    documents: &ConfigurationDocuments,
    input: &ExtensionSubAgentInput,
) -> Result<(), String> {
    if !is_lowercase_slug(&input.id) {
        return Err(format!(
            "extension SubAgent id must be a lowercase slug: {}",
            input.id
        ));
    }
    if input.name.trim().is_empty()
        || input.name.trim() != input.name
        || input.name.chars().any(char::is_control)
    {
        return Err(format!("extension SubAgent name is invalid: {}", input.id));
    }
    validate_known_client(&input.source_client_id)?;
    if input.source_agent_id.trim().is_empty()
        || input.source_agent_id.trim() != input.source_agent_id
        || input.source_agent_id.chars().any(char::is_control)
    {
        return Err(format!(
            "extension SubAgent source Agent id is invalid: {}",
            input.source_agent_id
        ));
    }
    if let Some(model_id) = &input.model_id {
        let model = documents
            .models
            .models
            .iter()
            .find(|model| model.id == *model_id)
            .ok_or_else(|| {
                format!(
                    "extension SubAgent {} references unknown model: {model_id}",
                    input.id
                )
            })?;
        let provider = documents
            .config
            .providers
            .iter()
            .find(|provider| provider.id == model.provider_id)
            .ok_or_else(|| format!("model {model_id} references unknown provider"))?;
        if !provider.enabled {
            return Err(format!(
                "extension SubAgent {} model uses disabled provider: {}",
                input.id, provider.id
            ));
        }
    }
    let mut capabilities = HashSet::new();
    for capability in &input.capabilities {
        let normalized = capability.to_ascii_lowercase();
        if !is_capability_label(capability) || !capabilities.insert(normalized) {
            return Err(format!(
                "invalid or duplicate extension SubAgent capability: {capability}"
            ));
        }
    }
    Ok(())
}

fn is_capability_label(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with(['-', '_'])
        && !value.ends_with(['-', '_'])
        && !value.contains("--")
        && !value.contains("__")
        && value
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, b'-' | b'_'))
}

fn is_lowercase_slug(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with(['-', '_'])
        && !value.ends_with(['-', '_'])
        && !value.contains("--")
        && !value.contains("__")
        && value.bytes().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, b'-' | b'_')
        })
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
        model_pool: Vec::new(),
        codex_agent_models: Vec::new(),
        extension_subagent_ids: Vec::new(),
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
    // Every managed client is pointed at GrillForge's protocol-specific local
    // ingress. Provider protocol compatibility is therefore resolved per
    // model by the gateway, not by rejecting the selection here.
    let compatible = matches!(
        client_id,
        GEMINI_AGENT | GROK_BUILD_AGENT | OPENCODE_AGENT | HERMES_AGENT | KIMI_CODE_AGENT
    );
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
                    enabled_model_ids: agent
                        .map(|agent| agent.model_pool.clone())
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
                protocol_endpoints: provider
                    .protocol_endpoints
                    .iter()
                    .map(|entry| PublicProviderProtocolEndpoint {
                        protocol: entry.protocol,
                        endpoint: entry.endpoint.clone(),
                        endpoint_mode: entry.endpoint_mode,
                        api_key_placement: entry.api_key_placement,
                    })
                    .collect(),
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
                native_protocols: model.native_protocols.clone().unwrap_or_default(),
                unsupported_native_protocols: model.unsupported_native_protocols.clone(),
                route_alias: format!("grillforge/{}", model.id),
            })
            .collect(),
        agent_enabled: agent.enabled,
        main_model_id: match &agent.main {
            MainRecord::Native => None,
            MainRecord::Managed(id) => Some(id.clone()),
        },
        model_slots: agent.model_slots.clone(),
        claude_native_model_slots: agent.native_model_slots.clone(),
        claude_desktop_model_slots,
        pi_enabled: pi.is_some_and(|agent| agent.enabled),
        pi_main_model_id: pi.and_then(|agent| match &agent.main {
            MainRecord::Native => None,
            MainRecord::Managed(id) => Some(id.clone()),
        }),
        pi_enabled_model_ids: pi.map(|agent| agent.model_pool.clone()).unwrap_or_default(),
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
                    .codex_agent_models
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
        extension_subagents: documents
            .agents
            .extension_subagents
            .iter()
            .map(|extension| PublicExtensionSubAgent {
                id: extension.id.clone(),
                name: extension.name.clone(),
                source_client_id: extension.source_client_id.clone(),
                source_agent_id: extension.source_agent_id.clone(),
                model_id: extension.model_id.clone(),
                capabilities: extension.capabilities.clone(),
            })
            .collect(),
        client_extension_subagent_ids: documents
            .agents
            .agents
            .iter()
            .map(|agent| (agent.id.clone(), agent.extension_subagent_ids.clone()))
            .collect(),
        mcp_mounted_client_ids: documents.agents.mcp_mounted_client_ids.clone(),
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
pub async fn sync_provider_models(
    service: State<'_, ControlPlaneService>,
    provider_id: String,
) -> Result<ControlPlaneState, String> {
    service.sync_provider_models(&provider_id).await
}

#[tauri::command]
pub async fn save_provider_with_model_check(
    service: State<'_, ControlPlaneService>,
    input: ProviderInput,
) -> Result<ControlPlaneState, String> {
    service.save_provider_with_model_check(input).await
}

#[tauri::command]
pub fn save_model_with_native_protocols(
    service: State<'_, ControlPlaneService>,
    input: ModelWithNativeProtocolsInput,
) -> Result<ControlPlaneState, String> {
    service.save_model_with_native_protocols(input)
}

#[tauri::command]
pub fn update_model_with_native_protocols(
    service: State<'_, ControlPlaneService>,
    input: ModelWithNativeProtocolsInput,
) -> Result<ControlPlaneState, String> {
    service.update_model_with_native_protocols(input)
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
pub fn set_claude_native_model(
    service: State<'_, ControlPlaneService>,
    slot: String,
    model: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_claude_native_model(slot, model)
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
pub fn set_claude_desktop_model_slot(
    service: State<'_, ControlPlaneService>,
    slot: String,
    id: Option<String>,
) -> Result<ControlPlaneState, String> {
    service.set_claude_desktop_model_slot(slot, id)
}

#[tauri::command]
pub fn save_extension_subagent(
    service: State<'_, ControlPlaneService>,
    input: ExtensionSubAgentInput,
) -> Result<ControlPlaneState, String> {
    service.save_extension_subagent(input)
}

#[tauri::command]
pub fn delete_extension_subagent(
    service: State<'_, ControlPlaneService>,
    id: String,
) -> Result<ControlPlaneState, String> {
    service.delete_extension_subagent(&id)
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
