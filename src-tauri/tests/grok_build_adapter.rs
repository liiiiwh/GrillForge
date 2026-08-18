use grillforge_lib::adapters::grok_build::{
    GrokBuildAdapter, GrokBuildPaths, GrokBuildRequest, GrokBuildTakeoverStatus,
};
use std::fs;
use tempfile::tempdir;

#[cfg(unix)]
#[test]
fn grok_build_cli_inspection_reads_version() {
    use grillforge_lib::adapters::grok_build::inspect_grok_build_cli;
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let cli = temp.path().join("grok");
    fs::write(&cli, "#!/bin/sh\nprintf 'grok 0.2.112\\n'\n").unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(inspect_grok_build_cli(cli).unwrap().version, "grok 0.2.112");
}

#[cfg(unix)]
#[test]
fn grok_build_detector_checks_the_official_user_install_candidate() {
    use grillforge_lib::adapters::grok_build::detect_grok_build_cli_in;
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let cli = temp.path().join(".grok/bin/grok");
    fs::create_dir_all(cli.parent().unwrap()).unwrap();
    fs::write(&cli, "#!/bin/sh\nprintf 'grok user-install\n'\n").unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();

    let detected = detect_grok_build_cli_in([temp.path().join("missing/grok"), cli.clone()])
        .unwrap()
        .expect("user-installed Grok Build CLI");

    assert_eq!(detected.path, cli);
    assert_eq!(detected.version, "grok user-install");
}

#[test]
fn grok_build_projects_one_selected_responses_model_and_preserves_other_tables() {
    let temp = tempdir().unwrap();
    let config = temp.path().join(".grok/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        "[cli]\ninstaller = \"internal\"\n[mcp_servers.echo]\ncommand = \"echo\"\n",
    )
    .unwrap();
    let adapter = GrokBuildAdapter::new(
        GrokBuildPaths::new(config.clone()),
        temp.path().join(".grillforge"),
    );

    adapter
        .apply(
            GrokBuildRequest::new(
                "https://api.example.com/v1",
                "provider-secret",
                "deepseek-chat",
                "DeepSeek Chat",
                None,
            )
            .unwrap(),
        )
        .unwrap();

    let parsed = fs::read_to_string(&config)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert_eq!(parsed["cli"]["installer"].as_str(), Some("internal"));
    assert_eq!(
        parsed["mcp_servers"]["echo"]["command"].as_str(),
        Some("echo")
    );
    assert_eq!(parsed["models"]["default"].as_str(), Some("grillforge"));
    assert_eq!(
        parsed["model"]["grillforge"]["model"].as_str(),
        Some("deepseek-chat")
    );
    assert_eq!(
        parsed["model"]["grillforge"]["base_url"].as_str(),
        Some("https://api.example.com/v1")
    );
    assert_eq!(
        parsed["model"]["grillforge"]["api_backend"].as_str(),
        Some("responses")
    );
    assert_eq!(
        parsed["model"]["grillforge"]["context_window"].as_integer(),
        Some(500_000)
    );
    assert_eq!(
        adapter.status().unwrap().takeover,
        GrokBuildTakeoverStatus::Active
    );
}

#[test]
fn grok_build_disable_restores_exact_original_and_drift_fails_fast() {
    let temp = tempdir().unwrap();
    let config = temp.path().join(".grok/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = b"# official xAI OAuth state\n[cli]\ninstaller = \"npm\"\n";
    fs::write(&config, original).unwrap();
    let adapter = GrokBuildAdapter::new(
        GrokBuildPaths::new(config.clone()),
        temp.path().join(".grillforge"),
    );
    adapter
        .apply(
            GrokBuildRequest::new("http://127.0.0.1:9191/v1", "local-token", "coder", "Coder", None)
                .unwrap(),
        )
        .unwrap();

    fs::write(&config, b"changed outside GrillForge\n").unwrap();
    assert!(
        adapter
            .disable()
            .unwrap_err()
            .to_string()
            .contains("differs")
    );
    assert_eq!(
        adapter.status().unwrap().takeover,
        GrokBuildTakeoverStatus::Drifted
    );

    fs::write(&config, original).unwrap();
    // A fresh adapter without the previous snapshot proves exact normal restore.
    let clean_root = temp.path().join("clean-grillforge");
    let clean = GrokBuildAdapter::new(GrokBuildPaths::new(config.clone()), clean_root);
    clean
        .apply(
            GrokBuildRequest::new("https://api.example.com/v1", "secret", "model", "Model", None)
                .unwrap(),
        )
        .unwrap();
    clean.disable().unwrap();
    assert_eq!(fs::read(config).unwrap(), original);
}

#[test]
fn grok_build_request_rejects_non_base_or_unsafe_values_without_leaking_secret() {
    let error = GrokBuildRequest::new(
        "https://user:pass@example.com/v1",
        "do-not-print",
        "model",
        "Model",
        None,
    )
    .unwrap_err();
    assert!(!error.to_string().contains("do-not-print"));
    assert!(GrokBuildRequest::new("https://api.example.com/v1?x=1", "key", "m", "M", None).is_err());
    assert!(GrokBuildRequest::new("https://api.example.com/v1", "", "m", "M", None).is_err());
}
