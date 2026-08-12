use grillforge_lib::local_agents::{
    discover_claude_builtin_agents, discover_claude_code_agents,
    discover_claude_code_agents_for_project,
};
use std::fs;

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
