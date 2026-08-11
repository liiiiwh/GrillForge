use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRoute {
    alias: String,
    provider_id: String,
    upstream_model: String,
}

impl ModelRoute {
    pub fn new(
        alias: impl Into<String>,
        provider_id: impl Into<String>,
        upstream_model: impl Into<String>,
    ) -> Self {
        Self {
            alias: alias.into(),
            provider_id: provider_id.into(),
            upstream_model: upstream_model.into(),
        }
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    pub fn upstream_model(&self) -> &str {
        &self.upstream_model
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RouteError {
    DuplicateAlias(String),
    UnknownAlias(String),
}

impl Display for RouteError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateAlias(alias) => {
                write!(formatter, "duplicate model route alias: {alias}")
            }
            Self::UnknownAlias(alias) => write!(formatter, "unknown model route alias: {alias}"),
        }
    }
}

impl Error for RouteError {}

pub struct RouteTable {
    routes: HashMap<String, ModelRoute>,
}

impl RouteTable {
    pub fn new(routes: impl IntoIterator<Item = ModelRoute>) -> Result<Self, RouteError> {
        let mut by_alias = HashMap::new();

        for route in routes {
            let alias = route.alias.clone();
            if by_alias.insert(alias.clone(), route).is_some() {
                return Err(RouteError::DuplicateAlias(alias));
            }
        }

        Ok(Self { routes: by_alias })
    }

    pub fn resolve(&self, alias: &str) -> Result<&ModelRoute, RouteError> {
        self.routes
            .get(alias)
            .ok_or_else(|| RouteError::UnknownAlias(alias.to_string()))
    }
}
