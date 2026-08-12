use crate::core::agent::{AgentConfiguration, MainSelection};
use crate::core::model::{Model, ModelDraft, ModelRegistry, ProtocolCapability};
use crate::core::provider::{
    ApiKeyPlacement, Auth, EndpointMode, Protocol, Provider, ProviderDraft, ProviderRegistry,
};
use crate::storage::{StoreError, YamlStore};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::fs;
use std::path::PathBuf;

const FORMAT_VERSION: u8 = 2;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderRecord {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub protocol: Protocol,
    pub endpoint: String,
    pub endpoint_mode: EndpointMode,
    pub api_key_placement: ApiKeyPlacement,
    pub api_key: String,
    pub models_url: Option<String>,
}

impl Debug for ProviderRecord {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderRecord")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("protocol", &self.protocol)
            .field("endpoint", &self.endpoint)
            .field("endpoint_mode", &self.endpoint_mode)
            .field("api_key_placement", &self.api_key_placement)
            .field("api_key", &"[REDACTED]")
            .field("models_url", &self.models_url)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigDocument {
    pub version: u8,
    pub providers: Vec<ProviderRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRecord {
    pub id: String,
    pub provider_id: String,
    pub upstream_id: String,
    pub display_name: String,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocol_capabilities: Vec<ProtocolCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelsDocument {
    pub version: u8,
    pub models: Vec<ModelRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", content = "model_id", rename_all = "snake_case")]
pub enum MainRecord {
    Native,
    Managed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexAgentModelRecord {
    pub id: String,
    pub name: String,
    pub model_id: String,
    pub capabilities: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionSubAgentRecord {
    pub id: String,
    pub name: String,
    pub source_client_id: String,
    pub source_agent_id: String,
    pub model_id: Option<String>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRecord {
    pub id: String,
    pub adapter: String,
    pub enabled: bool,
    pub main: MainRecord,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_slots: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub native_model_slots: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_pool: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codex_agent_models: Vec<CodexAgentModelRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extension_subagent_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentsDocument {
    pub version: u8,
    pub agents: Vec<AgentRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extension_subagents: Vec<ExtensionSubAgentRecord>,
}

impl AgentsDocument {
    pub fn new(agents: Vec<AgentRecord>) -> Self {
        Self {
            version: FORMAT_VERSION,
            agents,
            extension_subagents: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigurationDocuments {
    pub config: ConfigDocument,
    pub models: ModelsDocument,
    pub agents: AgentsDocument,
}

#[derive(Clone)]
pub struct ConfigurationFiles {
    root: PathBuf,
}

#[derive(Debug)]
pub enum ConfigurationError {
    Invalid(String),
    Store(StoreError),
}

impl Display for ConfigurationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Store(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for ConfigurationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<StoreError> for ConfigurationError {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

impl ConfigurationFiles {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn read(&self) -> Result<ConfigurationDocuments, ConfigurationError> {
        let documents = ConfigurationDocuments {
            config: self.store("config.yaml").read()?,
            models: self.store("models.yaml").read()?,
            agents: self.store("agents.yaml").read()?,
        };
        validate(&documents.config, &documents.models, &documents.agents)?;
        Ok(documents)
    }

    pub fn open_or_initialize(&self) -> Result<ConfigurationDocuments, ConfigurationError> {
        let expected = ["config.yaml", "models.yaml", "agents.yaml"];
        let existing = expected
            .iter()
            .filter(|file| self.root.join(file).exists())
            .count();

        if existing == 0 {
            let documents = ConfigurationDocuments::default();
            self.save(&documents.config, &documents.models, &documents.agents)?;
            return Ok(documents);
        }

        self.read()
    }

    pub fn save(
        &self,
        config: &ConfigDocument,
        models: &ModelsDocument,
        agents: &AgentsDocument,
    ) -> Result<(), ConfigurationError> {
        validate(config, models, agents)?;
        let entries = [
            ("config.yaml", serialize_yaml("config.yaml", config)?),
            ("models.yaml", serialize_yaml("models.yaml", models)?),
            ("agents.yaml", serialize_yaml("agents.yaml", agents)?),
        ];
        fs::create_dir_all(&self.root).map_err(|source| {
            ConfigurationError::Store(StoreError::Io {
                file: self.root.display().to_string(),
                source,
            })
        })?;
        let mut originals = Vec::with_capacity(entries.len());
        for (file, _) in &entries {
            let path = self.root.join(file);
            let original = match fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(source) => {
                    return Err(ConfigurationError::Store(StoreError::Io {
                        file: (*file).to_string(),
                        source,
                    }));
                }
            };
            originals.push((path, original));
        }
        for ((file, _), (path, original)) in entries.iter().zip(&originals) {
            if let Some(original) = original {
                let backup = path.with_extension("yaml.bak");
                crate::storage::atomic_replace(&backup, original).map_err(|source| {
                    ConfigurationError::Store(StoreError::Io {
                        file: format!("{file}.bak"),
                        source,
                    })
                })?;
            }
        }
        for (index, ((file, bytes), (path, _))) in entries.iter().zip(&originals).enumerate() {
            if let Err(source) = crate::storage::atomic_replace(path, bytes) {
                let original_error = StoreError::Io {
                    file: (*file).to_string(),
                    source,
                };
                restore_configuration(&originals[..index]);
                return Err(ConfigurationError::Store(original_error));
            }
        }
        Ok(())
    }

    fn store(&self, file: &str) -> YamlStore {
        YamlStore::new(self.root.join(file))
    }
}

fn serialize_yaml<T: Serialize>(file: &str, value: &T) -> Result<Vec<u8>, ConfigurationError> {
    serde_yaml::to_string(value)
        .map(String::into_bytes)
        .map_err(|_| {
            ConfigurationError::Store(StoreError::Serialize {
                file: file.to_string(),
            })
        })
}

fn restore_configuration(originals: &[(PathBuf, Option<Vec<u8>>)]) {
    for (path, original) in originals.iter().rev() {
        match original {
            Some(bytes) => {
                let _ = crate::storage::atomic_replace(path, bytes);
            }
            None => {
                let _ = fs::remove_file(path);
            }
        }
    }
}

impl Default for ConfigurationDocuments {
    fn default() -> Self {
        Self {
            config: ConfigDocument {
                version: FORMAT_VERSION,
                providers: Vec::new(),
            },
            models: ModelsDocument {
                version: FORMAT_VERSION,
                models: Vec::new(),
            },
            agents: AgentsDocument {
                version: FORMAT_VERSION,
                extension_subagents: Vec::new(),
                agents: vec![AgentRecord {
                    id: "claude_code".to_string(),
                    adapter: "claude_code".to_string(),
                    enabled: false,
                    main: MainRecord::Native,
                    model_slots: BTreeMap::new(),
                    native_model_slots: BTreeMap::new(),
                    model_pool: Vec::new(),
                    codex_agent_models: Vec::new(),
                    extension_subagent_ids: Vec::new(),
                }],
            },
        }
    }
}

fn validate(
    config: &ConfigDocument,
    models: &ModelsDocument,
    agents: &AgentsDocument,
) -> Result<(), ConfigurationError> {
    validate_version("config.yaml", config.version)?;
    validate_version("models.yaml", models.version)?;
    validate_version("agents.yaml", agents.version)?;

    let providers = config
        .providers
        .iter()
        .cloned()
        .map(|record| {
            Provider::try_from(ProviderDraft {
                id: record.id,
                name: record.name,
                enabled: record.enabled,
                protocol: record.protocol,
                endpoint: record.endpoint,
                endpoint_mode: record.endpoint_mode,
                auth: match record.api_key_placement {
                    ApiKeyPlacement::None => Auth::none(),
                    placement => Auth::api_key(placement, record.api_key),
                },
                models_url: record.models_url,
            })
            .map_err(|error| ConfigurationError::Invalid(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let provider_registry = ProviderRegistry::new(providers)
        .map_err(|error| ConfigurationError::Invalid(error.to_string()))?;

    let model_values = models
        .models
        .iter()
        .cloned()
        .map(|record| {
            Model::try_from(ModelDraft {
                id: record.id,
                provider_id: record.provider_id,
                upstream_id: record.upstream_id,
                display_name: record.display_name,
                capabilities: record.capabilities,
                protocol_capabilities: record.protocol_capabilities,
            })
            .map_err(|error| ConfigurationError::Invalid(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let model_registry = ModelRegistry::new(model_values, provider_registry.ids())
        .map_err(|error| ConfigurationError::Invalid(error.to_string()))?;

    let mut extension_ids = HashSet::new();
    for extension in &agents.extension_subagents {
        if !is_agent_key(&extension.id) || !extension_ids.insert(extension.id.clone()) {
            return Err(ConfigurationError::Invalid(format!(
                "invalid or duplicate extension SubAgent id: {}",
                extension.id
            )));
        }
        if extension.name.trim().is_empty()
            || extension.name.trim() != extension.name
            || !is_agent_key(&extension.source_client_id)
            || extension.source_agent_id.trim().is_empty()
            || extension.source_agent_id.trim() != extension.source_agent_id
        {
            return Err(ConfigurationError::Invalid(format!(
                "invalid extension SubAgent definition: {}",
                extension.id
            )));
        }
        if extension
            .model_id
            .as_ref()
            .is_some_and(|model_id| model_registry.get(model_id).is_none())
        {
            return Err(ConfigurationError::Invalid(format!(
                "extension SubAgent {} references unknown model",
                extension.id
            )));
        }
        let mut capabilities = HashSet::new();
        for capability in &extension.capabilities {
            if !is_agent_key(capability) || !capabilities.insert(capability) {
                return Err(ConfigurationError::Invalid(format!(
                    "invalid or duplicate extension SubAgent capability: {capability}"
                )));
            }
        }
    }

    let mut agent_ids = HashSet::new();
    for record in &agents.agents {
        if !is_agent_key(&record.id) {
            return Err(ConfigurationError::Invalid(format!(
                "agent id must be a lowercase slug: {}",
                record.id
            )));
        }
        if !agent_ids.insert(&record.id) {
            return Err(ConfigurationError::Invalid(format!(
                "duplicate agent id: {}",
                record.id
            )));
        }
        if !is_agent_key(&record.adapter) {
            return Err(ConfigurationError::Invalid(format!(
                "adapter id must be a lowercase slug: {}",
                record.adapter
            )));
        }

        let main = match &record.main {
            MainRecord::Native => MainSelection::Native,
            MainRecord::Managed(id) => MainSelection::Managed(id.clone()),
        };
        let agent = AgentConfiguration::new(main, record.model_pool.clone(), &model_registry)
            .map_err(|error| ConfigurationError::Invalid(error.to_string()))?;

        let mut codex_agent_ids = HashSet::new();
        for agent_model in &record.codex_agent_models {
            if !is_agent_key(&agent_model.id) {
                return Err(ConfigurationError::Invalid(format!(
                    "Codex Agent id must be a lowercase slug: {}",
                    agent_model.id
                )));
            }
            if !codex_agent_ids.insert(&agent_model.id) {
                return Err(ConfigurationError::Invalid(format!(
                    "duplicate Codex Agent id: {}",
                    agent_model.id
                )));
            }
            if agent_model.name.trim().is_empty()
                || agent_model.name.trim() != agent_model.name
                || agent_model.name.chars().any(char::is_control)
            {
                return Err(ConfigurationError::Invalid(format!(
                    "Codex Agent name is invalid: {}",
                    agent_model.id
                )));
            }
            if model_registry.get(&agent_model.model_id).is_none() {
                return Err(ConfigurationError::Invalid(format!(
                    "Codex Agent {} references unknown model: {}",
                    agent_model.id, agent_model.model_id
                )));
            }
            let mut capabilities = HashSet::new();
            for capability in &agent_model.capabilities {
                if !is_agent_key(capability) {
                    return Err(ConfigurationError::Invalid(format!(
                        "Codex Agent capability must be a lowercase slug: {capability}"
                    )));
                }
                if !capabilities.insert(capability) {
                    return Err(ConfigurationError::Invalid(format!(
                        "duplicate Codex Agent capability: {capability}"
                    )));
                }
            }
        }

        let mut bindings = HashSet::new();
        for extension_id in &record.extension_subagent_ids {
            if !bindings.insert(extension_id) || !extension_ids.contains(extension_id.as_str()) {
                return Err(ConfigurationError::Invalid(format!(
                    "agent {} has an invalid extension SubAgent binding: {extension_id}",
                    record.id
                )));
            }
        }

        for (slot, model_id) in &record.model_slots {
            if !is_agent_key(slot) {
                return Err(ConfigurationError::Invalid(format!(
                    "model slot must be a lowercase slug: {slot}"
                )));
            }
            if model_registry.get(model_id).is_none() {
                return Err(ConfigurationError::Invalid(format!(
                    "unknown model slot selection: {model_id}"
                )));
            }
        }

        for (slot, model) in &record.native_model_slots {
            if !is_agent_key(slot) {
                return Err(ConfigurationError::Invalid(format!(
                    "native model slot must be a lowercase slug: {slot}"
                )));
            }
            if record.id != "codex" && record.id != "claude_code" {
                return Err(ConfigurationError::Invalid(format!(
                    "native model slots are not supported by agent: {}",
                    record.id
                )));
            }
            if record.id == "claude_code"
                && !matches!(
                    slot.as_str(),
                    "main" | "sonnet" | "opus" | "fable" | "haiku" | "subagent_default"
                )
            {
                return Err(ConfigurationError::Invalid(format!(
                    "unsupported Claude Code native model slot: {slot}"
                )));
            }
            if record.id == "claude_code"
                && !matches!(
                    model.as_str(),
                    "default" | "sonnet" | "opus" | "fable" | "haiku"
                )
            {
                return Err(ConfigurationError::Invalid(format!(
                    "unsupported Claude Code native model: {model}"
                )));
            }
            if model.trim().is_empty()
                || model.trim() != model
                || model.chars().any(char::is_control)
            {
                return Err(ConfigurationError::Invalid(format!(
                    "invalid native model selection for slot {slot}"
                )));
            }
        }

        if record.enabled {
            validate_available_models(
                &agent,
                record.model_slots.values().cloned().chain(
                    record
                        .codex_agent_models
                        .iter()
                        .filter(|agent_model| agent_model.enabled)
                        .map(|agent_model| agent_model.model_id.clone()),
                ),
                &model_registry,
                &provider_registry,
            )?;
        }
    }

    Ok(())
}

fn is_agent_key(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.starts_with('_')
        && !value.ends_with('-')
        && !value.ends_with('_')
        && !value.contains("--")
        && !value.contains("__")
        && value.bytes().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, b'-' | b'_')
        })
}

fn validate_version(file: &str, version: u8) -> Result<(), ConfigurationError> {
    if version == FORMAT_VERSION {
        Ok(())
    } else {
        Err(ConfigurationError::Invalid(format!(
            "unsupported {file} version: {version}"
        )))
    }
}

fn validate_available_models(
    agent: &AgentConfiguration,
    model_slots: impl Iterator<Item = String>,
    models: &ModelRegistry,
    providers: &ProviderRegistry,
) -> Result<(), ConfigurationError> {
    let selected = match agent.main() {
        MainSelection::Native => None,
        MainSelection::Managed(id) => Some(id.clone()),
    }
    .into_iter()
    .chain(agent.model_pool().iter().cloned())
    .chain(model_slots)
    .collect::<Vec<_>>();

    for model_id in &selected {
        let model = models
            .get(model_id)
            .expect("agent configuration was validated");
        let provider = providers
            .get(model.provider_id())
            .expect("model registry was validated");
        if !provider.is_enabled() {
            return Err(ConfigurationError::Invalid(format!(
                "model {model_id} uses disabled provider {}",
                provider.id()
            )));
        }
    }
    Ok(())
}
