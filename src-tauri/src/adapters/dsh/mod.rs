//! DeepSeek Harness (`dsh`) adapter.
//!
//! A dsh profile composes plugin layers and reads one user layer,
//! `$DSH_HOME/profiles/<profile>/cordis.patch.yml`. GrillForge owns entries in
//! that layer and nothing else: the model route it declares, and the MCP server
//! that carries its extension SubAgents. Credentials stay in `$DSH_HOME/.env`,
//! which the harness resolves per request, so no secret enters the patch layer.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::env;
use std::fmt::{self, Debug, Display, Formatter};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

const CLI_TIMEOUT: Duration = Duration::from_secs(10);

/// The one plugin id GrillForge declares for its own model route.
const LLM_PLUGIN: &str = "@deepseek-ai/dsh-llm-pi-ai";
const PROVIDER_ROUTE: &str = "grillforge";
const CREDENTIAL_ENV: &str = "GRILLFORGE_DSH_API_KEY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DshAdapterError {
    Invalid(String),
    Io(String),
    Drifted,
}

impl Display for DshAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "{message}"),
            Self::Io(message) => write!(formatter, "{message}"),
            Self::Drifted => write!(
                formatter,
                "DeepSeek Harness configuration changed outside GrillForge; reapply or restore it"
            ),
        }
    }
}

impl std::error::Error for DshAdapterError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DshCliDetection {
    pub path: PathBuf,
    pub version: String,
}

pub fn detect_dsh_cli() -> Result<Option<DshCliDetection>, DshAdapterError> {
    let executable = if cfg!(windows) { "dsh.exe" } else { "dsh" };
    let mut candidates = env::var_os("PATH")
        .map(|path| {
            env::split_paths(&path)
                .map(|directory| directory.join(executable))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".dsh/bin").join(executable));
    }
    #[cfg(target_os = "macos")]
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin").join(executable),
        PathBuf::from("/usr/local/bin").join(executable),
    ]);
    crate::cli_discovery::first_valid_candidate_across_sources(
        candidates,
        || {
            crate::cli_discovery::login_shell_candidates(executable).map_err(|error| {
                DshAdapterError::Invalid(format!(
                    "discover DeepSeek Harness CLI through the login shell: {error}"
                ))
            })
        },
        |path| inspect_dsh_cli(path),
    )
}

pub fn detect_dsh_cli_in(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<Option<DshCliDetection>, DshAdapterError> {
    crate::cli_discovery::first_valid_candidate(candidates, |path| inspect_dsh_cli(path))
}

pub fn inspect_dsh_cli(path: impl AsRef<Path>) -> Result<DshCliDetection, DshAdapterError> {
    let path = path.as_ref().to_path_buf();
    let mut command = crate::cli_discovery::version_command(&path).map_err(|error| {
        DshAdapterError::Io(format!(
            "could not prepare DeepSeek Harness CLI inspection: {error}"
        ))
    })?;
    let mut child = command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            DshAdapterError::Io(format!("could not inspect DeepSeek Harness CLI: {error}"))
        })?;
    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DshAdapterError::Io(
                    "DeepSeek Harness CLI inspection timed out".into(),
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                return Err(DshAdapterError::Io(format!(
                    "could not inspect DeepSeek Harness CLI: {error}"
                )));
            }
        }
    }
    let output = child.wait_with_output().map_err(|error| {
        DshAdapterError::Io(format!("could not inspect DeepSeek Harness CLI: {error}"))
    })?;
    if !output.status.success() {
        return Err(DshAdapterError::Invalid(
            "DeepSeek Harness CLI did not report a version".into(),
        ));
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() {
        return Err(DshAdapterError::Invalid(
            "DeepSeek Harness CLI reported an empty version".into(),
        ));
    }
    Ok(DshCliDetection { path, version })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DshPaths {
    pub patch_path: PathBuf,
    pub credentials_path: PathBuf,
}

impl DshPaths {
    pub fn new(patch_path: impl Into<PathBuf>, credentials_path: impl Into<PathBuf>) -> Self {
        Self {
            patch_path: patch_path.into(),
            credentials_path: credentials_path.into(),
        }
    }
}

/// The harness delegates through its headless profile, so that is the profile
/// GrillForge configures.
pub fn paths_from_home(home: impl AsRef<Path>) -> DshPaths {
    let root = home.as_ref().join(".dsh");
    DshPaths::new(
        root.join("profiles/headless/cordis.patch.yml"),
        root.join(".env"),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DshModelSpec {
    id: String,
    name: String,
    context_window: Option<u64>,
}

impl DshModelSpec {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        context_window: Option<u64>,
    ) -> Result<Self, DshAdapterError> {
        let id = id.into();
        let name = name.into();
        if !id.starts_with("grillforge/") || id.len() <= "grillforge/".len() {
            return Err(DshAdapterError::Invalid(format!(
                "DeepSeek Harness model must use a GrillForge route alias: {id}"
            )));
        }
        if name.trim().is_empty() || name.chars().any(char::is_control) {
            return Err(DshAdapterError::Invalid(format!(
                "DeepSeek Harness model name is invalid: {id}"
            )));
        }
        if context_window == Some(0) {
            return Err(DshAdapterError::Invalid(format!(
                "DeepSeek Harness model context window is invalid: {id}"
            )));
        }
        Ok(Self {
            id,
            name,
            context_window,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Clone)]
pub struct DshRequest {
    base_url: String,
    api_key: String,
    models: Vec<DshModelSpec>,
    default_model: Option<String>,
}

impl Debug for DshRequest {
    /// The credential never reaches a log or an error.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DshRequest")
            .field("base_url", &self.base_url)
            .field("models", &self.models)
            .field("default_model", &self.default_model)
            .finish_non_exhaustive()
    }
}

impl DshRequest {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        models: Vec<DshModelSpec>,
        default_model: Option<String>,
    ) -> Result<Self, DshAdapterError> {
        let base_url = base_url.into();
        let api_key = api_key.into();
        validate_loopback(&base_url, "DeepSeek Harness base URL")?;
        if api_key.trim().is_empty() || api_key.chars().any(char::is_control) {
            return Err(DshAdapterError::Invalid(
                "DeepSeek Harness gateway credential is invalid".into(),
            ));
        }
        if models.is_empty() {
            return Err(DshAdapterError::Invalid(
                "DeepSeek Harness requires at least one model".into(),
            ));
        }
        let mut seen = HashSet::new();
        for model in &models {
            if !seen.insert(model.id.clone()) {
                return Err(DshAdapterError::Invalid(format!(
                    "duplicate DeepSeek Harness model: {}",
                    model.id
                )));
            }
        }
        if let Some(default_model) = &default_model {
            if !models.iter().any(|model| &model.id == default_model) {
                return Err(DshAdapterError::Invalid(format!(
                    "DeepSeek Harness default model is not configured: {default_model}"
                )));
            }
        }
        Ok(Self {
            base_url,
            api_key,
            models,
            default_model,
        })
    }
}

fn validate_loopback(value: &str, subject: &str) -> Result<(), DshAdapterError> {
    let url = url::Url::parse(value)
        .map_err(|_| DshAdapterError::Invalid(format!("{subject} is invalid: {value}")))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1") | Some("localhost"));
    if url.scheme() != "http" || !loopback || url.port().is_none() {
        return Err(DshAdapterError::Invalid(format!(
            "{subject} must be an exact loopback URL: {value}"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DshTakeoverStatus {
    Inactive,
    Active,
    Drifted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DshStatus {
    pub installed: bool,
    pub snapshot_present: bool,
    pub takeover: DshTakeoverStatus,
}

/// Owns the GrillForge entries in one dsh profile's user patch layer.
pub struct DshAdapter {
    paths: DshPaths,
    snapshot_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedFiles {
    patch: Option<String>,
    credentials: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Snapshot {
    original: ManagedFiles,
    expected: ManagedFiles,
}

impl DshAdapter {
    pub fn new(paths: DshPaths, grillforge_root: impl AsRef<Path>) -> Self {
        Self {
            paths,
            snapshot_path: grillforge_root.as_ref().join("dsh.snapshot.json"),
        }
    }

    pub fn status(&self) -> Result<DshStatus, DshAdapterError> {
        let installed = self.paths.patch_path.exists();
        let takeover = if !self.snapshot_path.exists() {
            DshTakeoverStatus::Inactive
        } else {
            let snapshot =
                parse_snapshot(&read_optional(&self.snapshot_path)?.unwrap_or_default())?;
            if self.capture()? == snapshot.expected {
                DshTakeoverStatus::Active
            } else {
                DshTakeoverStatus::Drifted
            }
        };
        Ok(DshStatus {
            installed,
            snapshot_present: self.snapshot_path.exists(),
            takeover,
        })
    }

    pub fn apply(&self, request: DshRequest) -> Result<(), DshAdapterError> {
        let current = self.capture()?;
        let original = match read_optional(&self.snapshot_path)? {
            Some(bytes) => parse_snapshot(&bytes)?.original,
            None => current.clone(),
        };
        let desired = ManagedFiles {
            patch: Some(render_patch(&request, current.patch.as_deref())?),
            credentials: Some(render_credentials(&request, current.credentials.as_deref())),
        };
        self.write_files(&desired)?;
        let snapshot = Snapshot {
            original,
            expected: desired,
        };
        let encoded = serde_json::to_vec_pretty(&snapshot)
            .map_err(|error| DshAdapterError::Invalid(format!("invalid dsh snapshot: {error}")))?;
        if let Some(parent) = self.snapshot_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                DshAdapterError::Io(format!("could not create {}: {error}", parent.display()))
            })?;
        }
        crate::storage::atomic_replace(&self.snapshot_path, &encoded)
            .map_err(|error| DshAdapterError::Io(error.to_string()))
    }

    pub fn disable(&self) -> Result<(), DshAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(());
        };
        let snapshot = parse_snapshot(&bytes)?;
        if self.capture()? != snapshot.expected {
            return Err(DshAdapterError::Drifted);
        }
        self.write_files(&snapshot.original)?;
        std::fs::remove_file(&self.snapshot_path)
            .map_err(|error| DshAdapterError::Io(format!("could not clear dsh snapshot: {error}")))
    }

    fn capture(&self) -> Result<ManagedFiles, DshAdapterError> {
        Ok(ManagedFiles {
            patch: read_optional_string(&self.paths.patch_path)?,
            credentials: read_optional_string(&self.paths.credentials_path)?,
        })
    }

    fn write_files(&self, files: &ManagedFiles) -> Result<(), DshAdapterError> {
        write_optional(&self.paths.patch_path, files.patch.as_deref())?;
        write_optional(&self.paths.credentials_path, files.credentials.as_deref())
    }
}

/// Rebuilds the patch layer with exactly one GrillForge block, preserving every
/// entry the user wrote around it.
fn render_patch(request: &DshRequest, current: Option<&str>) -> Result<String, DshAdapterError> {
    let mut kept = String::new();
    if let Some(current) = current {
        let mut skipping = false;
        for line in current.lines() {
            if line.trim_start().starts_with("# >>> grillforge") {
                skipping = true;
                continue;
            }
            if line.trim_start().starts_with("# <<< grillforge") {
                skipping = false;
                continue;
            }
            if !skipping {
                kept.push_str(line);
                kept.push('\n');
            }
        }
    }
    // The harness writes `[]` as its empty layer; a managed block replaces that
    // placeholder line, and nothing else.
    let mut without_placeholder = String::new();
    for line in kept.lines().filter(|line| line.trim() != "[]") {
        without_placeholder.push_str(line);
        without_placeholder.push('\n');
    }
    let kept = without_placeholder;
    let mut models = String::new();
    for model in &request.models {
        models.push_str(&format!(
            "          - id: {}\n            name: {}\n",
            yaml_scalar(&model.id),
            yaml_scalar(&model.name)
        ));
        if let Some(context_window) = model.context_window {
            models.push_str(&format!("            contextWindow: {context_window}\n"));
        }
    }
    let mut block = String::from("# >>> grillforge (managed; edits are replaced on Apply)\n");
    block.push_str("- id: llm\n");
    block.push_str(&format!("  name: {}\n", yaml_scalar(LLM_PLUGIN)));
    block.push_str("  config:\n    providers:\n");
    block.push_str(&format!("      {PROVIDER_ROUTE}:\n"));
    block.push_str("        displayName: GrillForge\n");
    block.push_str("        api: openai-completions\n");
    block.push_str(&format!(
        "        baseURL: {}\n",
        yaml_scalar(&request.base_url)
    ));
    block.push_str(&format!("        apiKeyEnv: {CREDENTIAL_ENV}\n"));
    block.push_str("        models:\n");
    block.push_str(&models);
    if let Some(default_model) = &request.default_model {
        block.push_str("- id: agent-default-model\n  config:\n");
        block.push_str(&format!("    provider: {PROVIDER_ROUTE}\n"));
        block.push_str(&format!("    model: {}\n", yaml_scalar(default_model)));
    }
    block.push_str("# <<< grillforge\n");
    Ok(format!("{}{block}", kept.trim_start_matches('\n')))
}

/// Writes the credential into the harness credential file, replacing only the
/// GrillForge line.
fn render_credentials(request: &DshRequest, current: Option<&str>) -> String {
    let prefix = format!("{CREDENTIAL_ENV}=");
    let mut kept = String::new();
    for line in current
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim_start().starts_with(&prefix))
    {
        kept.push_str(line);
        kept.push('\n');
    }
    kept.push_str(&prefix);
    kept.push_str(&request.api_key);
    kept.push('\n');
    kept
}

fn yaml_scalar(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn parse_snapshot(bytes: &[u8]) -> Result<Snapshot, DshAdapterError> {
    serde_json::from_slice(bytes)
        .map_err(|error| DshAdapterError::Invalid(format!("invalid dsh snapshot: {error}")))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, DshAdapterError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DshAdapterError::Io(format!(
            "could not read {}: {error}",
            path.display()
        ))),
    }
}

fn read_optional_string(path: &Path) -> Result<Option<String>, DshAdapterError> {
    match read_optional(path)? {
        None => Ok(None),
        Some(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| DshAdapterError::Invalid(format!("{} is not UTF-8", path.display()))),
    }
}

fn write_optional(path: &Path, contents: Option<&str>) -> Result<(), DshAdapterError> {
    match contents {
        Some(contents) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    DshAdapterError::Io(format!("could not create {}: {error}", parent.display()))
                })?;
            }
            crate::storage::atomic_replace(path, contents.as_bytes())
                .map_err(|error| DshAdapterError::Io(error.to_string()))
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DshAdapterError::Io(format!(
                "could not remove {}: {error}",
                path.display()
            ))),
        },
    }
}
