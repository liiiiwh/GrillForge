use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalAgent {
    pub runtime: &'static str,
    pub agent_id: String,
    pub description: String,
}

pub fn discover_claude_code_agents(claude_root: &Path) -> Result<Vec<LocalAgent>, String> {
    let mut agents = discover_agents_in(&claude_root.join("agents"), None)?;
    let enabled_plugins = read_plugin_settings(&claude_root.join("settings.json"))?;
    agents.extend(discover_enabled_plugin_agents(
        claude_root,
        &enabled_plugins,
    )?);
    agents.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
    agents.dedup_by(|left, right| left.agent_id == right.agent_id);
    Ok(agents)
}

pub fn discover_claude_code_agents_for_project(
    claude_root: &Path,
    project_root: &Path,
) -> Result<Vec<LocalAgent>, String> {
    let mut enabled_plugins = read_plugin_settings(&claude_root.join("settings.json"))?;
    enabled_plugins.extend(read_plugin_settings(
        &project_root.join(".claude/settings.json"),
    )?);
    enabled_plugins.extend(read_plugin_settings(
        &project_root.join(".claude/settings.local.json"),
    )?);

    let mut agents = BTreeMap::new();
    for agent in discover_agents_in(&claude_root.join("agents"), None)? {
        agents.insert(agent.agent_id.clone(), agent);
    }
    for agent in discover_enabled_plugin_agents(claude_root, &enabled_plugins)? {
        agents.insert(agent.agent_id.clone(), agent);
    }
    for agent in discover_agents_in(&project_root.join(".claude/agents"), None)? {
        agents.insert(agent.agent_id.clone(), agent);
    }
    Ok(agents.into_values().collect())
}

pub fn discover_codex_agents(codex_root: &Path) -> Result<Vec<LocalAgent>, String> {
    let mut agents = codex_builtin_agents()
        .into_iter()
        .map(|agent| (agent.agent_id.clone(), agent))
        .collect::<BTreeMap<_, _>>();
    for (agent, _) in discover_codex_agents_in(&codex_root.join("agents"))? {
        agents.insert(agent.agent_id.clone(), agent);
    }
    Ok(agents.into_values().collect())
}

pub fn discover_codex_agents_for_project(
    codex_root: &Path,
    project_root: &Path,
) -> Result<Vec<LocalAgent>, String> {
    let mut agents = discover_codex_agents(codex_root)?
        .into_iter()
        .map(|agent| (agent.agent_id.clone(), agent))
        .collect::<BTreeMap<_, _>>();
    for (agent, _) in discover_codex_agents_in(&project_root.join(".codex/agents"))? {
        agents.insert(agent.agent_id.clone(), agent);
    }
    Ok(agents.into_values().collect())
}

pub fn resolve_codex_custom_agent_file(
    codex_root: &Path,
    project_root: &Path,
    agent_id: &str,
) -> Result<Option<PathBuf>, String> {
    for directory in [
        project_root.join(".codex/agents"),
        codex_root.join("agents"),
    ] {
        if let Some((_, path)) = discover_codex_agents_in(&directory)?
            .into_iter()
            .find(|(agent, _)| agent.agent_id == agent_id)
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

pub(crate) fn is_codex_builtin_agent(agent_id: &str) -> bool {
    matches!(agent_id, "default" | "worker" | "explorer")
}

fn codex_builtin_agents() -> Vec<LocalAgent> {
    [
        ("default", "Codex 内建通用 Agent"),
        ("worker", "Codex 内建执行 Agent"),
        ("explorer", "Codex 内建探索 Agent"),
    ]
    .into_iter()
    .map(|(agent_id, description)| LocalAgent {
        runtime: "codex",
        agent_id: agent_id.into(),
        description: description.into(),
    })
    .collect()
}

fn discover_codex_agents_in(directory: &Path) -> Result<Vec<(LocalAgent, PathBuf)>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read {}: {error}", directory.display())),
    };
    let mut agents = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read Codex Agent entry: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("toml") {
            continue;
        }
        let contents = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let Ok(value) = contents.parse::<toml_edit::DocumentMut>() else {
            continue;
        };
        let Some(name) = value.get("name").and_then(toml_edit::Item::as_str) else {
            continue;
        };
        let Some(description) = value.get("description").and_then(toml_edit::Item::as_str) else {
            continue;
        };
        if value
            .get("developer_instructions")
            .and_then(toml_edit::Item::as_str)
            .is_none()
            || !valid_codex_agent_name(name)
            || description.trim().is_empty()
        {
            continue;
        }
        agents.push((
            LocalAgent {
                runtime: "codex",
                agent_id: name.into(),
                description: description.into(),
            },
            path,
        ));
    }
    agents.sort_by(|left, right| left.0.agent_id.cmp(&right.0.agent_id));
    Ok(agents)
}

fn valid_codex_agent_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

pub fn discover_claude_builtin_agents(runtime: &Path) -> Result<Vec<LocalAgent>, String> {
    const PROBE: &str = "grillforge-discovery-probe";
    let mut command = crate::cli_discovery::version_command(runtime)
        .map_err(|error| format!("could not inspect {}: {error}", runtime.display()))?;
    let mut child = command
        .args(["--agent", PROBE, "--print", "--no-session-persistence", ""])
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env("ANTHROPIC_API_KEY", "grillforge-loopback-discovery")
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not inspect {} Agents: {error}", runtime.display()))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child
            .try_wait()
            .map_err(|error| format!("could not inspect {} Agents: {error}", runtime.display()))?
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "Claude Code Agent discovery timed out: {}",
                runtime.display()
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("could not inspect {} Agents: {error}", runtime.display()))?;
    let output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let available = output
        .lines()
        .find_map(|line| {
            line.split_once("Available agents:")
                .map(|(_, agents)| agents)
        })
        .ok_or_else(|| {
            format!(
                "Claude Code did not report callable Agents: {}",
                runtime.display()
            )
        })?;
    let mut agents = available
        .split(',')
        .map(str::trim)
        .filter(|agent_id| {
            !agent_id.is_empty()
                && agent_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b':' | b'_'))
        })
        .map(|agent_id| LocalAgent {
            runtime: "claude_code",
            agent_id: agent_id.to_string(),
            description: format!("Claude Code 内建 Agent · {agent_id}"),
        })
        .collect::<Vec<_>>();
    agents.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
    agents.dedup_by(|left, right| left.agent_id == right.agent_id);
    Ok(agents)
}

fn discover_agents_in(
    directory: &Path,
    plugin_namespace: Option<(&str, &Path)>,
) -> Result<Vec<LocalAgent>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read {}: {error}", directory.display())),
    };
    let mut agents = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read Agent entry: {error}"))?;
        if entry
            .file_type()
            .map_err(|error| format!("could not inspect {}: {error}", entry.path().display()))?
            .is_dir()
        {
            agents.extend(discover_agents_in(&entry.path(), plugin_namespace)?);
            continue;
        }
        if entry.path().extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let contents = fs::read_to_string(entry.path())
            .map_err(|error| format!("could not read {}: {error}", entry.path().display()))?;
        if let Some(mut agent) = parse_claude_agent(&contents) {
            if let Some((plugin_name, plugin_agents_root)) = plugin_namespace {
                let entry_path = entry.path();
                let relative_parent = entry_path
                    .parent()
                    .and_then(|parent| parent.strip_prefix(plugin_agents_root).ok());
                let mut segments = vec![plugin_name.to_string()];
                if let Some(parent) = relative_parent {
                    segments.extend(parent.components().filter_map(|component| {
                        component.as_os_str().to_str().map(ToString::to_string)
                    }));
                }
                segments.push(agent.agent_id);
                agent.agent_id = segments.join(":");
            }
            agents.push(agent);
        }
    }
    agents.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
    Ok(agents)
}

fn discover_enabled_plugin_agents(
    claude_root: &Path,
    plugin_settings: &BTreeMap<String, bool>,
) -> Result<Vec<LocalAgent>, String> {
    if !plugin_settings.values().any(|enabled| *enabled) {
        return Ok(Vec::new());
    }
    let registry_path = claude_root.join("plugins/installed_plugins.json");
    let registry = match fs::read_to_string(&registry_path) {
        Ok(contents) => serde_json::from_str::<serde_json::Value>(&contents)
            .map_err(|error| format!("could not parse {}: {error}", registry_path.display()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "could not read {}: {error}",
                registry_path.display()
            ));
        }
    };
    let mut agents = Vec::new();
    let Some(installed) = registry.get("plugins").and_then(|value| value.as_object()) else {
        return Err(format!(
            "{} does not contain a plugins object",
            registry_path.display()
        ));
    };
    for (plugin_id, installations) in installed {
        if plugin_settings.get(plugin_id) != Some(&true) {
            continue;
        }
        let Some(installations) = installations.as_array() else {
            continue;
        };
        for installation in installations {
            let Some(install_path) = installation
                .get("installPath")
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            let install_path = PathBuf::from(install_path);
            let manifest_path = install_path.join(".claude-plugin/plugin.json");
            let manifest = fs::read_to_string(&manifest_path)
                .map_err(|error| format!("could not read {}: {error}", manifest_path.display()))?;
            let manifest: serde_json::Value = serde_json::from_str(&manifest)
                .map_err(|error| format!("could not parse {}: {error}", manifest_path.display()))?;
            let plugin_name = manifest
                .get("name")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    format!("{} does not define a plugin name", manifest_path.display())
                })?;
            let plugin_agents_root = install_path.join("agents");
            agents.extend(discover_agents_in(
                &plugin_agents_root,
                Some((plugin_name, &plugin_agents_root)),
            )?);
        }
    }
    Ok(agents)
}

fn read_plugin_settings(settings_path: &Path) -> Result<BTreeMap<String, bool>, String> {
    let contents = match fs::read_to_string(settings_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(format!(
                "could not read {}: {error}",
                settings_path.display()
            ));
        }
    };
    let settings: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|error| format!("could not parse {}: {error}", settings_path.display()))?;
    Ok(settings
        .get("enabledPlugins")
        .and_then(|value| value.as_object())
        .into_iter()
        .flat_map(|plugins| plugins.iter())
        .filter_map(|(plugin_id, enabled)| {
            enabled
                .as_bool()
                .map(|enabled| (plugin_id.clone(), enabled))
        })
        .collect())
}

#[tauri::command]
pub async fn discover_local_agents(
    project_root: Option<String>,
) -> Result<Vec<LocalAgent>, String> {
    let home = dirs::home_dir().ok_or_else(|| "home directory is unavailable".to_string())?;
    let claude_root = home.join(".claude");
    let project_root = project_root
        .filter(|project_root| !project_root.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let discovered = match &project_root {
        Some(project_root) => discover_claude_code_agents_for_project(&claude_root, project_root)?,
        None => discover_claude_code_agents(&claude_root)?,
    };
    let mut agents = BTreeMap::new();
    if let Some(runtime) =
        crate::adapters::claude_code::detect_claude_cli().map_err(|error| error.to_string())?
    {
        for agent in discover_claude_builtin_agents(&runtime.path)? {
            agents.insert((agent.runtime, agent.agent_id.clone()), agent);
        }
    }
    for agent in discovered {
        agents.insert((agent.runtime, agent.agent_id.clone()), agent);
    }
    if crate::adapters::codex::detect_codex_cli()
        .map_err(|error| error.to_string())?
        .is_some()
    {
        let codex_root = home.join(".codex");
        let codex_agents = match &project_root {
            Some(project_root) => discover_codex_agents_for_project(&codex_root, project_root)?,
            None => discover_codex_agents(&codex_root)?,
        };
        for agent in codex_agents {
            agents.insert((agent.runtime, agent.agent_id.clone()), agent);
        }
    }
    Ok(agents.into_values().collect())
}

fn parse_claude_agent(contents: &str) -> Option<LocalAgent> {
    let frontmatter = contents.strip_prefix("---\n")?.split_once("\n---\n")?.0;
    let value: serde_yaml::Value = serde_yaml::from_str(frontmatter).ok()?;
    let object = value.as_mapping()?;
    let agent_id = object
        .get(serde_yaml::Value::String("name".into()))?
        .as_str()?
        .to_string();
    let description = object
        .get(serde_yaml::Value::String("description".into()))?
        .as_str()?
        .to_string();
    if agent_id.is_empty()
        || description.trim().is_empty()
        || agent_id
            .bytes()
            .any(|byte| !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
    {
        return None;
    }
    Some(LocalAgent {
        runtime: "claude_code",
        agent_id,
        description,
    })
}
