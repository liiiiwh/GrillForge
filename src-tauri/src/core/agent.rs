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
    worker_mode: bool,
    enabled_workers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentConfigurationError {
    UnknownMainModel(String),
    UnknownWorkerModel(String),
    DuplicateWorkerModel(String),
    EmptyWorkerPool,
}

impl Display for AgentConfigurationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownMainModel(id) => write!(formatter, "unknown main model: {id}"),
            Self::UnknownWorkerModel(id) => write!(formatter, "unknown worker model: {id}"),
            Self::DuplicateWorkerModel(id) => write!(formatter, "duplicate worker model: {id}"),
            Self::EmptyWorkerPool => {
                write!(
                    formatter,
                    "worker mode requires at least one valid enabled model"
                )
            }
        }
    }
}

impl Error for AgentConfigurationError {}

impl AgentConfiguration {
    pub fn new(
        main: MainSelection,
        worker_mode: bool,
        enabled_workers: impl IntoIterator<Item = impl Into<String>>,
        models: &ModelRegistry,
    ) -> Result<Self, AgentConfigurationError> {
        if let MainSelection::Managed(id) = &main {
            if models.get(id).is_none() {
                return Err(AgentConfigurationError::UnknownMainModel(id.clone()));
            }
        }

        let mut workers = Vec::new();
        let mut seen = HashSet::new();
        for worker in enabled_workers {
            let worker = worker.into();
            if models.get(&worker).is_none() {
                return Err(AgentConfigurationError::UnknownWorkerModel(worker));
            }
            if !seen.insert(worker.clone()) {
                return Err(AgentConfigurationError::DuplicateWorkerModel(worker));
            }
            workers.push(worker);
        }

        if worker_mode && workers.is_empty() {
            return Err(AgentConfigurationError::EmptyWorkerPool);
        }

        Ok(Self {
            main,
            worker_mode,
            enabled_workers: workers,
        })
    }

    pub fn effective_workers(&self) -> &[String] {
        if self.worker_mode {
            &self.enabled_workers
        } else {
            &[]
        }
    }

    pub fn main(&self) -> &MainSelection {
        &self.main
    }

    pub fn enabled_workers(&self) -> &[String] {
        &self.enabled_workers
    }
}
