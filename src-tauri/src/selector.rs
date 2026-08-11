use crate::configuration::{ConfigurationError, ConfigurationFiles};
use serde::Serialize;
use std::error::Error;
use std::ffi::OsString;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

const CLAUDE_CODE_AGENT: &str = "claude_code";
const CLAUDE_CODE_SNAPSHOT: &str = "claude-code.snapshot.json";
const CLAUDE_DESKTOP_SNAPSHOT: &str = "claude-desktop.snapshot.json";

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SelectorWorker {
    pub model_id: String,
    pub display_name: String,
    pub capabilities: Vec<String>,
    pub agent_name: String,
    pub route_alias: String,
    pub provider_id: String,
    pub upstream_id: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SelectorOutput {
    pub workers: Vec<SelectorWorker>,
}

#[derive(Debug)]
pub enum SelectorError {
    Configuration(ConfigurationError),
    MissingClaudeCode,
    ClaudeClientOfficialRoute,
    ClaudeClientUnmanagedThreepRoute,
}

#[derive(Debug)]
pub enum CliError {
    InvalidArguments,
    MissingHomeDirectory,
    Selector(SelectorError),
    Serialize(serde_json::Error),
}

impl Display for SelectorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(error) => Display::fmt(error, formatter),
            Self::MissingClaudeCode => formatter.write_str("agents.yaml is missing claude_code"),
            Self::ClaudeClientOfficialRoute => formatter.write_str(
                "Claude Client Code 正在使用官方路由，不能调用 GrillForge 外部 SubAgent；请先在 GrillForge 的 Claude Client 页面配置模型并应用，然后重新启动 Claude Client",
            ),
            Self::ClaudeClientUnmanagedThreepRoute => formatter.write_str(
                "Claude Client Code 正在使用第三方路由，但当前路由不是已生效的 GrillForge 配置；请在 GrillForge 的 Claude Client 页面重新应用",
            ),
        }
    }
}

impl Error for SelectorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Configuration(error) => Some(error),
            Self::MissingClaudeCode
            | Self::ClaudeClientOfficialRoute
            | Self::ClaudeClientUnmanagedThreepRoute => None,
        }
    }
}

impl Display for CliError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidArguments => formatter.write_str(
                "usage: grillforge selector [--config-dir PATH] [--claude-entrypoint NAME]",
            ),
            Self::MissingHomeDirectory => {
                formatter.write_str("could not resolve the user home directory")
            }
            Self::Selector(error) => Display::fmt(error, formatter),
            Self::Serialize(error) => {
                write!(formatter, "could not serialize selector output: {error}")
            }
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Selector(error) => Some(error),
            Self::Serialize(error) => Some(error),
            Self::InvalidArguments | Self::MissingHomeDirectory => None,
        }
    }
}

impl From<ConfigurationError> for SelectorError {
    fn from(value: ConfigurationError) -> Self {
        Self::Configuration(value)
    }
}

pub fn run_cli(args: impl IntoIterator<Item = OsString>) -> Result<Option<String>, CliError> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args.first().is_none_or(|argument| argument != "selector") {
        return Ok(None);
    }

    let mut config_dir = None;
    let mut claude_entrypoint = None;
    let mut index = 1;
    while index < args.len() {
        let flag = &args[index];
        let value = args.get(index + 1).ok_or(CliError::InvalidArguments)?;
        if value.is_empty() {
            return Err(CliError::InvalidArguments);
        }
        if flag == "--config-dir" && config_dir.is_none() {
            config_dir = Some(PathBuf::from(value));
        } else if flag == "--claude-entrypoint" && claude_entrypoint.is_none() {
            claude_entrypoint = Some(value.to_string_lossy().into_owned());
        } else {
            return Err(CliError::InvalidArguments);
        }
        index += 2;
    }
    let config_dir = match config_dir {
        Some(path) => path,
        None => default_config_dir()?,
    };
    let output = select(&config_dir).map_err(CliError::Selector)?;
    validate_claude_client_context(&output, claude_entrypoint.as_deref(), config_dir.as_path())
        .map_err(CliError::Selector)?;
    serde_json::to_string(&output)
        .map(Some)
        .map_err(CliError::Serialize)
}

fn validate_claude_client_context(
    output: &SelectorOutput,
    entrypoint: Option<&str>,
    config_dir: &Path,
) -> Result<(), SelectorError> {
    if !output
        .workers
        .iter()
        .any(|worker| worker.route_alias.starts_with("grillforge/"))
    {
        return Ok(());
    }
    match entrypoint {
        Some("claude-desktop") => Err(SelectorError::ClaudeClientOfficialRoute),
        Some("claude-desktop-3p") if !config_dir.join(CLAUDE_DESKTOP_SNAPSHOT).is_file() => {
            Err(SelectorError::ClaudeClientUnmanagedThreepRoute)
        }
        _ => Ok(()),
    }
}

fn default_config_dir() -> Result<PathBuf, CliError> {
    dirs::home_dir()
        .map(|home| home.join(".grillforge"))
        .ok_or(CliError::MissingHomeDirectory)
}

pub fn select(config_dir: impl AsRef<Path>) -> Result<SelectorOutput, SelectorError> {
    let documents = ConfigurationFiles::new(config_dir.as_ref()).read()?;
    let agent = documents
        .agents
        .agents
        .iter()
        .find(|agent| agent.id == CLAUDE_CODE_AGENT)
        .ok_or(SelectorError::MissingClaudeCode)?;

    // The recoverable snapshot is the source of truth for whether the Claude
    // integration is live. `agent.enabled` is the durable startup preference:
    // it deliberately remains true while a normal app exit restores Claude's
    // files, so it must not make a stopped integration appear selectable.
    if !config_dir.as_ref().join(CLAUDE_CODE_SNAPSHOT).is_file() {
        return Ok(SelectorOutput { workers: vec![] });
    }

    let mut workers = if agent.subagents.is_empty() {
        if !agent.worker_mode {
            return Ok(SelectorOutput { workers: vec![] });
        }
        agent
            .enabled_workers
            .iter()
            .map(|id| {
                let model = documents
                    .models
                    .models
                    .iter()
                    .find(|model| &model.id == id)
                    .expect("configuration validation guarantees enabled Worker models exist");
                SelectorWorker {
                    model_id: model.id.clone(),
                    display_name: model.display_name.clone(),
                    capabilities: model.capabilities.clone(),
                    agent_name: format!("grillforge-worker-{}", model.id),
                    route_alias: format!("grillforge/{}", model.id),
                    provider_id: model.provider_id.clone(),
                    upstream_id: model.upstream_id.clone(),
                }
            })
            .collect::<Vec<_>>()
    } else {
        agent
            .subagents
            .iter()
            .filter(|subagent| subagent.enabled)
            .map(|subagent| {
                let model = documents
                    .models
                    .models
                    .iter()
                    .find(|model| model.id == subagent.model_id)
                    .expect("configuration validation guarantees SubAgent models exist");
                SelectorWorker {
                    model_id: model.id.clone(),
                    display_name: subagent.name.clone(),
                    capabilities: subagent.capabilities.clone(),
                    agent_name: format!("grillforge-worker-{}", subagent.id),
                    route_alias: format!("grillforge/{}", model.id),
                    provider_id: model.provider_id.clone(),
                    upstream_id: model.upstream_id.clone(),
                }
            })
            .collect::<Vec<_>>()
    };
    if agent.native_subagent_enabled {
        workers.push(SelectorWorker {
            model_id: "claude-native".into(),
            display_name: "Claude 原生 SubAgent".into(),
            capabilities: vec!["coding".into(), "general".into()],
            agent_name: "grillforge-worker-claude-native".into(),
            route_alias: "inherit".into(),
            provider_id: "anthropic-native".into(),
            upstream_id: "inherit".into(),
        });
    }
    workers.sort_by(|left, right| left.agent_name.cmp(&right.agent_name));

    Ok(SelectorOutput { workers })
}
