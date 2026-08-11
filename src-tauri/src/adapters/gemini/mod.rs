use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

const SNAPSHOT_FILE: &str = "gemini.snapshot.json";
const CLI_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiCliDetection {
    pub path: PathBuf,
    pub version: String,
}

pub fn detect_gemini_cli() -> Result<Option<GeminiCliDetection>, GeminiAdapterError> {
    let executable = if cfg!(windows) {
        "gemini.cmd"
    } else {
        "gemini"
    };
    let mut candidates = env::var_os("PATH")
        .map(|path| {
            env::split_paths(&path)
                .map(|directory| directory.join(executable))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        candidates.extend(crate::cli_discovery::node_cli_candidates_from_home(
            home, executable,
        ));
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
                GeminiAdapterError::Invalid(format!(
                    "discover Gemini CLI through the login shell: {error}"
                ))
            })
        },
        |path| inspect_gemini_cli(path),
    )
}

pub fn detect_gemini_cli_in(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<Option<GeminiCliDetection>, GeminiAdapterError> {
    crate::cli_discovery::first_valid_candidate(candidates, |path| inspect_gemini_cli(path))
}

pub fn inspect_gemini_cli(
    path: impl AsRef<Path>,
) -> Result<GeminiCliDetection, GeminiAdapterError> {
    inspect_cli(path.as_ref(), "Gemini CLI").map(|version| GeminiCliDetection {
        path: path.as_ref().to_path_buf(),
        version,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiPaths {
    pub env_path: PathBuf,
    pub settings_path: PathBuf,
}

impl GeminiPaths {
    pub fn new(env_path: impl Into<PathBuf>, settings_path: impl Into<PathBuf>) -> Self {
        Self {
            env_path: env_path.into(),
            settings_path: settings_path.into(),
        }
    }
}

pub fn paths_from_home(home: impl AsRef<Path>) -> GeminiPaths {
    let root = home.as_ref().join(".gemini");
    GeminiPaths::new(root.join(".env"), root.join("settings.json"))
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeminiRequest {
    base_url: String,
    api_key: String,
    model: String,
}

impl GeminiRequest {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, GeminiAdapterError> {
        let base_url = base_url.into();
        validate_base_url(&base_url)?;
        let api_key = api_key.into();
        validate_value("API key", &api_key)?;
        let model = model.into();
        validate_value("model", &model)?;
        Ok(Self {
            base_url,
            api_key,
            model,
        })
    }
}

impl Debug for GeminiRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GeminiRequest")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiTakeoverStatus {
    Inactive,
    Active,
    Drifted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiStatus {
    pub snapshot_present: bool,
    pub takeover: GeminiTakeoverStatus,
}

#[derive(Debug)]
pub struct GeminiAdapter {
    paths: GeminiPaths,
    snapshot_path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
struct RecoverySnapshot {
    version: u8,
    original_env: Option<Vec<u8>>,
    original_settings: Option<Vec<u8>>,
    expected_env: Vec<u8>,
    expected_settings: Vec<u8>,
}

impl GeminiAdapter {
    pub fn new(paths: GeminiPaths, grillforge_root: impl Into<PathBuf>) -> Self {
        Self {
            paths,
            snapshot_path: grillforge_root.into().join(SNAPSHOT_FILE),
        }
    }

    pub fn apply(&self, request: GeminiRequest) -> Result<GeminiStatus, GeminiAdapterError> {
        let current_env = read_optional(&self.paths.env_path)?;
        let current_settings = read_optional(&self.paths.settings_path)?;
        let previous_snapshot = read_optional(&self.snapshot_path)?;
        let (original_env, original_settings) = match &previous_snapshot {
            Some(bytes) => {
                let snapshot = parse_snapshot(bytes)?;
                if current_env.as_deref() != Some(snapshot.expected_env.as_slice())
                    || current_settings.as_deref() != Some(snapshot.expected_settings.as_slice())
                {
                    return Err(GeminiAdapterError::Drifted);
                }
                (snapshot.original_env, snapshot.original_settings)
            }
            None => (current_env.clone(), current_settings.clone()),
        };
        let expected_env = project_env(current_env.as_deref(), &request)?;
        let expected_settings = project_settings(current_settings.as_deref())?;
        let snapshot = RecoverySnapshot {
            version: 1,
            original_env,
            original_settings,
            expected_env: expected_env.clone(),
            expected_settings: expected_settings.clone(),
        };
        let snapshot_bytes = serde_json::to_vec_pretty(&snapshot).map_err(|error| {
            GeminiAdapterError::Invalid(format!("could not encode Gemini snapshot: {error}"))
        })?;

        let write = write_optional(&self.snapshot_path, Some(&snapshot_bytes))
            .and_then(|()| write_optional(&self.paths.env_path, Some(&expected_env)))
            .and_then(|()| write_optional(&self.paths.settings_path, Some(&expected_settings)));
        if let Err(primary) = write {
            let rollback = write_optional(&self.paths.env_path, current_env.as_deref())
                .and_then(|()| {
                    write_optional(&self.paths.settings_path, current_settings.as_deref())
                })
                .and_then(|()| write_optional(&self.snapshot_path, previous_snapshot.as_deref()));
            return Err(combine_rollback(primary, rollback));
        }
        if read_optional(&self.paths.env_path)?.as_deref() != Some(expected_env.as_slice())
            || read_optional(&self.paths.settings_path)?.as_deref()
                != Some(expected_settings.as_slice())
        {
            return Err(GeminiAdapterError::Invalid(
                "Gemini apply verification failed".into(),
            ));
        }
        self.status()
    }

    pub fn disable(&self) -> Result<GeminiStatus, GeminiAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(GeminiStatus {
                snapshot_present: false,
                takeover: GeminiTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        if read_optional(&self.paths.env_path)?.as_deref() != Some(snapshot.expected_env.as_slice())
            || read_optional(&self.paths.settings_path)?.as_deref()
                != Some(snapshot.expected_settings.as_slice())
        {
            return Err(GeminiAdapterError::Drifted);
        }
        write_optional(&self.paths.env_path, snapshot.original_env.as_deref())?;
        write_optional(
            &self.paths.settings_path,
            snapshot.original_settings.as_deref(),
        )?;
        if read_optional(&self.paths.env_path)? != snapshot.original_env
            || read_optional(&self.paths.settings_path)? != snapshot.original_settings
        {
            return Err(GeminiAdapterError::Invalid(
                "Gemini restore verification failed; recovery snapshot was retained".into(),
            ));
        }
        fs::remove_file(&self.snapshot_path).map_err(|source| GeminiAdapterError::Io {
            operation: "remove Gemini recovery snapshot",
            path: self.snapshot_path.clone(),
            source,
        })?;
        self.status()
    }

    pub fn status(&self) -> Result<GeminiStatus, GeminiAdapterError> {
        let Some(bytes) = read_optional(&self.snapshot_path)? else {
            return Ok(GeminiStatus {
                snapshot_present: false,
                takeover: GeminiTakeoverStatus::Inactive,
            });
        };
        let snapshot = parse_snapshot(&bytes)?;
        let takeover = if read_optional(&self.paths.env_path)?.as_deref()
            == Some(snapshot.expected_env.as_slice())
            && read_optional(&self.paths.settings_path)?.as_deref()
                == Some(snapshot.expected_settings.as_slice())
        {
            GeminiTakeoverStatus::Active
        } else {
            GeminiTakeoverStatus::Drifted
        };
        Ok(GeminiStatus {
            snapshot_present: true,
            takeover,
        })
    }
}

fn project_env(
    original: Option<&[u8]>,
    request: &GeminiRequest,
) -> Result<Vec<u8>, GeminiAdapterError> {
    let text = match original {
        Some(bytes) => std::str::from_utf8(bytes)
            .map_err(|_| GeminiAdapterError::Invalid("Gemini .env must be UTF-8".into()))?,
        None => "",
    };
    let mut entries = parse_env_strict(text)?;
    entries.insert("GEMINI_API_KEY".into(), request.api_key.clone());
    entries.insert("GEMINI_MODEL".into(), request.model.clone());
    entries.insert("GOOGLE_GEMINI_BASE_URL".into(), request.base_url.clone());
    let mut output = entries
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");
    output.push('\n');
    Ok(output.into_bytes())
}

fn parse_env_strict(text: &str) -> Result<BTreeMap<String, String>, GeminiAdapterError> {
    let mut entries = BTreeMap::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(GeminiAdapterError::Invalid(format!(
                "Gemini .env line {} is missing '='",
                index + 1
            )));
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty()
            || !key.bytes().enumerate().all(|(position, byte)| {
                byte == b'_'
                    || byte.is_ascii_alphanumeric() && (position > 0 || byte.is_ascii_alphabetic())
            })
        {
            return Err(GeminiAdapterError::Invalid(format!(
                "Gemini .env line {} has an invalid key",
                index + 1
            )));
        }
        if entries.insert(key.to_string(), value.to_string()).is_some() {
            return Err(GeminiAdapterError::Invalid(format!(
                "Gemini .env line {} duplicates {key}",
                index + 1
            )));
        }
    }
    Ok(entries)
}

fn project_settings(original: Option<&[u8]>) -> Result<Vec<u8>, GeminiAdapterError> {
    let mut root = match original {
        Some(bytes) => serde_json::from_slice::<Value>(bytes).map_err(|error| {
            GeminiAdapterError::Invalid(format!("Gemini settings.json is invalid JSON: {error}"))
        })?,
        None => Value::Object(Map::new()),
    };
    let root = root.as_object_mut().ok_or_else(|| {
        GeminiAdapterError::Invalid("Gemini settings.json must be an object".into())
    })?;
    let security = object_entry(root, "security")?;
    let auth = object_entry(security, "auth")?;
    auth.insert(
        "selectedType".into(),
        Value::String("gemini-api-key".into()),
    );
    serde_json::to_vec_pretty(&Value::Object(root.clone())).map_err(|error| {
        GeminiAdapterError::Invalid(format!("could not encode Gemini settings: {error}"))
    })
}

fn object_entry<'a>(
    root: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>, GeminiAdapterError> {
    root.entry(key)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            GeminiAdapterError::Invalid(format!("Gemini settings {key} must be an object"))
        })
}

fn validate_base_url(value: &str) -> Result<(), GeminiAdapterError> {
    let url = Url::parse(value)
        .map_err(|_| GeminiAdapterError::Invalid(format!("invalid Gemini base URL: {value}")))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(GeminiAdapterError::Invalid(format!(
            "invalid Gemini base URL: {value}"
        )));
    }
    Ok(())
}

fn validate_value(field: &str, value: &str) -> Result<(), GeminiAdapterError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(GeminiAdapterError::Invalid(format!(
            "Gemini {field} is empty or invalid"
        )));
    }
    Ok(())
}

fn parse_snapshot(bytes: &[u8]) -> Result<RecoverySnapshot, GeminiAdapterError> {
    let snapshot: RecoverySnapshot = serde_json::from_slice(bytes).map_err(|error| {
        GeminiAdapterError::Invalid(format!("Gemini recovery snapshot is invalid: {error}"))
    })?;
    if snapshot.version != 1 {
        return Err(GeminiAdapterError::Invalid(format!(
            "unsupported Gemini recovery snapshot version: {}",
            snapshot.version
        )));
    }
    Ok(snapshot)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, GeminiAdapterError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(GeminiAdapterError::Io {
            operation: "read Gemini configuration",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_optional(path: &Path, bytes: Option<&[u8]>) -> Result<(), GeminiAdapterError> {
    match bytes {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| GeminiAdapterError::Io {
                    operation: "create Gemini configuration directory",
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            crate::storage::atomic_replace(path, bytes).map_err(|source| GeminiAdapterError::Io {
                operation: "write Gemini configuration",
                path: path.to_path_buf(),
                source,
            })
        }
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(GeminiAdapterError::Io {
                operation: "remove Gemini configuration",
                path: path.to_path_buf(),
                source,
            }),
        },
    }
}

fn combine_rollback(
    primary: GeminiAdapterError,
    rollback: Result<(), GeminiAdapterError>,
) -> GeminiAdapterError {
    match rollback {
        Ok(()) => primary,
        Err(rollback) => {
            GeminiAdapterError::Invalid(format!("{primary}; rollback also failed: {rollback}"))
        }
    }
}

fn inspect_cli(path: &Path, name: &'static str) -> Result<String, GeminiAdapterError> {
    let mut command =
        crate::cli_discovery::version_command(path).map_err(|source| GeminiAdapterError::Io {
            operation: "prepare Gemini CLI inspection",
            path: path.to_path_buf(),
            source,
        })?;
    let mut child = command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| GeminiAdapterError::Io {
            operation: "inspect Gemini CLI",
            path: path.to_path_buf(),
            source,
        })?;
    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(|source| GeminiAdapterError::Io {
            operation: "inspect Gemini CLI",
            path: path.to_path_buf(),
            source,
        })? {
            let output = child
                .wait_with_output()
                .map_err(|source| GeminiAdapterError::Io {
                    operation: "inspect Gemini CLI",
                    path: path.to_path_buf(),
                    source,
                })?;
            if !status.success() {
                return Err(GeminiAdapterError::Invalid(format!(
                    "{name} did not return a version: {}",
                    path.display()
                )));
            }
            return String::from_utf8(output.stdout)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    GeminiAdapterError::Invalid(format!(
                        "{name} did not return a version: {}",
                        path.display()
                    ))
                });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GeminiAdapterError::Invalid(format!(
                "{name} version check timed out: {}",
                path.display()
            )));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Debug)]
pub enum GeminiAdapterError {
    Invalid(String),
    Drifted,
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl Display for GeminiAdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Drifted => formatter.write_str(
                "Gemini configuration differs from the last GrillForge apply; resolve the drift before continuing",
            ),
            Self::Io { operation, path, source } => write!(formatter, "{operation} failed for {}: {source}", path.display()),
        }
    }
}

impl Error for GeminiAdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
