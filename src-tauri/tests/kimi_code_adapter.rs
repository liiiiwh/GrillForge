use grillforge_lib::adapters::kimi_code::{
    KimiCodeAdapter, KimiCodeModel, KimiCodePaths, KimiCodeRequest, KimiCodeTakeoverStatus,
    detect_kimi_code_cli_in, discover_kimi_code_agents, set_kimi_code_agent_model_preference,
};
use std::fs;
use tempfile::tempdir;
use toml_edit::DocumentMut;

fn request() -> KimiCodeRequest {
    KimiCodeRequest::new(
        "http://127.0.0.1:19191/clients/kimi-code",
        "gateway-token",
        vec![
            KimiCodeModel::new("grillforge/coder", "Coder", ["tool_use"]).unwrap(),
            KimiCodeModel::new("grillforge/reviewer", "Reviewer", ["tool_use"]).unwrap(),
        ],
        "grillforge/coder",
        Some("grillforge/reviewer"),
    )
    .unwrap()
}

#[test]
fn kimi_code_apply_preserves_user_toml_and_writes_primary_and_secondary_models() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.kimi-code/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        r#"telemetry = false
default_permission_mode = "manual"

[providers.user]
type = "openai"
base_url = "https://example.test/v1"
api_key = "user-secret"
"#,
    )
    .unwrap();
    let adapter = KimiCodeAdapter::new(
        KimiCodePaths::new(
            &config,
            temp.path().join("home/.kimi-code/agents"),
            temp.path().join("home/.agents/agents"),
        ),
        temp.path().join("grillforge"),
    );

    let status = adapter.apply(request()).unwrap();
    let written = fs::read_to_string(&config)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();

    assert!(!written["telemetry"].as_bool().unwrap());
    assert_eq!(
        written["providers"]["user"]["api_key"].as_str(),
        Some("user-secret")
    );
    assert_eq!(
        written["providers"]["grillforge"]["type"].as_str(),
        Some("anthropic")
    );
    assert_eq!(
        written["providers"]["grillforge"]["base_url"].as_str(),
        Some("http://127.0.0.1:19191/clients/kimi-code")
    );
    assert_eq!(written["default_model"].as_str(), Some("grillforge/coder"));
    assert_eq!(
        written["secondary_model"]["model"].as_str(),
        Some("grillforge/reviewer")
    );
    assert_eq!(
        written["models"]["grillforge/coder"]["provider"].as_str(),
        Some("grillforge")
    );
    assert_eq!(status.takeover, KimiCodeTakeoverStatus::Active);
}

#[test]
fn kimi_code_disable_restores_exact_original_bytes_and_drift_refuses_overwrite() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.kimi-code/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = b"# keep this comment\ndefault_permission_mode = \"manual\"\n";
    fs::write(&config, original).unwrap();
    let adapter = KimiCodeAdapter::new(
        KimiCodePaths::new(
            &config,
            temp.path().join("home/.kimi-code/agents"),
            temp.path().join("home/.agents/agents"),
        ),
        temp.path().join("grillforge"),
    );

    adapter.apply(request()).unwrap();
    fs::write(&config, "user_changed = true\n").unwrap();
    assert!(adapter.disable().is_err());
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "user_changed = true\n"
    );

    adapter
        .apply(request())
        .expect_err("drift must require an explicit reapply path");
    fs::write(&config, adapter_snapshot_applied(adapter.snapshot_path())).unwrap();
    assert_eq!(
        adapter.disable().unwrap().takeover,
        KimiCodeTakeoverStatus::Inactive
    );
    assert_eq!(fs::read(&config).unwrap(), original);
}

fn adapter_snapshot_applied(path: &std::path::Path) -> Vec<u8> {
    let snapshot: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    snapshot["applied"]
        .as_array()
        .unwrap()
        .iter()
        .map(|byte| byte.as_u64().unwrap() as u8)
        .collect()
}

#[cfg(unix)]
#[test]
fn kimi_code_cli_detection_executes_the_real_candidate() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir().unwrap();
    let cli = temp.path().join("bin/kimi");
    fs::create_dir_all(cli.parent().unwrap()).unwrap();
    fs::write(&cli, "#!/bin/sh\necho 'kimi 1.7.0'\n").unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();

    let detection = detect_kimi_code_cli_in([temp.path().join("missing"), cli.clone()])
        .unwrap()
        .unwrap();
    assert_eq!(detection.path, cli);
    assert_eq!(detection.version, "kimi 1.7.0");
}

#[test]
fn kimi_code_agent_discovery_syncs_built_in_and_persistent_user_agents() {
    let temp = tempdir().unwrap();
    let user = temp.path().join(".kimi-code/agents");
    let shared = temp.path().join(".agents/agents");
    fs::create_dir_all(user.join("team")).unwrap();
    fs::create_dir_all(&shared).unwrap();
    fs::write(
        user.join("team/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews changes\nmodel_preference: secondary\n---\nReview carefully.\n",
    )
    .unwrap();
    fs::write(
        shared.join("researcher.md"),
        "---\nname: researcher\ndescription: Researches APIs\nmodel_preference: primary\n---\nResearch.\n",
    )
    .unwrap();

    let agents = discover_kimi_code_agents(&KimiCodePaths::new(
        temp.path().join(".kimi-code/config.toml"),
        &user,
        &shared,
    ))
    .unwrap();

    assert!(
        agents
            .iter()
            .any(|agent| agent.name == "coder" && agent.built_in)
    );
    assert!(agents.iter().any(|agent| {
        agent.name == "reviewer"
            && agent.model_preference.as_deref() == Some("secondary")
            && !agent.built_in
    }));
    assert!(agents.iter().any(|agent| agent.name == "researcher"));

    let changed = set_kimi_code_agent_model_preference(
        &KimiCodePaths::new(temp.path().join(".kimi-code/config.toml"), &user, &shared),
        "reviewer",
        "primary",
    )
    .unwrap();
    assert_eq!(changed.model_preference.as_deref(), Some("primary"));
    let reviewer = fs::read_to_string(user.join("team/reviewer.md")).unwrap();
    assert!(reviewer.contains("model_preference: primary"));
    assert!(reviewer.ends_with("Review carefully.\n"));

    let error = set_kimi_code_agent_model_preference(
        &KimiCodePaths::new(temp.path().join(".kimi-code/config.toml"), &user, &shared),
        "coder",
        "secondary",
    )
    .unwrap_err();
    assert!(error.to_string().contains("built-in"));
}
