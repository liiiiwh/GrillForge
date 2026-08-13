use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};
use toml_edit::Item;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalAgent {
    pub runtime: &'static str,
    pub agent_id: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalAgentDiscoveryError {
    pub runtime: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalAgentDiscovery {
    pub agents: Vec<LocalAgent>,
    pub errors: Vec<LocalAgentDiscoveryError>,
}

impl LocalAgentDiscovery {
    pub fn from_runtime_results(
        results: impl IntoIterator<Item = (&'static str, Result<Vec<LocalAgent>, String>)>,
    ) -> Self {
        let mut agents = BTreeMap::new();
        let mut errors = Vec::new();
        for (runtime, result) in results {
            match result {
                Ok(discovered) => {
                    for agent in discovered {
                        agents.insert((agent.runtime, agent.agent_id.clone()), agent);
                    }
                }
                Err(message) => errors.push(LocalAgentDiscoveryError { runtime, message }),
            }
        }
        Self {
            agents: agents.into_values().collect(),
            errors,
        }
    }
}

pub fn discover_grok_build_agents(
    runtime: &Path,
    project_root: &Path,
) -> Result<Vec<LocalAgent>, String> {
    let mut command = crate::cli_discovery::version_command(runtime)
        .map_err(|error| format!("could not inspect {}: {error}", runtime.display()))?;
    let mut child = command
        .current_dir(project_root)
        .args(["inspect", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not inspect {} Agents: {error}", runtime.display()))?;
    let deadline = Instant::now() + Duration::from_secs(8);
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
                "Grok Build Agent inspection timed out: {}",
                runtime.display()
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("could not inspect {} Agents: {error}", runtime.display()))?;
    if !output.status.success() {
        return Err(format!(
            "Grok Build Agent inspection exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Grok Build Agent inspection returned invalid JSON".to_string())?;
    let reported = report
        .get("agents")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Grok Build Agent inspection did not return agents".to_string())?;
    let mut agents = Vec::with_capacity(reported.len());
    for agent in reported {
        let agent_id = agent
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                "Grok Build Agent inspection returned an Agent without name".to_string()
            })?;
        let description = agent
            .get("description")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                format!("Grok Build Agent inspection returned {agent_id} without description")
            })?;
        if !valid_grok_build_agent_name(agent_id) || description.trim().is_empty() {
            return Err(format!(
                "Grok Build Agent inspection returned an invalid Agent: {agent_id}"
            ));
        }
        agents.push(LocalAgent {
            runtime: "grok_build",
            agent_id: agent_id.into(),
            description: description.into(),
        });
    }
    agents.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
    agents.dedup_by(|left, right| left.agent_id == right.agent_id);
    Ok(agents)
}

fn valid_grok_build_agent_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
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

pub fn discover_gemini_agents(gemini_root: &Path) -> Result<Vec<LocalAgent>, String> {
    let mut agents = gemini_builtin_agents()
        .into_iter()
        .map(|agent| (agent.agent_id.clone(), agent))
        .collect::<BTreeMap<_, _>>();
    for agent in discover_gemini_agents_in(&gemini_root.join("agents"))? {
        agents.insert(agent.agent_id.clone(), agent);
    }
    Ok(agents.into_values().collect())
}

pub fn discover_gemini_agents_for_project(
    gemini_root: &Path,
    project_root: &Path,
) -> Result<Vec<LocalAgent>, String> {
    let mut agents = discover_gemini_agents(gemini_root)?
        .into_iter()
        .map(|agent| (agent.agent_id.clone(), agent))
        .collect::<BTreeMap<_, _>>();
    for agent in discover_gemini_agents_in(&project_root.join(".gemini/agents"))? {
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

pub fn discover_pi_agents(pi_root: &Path) -> Result<Vec<LocalAgent>, String> {
    Ok(discover_pi_agents_in(&pi_root.join("agents"))?
        .into_iter()
        .map(|(agent, _)| agent)
        .collect())
}

pub fn discover_pi_agents_for_project(
    pi_root: &Path,
    project_root: &Path,
) -> Result<Vec<LocalAgent>, String> {
    let mut agents = discover_pi_agents(pi_root)?
        .into_iter()
        .map(|agent| (agent.agent_id.clone(), agent))
        .collect::<BTreeMap<_, _>>();
    if let Some(directory) = nearest_pi_agents_directory(project_root) {
        for (agent, _) in discover_pi_agents_in(&directory)? {
            agents.insert(agent.agent_id.clone(), agent);
        }
    }
    Ok(agents.into_values().collect())
}

pub fn resolve_pi_agent_file(
    pi_root: &Path,
    project_root: &Path,
    agent_id: &str,
) -> Result<Option<PathBuf>, String> {
    let mut directories = Vec::with_capacity(2);
    if let Some(directory) = nearest_pi_agents_directory(project_root) {
        directories.push(directory);
    }
    directories.push(pi_root.join("agents"));
    for directory in directories {
        if let Some((_, path)) = discover_pi_agents_in(&directory)?
            .into_iter()
            .find(|(agent, _)| agent.agent_id == agent_id)
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenCodeAgentMode {
    Primary,
    Subagent,
    All,
}

pub fn discover_opencode_agents(config_root: &Path) -> Result<Vec<LocalAgent>, String> {
    let mut agents = opencode_builtin_agents()
        .into_iter()
        .map(|agent| (agent.agent_id.clone(), agent))
        .collect::<BTreeMap<_, _>>();
    for config_name in ["opencode.json", "opencode.jsonc"] {
        for (agent, mode, _) in discover_opencode_agents_in_config(&config_root.join(config_name))?
        {
            if matches!(mode, OpenCodeAgentMode::Subagent | OpenCodeAgentMode::All) {
                agents.insert(agent.agent_id.clone(), agent);
            }
        }
    }
    for (agent, mode, _) in discover_opencode_agents_in(&config_root.join("agents"))? {
        if matches!(mode, OpenCodeAgentMode::Subagent | OpenCodeAgentMode::All) {
            agents.insert(agent.agent_id.clone(), agent);
        }
    }
    Ok(agents.into_values().collect())
}

pub fn discover_kimi_agents(kimi_root: &Path, home: &Path) -> Result<Vec<LocalAgent>, String> {
    discover_kimi_agents_from_scopes(kimi_root, home, None)
}

pub(crate) fn kimi_user_home(kimi_root: &Path) -> Result<PathBuf, String> {
    if std::env::var_os("KIMI_CODE_HOME")
        .map(PathBuf::from)
        .as_deref()
        == Some(kimi_root)
    {
        return dirs::home_dir().ok_or_else(|| "home directory is unavailable".to_string());
    }
    kimi_root.parent().map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "Kimi Code configuration root has no home directory: {}",
            kimi_root.display()
        )
    })
}

pub fn discover_kimi_agents_for_project(
    kimi_root: &Path,
    home: &Path,
    project_root: &Path,
) -> Result<Vec<LocalAgent>, String> {
    discover_kimi_agents_from_scopes(kimi_root, home, Some(project_root))
}

pub fn resolve_kimi_agent_file(
    kimi_root: &Path,
    home: &Path,
    project_root: &Path,
    agent_id: &str,
) -> Result<Option<PathBuf>, String> {
    Ok(
        discover_kimi_agent_definitions(kimi_root, home, Some(project_root))?
            .remove(agent_id)
            .and_then(|definition| definition.path),
    )
}

fn discover_kimi_agents_from_scopes(
    kimi_root: &Path,
    home: &Path,
    project_root: Option<&Path>,
) -> Result<Vec<LocalAgent>, String> {
    Ok(
        discover_kimi_agent_definitions(kimi_root, home, project_root)?
            .into_values()
            .map(|definition| definition.agent)
            .collect(),
    )
}

#[derive(Debug)]
struct KimiAgentDefinition {
    agent: LocalAgent,
    path: Option<PathBuf>,
    overrides_builtin: bool,
}

fn discover_kimi_agent_definitions(
    kimi_root: &Path,
    home: &Path,
    project_root: Option<&Path>,
) -> Result<BTreeMap<String, KimiAgentDefinition>, String> {
    let mut agents = kimi_builtin_agents()
        .into_iter()
        .map(|agent| {
            (
                agent.agent_id.clone(),
                KimiAgentDefinition {
                    agent,
                    path: None,
                    overrides_builtin: true,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut merge_scope = |roots: Vec<PathBuf>| -> Result<(), String> {
        let mut scope = BTreeMap::new();
        for root in roots {
            for definition in discover_kimi_agents_in(&root)? {
                scope
                    .entry(definition.agent.agent_id.clone())
                    .or_insert(definition);
            }
        }
        for (name, definition) in scope {
            if is_kimi_builtin_agent(&name) && !definition.overrides_builtin {
                continue;
            }
            agents.insert(name, definition);
        }
        Ok(())
    };

    merge_scope(kimi_plugin_agent_roots(kimi_root)?)?;
    merge_scope(vec![kimi_root.join("agents"), home.join(".agents/agents")])?;

    let project_base = project_root.map(nearest_project_root);
    let extra_roots = kimi_extra_agent_roots(kimi_root, home, project_base.as_deref())?;
    merge_scope(extra_roots)?;

    if let Some(project) = project_base {
        merge_scope(vec![
            project.join(".kimi-code/agents"),
            project.join(".agents/agents"),
        ])?;
    }
    Ok(agents)
}

fn kimi_plugin_agent_roots(kimi_root: &Path) -> Result<Vec<PathBuf>, String> {
    let installed_path = kimi_root.join("plugins/installed.json");
    let contents = match fs::read_to_string(&installed_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "could not read {}: {error}",
                installed_path.display()
            ));
        }
    };
    let installed: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|error| format!("could not parse {}: {error}", installed_path.display()))?;
    let plugins = installed
        .get("plugins")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            format!(
                "{} does not contain a plugins array",
                installed_path.display()
            )
        })?;
    let mut roots = Vec::new();
    for plugin in plugins {
        if plugin.get("enabled").and_then(serde_json::Value::as_bool) != Some(true) {
            continue;
        }
        let Some(root) = plugin
            .get("root")
            .and_then(serde_json::Value::as_str)
            .filter(|root| !root.trim().is_empty())
            .map(PathBuf::from)
        else {
            continue;
        };
        let root_manifest = root.join("kimi.plugin.json");
        let nested_manifest = root.join(".kimi-plugin/plugin.json");
        let manifest_path = if root_manifest.is_file() {
            root_manifest
        } else if nested_manifest.is_file() {
            nested_manifest
        } else {
            continue;
        };
        let manifest = fs::read_to_string(&manifest_path)
            .map_err(|error| format!("could not read {}: {error}", manifest_path.display()))?;
        let manifest: serde_json::Value = serde_json::from_str(&manifest)
            .map_err(|error| format!("could not parse {}: {error}", manifest_path.display()))?;
        match manifest.get("agents") {
            None => roots.push(root.join("agents")),
            Some(serde_json::Value::String(directory)) => roots.push(root.join(directory)),
            Some(serde_json::Value::Array(directories)) => {
                for directory in directories {
                    let Some(directory) = directory.as_str() else {
                        return Err(format!(
                            "Kimi Code plugin agents must contain strings: {}",
                            manifest_path.display()
                        ));
                    };
                    roots.push(root.join(directory));
                }
            }
            Some(_) => {
                return Err(format!(
                    "Kimi Code plugin agents must be a string or array: {}",
                    manifest_path.display()
                ));
            }
        }
    }
    Ok(roots)
}

fn kimi_builtin_agents() -> Vec<LocalAgent> {
    [
        ("agent", "Kimi Code 内建主 Agent"),
        ("coder", "Kimi Code 内建编码 SubAgent"),
        ("explore", "Kimi Code 内建探索 SubAgent"),
        ("plan", "Kimi Code 内建规划 SubAgent"),
    ]
    .into_iter()
    .map(|(agent_id, description)| LocalAgent {
        runtime: "kimi_code",
        agent_id: agent_id.into(),
        description: description.into(),
    })
    .collect()
}

pub(crate) fn is_kimi_builtin_agent(agent_id: &str) -> bool {
    matches!(agent_id, "agent" | "coder" | "explore" | "plan")
}

fn discover_kimi_agents_in(directory: &Path) -> Result<Vec<KimiAgentDefinition>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read {}: {error}", directory.display())),
    };
    let mut pending = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("could not read Kimi Code Agent entry: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut agents = Vec::new();
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            let children = fs::read_dir(&path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))?;
            for child in children {
                pending.push(
                    child
                        .map_err(|error| format!("could not read Kimi Code Agent entry: {error}"))?
                        .path(),
                );
            }
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }
        let contents = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let frontmatter = contents
            .strip_prefix("---\n")
            .and_then(|contents| contents.split_once("\n---\n"))
            .map(|(frontmatter, _)| frontmatter)
            .ok_or_else(|| {
                format!(
                    "Kimi Code Agent must contain YAML frontmatter: {}",
                    path.display()
                )
            })?;
        let value = serde_yaml::from_str::<serde_yaml::Value>(frontmatter).map_err(|error| {
            format!(
                "invalid Kimi Code Agent frontmatter {}: {error}",
                path.display()
            )
        })?;
        let object = value.as_mapping().ok_or_else(|| {
            format!(
                "Kimi Code Agent frontmatter must be a mapping: {}",
                path.display()
            )
        })?;
        let name = object
            .get(serde_yaml::Value::String("name".into()))
            .and_then(serde_yaml::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_string)
            });
        let description = object
            .get(serde_yaml::Value::String("description".into()))
            .and_then(serde_yaml::Value::as_str);
        let name = name
            .ok_or_else(|| format!("Kimi Code Agent has no valid file name: {}", path.display()))?;
        let description = description
            .filter(|description| !description.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "Kimi Code Agent description is required: {}",
                    path.display()
                )
            })?;
        if !valid_kimi_agent_name(&name) {
            return Err(format!(
                "invalid Kimi Code Agent name `{name}`: {}",
                path.display()
            ));
        }
        let overrides_builtin = match object.get(serde_yaml::Value::String("override".into())) {
            Some(value) => value.as_bool().ok_or_else(|| {
                format!(
                    "Kimi Code Agent override must be boolean: {}",
                    path.display()
                )
            })?,
            None => false,
        };
        agents.push(KimiAgentDefinition {
            agent: LocalAgent {
                runtime: "kimi_code",
                agent_id: name,
                description: description.to_string(),
            },
            path: Some(path),
            overrides_builtin,
        });
    }
    agents.sort_by(|left, right| left.agent.agent_id.cmp(&right.agent.agent_id));
    Ok(agents)
}

fn kimi_extra_agent_roots(
    kimi_root: &Path,
    home: &Path,
    project_root: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    let config = kimi_root.join("config.toml");
    let contents = match fs::read_to_string(&config) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read {}: {error}", config.display())),
    };
    let document = contents
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("invalid Kimi Code TOML {}: {error}", config.display()))?;
    let Some(directories) = document.get("extra_agent_dirs").and_then(Item::as_array) else {
        return Ok(Vec::new());
    };
    let base = project_root.unwrap_or(kimi_root);
    directories
        .iter()
        .map(|item| {
            let directory = item.as_str().ok_or_else(|| {
                format!(
                    "Kimi Code extra_agent_dirs must contain strings: {}",
                    config.display()
                )
            })?;
            if directory == "~" {
                Ok(home.to_path_buf())
            } else if let Some(relative) = directory.strip_prefix("~/") {
                Ok(home.join(relative))
            } else {
                let path = PathBuf::from(directory);
                Ok(if path.is_absolute() {
                    path
                } else {
                    base.join(path)
                })
            }
        })
        .collect()
}

fn nearest_project_root(path: &Path) -> PathBuf {
    path.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .unwrap_or(path)
        .to_path_buf()
}

fn valid_kimi_agent_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub fn discover_opencode_agents_for_project(
    config_root: &Path,
    project_root: &Path,
) -> Result<Vec<LocalAgent>, String> {
    let mut agents = discover_opencode_agents(config_root)?
        .into_iter()
        .map(|agent| (agent.agent_id.clone(), agent))
        .collect::<BTreeMap<_, _>>();
    for config_name in ["opencode.json", "opencode.jsonc"] {
        for (agent, mode, _) in discover_opencode_agents_in_config(&project_root.join(config_name))?
        {
            if matches!(mode, OpenCodeAgentMode::Subagent | OpenCodeAgentMode::All) {
                agents.insert(agent.agent_id.clone(), agent);
            }
        }
    }
    for (agent, mode, _) in discover_opencode_agents_in(&project_root.join(".opencode/agents"))? {
        if matches!(mode, OpenCodeAgentMode::Subagent | OpenCodeAgentMode::All) {
            agents.insert(agent.agent_id.clone(), agent);
        }
    }
    Ok(agents.into_values().collect())
}

pub(crate) fn resolve_opencode_agent(
    config_root: &Path,
    project_root: &Path,
    agent_id: &str,
) -> Result<Option<(OpenCodeAgentMode, Option<PathBuf>)>, String> {
    let builtin = match agent_id {
        "build" | "plan" => Some(OpenCodeAgentMode::Primary),
        "general" | "explore" | "scout" => Some(OpenCodeAgentMode::Subagent),
        _ => None,
    };
    for (directory, configs) in [
        (
            project_root.join(".opencode/agents"),
            [
                project_root.join("opencode.json"),
                project_root.join("opencode.jsonc"),
            ],
        ),
        (
            config_root.join("agents"),
            [
                config_root.join("opencode.json"),
                config_root.join("opencode.jsonc"),
            ],
        ),
    ] {
        if let Some((_, mode, path)) = discover_opencode_agents_in(&directory)?
            .into_iter()
            .find(|(agent, _, _)| agent.agent_id == agent_id)
        {
            return Ok(Some((mode, Some(path))));
        }
        for config in configs.into_iter().rev() {
            if let Some((_, mode, path)) = discover_opencode_agents_in_config(&config)?
                .into_iter()
                .find(|(agent, _, _)| agent.agent_id == agent_id)
            {
                return Ok(Some((mode, Some(path))));
            }
        }
    }
    Ok(builtin.map(|mode| (mode, None)))
}

fn nearest_pi_agents_directory(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .map(|ancestor| ancestor.join(".pi/agents"))
        .find(|candidate| candidate.is_dir())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PiAgentDefinition {
    pub tools: Vec<String>,
    pub model: Option<String>,
    pub system_prompt: String,
}

pub(crate) fn read_pi_agent_definition(path: &Path) -> Result<PiAgentDefinition, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let (frontmatter, system_prompt) = contents
        .strip_prefix("---\n")
        .and_then(|contents| contents.split_once("\n---\n"))
        .ok_or_else(|| format!("invalid Pi Agent frontmatter: {}", path.display()))?;
    let value: serde_yaml::Value = serde_yaml::from_str(frontmatter)
        .map_err(|error| format!("invalid Pi Agent frontmatter {}: {error}", path.display()))?;
    let object = value
        .as_mapping()
        .ok_or_else(|| format!("invalid Pi Agent frontmatter: {}", path.display()))?;
    let tools = object
        .get(serde_yaml::Value::String("tools".into()))
        .and_then(serde_yaml::Value::as_str)
        .map(|tools| {
            tools
                .split(',')
                .map(str::trim)
                .filter(|tool| !tool.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if tools.iter().any(|tool| {
        tool.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
        })
    }) {
        return Err(format!("Pi Agent tools are invalid: {}", path.display()));
    }
    let model = object
        .get(serde_yaml::Value::String("model".into()))
        .and_then(serde_yaml::Value::as_str)
        .map(str::to_string);
    if model.as_ref().is_some_and(|model| {
        model.trim().is_empty() || model.trim() != model || model.chars().any(char::is_control)
    }) {
        return Err(format!("Pi Agent model is invalid: {}", path.display()));
    }
    Ok(PiAgentDefinition {
        tools,
        model,
        system_prompt: system_prompt.to_string(),
    })
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

fn gemini_builtin_agents() -> Vec<LocalAgent> {
    [
        ("codebase_investigator", "Gemini CLI 内建代码库调查 Agent"),
        ("cli_help", "Gemini CLI 内建帮助 Agent"),
        ("generalist", "Gemini CLI 内建通用 Agent"),
    ]
    .into_iter()
    .map(|(agent_id, description)| LocalAgent {
        runtime: "gemini",
        agent_id: agent_id.into(),
        description: description.into(),
    })
    .collect()
}

fn discover_gemini_agents_in(directory: &Path) -> Result<Vec<LocalAgent>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read {}: {error}", directory.display())),
    };
    let mut paths = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("could not read Gemini Agent entry: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut agents = Vec::new();
    let mut names = BTreeMap::<String, PathBuf>::new();
    for path in paths {
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }
        let contents = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let frontmatter = contents
            .strip_prefix("---\n")
            .and_then(|contents| contents.split_once("\n---\n"))
            .map(|(frontmatter, _)| frontmatter)
            .ok_or_else(|| format!("invalid Gemini Agent frontmatter: {}", path.display()))?;
        let value: serde_yaml::Value = serde_yaml::from_str(frontmatter).map_err(|error| {
            format!(
                "invalid Gemini Agent frontmatter {}: {error}",
                path.display()
            )
        })?;
        let object = value
            .as_mapping()
            .ok_or_else(|| format!("invalid Gemini Agent frontmatter: {}", path.display()))?;
        let name = object
            .get(serde_yaml::Value::String("name".into()))
            .and_then(serde_yaml::Value::as_str)
            .ok_or_else(|| format!("Gemini Agent name is required: {}", path.display()))?;
        if name.is_empty()
            || !name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
        {
            return Err(format!(
                "invalid Gemini Agent name in {}: {name}",
                path.display()
            ));
        }
        let description = object
            .get(serde_yaml::Value::String("description".into()))
            .and_then(serde_yaml::Value::as_str)
            .filter(|description| {
                !description.trim().is_empty()
                    && description.trim() == *description
                    && !description.chars().any(char::is_control)
            })
            .ok_or_else(|| format!("Gemini Agent description is invalid: {}", path.display()))?;
        if let Some(first) = names.insert(name.to_string(), path.clone()) {
            return Err(format!(
                "duplicate Gemini Agent name: {name} ({} and {})",
                first.display(),
                path.display()
            ));
        }
        agents.push(LocalAgent {
            runtime: "gemini",
            agent_id: name.into(),
            description: description.into(),
        });
    }
    Ok(agents)
}

fn opencode_builtin_agents() -> Vec<LocalAgent> {
    [
        ("general", "OpenCode 内建通用 SubAgent"),
        ("explore", "OpenCode 内建探索 SubAgent"),
        ("scout", "OpenCode 内建调研 SubAgent"),
    ]
    .into_iter()
    .map(|(agent_id, description)| LocalAgent {
        runtime: "opencode",
        agent_id: agent_id.into(),
        description: description.into(),
    })
    .collect()
}

fn discover_opencode_agents_in_config(
    path: &Path,
) -> Result<Vec<(LocalAgent, OpenCodeAgentMode, PathBuf)>, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    let value = json5::from_str::<serde_json::Value>(&contents)
        .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
    let Some(configured) = value.get("agent") else {
        return Ok(Vec::new());
    };
    let configured = configured.as_object().ok_or_else(|| {
        format!(
            "OpenCode agent configuration must be an object: {}",
            path.display()
        )
    })?;
    let mut agents = Vec::new();
    for (agent_id, definition) in configured {
        let Some(definition) = definition.as_object() else {
            continue;
        };
        if definition
            .get("disable")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            continue;
        }
        let Some(description) = definition
            .get("description")
            .and_then(serde_json::Value::as_str)
            .filter(|description| !description.trim().is_empty())
        else {
            continue;
        };
        let Some(mode) =
            opencode_agent_mode(definition.get("mode").and_then(serde_json::Value::as_str))
        else {
            continue;
        };
        if !valid_opencode_agent_name(agent_id) {
            continue;
        }
        agents.push((
            LocalAgent {
                runtime: "opencode",
                agent_id: agent_id.clone(),
                description: description.to_string(),
            },
            mode,
            path.to_path_buf(),
        ));
    }
    agents.sort_by(|left, right| left.0.agent_id.cmp(&right.0.agent_id));
    Ok(agents)
}

fn discover_opencode_agents_in(
    directory: &Path,
) -> Result<Vec<(LocalAgent, OpenCodeAgentMode, PathBuf)>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read {}: {error}", directory.display())),
    };
    let mut pending = Vec::new();
    for entry in entries {
        pending.push(
            entry
                .map_err(|error| format!("could not read OpenCode Agent entry: {error}"))?
                .path(),
        );
    }
    let mut agents = Vec::new();
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            let entries = fs::read_dir(&path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))?;
            for entry in entries {
                pending.push(
                    entry
                        .map_err(|error| format!("could not read OpenCode Agent entry: {error}"))?
                        .path(),
                );
            }
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let contents = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let Some(frontmatter) = contents
            .strip_prefix("---\n")
            .and_then(|contents| contents.split_once("\n---\n"))
            .map(|(frontmatter, _)| frontmatter)
        else {
            continue;
        };
        let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(frontmatter) else {
            continue;
        };
        let Some(object) = value.as_mapping() else {
            continue;
        };
        if object
            .get(serde_yaml::Value::String("disable".into()))
            .and_then(serde_yaml::Value::as_bool)
            == Some(true)
        {
            continue;
        }
        let Some(description) = object
            .get(serde_yaml::Value::String("description".into()))
            .and_then(serde_yaml::Value::as_str)
            .filter(|description| !description.trim().is_empty())
        else {
            continue;
        };
        let Some(mode) = opencode_agent_mode(
            object
                .get(serde_yaml::Value::String("mode".into()))
                .and_then(serde_yaml::Value::as_str),
        ) else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(directory) else {
            continue;
        };
        let mut agent_id = relative.with_extension("").to_string_lossy().to_string();
        if std::path::MAIN_SEPARATOR != '/' {
            agent_id = agent_id.replace(std::path::MAIN_SEPARATOR, "/");
        }
        if !valid_opencode_agent_name(&agent_id) {
            continue;
        }
        agents.push((
            LocalAgent {
                runtime: "opencode",
                agent_id,
                description: description.to_string(),
            },
            mode,
            path,
        ));
    }
    agents.sort_by(|left, right| left.0.agent_id.cmp(&right.0.agent_id));
    Ok(agents)
}

fn opencode_agent_mode(value: Option<&str>) -> Option<OpenCodeAgentMode> {
    match value.unwrap_or("all") {
        "primary" => Some(OpenCodeAgentMode::Primary),
        "subagent" => Some(OpenCodeAgentMode::Subagent),
        "all" => Some(OpenCodeAgentMode::All),
        _ => None,
    }
}

fn valid_opencode_agent_name(value: &str) -> bool {
    !value.is_empty()
        && value.split('/').all(|segment| {
            !segment.is_empty()
                && segment.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'_')
                })
        })
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

fn discover_pi_agents_in(directory: &Path) -> Result<Vec<(LocalAgent, PathBuf)>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read {}: {error}", directory.display())),
    };
    let mut agents = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read Pi Agent entry: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let contents = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let Some(frontmatter) = contents
            .strip_prefix("---\n")
            .and_then(|contents| contents.split_once("\n---\n"))
            .map(|(frontmatter, _)| frontmatter)
        else {
            continue;
        };
        let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(frontmatter) else {
            continue;
        };
        let Some(object) = value.as_mapping() else {
            continue;
        };
        let Some(name) = object
            .get(serde_yaml::Value::String("name".into()))
            .and_then(serde_yaml::Value::as_str)
        else {
            continue;
        };
        let Some(description) = object
            .get(serde_yaml::Value::String("description".into()))
            .and_then(serde_yaml::Value::as_str)
        else {
            continue;
        };
        if !valid_pi_agent_name(name) || description.trim().is_empty() {
            continue;
        }
        agents.push((
            LocalAgent {
                runtime: "pi",
                agent_id: name.into(),
                description: description.into(),
            },
            path,
        ));
    }
    agents.sort_by(|left, right| left.0.agent_id.cmp(&right.0.agent_id));
    Ok(agents)
}

fn valid_pi_agent_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
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
) -> Result<LocalAgentDiscovery, String> {
    let home = dirs::home_dir().ok_or_else(|| "home directory is unavailable".to_string())?;
    let claude_root = home.join(".claude");
    let project_root = project_root
        .filter(|project_root| !project_root.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let claude = (|| {
        let mut agents = Vec::new();
        if let Some(runtime) =
            crate::adapters::claude_code::detect_claude_cli().map_err(|error| error.to_string())?
        {
            agents.extend(discover_claude_builtin_agents(&runtime.path)?);
        }
        agents.extend(match &project_root {
            Some(project_root) => {
                discover_claude_code_agents_for_project(&claude_root, project_root)?
            }
            None => discover_claude_code_agents(&claude_root)?,
        });
        Ok(agents)
    })();

    let codex = (|| {
        if crate::adapters::codex::detect_codex_cli()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(Vec::new());
        }
        let root = home.join(".codex");
        match &project_root {
            Some(project_root) => discover_codex_agents_for_project(&root, project_root),
            None => discover_codex_agents(&root),
        }
    })();

    let gemini = (|| {
        if crate::adapters::gemini::detect_gemini_cli()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(Vec::new());
        }
        let root = home.join(".gemini");
        match &project_root {
            Some(project_root) => discover_gemini_agents_for_project(&root, project_root),
            None => discover_gemini_agents(&root),
        }
    })();

    let pi = (|| {
        if crate::adapters::pi::detect_pi_cli()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(Vec::new());
        }
        let root = home.join(".pi/agent");
        match &project_root {
            Some(project_root) => discover_pi_agents_for_project(&root, project_root),
            None => discover_pi_agents(&root),
        }
    })();

    let opencode = (|| {
        if crate::adapters::opencode::detect_opencode_cli()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(Vec::new());
        }
        let root = home.join(".config/opencode");
        match &project_root {
            Some(project_root) => discover_opencode_agents_for_project(&root, project_root),
            None => discover_opencode_agents(&root),
        }
    })();

    let kimi = (|| {
        if crate::adapters::kimi_code::detect_kimi_code_cli()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(Vec::new());
        }
        let root = std::env::var_os("KIMI_CODE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".kimi-code"));
        match &project_root {
            Some(project_root) => discover_kimi_agents_for_project(&root, &home, project_root),
            None => discover_kimi_agents(&root, &home),
        }
    })();

    let grok_build = (|| {
        let Some(runtime) = crate::adapters::grok_build::detect_grok_build_cli()
            .map_err(|error| error.to_string())?
        else {
            return Ok(Vec::new());
        };
        let root = project_root.as_deref().unwrap_or(Path::new("."));
        discover_grok_build_agents(&runtime.path, root)
    })();

    Ok(LocalAgentDiscovery::from_runtime_results([
        ("claude_code", claude),
        ("codex", codex),
        ("gemini", gemini),
        ("pi", pi),
        ("opencode", opencode),
        ("kimi_code", kimi),
        ("grok_build", grok_build),
    ]))
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
