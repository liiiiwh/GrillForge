use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use url::Url;

const SNAPSHOT_VERSION: u8 = 3;
const PI_REQUEST_TIMEOUT_MS: u64 = 3 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpClientFormat {
    ClaudeJson,
    ClaudeDesktopJson,
    CodexToml,
    GeminiJson,
    OpenCodeJson,
    KimiJson,
    PiExtensionJson,
    DshPatchYaml,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpMountTarget {
    pub client_id: String,
    pub config_path: PathBuf,
    pub format: McpClientFormat,
    stdio_command: Option<PathBuf>,
    claude_route_hook_settings: Option<PathBuf>,
}

pub fn pi_mcp_extension_installed(settings_path: &Path) -> Result<bool, String> {
    let Some(bytes) = read_optional(settings_path)? else {
        return Ok(false);
    };
    let root = parse_json_object(Some(&bytes), settings_path)?;
    Ok(root
        .get("packages")
        .and_then(Value::as_array)
        .is_some_and(|packages| {
            packages.iter().any(|package| {
                package.as_str().is_some_and(is_pi_mcp_extension_source)
                    || package
                        .get("source")
                        .and_then(Value::as_str)
                        .is_some_and(is_pi_mcp_extension_source)
            })
        }))
}

fn is_pi_mcp_extension_source(source: &str) -> bool {
    source == "npm:pi-mcp-extension"
        || source
            .strip_prefix("npm:pi-mcp-extension@")
            .is_some_and(|version| !version.is_empty())
}

impl McpMountTarget {
    pub fn new(
        client_id: impl Into<String>,
        config_path: impl Into<PathBuf>,
        format: McpClientFormat,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            config_path: config_path.into(),
            format,
            stdio_command: None,
            claude_route_hook_settings: None,
        }
    }

    pub fn with_stdio_command(mut self, command: impl Into<PathBuf>) -> Self {
        self.stdio_command = Some(command.into());
        self
    }

    pub fn with_claude_route_hook(mut self, settings_path: impl Into<PathBuf>) -> Self {
        self.claude_route_hook_settings = Some(settings_path.into());
        self
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RouteHookSnapshot {
    version: u8,
    file_existed: bool,
    hooks_existed: bool,
    pre_tool_use_existed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct MountSnapshot {
    version: u8,
    mounted_url: String,
    entry: MountEntrySnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pi_request_timeout: Option<PiRequestTimeoutSnapshot>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PiRequestTimeoutSnapshot {
    settings_existed: bool,
    request_timeout_existed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MountEntrySnapshot {
    file_existed: bool,
    section_existed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_json: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_toml: Option<Vec<u8>>,
}

pub struct McpMountManager {
    snapshot_root: PathBuf,
    targets: HashMap<String, McpMountTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpMountStatus {
    pub mounted: bool,
    pub configuration_changed: bool,
}

impl McpMountManager {
    pub fn new(
        snapshot_root: impl Into<PathBuf>,
        targets: impl IntoIterator<Item = McpMountTarget>,
    ) -> Result<Self, String> {
        let mut by_client = HashMap::new();
        for target in targets {
            if !valid_slug(&target.client_id)
                || !target.config_path.is_absolute()
                || by_client.insert(target.client_id.clone(), target).is_some()
            {
                return Err("invalid or duplicate MCP mount target".into());
            }
        }
        Ok(Self {
            snapshot_root: snapshot_root.into(),
            targets: by_client,
        })
    }

    pub fn mount(&self, client_id: &str, url: &str, token: &str) -> Result<(), String> {
        let target = self.target(client_id)?;
        validate_url(client_id, url)?;
        validate_token(token)?;
        if target.format == McpClientFormat::Unsupported {
            return Err(format!(
                "client {client_id} does not provide a verified MCP configuration format"
            ));
        }
        let snapshot_path = self.snapshot_path(client_id);
        let current = read_optional(&target.config_path)?;
        let previous_snapshot = read_optional(&snapshot_path)?;
        let mut snapshot = match read_optional(&snapshot_path)? {
            Some(bytes) => parse_snapshot(&snapshot_path, &bytes)?,
            None => capture_mount_snapshot(target, current.as_deref(), url)?,
        };
        if target.format == McpClientFormat::PiExtensionJson
            && snapshot.pi_request_timeout.is_none()
        {
            snapshot.pi_request_timeout = Some(capture_pi_request_timeout(
                current.as_deref(),
                &target.config_path,
            )?);
        }
        let updated = match target.format {
            McpClientFormat::ClaudeJson => update_claude_json(
                current.as_deref(),
                &target.config_path,
                client_id,
                url,
                token,
                target.stdio_command.as_deref(),
            )?,
            McpClientFormat::ClaudeDesktopJson => update_claude_desktop_json(
                current.as_deref(),
                &target.config_path,
                client_id,
                url,
                token,
                target.stdio_command.as_deref(),
            )?,
            McpClientFormat::CodexToml => update_codex_toml(
                current.as_deref(),
                &target.config_path,
                client_id,
                url,
                token,
            )?,
            McpClientFormat::GeminiJson => update_remote_json(
                current.as_deref(),
                &target.config_path,
                client_id,
                url,
                token,
                JsonMcpShape::Gemini,
            )?,
            McpClientFormat::OpenCodeJson => update_remote_json(
                current.as_deref(),
                &target.config_path,
                client_id,
                url,
                token,
                JsonMcpShape::OpenCode,
            )?,
            McpClientFormat::KimiJson => update_remote_json(
                current.as_deref(),
                &target.config_path,
                client_id,
                url,
                token,
                JsonMcpShape::Kimi,
            )?,
            McpClientFormat::PiExtensionJson => update_remote_json(
                current.as_deref(),
                &target.config_path,
                client_id,
                url,
                token,
                JsonMcpShape::PiExtension,
            )?,
            McpClientFormat::DshPatchYaml => {
                update_dsh_patch_yaml(current.as_deref(), &target.config_path, url, token)?
            }
            McpClientFormat::Unsupported => unreachable!(),
        };
        let encoded = serde_json::to_vec_pretty(&snapshot)
            .map_err(|error| format!("could not serialize MCP mount snapshot: {error}"))?;
        fs::create_dir_all(&self.snapshot_root).map_err(|error| {
            format!(
                "could not create MCP snapshot directory {}: {error}",
                self.snapshot_root.display()
            )
        })?;
        crate::storage::atomic_replace(&snapshot_path, &encoded)
            .map_err(|error| format!("could not write {}: {error}", snapshot_path.display()))?;
        if let Err(error) = crate::storage::atomic_replace(&target.config_path, &updated)
            .map_err(|error| format!("could not write {}: {error}", target.config_path.display()))
        {
            let _ = restore_optional(&target.config_path, current.as_deref());
            let _ = fs::remove_file(&snapshot_path);
            return Err(error);
        }
        if read_optional(&target.config_path)?.as_deref() != Some(updated.as_slice()) {
            return Err(format!(
                "MCP mount verification failed: {}",
                target.config_path.display()
            ));
        }
        if let Err(error) = self.mount_route_hook(target) {
            let restore_config = restore_optional(&target.config_path, current.as_deref());
            let restore_snapshot = restore_optional(&snapshot_path, previous_snapshot.as_deref());
            return Err(combine_errors(error, restore_config, restore_snapshot));
        }
        Ok(())
    }

    pub fn credential(&self, client_id: &str) -> Result<String, String> {
        let target = self.target(client_id)?;
        let path = self.credential_path(client_id);
        match fs::read_to_string(&path) {
            Ok(token) => {
                validate_token(&token)?;
                Ok(token)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let token =
                    mounted_credential(target)?.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                fs::create_dir_all(&self.snapshot_root).map_err(|error| {
                    format!(
                        "could not create MCP credential directory {}: {error}",
                        self.snapshot_root.display()
                    )
                })?;
                crate::storage::atomic_replace(&path, token.as_bytes())
                    .map_err(|error| format!("could not write MCP credential: {error}"))?;
                Ok(token)
            }
            Err(error) => Err(format!("could not read MCP credential: {error}")),
        }
    }

    pub fn unmount(&self, client_id: &str) -> Result<(), String> {
        self.unmount_inner(client_id, true)
    }

    pub fn unmount_preserving_credential(&self, client_id: &str) -> Result<(), String> {
        self.unmount_inner(client_id, false)
    }

    fn unmount_inner(&self, client_id: &str, remove_credential: bool) -> Result<(), String> {
        let target = self.target(client_id)?;
        let snapshot_path = self.snapshot_path(client_id);
        let credential_path = self.credential_path(client_id);
        let mut backups = vec![
            (
                target.config_path.clone(),
                read_optional(&target.config_path)?,
            ),
            (snapshot_path.clone(), read_optional(&snapshot_path)?),
            (credential_path.clone(), read_optional(&credential_path)?),
        ];
        if let Some(settings_path) = target.claude_route_hook_settings.as_deref() {
            let hook_snapshot_path = self.route_hook_snapshot_path(client_id);
            backups.push((settings_path.to_path_buf(), read_optional(settings_path)?));
            backups.push((
                hook_snapshot_path.clone(),
                read_optional(&hook_snapshot_path)?,
            ));
        }

        let result = (|| {
            self.unmount_route_hook(target)?;
            let Some(bytes) = read_optional(&snapshot_path)? else {
                if remove_credential {
                    remove_optional_file(&credential_path, "MCP credential")?;
                }
                return Ok(());
            };
            let snapshot = parse_snapshot(&snapshot_path, &bytes)?;
            let current = read_optional(&target.config_path)?;
            let expected = remove_mount(target, current.as_deref(), &snapshot)?;
            if current != expected {
                restore_optional(&target.config_path, expected.as_deref())?;
            }
            if read_optional(&target.config_path)? != expected {
                return Err(format!(
                    "MCP unmount verification failed: {}",
                    target.config_path.display()
                ));
            }
            if remove_credential {
                remove_optional_file(&credential_path, "MCP credential")?;
            }
            fs::remove_file(&snapshot_path).map_err(|error| {
                format!(
                    "could not remove MCP mount snapshot {}: {error}",
                    snapshot_path.display()
                )
            })
        })();

        match result {
            Ok(()) => Ok(()),
            Err(error) => Err(rollback_files(error, &backups)),
        }
    }

    pub fn is_mounted(&self, client_id: &str) -> Result<bool, String> {
        Ok(self.status(client_id)?.mounted)
    }

    pub fn status(&self, client_id: &str) -> Result<McpMountStatus, String> {
        let target = self.target(client_id)?;
        let snapshot_path = self.snapshot_path(client_id);
        let Some(bytes) = read_optional(&snapshot_path)? else {
            return Ok(McpMountStatus {
                mounted: false,
                configuration_changed: false,
            });
        };
        let snapshot = parse_snapshot(&snapshot_path, &bytes)?;
        let current = read_optional(&target.config_path)?;
        let mounted = if target.format == McpClientFormat::DshPatchYaml {
            dsh_block_field(current.as_deref(), "url").as_deref() == Some(snapshot.mounted_url.as_str())
        } else if target.format == McpClientFormat::CodexToml {
            let document = parse_toml(current.as_deref(), &target.config_path)?;
            let server = document
                .get("mcp_servers")
                .and_then(toml_edit::Item::as_table_like)
                .and_then(|servers| servers.get(&server_name(client_id)));
            // The tool list is checked as well as the url: a list written before
            // a tool existed keeps the url intact while leaving the client
            // unable to collect what it starts.
            server
                .and_then(|entry| entry.get("url"))
                .and_then(toml_edit::Item::as_str)
                == Some(snapshot.mounted_url.as_str())
                && server
                    .and_then(|entry| entry.get("enabled_tools"))
                    .and_then(toml_edit::Item::as_array)
                    .is_some_and(|tools| {
                        tools
                            .iter()
                            .filter_map(toml_edit::Value::as_str)
                            .eq(crate::gateway::AGENT_MCP_TOOLS)
                    })
        } else {
            let root = parse_json_object(current.as_deref(), &target.config_path)?;
            root.get(json_section(target.format))
                .and_then(Value::as_object)
                .and_then(|servers| servers.get(&server_name(client_id)))
                .and_then(|entry| mounted_json_url(target.format, entry))
                == Some(snapshot.mounted_url.as_str())
        };
        let hook_mounted = self.route_hook_is_mounted(target)?;
        let pi_timeout_mounted = if target.format == McpClientFormat::PiExtensionJson {
            pi_request_timeout_is_mounted(current.as_deref(), &target.config_path)?
        } else {
            true
        };
        let mounted = mounted && hook_mounted && pi_timeout_mounted;
        Ok(McpMountStatus {
            mounted,
            configuration_changed: !mounted,
        })
    }

    pub fn supported_clients(&self) -> Vec<String> {
        let mut clients = self
            .targets
            .values()
            .filter(|target| target.format != McpClientFormat::Unsupported)
            .map(|target| target.client_id.clone())
            .collect::<Vec<_>>();
        clients.sort();
        clients
    }

    fn target(&self, client_id: &str) -> Result<&McpMountTarget, String> {
        self.targets
            .get(client_id)
            .ok_or_else(|| format!("unknown MCP mount client: {client_id}"))
    }

    fn snapshot_path(&self, client_id: &str) -> PathBuf {
        self.snapshot_root.join(format!("mcp-{client_id}.json"))
    }

    fn credential_path(&self, client_id: &str) -> PathBuf {
        self.snapshot_root.join(format!("mcp-{client_id}.token"))
    }

    fn route_hook_snapshot_path(&self, client_id: &str) -> PathBuf {
        self.snapshot_root
            .join(format!("mcp-{client_id}-route-hook.json"))
    }

    fn mount_route_hook(&self, target: &McpMountTarget) -> Result<(), String> {
        let Some(settings_path) = target.claude_route_hook_settings.as_deref() else {
            return Ok(());
        };
        let executable = resolve_stdio_command(target.stdio_command.as_deref(), "Claude Code")?;
        let command = route_hook_command(&executable);
        let current = read_optional(settings_path)?;
        let snapshot_path = self.route_hook_snapshot_path(&target.client_id);
        let previous_snapshot = read_optional(&snapshot_path)?;
        let snapshot = match previous_snapshot.as_deref() {
            Some(bytes) => parse_route_hook_snapshot(&snapshot_path, bytes)?,
            None => capture_route_hook_snapshot(current.as_deref(), settings_path)?,
        };
        let updated = update_route_hook(current.as_deref(), settings_path, &command)?;
        let encoded = serde_json::to_vec_pretty(&snapshot)
            .map_err(|error| format!("could not serialize Claude route hook snapshot: {error}"))?;
        fs::create_dir_all(&self.snapshot_root).map_err(|error| {
            format!(
                "could not create MCP snapshot directory {}: {error}",
                self.snapshot_root.display()
            )
        })?;
        crate::storage::atomic_replace(&snapshot_path, &encoded).map_err(|error| {
            format!(
                "could not write Claude route hook snapshot {}: {error}",
                snapshot_path.display()
            )
        })?;
        if let Err(error) = crate::storage::atomic_replace(settings_path, &updated)
            .map_err(|error| format!("could not write {}: {error}", settings_path.display()))
        {
            let _ = restore_optional(&snapshot_path, previous_snapshot.as_deref());
            return Err(error);
        }
        if !route_hook_is_present(
            read_optional(settings_path)?.as_deref(),
            settings_path,
            &command,
        )? {
            let _ = restore_optional(settings_path, current.as_deref());
            let _ = restore_optional(&snapshot_path, previous_snapshot.as_deref());
            return Err(format!(
                "Claude route hook verification failed: {}",
                settings_path.display()
            ));
        }
        Ok(())
    }

    fn unmount_route_hook(&self, target: &McpMountTarget) -> Result<(), String> {
        let Some(settings_path) = target.claude_route_hook_settings.as_deref() else {
            return Ok(());
        };
        let snapshot_path = self.route_hook_snapshot_path(&target.client_id);
        let Some(snapshot_bytes) = read_optional(&snapshot_path)? else {
            return Ok(());
        };
        let snapshot = parse_route_hook_snapshot(&snapshot_path, &snapshot_bytes)?;
        let executable = resolve_stdio_command(target.stdio_command.as_deref(), "Claude Code")?;
        let command = route_hook_command(&executable);
        let current = read_optional(settings_path)?;
        let expected = remove_route_hook(current.as_deref(), settings_path, &snapshot, &command)?;
        if current != expected {
            restore_optional(settings_path, expected.as_deref())?;
        }
        if read_optional(settings_path)? != expected {
            return Err(format!(
                "Claude route hook removal verification failed: {}",
                settings_path.display()
            ));
        }
        fs::remove_file(&snapshot_path).map_err(|error| {
            format!(
                "could not remove Claude route hook snapshot {}: {error}",
                snapshot_path.display()
            )
        })
    }

    fn route_hook_is_mounted(&self, target: &McpMountTarget) -> Result<bool, String> {
        let Some(settings_path) = target.claude_route_hook_settings.as_deref() else {
            return Ok(true);
        };
        let executable = resolve_stdio_command(target.stdio_command.as_deref(), "Claude Code")?;
        route_hook_is_present(
            read_optional(settings_path)?.as_deref(),
            settings_path,
            &route_hook_command(&executable),
        )
    }
}

fn combine_errors(
    primary: String,
    first_restore: Result<(), String>,
    second_restore: Result<(), String>,
) -> String {
    let restores = [first_restore.err(), second_restore.err()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if restores.is_empty() {
        primary
    } else {
        format!("{primary}; rollback failed: {}", restores.join("; "))
    }
}

fn rollback_files(primary: String, backups: &[(PathBuf, Option<Vec<u8>>)]) -> String {
    let failures = backups
        .iter()
        .rev()
        .filter_map(|(path, bytes)| restore_optional(path, bytes.as_deref()).err())
        .collect::<Vec<_>>();
    if failures.is_empty() {
        primary
    } else {
        format!("{primary}; rollback failed: {}", failures.join("; "))
    }
}

fn remove_optional_file(path: &Path, label: &str) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not remove {label} {}: {error}",
            path.display()
        )),
    }
}

fn mounted_credential(target: &McpMountTarget) -> Result<Option<String>, String> {
    let current = read_optional(&target.config_path)?;
    let name = server_name(&target.client_id);
    if target.format == McpClientFormat::DshPatchYaml {
        return Ok(dsh_block_field(current.as_deref(), "Authorization")
            .and_then(|value| value.strip_prefix("Bearer ").map(str::to_string)));
    }
    let token = if target.format == McpClientFormat::CodexToml {
        parse_toml(current.as_deref(), &target.config_path)?
            .get("mcp_servers")
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|servers| servers.get(&name))
            .and_then(|entry| entry.get("http_headers"))
            .and_then(|headers| headers.get("Authorization"))
            .and_then(toml_edit::Item::as_str)
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::to_string)
    } else {
        let root = parse_json_object(current.as_deref(), &target.config_path)?;
        let entry = root
            .get(json_section(target.format))
            .and_then(Value::as_object)
            .and_then(|servers| servers.get(&name));
        if matches!(
            target.format,
            McpClientFormat::ClaudeJson | McpClientFormat::ClaudeDesktopJson
        ) {
            entry
                .and_then(|entry| entry.pointer("/env/GRILLFORGE_MCP_TOKEN"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    entry
                        .and_then(|entry| entry.pointer("/headers/Authorization"))
                        .and_then(Value::as_str)
                        .and_then(|value| value.strip_prefix("Bearer "))
                        .map(str::to_string)
                })
        } else {
            entry
                .and_then(|entry| entry.pointer("/headers/Authorization"))
                .and_then(Value::as_str)
                .and_then(|value| value.strip_prefix("Bearer "))
                .map(str::to_string)
        }
    };
    token
        .map(|token| validate_token(&token).map(|()| token))
        .transpose()
}

fn update_claude_json(
    current: Option<&[u8]>,
    path: &Path,
    client_id: &str,
    url: &str,
    token: &str,
    stdio_command: Option<&Path>,
) -> Result<Vec<u8>, String> {
    let stdio_command = resolve_stdio_command(stdio_command, "Claude Code")?;
    let mut root = parse_json_object(current, path)?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("mcpServers must be an object: {}", path.display()))?;
    let name = server_name(client_id);
    if servers.get(&name).is_some_and(|existing| {
        mounted_json_url(McpClientFormat::ClaudeJson, existing) != Some(url)
    }) {
        return Err(format!(
            "refusing to overwrite non-GrillForge MCP server: {name}"
        ));
    }
    servers.insert(
        name,
        json!({
            "command": &stdio_command,
            "args": ["mcp-stdio"],
            "env": {
                "GRILLFORGE_MCP_URL": url,
                "GRILLFORGE_MCP_TOKEN": token
            },
            "alwaysLoad": true,
        }),
    );
    serde_json::to_vec_pretty(&Value::Object(root))
        .map_err(|error| format!("could not serialize {}: {error}", path.display()))
}

fn update_claude_desktop_json(
    current: Option<&[u8]>,
    path: &Path,
    client_id: &str,
    url: &str,
    token: &str,
    stdio_command: Option<&Path>,
) -> Result<Vec<u8>, String> {
    let stdio_command = resolve_stdio_command(stdio_command, "Claude Client")?;
    let mut root = parse_json_object(current, path)?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("mcpServers must be an object: {}", path.display()))?;
    let name = server_name(client_id);
    if servers.get(&name).is_some_and(|existing| {
        mounted_json_url(McpClientFormat::ClaudeDesktopJson, existing) != Some(url)
            && legacy_claude_desktop_url(existing) != Some(url)
    }) {
        return Err(format!(
            "refusing to overwrite non-GrillForge MCP server: {name}"
        ));
    }
    servers.insert(
        name,
        json!({
            "command": &stdio_command,
            "args": ["mcp-stdio"],
            "env": {
                "GRILLFORGE_MCP_URL": url,
                "GRILLFORGE_MCP_TOKEN": token
            }
        }),
    );
    serde_json::to_vec_pretty(&Value::Object(root))
        .map_err(|error| format!("could not serialize {}: {error}", path.display()))
}

fn resolve_stdio_command(command: Option<&Path>, client: &str) -> Result<PathBuf, String> {
    let command = match command {
        Some(command) => command.to_path_buf(),
        None => std::env::current_exe()
            .map_err(|error| format!("could not resolve the GrillForge executable: {error}"))?,
    };
    if !command.is_absolute() {
        return Err(format!(
            "{client} MCP stdio command must be an absolute path"
        ));
    }
    Ok(command)
}

fn route_hook_command(executable: &Path) -> String {
    let path = executable.to_string_lossy();
    let executable = if path
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'.'))
    {
        path.into_owned()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    };
    format!("{executable} claude-route-hook")
}

fn capture_route_hook_snapshot(
    current: Option<&[u8]>,
    path: &Path,
) -> Result<RouteHookSnapshot, String> {
    let root = parse_json_object(current, path)?;
    let hooks = root.get("hooks").and_then(Value::as_object);
    Ok(RouteHookSnapshot {
        version: 1,
        file_existed: current.is_some(),
        hooks_existed: hooks.is_some(),
        pre_tool_use_existed: hooks.is_some_and(|hooks| hooks.contains_key("PreToolUse")),
    })
}

fn parse_route_hook_snapshot(path: &Path, bytes: &[u8]) -> Result<RouteHookSnapshot, String> {
    let snapshot: RouteHookSnapshot = serde_json::from_slice(bytes).map_err(|error| {
        format!(
            "invalid Claude route hook snapshot {}: {error}",
            path.display()
        )
    })?;
    if snapshot.version != 1 {
        return Err(format!(
            "unsupported Claude route hook snapshot version: {}",
            snapshot.version
        ));
    }
    Ok(snapshot)
}

fn update_route_hook(
    current: Option<&[u8]>,
    path: &Path,
    command: &str,
) -> Result<Vec<u8>, String> {
    let mut root = parse_json_object(current, path)?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("hooks must be an object: {}", path.display()))?;
    let pre_tool_use = hooks
        .entry("PreToolUse")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| format!("hooks.PreToolUse must be an array: {}", path.display()))?;
    if !pre_tool_use
        .iter()
        .any(|entry| route_hook_entry_contains(entry, command))
    {
        pre_tool_use.push(json!({
            "matcher": "Workflow|Agent",
            "hooks": [{
                "type": "command",
                "command": command,
                "timeout": 10
            }]
        }));
    }
    serde_json::to_vec_pretty(&Value::Object(root))
        .map_err(|error| format!("could not serialize {}: {error}", path.display()))
}

fn route_hook_entry_contains(entry: &Value, command: &str) -> bool {
    entry.get("matcher").and_then(Value::as_str) == Some("Workflow|Agent")
        && entry
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|hooks| {
                hooks.iter().any(|hook| {
                    hook.get("type").and_then(Value::as_str) == Some("command")
                        && hook.get("command").and_then(Value::as_str) == Some(command)
                })
            })
}

fn route_hook_is_present(
    current: Option<&[u8]>,
    path: &Path,
    command: &str,
) -> Result<bool, String> {
    let root = parse_json_object(current, path)?;
    Ok(root
        .get("hooks")
        .and_then(Value::as_object)
        .and_then(|hooks| hooks.get("PreToolUse"))
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| route_hook_entry_contains(entry, command))
        }))
}

fn remove_route_hook(
    current: Option<&[u8]>,
    path: &Path,
    snapshot: &RouteHookSnapshot,
    command: &str,
) -> Result<Option<Vec<u8>>, String> {
    let mut root = parse_json_object(current, path)?;
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(current.map(<[u8]>::to_vec));
    };
    let Some(pre_tool_use) = hooks.get_mut("PreToolUse").and_then(Value::as_array_mut) else {
        return Ok(current.map(<[u8]>::to_vec));
    };
    for entry in pre_tool_use.iter_mut() {
        if entry.get("matcher").and_then(Value::as_str) != Some("Workflow|Agent") {
            continue;
        }
        let Some(commands) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
            continue;
        };
        commands.retain(|hook| {
            !(hook.get("type").and_then(Value::as_str) == Some("command")
                && hook.get("command").and_then(Value::as_str) == Some(command))
        });
    }
    pre_tool_use.retain(|entry| {
        entry
            .get("hooks")
            .and_then(Value::as_array)
            .is_none_or(|hooks| !hooks.is_empty())
    });
    if !snapshot.pre_tool_use_existed && pre_tool_use.is_empty() {
        hooks.remove("PreToolUse");
    }
    if !snapshot.hooks_existed && hooks.is_empty() {
        root.remove("hooks");
    }
    if !snapshot.file_existed && root.is_empty() {
        return Ok(None);
    }
    serde_json::to_vec_pretty(&Value::Object(root))
        .map(Some)
        .map_err(|error| format!("could not serialize {}: {error}", path.display()))
}

const DSH_BLOCK_START: &str = "# >>> grillforge mcp (managed)";
const DSH_BLOCK_END: &str = "# <<< grillforge mcp";

/// Rewrites the single GrillForge block in the harness user layer, leaving every
/// entry the user wrote around it untouched.
fn update_dsh_patch_yaml(
    current: Option<&[u8]>,
    path: &Path,
    url: &str,
    token: &str,
) -> Result<Vec<u8>, String> {
    let kept = strip_dsh_block(current)?;
    let mut out = kept;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(DSH_BLOCK_START);
    out.push('\n');
    // The MCP client is not in the base profile, so it is inserted rather than
    // patched: a patch entry can only target an id that already exists.
    out.push_str("- insert:\n");
    out.push_str("    - id: grillforge-mcp\n");
    out.push_str("      name: '@deepseek-ai/dsh-mcp-client'\n");
    out.push_str("      config:\n");
    out.push_str("        transport: streamable-http\n");
    out.push_str("        serverName: grillforge\n");
    out.push_str(&format!("        url: {}\n", yaml_quote(url)));
    out.push_str("        headers:\n");
    out.push_str(&format!(
        "          Authorization: {}\n",
        yaml_quote(&format!("Bearer {token}"))
    ));
    out.push_str(DSH_BLOCK_END);
    out.push('\n');
    let _ = path;
    Ok(out.into_bytes())
}

fn strip_dsh_block(current: Option<&[u8]>) -> Result<String, String> {
    let Some(current) = current else {
        return Ok(String::new());
    };
    let text = std::str::from_utf8(current)
        .map_err(|_| "DeepSeek Harness patch layer is not UTF-8".to_string())?;
    let mut kept = String::new();
    let mut skipping = false;
    for line in text.lines() {
        if line.trim_start().starts_with(DSH_BLOCK_START) {
            skipping = true;
            continue;
        }
        if line.trim_start().starts_with(DSH_BLOCK_END) {
            skipping = false;
            continue;
        }
        // The harness writes `[]` for an empty layer; a real entry replaces it.
        if skipping || line.trim() == "[]" {
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    Ok(kept)
}

fn dsh_block_field(current: Option<&[u8]>, key: &str) -> Option<String> {
    let text = std::str::from_utf8(current?).ok()?;
    let mut inside = false;
    for line in text.lines() {
        if line.trim_start().starts_with(DSH_BLOCK_START) {
            inside = true;
            continue;
        }
        if line.trim_start().starts_with(DSH_BLOCK_END) {
            inside = false;
            continue;
        }
        if !inside {
            continue;
        }
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(key) {
            let rest = rest.trim_start().trim_start_matches(':').trim();
            return Some(rest.trim_matches('"').to_string());
        }
    }
    None
}

fn yaml_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn capture_mount_snapshot(
    target: &McpMountTarget,
    current: Option<&[u8]>,
    mounted_url: &str,
) -> Result<MountSnapshot, String> {
    let name = server_name(&target.client_id);
    // The harness layer is a YAML list, not a keyed object; the whole file is the
    // only faithful record of what was there before.
    if target.format == McpClientFormat::DshPatchYaml {
        return Ok(MountSnapshot {
            version: SNAPSHOT_VERSION,
            mounted_url: mounted_url.to_string(),
            entry: MountEntrySnapshot {
                file_existed: current.is_some(),
                section_existed: current.is_some(),
                original_json: None,
                original_toml: current.map(<[u8]>::to_vec),
            },
            pi_request_timeout: None,
        });
    }
    let entry = if target.format == McpClientFormat::CodexToml {
        let document = parse_toml(current, &target.config_path)?;
        let section = document.get("mcp_servers");
        MountEntrySnapshot {
            file_existed: current.is_some(),
            section_existed: section.is_some(),
            original_json: None,
            original_toml: section
                .and_then(toml_edit::Item::as_table_like)
                .and_then(|servers| servers.get(&name))
                .map(|item| item.to_string().into_bytes()),
        }
    } else {
        let root = parse_json_object(current, &target.config_path)?;
        let section = json_section(target.format);
        let servers = root.get(section).and_then(Value::as_object);
        let original_json = servers.and_then(|servers| servers.get(&name).cloned());
        let original_json = if target.format == McpClientFormat::ClaudeDesktopJson
            && original_json.as_ref().and_then(legacy_claude_desktop_url) == Some(mounted_url)
        {
            None
        } else {
            original_json
        };
        MountEntrySnapshot {
            file_existed: current.is_some(),
            section_existed: root.contains_key(section),
            original_json,
            original_toml: None,
        }
    };
    Ok(MountSnapshot {
        version: SNAPSHOT_VERSION,
        mounted_url: mounted_url.to_string(),
        entry,
        pi_request_timeout: if target.format == McpClientFormat::PiExtensionJson {
            Some(capture_pi_request_timeout(current, &target.config_path)?)
        } else {
            None
        },
    })
}

fn remove_mount(
    target: &McpMountTarget,
    current: Option<&[u8]>,
    snapshot: &MountSnapshot,
) -> Result<Option<Vec<u8>>, String> {
    if target.format == McpClientFormat::DshPatchYaml {
        let stripped = strip_dsh_block(current)?;
        // Nothing of the user's remains and nothing was there before: leave no file.
        if stripped.trim().is_empty() && !snapshot.entry.file_existed {
            return Ok(None);
        }
        return Ok(Some(stripped.into_bytes()));
    }
    if target.format == McpClientFormat::CodexToml {
        return remove_codex_mount(
            current,
            &target.config_path,
            &target.client_id,
            &snapshot.entry,
            &snapshot.mounted_url,
        );
    }
    let path = &target.config_path;
    let mut root = parse_json_object(current, path)?;
    let section = json_section(target.format);
    if let Some(section_value) = root.get_mut(section) {
        let servers = section_value
            .as_object_mut()
            .ok_or_else(|| format!("{section} must be an object: {}", path.display()))?;
        let name = server_name(&target.client_id);
        let owned = servers
            .get(&name)
            .and_then(|entry| mounted_json_url(target.format, entry))
            == Some(snapshot.mounted_url.as_str());
        if owned {
            match &snapshot.entry.original_json {
                Some(original) => {
                    servers.insert(name, original.clone());
                }
                None => {
                    servers.remove(&name);
                }
            }
            if !snapshot.entry.section_existed && servers.is_empty() {
                root.remove(section);
            }
        }
    }
    if target.format == McpClientFormat::PiExtensionJson {
        restore_pi_request_timeout(&mut root, snapshot.pi_request_timeout.as_ref(), path)?;
    }
    if !snapshot.entry.file_existed && root.is_empty() {
        return Ok(None);
    }
    serde_json::to_vec_pretty(&Value::Object(root))
        .map(Some)
        .map_err(|error| format!("could not serialize {}: {error}", path.display()))
}

fn remove_codex_mount(
    current: Option<&[u8]>,
    path: &Path,
    client_id: &str,
    snapshot: &MountEntrySnapshot,
    mounted_url: &str,
) -> Result<Option<Vec<u8>>, String> {
    let mut document = parse_toml(current, path)?;
    let name = server_name(client_id);
    let Some(section) = document.get_mut("mcp_servers") else {
        return Ok(current.map(<[u8]>::to_vec));
    };
    let servers = section
        .as_table_like_mut()
        .ok_or_else(|| format!("mcp_servers must be a table: {}", path.display()))?;
    let owned = servers
        .get(&name)
        .and_then(|entry| entry.get("url"))
        .and_then(toml_edit::Item::as_str)
        == Some(mounted_url);
    if !owned {
        return Ok(current.map(<[u8]>::to_vec));
    }
    match snapshot.original_toml.as_deref() {
        Some(bytes) => {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| format!("invalid MCP snapshot entry: {}", path.display()))?;
            let item = text.parse::<toml_edit::Item>().map_err(|error| {
                format!("invalid MCP snapshot entry {}: {error}", path.display())
            })?;
            servers.insert(&name, item);
        }
        None => {
            servers.remove(&name);
        }
    }
    if !snapshot.section_existed && servers.is_empty() {
        document.remove("mcp_servers");
    }
    if !snapshot.file_existed && document.is_empty() {
        return Ok(None);
    }
    Ok(Some(document.to_string().into_bytes()))
}

fn mounted_json_url(format: McpClientFormat, entry: &Value) -> Option<&str> {
    if matches!(
        format,
        McpClientFormat::ClaudeJson | McpClientFormat::ClaudeDesktopJson
    ) {
        return entry
            .pointer("/env/GRILLFORGE_MCP_URL")
            .and_then(Value::as_str)
            .or_else(|| entry.get("url").and_then(Value::as_str));
    }
    let key = match format {
        McpClientFormat::GeminiJson => "httpUrl",
        McpClientFormat::OpenCodeJson
        | McpClientFormat::KimiJson
        | McpClientFormat::PiExtensionJson => "url",
        McpClientFormat::ClaudeJson
        | McpClientFormat::ClaudeDesktopJson
        | McpClientFormat::CodexToml
        | McpClientFormat::DshPatchYaml
        | McpClientFormat::Unsupported => return None,
    };
    entry.get(key).and_then(Value::as_str)
}

fn legacy_claude_desktop_url(entry: &Value) -> Option<&str> {
    (entry.get("transport").and_then(Value::as_str) == Some("http"))
        .then(|| entry.get("url").and_then(Value::as_str))
        .flatten()
}

fn json_section(format: McpClientFormat) -> &'static str {
    match format {
        McpClientFormat::ClaudeJson
        | McpClientFormat::ClaudeDesktopJson
        | McpClientFormat::GeminiJson
        | McpClientFormat::KimiJson
        | McpClientFormat::PiExtensionJson => "mcpServers",
        McpClientFormat::OpenCodeJson => "mcp",
        McpClientFormat::CodexToml
        | McpClientFormat::DshPatchYaml
        | McpClientFormat::Unsupported => unreachable!(),
    }
}

fn parse_toml(current: Option<&[u8]>, path: &Path) -> Result<toml_edit::DocumentMut, String> {
    let text = match current {
        None => String::new(),
        Some(bytes) => std::str::from_utf8(bytes)
            .map_err(|_| format!("configuration is not UTF-8: {}", path.display()))?
            .to_string(),
    };
    text.parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("invalid TOML {}: {error}", path.display()))
}

fn update_codex_toml(
    current: Option<&[u8]>,
    path: &Path,
    client_id: &str,
    url: &str,
    token: &str,
) -> Result<Vec<u8>, String> {
    let mut document = parse_toml(current, path)?;
    if document
        .get_mut("mcp_servers")
        .and_then(toml_edit::Item::as_table_like_mut)
        .is_none()
    {
        if document
            .get("mcp_servers")
            .is_some_and(|item| !item.is_none())
        {
            return Err(format!("mcp_servers must be a table: {}", path.display()));
        }
        document["mcp_servers"] = toml_edit::table();
    }
    let name = server_name(client_id);
    let servers = document
        .get_mut("mcp_servers")
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or_else(|| format!("mcp_servers must be a table: {}", path.display()))?;
    if let Some(existing) = servers.get(&name) {
        let owned = existing.get("url").and_then(toml_edit::Item::as_str) == Some(url);
        if !owned {
            return Err(format!(
                "refusing to overwrite non-GrillForge MCP server: {name}"
            ));
        }
    }
    let mut server = toml_edit::Table::new();
    server["url"] = toml_edit::value(url);
    server["enabled"] = toml_edit::value(true);
    server["required"] = toml_edit::value(true);
    let mut enabled_tools = toml_edit::Array::new();
    // Every tool, not a chosen few: a client allowed to start runs but not to
    // collect them orphans every result it asks for.
    for tool in crate::gateway::AGENT_MCP_TOOLS {
        enabled_tools.push(tool);
    }
    server["enabled_tools"] = toml_edit::value(enabled_tools);
    let mut omitted_surfaces = toml_edit::Array::new();
    omitted_surfaces.push("deferred");
    omitted_surfaces.push("code_mode");
    server["omit_tools_from"] = toml_edit::value(omitted_surfaces);
    server["default_tools_approval_mode"] = toml_edit::value("approve");
    let mut headers = toml_edit::InlineTable::new();
    headers.insert("Authorization", format!("Bearer {token}").into());
    server["http_headers"] = toml_edit::value(headers);
    servers.insert(&name, toml_edit::Item::Table(server));
    Ok(document.to_string().into_bytes())
}

#[derive(Clone, Copy)]
enum JsonMcpShape {
    Gemini,
    OpenCode,
    Kimi,
    PiExtension,
}

fn update_remote_json(
    current: Option<&[u8]>,
    path: &Path,
    client_id: &str,
    url: &str,
    token: &str,
    shape: JsonMcpShape,
) -> Result<Vec<u8>, String> {
    let mut root = parse_json_object(current, path)?;
    if matches!(shape, JsonMcpShape::PiExtension) {
        set_pi_request_timeout(&mut root, path)?;
    }
    let (section, url_key) = match shape {
        JsonMcpShape::Gemini => ("mcpServers", "httpUrl"),
        JsonMcpShape::OpenCode => ("mcp", "url"),
        JsonMcpShape::Kimi => ("mcpServers", "url"),
        JsonMcpShape::PiExtension => ("mcpServers", "url"),
    };
    let servers = root
        .entry(section)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("{section} must be an object: {}", path.display()))?;
    let name = server_name(client_id);
    let existing_url = servers.get(&name).and_then(|entry| {
        entry
            .get(url_key)
            .or_else(|| entry.get("url"))
            .and_then(Value::as_str)
    });
    if existing_url.is_some_and(|existing| existing != url) {
        return Err(format!(
            "refusing to overwrite non-GrillForge MCP server: {name}"
        ));
    }
    let entry = match shape {
        JsonMcpShape::Gemini => json!({
            "httpUrl": url,
            "headers": {"Authorization": format!("Bearer {token}")}
        }),
        JsonMcpShape::OpenCode => json!({
            "type": "remote",
            "url": url,
            "headers": {"Authorization": format!("Bearer {token}")},
            "enabled": true
        }),
        JsonMcpShape::Kimi => json!({
            "url": url,
            "headers": {"Authorization": format!("Bearer {token}")}
        }),
        JsonMcpShape::PiExtension => json!({
            "transport": "streamable-http",
            "url": url,
            "headers": {"Authorization": format!("Bearer {token}")},
            "lifecycle": "eager"
        }),
    };
    servers.insert(name, entry);
    serde_json::to_vec_pretty(&Value::Object(root))
        .map_err(|error| format!("could not serialize {}: {error}", path.display()))
}

fn capture_pi_request_timeout(
    current: Option<&[u8]>,
    path: &Path,
) -> Result<PiRequestTimeoutSnapshot, String> {
    let root = parse_json_object(current, path)?;
    let settings_existed = root.contains_key("settings");
    let settings = match root.get("settings") {
        Some(value) => Some(
            value
                .as_object()
                .ok_or_else(|| format!("settings must be an object: {}", path.display()))?,
        ),
        None => None,
    };
    let original = settings.and_then(|settings| settings.get("requestTimeoutMs").cloned());
    if let Some(value) = original.as_ref() {
        validate_pi_request_timeout(value, path)?;
    }
    Ok(PiRequestTimeoutSnapshot {
        settings_existed,
        request_timeout_existed: original.is_some(),
        original,
    })
}

fn set_pi_request_timeout(
    root: &mut serde_json::Map<String, Value>,
    path: &Path,
) -> Result<(), String> {
    let settings = root
        .entry("settings")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("settings must be an object: {}", path.display()))?;
    if let Some(value) = settings.get("requestTimeoutMs") {
        validate_pi_request_timeout(value, path)?;
    }
    settings.insert("requestTimeoutMs".into(), PI_REQUEST_TIMEOUT_MS.into());
    Ok(())
}

fn restore_pi_request_timeout(
    root: &mut serde_json::Map<String, Value>,
    snapshot: Option<&PiRequestTimeoutSnapshot>,
    path: &Path,
) -> Result<(), String> {
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    let Some(settings_value) = root.get_mut("settings") else {
        return Ok(());
    };
    let settings = settings_value
        .as_object_mut()
        .ok_or_else(|| format!("settings must be an object: {}", path.display()))?;
    if settings.get("requestTimeoutMs").and_then(Value::as_u64) != Some(PI_REQUEST_TIMEOUT_MS) {
        return Ok(());
    }
    if snapshot.request_timeout_existed {
        let original = snapshot
            .original
            .clone()
            .ok_or_else(|| format!("invalid Pi MCP timeout snapshot for {}", path.display()))?;
        settings.insert("requestTimeoutMs".into(), original);
    } else {
        settings.remove("requestTimeoutMs");
    }
    if !snapshot.settings_existed && settings.is_empty() {
        root.remove("settings");
    }
    Ok(())
}

fn pi_request_timeout_is_mounted(current: Option<&[u8]>, path: &Path) -> Result<bool, String> {
    let root = parse_json_object(current, path)?;
    Ok(root
        .get("settings")
        .and_then(Value::as_object)
        .and_then(|settings| settings.get("requestTimeoutMs"))
        .and_then(Value::as_u64)
        == Some(PI_REQUEST_TIMEOUT_MS))
}

fn validate_pi_request_timeout(value: &Value, path: &Path) -> Result<(), String> {
    if value.as_f64().is_some_and(|timeout| timeout > 0.0) {
        Ok(())
    } else {
        Err(format!(
            "settings.requestTimeoutMs must be a positive number: {}",
            path.display()
        ))
    }
}

fn parse_json_object(
    current: Option<&[u8]>,
    path: &Path,
) -> Result<serde_json::Map<String, Value>, String> {
    match current {
        None => Ok(serde_json::Map::new()),
        Some(bytes) => serde_json::from_slice::<Value>(bytes)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .ok_or_else(|| format!("configuration must be a JSON object: {}", path.display())),
    }
}

fn parse_snapshot(path: &Path, bytes: &[u8]) -> Result<MountSnapshot, String> {
    let snapshot: MountSnapshot = serde_json::from_slice(bytes)
        .map_err(|_| format!("invalid MCP mount snapshot: {}", path.display()))?;
    if snapshot.version != SNAPSHOT_VERSION {
        return Err(format!("invalid MCP mount snapshot: {}", path.display()));
    }
    Ok(snapshot)
}

fn validate_url(client_id: &str, value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|_| format!("invalid MCP URL: {value}"))?;
    let expected_path = format!("/mcp/{client_id}");
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || url.path() != expected_path
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!(
            "MCP URL must be an exact loopback {expected_path} URL"
        ));
    }
    Ok(())
}

pub(crate) fn validate_mcp_url(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|_| format!("invalid MCP URL: {value}"))?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || !url.path().starts_with("/mcp/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("MCP URL must be an exact loopback /mcp/<client> URL".into());
    }
    Ok(())
}

pub(crate) fn validate_token(token: &str) -> Result<(), String> {
    if token.is_empty()
        || token.trim() != token
        || token.chars().any(char::is_control)
        || token.len() > 512
    {
        return Err(
            "MCP token must not be empty, padded, oversized, or contain control characters".into(),
        );
    }
    Ok(())
}

fn server_name(client_id: &str) -> String {
    format!("grillforge-{}", client_id.replace('_', "-"))
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

fn restore_optional(path: &Path, bytes: Option<&[u8]>) -> Result<(), String> {
    match bytes {
        Some(bytes) => crate::storage::atomic_replace(path, bytes)
            .map_err(|error| format!("could not restore {}: {error}", path.display())),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("could not remove {}: {error}", path.display())),
        },
    }
}
