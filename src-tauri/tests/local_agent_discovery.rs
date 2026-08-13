use grillforge_lib::local_agents::{
    LocalAgent, LocalAgentDiscovery, discover_claude_builtin_agents, discover_claude_code_agents,
    discover_claude_code_agents_for_project, discover_codex_agents,
    discover_codex_agents_for_project, discover_gemini_agents, discover_gemini_agents_for_project,
    discover_grok_build_agents, discover_kimi_agents, discover_kimi_agents_for_project,
    discover_opencode_agents_for_project, discover_pi_agents_for_project,
    resolve_codex_custom_agent_file, resolve_kimi_agent_file, resolve_pi_agent_file,
};
use std::fs;

#[cfg(unix)]
#[test]
fn grok_build_discovers_exactly_the_agents_reported_by_inspect_json() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let runtime = directory.path().join("grok");
    fs::write(
        &runtime,
        format!(
            r#"#!/bin/sh
test "$(pwd -P)" = "{}" || exit 71
test "$1" = inspect || exit 72
test "$2" = --json || exit 73
printf '%s\n' '{{"agents":[{{"name":"general-purpose","description":"General agent","source":{{"type":"builtin"}}}},{{"name":"project-reviewer","description":"Project reviewer","source":{{"type":"project","path":"/private/prompt.md"}}}}]}}'
"#,
            fs::canonicalize(&project).unwrap().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();

    let discovered = discover_grok_build_agents(&runtime, &project).unwrap();

    assert_eq!(
        discovered
            .iter()
            .map(|agent| (
                agent.runtime,
                agent.agent_id.as_str(),
                agent.description.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("grok_build", "general-purpose", "General agent"),
            ("grok_build", "project-reviewer", "Project reviewer"),
        ]
    );
    assert!(!format!("{discovered:?}").contains("/private/prompt.md"));
}

#[cfg(unix)]
#[test]
fn grok_build_inspection_rejects_invalid_json_without_hiding_the_failure() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let runtime = directory.path().join("grok");
    fs::write(&runtime, "#!/bin/sh\nprintf 'not-json'\n").unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();

    let error = discover_grok_build_agents(&runtime, directory.path()).unwrap_err();

    assert_eq!(error, "Grok Build Agent inspection returned invalid JSON");
}

#[test]
fn one_broken_runtime_preserves_other_discovered_agents_and_reports_its_error() {
    let discovery = LocalAgentDiscovery::from_runtime_results([
        (
            "claude_code",
            Ok(vec![LocalAgent {
                runtime: "claude_code",
                agent_id: "reviewer".into(),
                description: "Reviews code".into(),
            }]),
        ),
        (
            "opencode",
            Err("OpenCode CLI did not return a version".into()),
        ),
    ]);

    assert_eq!(discovery.agents.len(), 1);
    assert_eq!(discovery.agents[0].agent_id, "reviewer");
    assert_eq!(discovery.errors.len(), 1);
    assert_eq!(discovery.errors[0].runtime, "opencode");
    assert_eq!(
        discovery.errors[0].message,
        "OpenCode CLI did not return a version"
    );
}

#[test]
fn discovers_only_valid_user_agents_without_copying_their_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let agents = directory.path().join("agents");
    fs::create_dir_all(&agents).unwrap();
    fs::write(
        agents.join("reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\nmodel: sonnet\n---\nPrivate instructions\n",
    )
    .unwrap();
    fs::write(agents.join("broken.md"), "not frontmatter").unwrap();

    let discovered = discover_claude_code_agents(directory.path()).unwrap();

    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].runtime, "claude_code");
    assert_eq!(discovered[0].agent_id, "reviewer");
    assert_eq!(discovered[0].description, "Reviews code");
    assert!(!format!("{discovered:?}").contains("Private instructions"));
}

#[test]
fn kimi_discovers_current_builtin_and_scoped_markdown_agents_with_project_precedence() {
    let home = tempfile::tempdir().unwrap();
    let kimi_root = home.path().join(".kimi-code");
    let project = tempfile::tempdir().unwrap();
    fs::create_dir_all(kimi_root.join("agents/nested")).unwrap();
    fs::create_dir_all(home.path().join(".agents/agents")).unwrap();
    fs::create_dir_all(project.path().join(".kimi-code/agents")).unwrap();
    fs::create_dir_all(project.path().join(".git")).unwrap();
    fs::write(
        kimi_root.join("agents/nested/reviewer.md"),
        "---\nname: reviewer\ndescription: User reviewer\n---\nprivate\n",
    )
    .unwrap();
    fs::write(
        home.path().join(".agents/agents/shared.md"),
        "---\ndescription: Shared agent\n---\nprivate\n",
    )
    .unwrap();
    fs::write(
        project.path().join(".kimi-code/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Project reviewer\n---\nprivate\n",
    )
    .unwrap();

    let discovered =
        discover_kimi_agents_for_project(&kimi_root, home.path(), project.path()).unwrap();

    assert_eq!(
        discovered
            .iter()
            .map(|agent| (
                agent.runtime,
                agent.agent_id.as_str(),
                agent.description.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("kimi_code", "agent", "Kimi Code 内建主 Agent"),
            ("kimi_code", "coder", "Kimi Code 内建编码 SubAgent"),
            ("kimi_code", "explore", "Kimi Code 内建探索 SubAgent"),
            ("kimi_code", "plan", "Kimi Code 内建规划 SubAgent"),
            ("kimi_code", "reviewer", "Project reviewer"),
            ("kimi_code", "shared", "Shared agent"),
        ]
    );
    assert_eq!(
        resolve_kimi_agent_file(&kimi_root, home.path(), project.path(), "reviewer").unwrap(),
        Some(project.path().join(".kimi-code/agents/reviewer.md"))
    );
}

#[test]
fn kimi_discovers_extra_and_enabled_plugin_agents_without_accepting_implicit_builtin_overrides() {
    let home = tempfile::tempdir().unwrap();
    let kimi_root = home.path().join(".kimi-code");
    let project = tempfile::tempdir().unwrap();
    let plugin = home.path().join("plugin");
    fs::create_dir_all(kimi_root.join("agents")).unwrap();
    fs::create_dir_all(home.path().join("team-agents")).unwrap();
    fs::create_dir_all(kimi_root.join("plugins")).unwrap();
    fs::create_dir_all(plugin.join("agents")).unwrap();
    fs::create_dir_all(project.path().join(".git")).unwrap();
    fs::write(
        kimi_root.join("config.toml"),
        "extra_agent_dirs = [\"~/team-agents\"]\n",
    )
    .unwrap();
    fs::write(
        home.path().join("team-agents/specialist.md"),
        "---\ndescription: Team specialist\n---\nprivate\n",
    )
    .unwrap();
    fs::write(
        kimi_root.join("agents/coder.md"),
        "---\nname: coder\ndescription: Unsafe implicit override\n---\nprivate\n",
    )
    .unwrap();
    fs::write(
        kimi_root.join("plugins/installed.json"),
        format!(
            r#"{{"version":1,"plugins":[{{"id":"review","root":{},"source":"local-path","enabled":true,"installedAt":"now"}}]}}"#,
            serde_json::to_string(&plugin.display().to_string()).unwrap()
        ),
    )
    .unwrap();
    fs::write(
        plugin.join("kimi.plugin.json"),
        r#"{"name":"review","agents":"agents"}"#,
    )
    .unwrap();
    fs::write(
        plugin.join("agents/plugin-reviewer.md"),
        "---\ndescription: Plugin reviewer\n---\nprivate\n",
    )
    .unwrap();

    let discovered =
        discover_kimi_agents_for_project(&kimi_root, home.path(), project.path()).unwrap();

    assert_eq!(
        discovered
            .iter()
            .find(|agent| agent.agent_id == "coder")
            .unwrap()
            .description,
        "Kimi Code 内建编码 SubAgent"
    );
    assert!(
        discovered.iter().any(|agent| {
            agent.agent_id == "specialist" && agent.description == "Team specialist"
        })
    );
    assert!(discovered.iter().any(|agent| {
        agent.agent_id == "plugin-reviewer" && agent.description == "Plugin reviewer"
    }));
}

#[test]
fn kimi_rejects_an_invalid_custom_agent_at_discovery_boundary() {
    let home = tempfile::tempdir().unwrap();
    let kimi_root = home.path().join(".kimi-code");
    fs::create_dir_all(kimi_root.join("agents")).unwrap();
    fs::write(
        kimi_root.join("agents/broken.md"),
        "---\nname: Broken Agent\ndescription: Invalid name\n---\nPrompt\n",
    )
    .unwrap();

    let error = discover_kimi_agents(&kimi_root, home.path())
        .expect_err("invalid Kimi Code Agent must fail fast");

    assert!(error.contains("invalid Kimi Code Agent name"));
    assert!(error.contains("broken.md"));
}

#[test]
fn opencode_discovers_builtin_and_configured_agents_with_project_precedence() {
    let config_root = tempfile::tempdir().unwrap();
    let project_root = tempfile::tempdir().unwrap();
    fs::create_dir_all(config_root.path().join("agents")).unwrap();
    fs::create_dir_all(project_root.path().join(".opencode/agents")).unwrap();
    fs::write(
        config_root.path().join("opencode.jsonc"),
        r#"{
          // OpenCode officially accepts JSONC.
          "agent": {
            "json-reviewer": {"description":"JSON reviewer","mode":"subagent"},
            "json-primary": {"description":"JSON primary","mode":"primary"},
            "json-all": {"description":"JSON all"},
            "json-disabled": {"description":"Disabled","mode":"subagent","disable":true}
          }
        }"#,
    )
    .unwrap();
    fs::write(
        config_root.path().join("agents/reviewer.md"),
        "---\ndescription: User reviewer\nmode: subagent\n---\nPrivate user prompt\n",
    )
    .unwrap();
    fs::write(
        project_root.path().join(".opencode/agents/reviewer.md"),
        "---\ndescription: Project reviewer\nmode: subagent\n---\nPrivate project prompt\n",
    )
    .unwrap();
    fs::write(
        project_root.path().join(".opencode/agents/architect.md"),
        "---\ndescription: Architecture agent\nmode: primary\n---\nPrivate prompt\n",
    )
    .unwrap();
    fs::write(
        project_root.path().join(".opencode/agents/collaborator.md"),
        "---\ndescription: General collaborator\nmode: all\n---\nPrivate prompt\n",
    )
    .unwrap();
    fs::write(
        project_root.path().join(".opencode/agents/hidden.md"),
        "---\nmode: subagent\n---\nMissing description\n",
    )
    .unwrap();
    fs::write(
        project_root.path().join(".opencode/agents/disabled.md"),
        "---\ndescription: Disabled markdown\nmode: subagent\ndisable: true\n---\nDisabled.\n",
    )
    .unwrap();

    let discovered =
        discover_opencode_agents_for_project(config_root.path(), project_root.path()).unwrap();

    assert_eq!(
        discovered
            .iter()
            .map(|agent| (agent.agent_id.as_str(), agent.description.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("collaborator", "General collaborator"),
            ("explore", "OpenCode 内建探索 SubAgent"),
            ("general", "OpenCode 内建通用 SubAgent"),
            ("json-all", "JSON all"),
            ("json-reviewer", "JSON reviewer"),
            ("reviewer", "Project reviewer"),
            ("scout", "OpenCode 内建调研 SubAgent"),
        ]
    );
    assert!(discovered.iter().all(|agent| agent.runtime == "opencode"));
    assert!(!format!("{discovered:?}").contains("Private"));
}

#[test]
fn discovers_only_installed_and_enabled_plugin_agents_under_scoped_ids() {
    let directory = tempfile::tempdir().unwrap();
    let plugin = directory
        .path()
        .join("plugins/cache/acme/review-tools/1.0.0");
    fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
    fs::create_dir_all(plugin.join("agents/review")).unwrap();
    fs::write(
        plugin.join(".claude-plugin/plugin.json"),
        r#"{"name":"review-tools"}"#,
    )
    .unwrap();
    fs::write(
        plugin.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews code\n---\nPrivate prompt\n",
    )
    .unwrap();
    fs::write(
        plugin.join("agents/review/security.md"),
        "---\nname: security\ndescription: Reviews security\n---\nPrivate prompt\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("settings.json"),
        r#"{"enabledPlugins":{"review-tools@acme":true}}"#,
    )
    .unwrap();
    fs::create_dir_all(directory.path().join("plugins")).unwrap();
    fs::write(
        directory.path().join("plugins/installed_plugins.json"),
        format!(
            r#"{{"version":2,"plugins":{{"review-tools@acme":[{{"scope":"user","installPath":{}}}]}}}}"#,
            serde_json::to_string(&plugin).unwrap()
        ),
    )
    .unwrap();

    let discovered = discover_claude_code_agents(directory.path()).unwrap();

    assert_eq!(
        discovered
            .iter()
            .map(|agent| agent.agent_id.as_str())
            .collect::<Vec<_>>(),
        vec!["review-tools:review:security", "review-tools:reviewer"]
    );
}

#[test]
fn project_agents_are_discovered_and_override_same_named_user_agents() {
    let claude_root = tempfile::tempdir().unwrap();
    let project_root = tempfile::tempdir().unwrap();
    fs::create_dir_all(claude_root.path().join("agents")).unwrap();
    fs::create_dir_all(project_root.path().join(".claude/agents")).unwrap();
    fs::write(
        claude_root.path().join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: User reviewer\n---\nUser prompt\n",
    )
    .unwrap();
    fs::write(
        project_root.path().join(".claude/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Project reviewer\n---\nProject prompt\n",
    )
    .unwrap();

    let discovered =
        discover_claude_code_agents_for_project(claude_root.path(), project_root.path()).unwrap();

    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].agent_id, "reviewer");
    assert_eq!(discovered[0].description, "Project reviewer");
}

#[test]
fn ignores_disabled_plugins_and_marketplace_source_agents() {
    let directory = tempfile::tempdir().unwrap();
    let installed = directory.path().join("plugins/cache/acme/disabled/1.0.0");
    let marketplace = directory
        .path()
        .join("plugins/marketplaces/acme/plugins/not-installed");
    for plugin in [&installed, &marketplace] {
        fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
        fs::create_dir_all(plugin.join("agents")).unwrap();
        fs::write(
            plugin.join(".claude-plugin/plugin.json"),
            r#"{"name":"hidden"}"#,
        )
        .unwrap();
        fs::write(
            plugin.join("agents/hidden.md"),
            "---\nname: hidden\ndescription: Must stay hidden\n---\nPrompt\n",
        )
        .unwrap();
    }
    fs::write(
        directory.path().join("settings.json"),
        r#"{"enabledPlugins":{"disabled@acme":false,"not-installed@acme":true}}"#,
    )
    .unwrap();
    fs::create_dir_all(directory.path().join("plugins")).unwrap();
    fs::write(
        directory.path().join("plugins/installed_plugins.json"),
        format!(
            r#"{{"version":2,"plugins":{{"disabled@acme":[{{"scope":"user","installPath":{}}}]}}}}"#,
            serde_json::to_string(&installed).unwrap()
        ),
    )
    .unwrap();

    let discovered = discover_claude_code_agents(directory.path()).unwrap();

    assert!(discovered.is_empty());
}

#[cfg(unix)]
#[test]
fn discovers_only_agent_ids_the_installed_cli_reports_as_callable() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let runtime = directory.path().join("claude");
    fs::write(
        &runtime,
        "#!/bin/sh\nprintf '%s\\n' \"--agent 'grillforge-discovery-probe' not found. Available agents: claude, Explore, general-purpose, Plan, statusline-setup\" >&2\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();

    let discovered = discover_claude_builtin_agents(&runtime).unwrap();

    assert_eq!(
        discovered
            .iter()
            .map(|agent| agent.agent_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Explore",
            "Plan",
            "claude",
            "general-purpose",
            "statusline-setup"
        ]
    );
}

#[test]
#[ignore = "requires an installed Claude Code CLI; discovery is loopback-only"]
fn installed_claude_cli_reports_callable_builtin_agents_without_network_access() {
    let runtime = grillforge_lib::adapters::claude_code::detect_claude_cli()
        .unwrap()
        .expect("Claude Code CLI is not installed");

    let discovered = discover_claude_builtin_agents(&runtime.path).unwrap();

    assert!(!discovered.is_empty());
}

#[test]
fn codex_discovers_builtins_and_valid_user_agents_by_their_toml_name() {
    let codex_root = tempfile::tempdir().unwrap();
    fs::create_dir_all(codex_root.path().join("agents")).unwrap();
    fs::write(
        codex_root
            .path()
            .join("agents/file-name-does-not-matter.toml"),
        r#"name = "reviewer"
description = "Reviews changes"
developer_instructions = "Private instructions"
"#,
    )
    .unwrap();
    fs::write(
        codex_root.path().join("agents/broken.toml"),
        "name = \"broken\"\n",
    )
    .unwrap();

    let discovered = discover_codex_agents(codex_root.path()).unwrap();

    assert_eq!(
        discovered
            .iter()
            .map(|agent| (agent.runtime, agent.agent_id.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("codex", "default"),
            ("codex", "explorer"),
            ("codex", "reviewer"),
            ("codex", "worker"),
        ]
    );
    assert!(!format!("{discovered:?}").contains("Private instructions"));
}

#[test]
fn codex_project_agent_overrides_user_agent_and_resolves_the_effective_file() {
    let codex_root = tempfile::tempdir().unwrap();
    let project_root = tempfile::tempdir().unwrap();
    fs::create_dir_all(codex_root.path().join("agents")).unwrap();
    fs::create_dir_all(project_root.path().join(".codex/agents")).unwrap();
    fs::write(
        codex_root.path().join("agents/reviewer.toml"),
        "name = \"reviewer\"\ndescription = \"User reviewer\"\ndeveloper_instructions = \"User\"\n",
    )
    .unwrap();
    let project_agent = project_root.path().join(".codex/agents/project.toml");
    fs::write(
        &project_agent,
        "name = \"reviewer\"\ndescription = \"Project reviewer\"\ndeveloper_instructions = \"Project\"\n",
    )
    .unwrap();

    let discovered =
        discover_codex_agents_for_project(codex_root.path(), project_root.path()).unwrap();
    let reviewer = discovered
        .iter()
        .find(|agent| agent.agent_id == "reviewer")
        .unwrap();
    assert_eq!(reviewer.description, "Project reviewer");
    assert_eq!(
        resolve_codex_custom_agent_file(codex_root.path(), project_root.path(), "reviewer")
            .unwrap(),
        Some(project_agent)
    );
}

#[test]
fn pi_project_agent_overrides_user_agent_and_resolves_the_effective_file() {
    let pi_root = tempfile::tempdir().unwrap();
    let project_root = tempfile::tempdir().unwrap();
    fs::create_dir_all(pi_root.path().join("agents")).unwrap();
    fs::create_dir_all(project_root.path().join(".pi/agents")).unwrap();
    fs::write(
        pi_root.path().join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: User reviewer\ntools: read, grep\nmodel: anthropic/claude-sonnet-4\n---\nUser private prompt\n",
    )
    .unwrap();
    let project_agent = project_root.path().join(".pi/agents/reviewer.md");
    fs::write(
        &project_agent,
        "---\nname: reviewer\ndescription: Project reviewer\ntools: read,grep,find\n---\nProject private prompt\n",
    )
    .unwrap();

    let nested = project_root.path().join("src/module");
    fs::create_dir_all(&nested).unwrap();
    let discovered = discover_pi_agents_for_project(pi_root.path(), &nested).unwrap();

    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].runtime, "pi");
    assert_eq!(discovered[0].agent_id, "reviewer");
    assert_eq!(discovered[0].description, "Project reviewer");
    assert!(!format!("{discovered:?}").contains("private prompt"));
    assert_eq!(
        resolve_pi_agent_file(pi_root.path(), &nested, "reviewer").unwrap(),
        Some(project_agent)
    );
}

#[test]
fn gemini_discovers_builtins_and_user_project_agents_with_project_precedence() {
    let gemini_root = tempfile::tempdir().unwrap();
    let project_root = tempfile::tempdir().unwrap();
    fs::create_dir_all(gemini_root.path().join("agents")).unwrap();
    fs::create_dir_all(project_root.path().join(".gemini/agents")).unwrap();
    fs::write(
        gemini_root.path().join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: User reviewer\nkind: local\n---\nUser private prompt\n",
    )
    .unwrap();
    fs::write(
        project_root.path().join(".gemini/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Project reviewer\ntools:\n  - read_file\n---\nProject private prompt\n",
    )
    .unwrap();

    let discovered =
        discover_gemini_agents_for_project(gemini_root.path(), project_root.path()).unwrap();

    assert_eq!(
        discovered
            .iter()
            .map(|agent| (agent.runtime, agent.agent_id.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("gemini", "cli_help"),
            ("gemini", "codebase_investigator"),
            ("gemini", "generalist"),
            ("gemini", "reviewer"),
        ]
    );
    assert_eq!(
        discovered
            .iter()
            .find(|agent| agent.agent_id == "reviewer")
            .unwrap()
            .description,
        "Project reviewer"
    );
    assert!(!format!("{discovered:?}").contains("private prompt"));
}

#[test]
fn gemini_rejects_an_invalid_custom_agent_at_discovery_boundary() {
    let gemini_root = tempfile::tempdir().unwrap();
    let project_root = tempfile::tempdir().unwrap();
    fs::create_dir_all(gemini_root.path().join("agents")).unwrap();
    fs::write(
        gemini_root.path().join("agents/broken.md"),
        "---\nname: Broken Agent\ndescription: Invalid name\n---\nPrompt\n",
    )
    .unwrap();

    let error = discover_gemini_agents_for_project(gemini_root.path(), project_root.path())
        .expect_err("invalid Gemini Agent must fail fast");

    assert!(error.contains("invalid Gemini Agent name"));
    assert!(error.contains("broken.md"));
}

#[test]
fn gemini_rejects_duplicate_agent_names_in_one_scope() {
    let gemini_root = tempfile::tempdir().unwrap();
    fs::create_dir_all(gemini_root.path().join("agents")).unwrap();
    for file in ["first.md", "second.md"] {
        fs::write(
            gemini_root.path().join("agents").join(file),
            "---\nname: reviewer\ndescription: Reviewer\n---\nPrompt\n",
        )
        .unwrap();
    }

    let error = discover_gemini_agents(gemini_root.path())
        .expect_err("ambiguous Gemini Agent names must fail fast");

    assert!(error.contains("duplicate Gemini Agent name: reviewer"));
    assert!(error.contains("first.md"));
    assert!(error.contains("second.md"));
}
