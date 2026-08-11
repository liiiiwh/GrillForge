use super::provider::is_slug;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDraft {
    pub id: String,
    pub provider_id: String,
    pub upstream_id: String,
    pub display_name: String,
    pub capabilities: Vec<String>,
    pub protocol_capabilities: Vec<ProtocolCapability>,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolCapability {
    ReasoningItems,
    ReasoningContent,
    ReasoningEffort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    id: String,
    provider_id: String,
    upstream_id: String,
    display_name: String,
    capabilities: Vec<String>,
    protocol_capabilities: Vec<ProtocolCapability>,
}

impl Model {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    pub fn upstream_id(&self) -> &str {
        &self.upstream_id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    pub fn protocol_capabilities(&self) -> &[ProtocolCapability] {
        &self.protocol_capabilities
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    InvalidId(String),
    InvalidProviderId(String),
    EmptyUpstreamId,
    EmptyDisplayName,
    InvalidCapability(String),
    DuplicateCapability(String),
    DuplicateProtocolCapability,
    DuplicateModel(String),
    UnknownProvider { model: String, provider: String },
}

impl Display for ModelError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(id) => write!(formatter, "model id must be a lowercase slug: {id}"),
            Self::InvalidProviderId(id) => {
                write!(formatter, "provider id must be a lowercase slug: {id}")
            }
            Self::EmptyUpstreamId => write!(formatter, "model upstream id must not be empty"),
            Self::EmptyDisplayName => write!(formatter, "model display name must not be empty"),
            Self::InvalidCapability(value) => {
                write!(
                    formatter,
                    "model capability must be a lowercase slug: {value}"
                )
            }
            Self::DuplicateCapability(value) => {
                write!(formatter, "duplicate model capability: {value}")
            }
            Self::DuplicateProtocolCapability => {
                write!(formatter, "duplicate model protocol capability")
            }
            Self::DuplicateModel(id) => write!(formatter, "duplicate model id: {id}"),
            Self::UnknownProvider { model, provider } => {
                write!(
                    formatter,
                    "model {model} references unknown provider {provider}"
                )
            }
        }
    }
}

impl Error for ModelError {}

impl TryFrom<ModelDraft> for Model {
    type Error = ModelError;

    fn try_from(draft: ModelDraft) -> Result<Self, Self::Error> {
        if !is_slug(&draft.id) {
            return Err(ModelError::InvalidId(draft.id));
        }
        if !is_slug(&draft.provider_id) {
            return Err(ModelError::InvalidProviderId(draft.provider_id));
        }
        if draft.upstream_id.trim().is_empty() {
            return Err(ModelError::EmptyUpstreamId);
        }
        if draft.display_name.trim().is_empty() {
            return Err(ModelError::EmptyDisplayName);
        }

        let mut seen = HashSet::new();
        for capability in &draft.capabilities {
            if !is_slug(capability) {
                return Err(ModelError::InvalidCapability(capability.clone()));
            }
            if !seen.insert(capability.clone()) {
                return Err(ModelError::DuplicateCapability(capability.clone()));
            }
        }
        let mut protocol_capabilities = HashSet::new();
        for capability in &draft.protocol_capabilities {
            if !protocol_capabilities.insert(*capability) {
                return Err(ModelError::DuplicateProtocolCapability);
            }
        }

        Ok(Self {
            id: draft.id,
            provider_id: draft.provider_id,
            upstream_id: draft.upstream_id,
            display_name: draft.display_name,
            capabilities: draft.capabilities,
            protocol_capabilities: draft.protocol_capabilities,
        })
    }
}

#[derive(Debug)]
pub struct ModelRegistry {
    models: HashMap<String, Model>,
}

impl ModelRegistry {
    pub fn new(
        models: impl IntoIterator<Item = Model>,
        provider_ids: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ModelError> {
        let providers: HashSet<String> = provider_ids.into_iter().map(Into::into).collect();
        let mut by_id = HashMap::new();

        for model in models {
            if !providers.contains(model.provider_id()) {
                return Err(ModelError::UnknownProvider {
                    model: model.id.clone(),
                    provider: model.provider_id.clone(),
                });
            }
            let id = model.id.clone();
            if by_id.insert(id.clone(), model).is_some() {
                return Err(ModelError::DuplicateModel(id));
            }
        }

        Ok(Self { models: by_id })
    }

    pub fn get(&self, id: &str) -> Option<&Model> {
        self.models.get(id)
    }
}
