use super::model::ModelRegistry;
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainSelection {
    Native,
    Managed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfiguration {
    main: MainSelection,
    model_pool: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentConfigurationError {
    UnknownMainModel(String),
    UnknownPoolModel(String),
    DuplicatePoolModel(String),
}

impl Display for AgentConfigurationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownMainModel(id) => write!(formatter, "unknown main model: {id}"),
            Self::UnknownPoolModel(id) => write!(formatter, "unknown model-pool model: {id}"),
            Self::DuplicatePoolModel(id) => write!(formatter, "duplicate model-pool model: {id}"),
        }
    }
}

impl Error for AgentConfigurationError {}

impl AgentConfiguration {
    pub fn new(
        main: MainSelection,
        model_pool: impl IntoIterator<Item = impl Into<String>>,
        models: &ModelRegistry,
    ) -> Result<Self, AgentConfigurationError> {
        if let MainSelection::Managed(id) = &main {
            if models.get(id).is_none() {
                return Err(AgentConfigurationError::UnknownMainModel(id.clone()));
            }
        }

        let mut pool = Vec::new();
        let mut seen = HashSet::new();
        for model in model_pool {
            let model = model.into();
            if models.get(&model).is_none() {
                return Err(AgentConfigurationError::UnknownPoolModel(model));
            }
            if !seen.insert(model.clone()) {
                return Err(AgentConfigurationError::DuplicatePoolModel(model));
            }
            pool.push(model);
        }

        Ok(Self {
            main,
            model_pool: pool,
        })
    }

    pub fn main(&self) -> &MainSelection {
        &self.main
    }

    pub fn model_pool(&self) -> &[String] {
        &self.model_pool
    }
}
