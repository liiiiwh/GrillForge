use grillforge_lib::adapters::codex::{
    CodexAdapter, CodexModelSelection, CodexPaths, CodexProviderRequest, CodexRequest,
    CodexTakeoverStatus, detect_codex_cli_in, inspect_codex_native_models,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use tempfile::tempdir;

#[cfg(unix)]
#[test]
fn detector_accepts_a_chatgpt_bundled_codex_cli_outside_path() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir().unwrap();
    let bundled = temp.path().join("ChatGPT.app/Contents/Resources/codex");
    fs::create_dir_all(bundled.parent().unwrap()).unwrap();
    fs::write(&bundled, "#!/bin/sh\nprintf 'codex-cli bundled-test\\n'\n").unwrap();
    fs::set_permissions(&bundled, fs::Permissions::from_mode(0o755)).unwrap();

    let detected = detect_codex_cli_in([temp.path().join("missing/codex"), bundled.clone()])
        .unwrap()
        .expect("bundled Codex CLI");

    assert_eq!(detected.path, bundled);
    assert_eq!(detected.version, "codex-cli bundled-test");
}

#[test]
fn codex_reports_the_model_from_the_current_real_configuration() {
    let temp = tempdir().unwrap();
    let codex = temp.path().join("home/.codex");
    fs::create_dir_all(&codex).unwrap();
    let config = codex.join("config.toml");
    fs::write(
        &config,
        "model = \"gpt-5.6-sol\"\nmodel_provider = \"openai\"\napproval_policy = \"on-request\"\n",
    )
    .unwrap();
    let adapter = CodexAdapter::new(CodexPaths::new(config), temp.path().join("grillforge"));

    let configured = adapter.configured_model().unwrap().unwrap();

    assert_eq!(configured.model, "gpt-5.6-sol");
    assert_eq!(configured.provider.as_deref(), Some("openai"));
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires an installed ChatGPT.app with its bundled Codex CLI"]
fn installed_chatgpt_codex_cli_is_detected() {
    let detected = grillforge_lib::adapters::codex::detect_codex_cli()
        .unwrap()
        .expect("installed ChatGPT Codex CLI");

    assert!(detected.path.is_file());
    assert!(detected.version.starts_with("codex-cli "));
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires an installed Codex CLI with a bundled model catalog"]
fn installed_chatgpt_codex_model_catalog_is_readable() {
    let detected = grillforge_lib::adapters::codex::detect_codex_cli()
        .unwrap()
        .expect("installed ChatGPT Codex CLI");
    let models = grillforge_lib::adapters::codex::inspect_codex_native_models(&detected.path)
        .expect("bundled model catalog");

    assert!(!models.is_empty());
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires an installed Codex CLI"]
fn installed_codex_cli_accepts_the_generated_native_configuration() {
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration;

    let detected = grillforge_lib::adapters::codex::detect_codex_cli()
        .unwrap()
        .expect("installed Codex CLI");
    let temp = tempdir().unwrap();
    let codex_home = temp.path().join(".codex");
    CodexAdapter::new(
        CodexPaths::new(codex_home.join("config.toml")),
        temp.path().join("grillforge"),
    )
    .apply(CodexRequest::native("gpt-5.6-sol").unwrap())
    .unwrap();

    let mut child = Command::new(detected.path)
        .args(["app-server", "--strict-config", "--listen", "stdio://"])
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(200));

    assert!(child.try_wait().unwrap().is_none());
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn codex_apply_preserves_unrelated_toml_and_does_not_touch_auth() {
    let temp = tempdir().unwrap();
    let codex = temp.path().join("home/.codex");
    fs::create_dir_all(&codex).unwrap();
    let config = codex.join("config.toml");
    let auth = codex.join("auth.json");
    fs::write(
        &config,
        "approval_policy = \"on-request\"\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n",
    )
    .unwrap();
    fs::write(&auth, b"{\"tokens\":\"must-stay-exact\"}\n").unwrap();
    let adapter = CodexAdapter::new(
        CodexPaths::new(config.clone()),
        temp.path().join("grillforge"),
    );

    adapter
        .apply(
            CodexRequest::new(
                "https://api.example.com/v1",
                "provider-secret",
                "gpt-5-codex",
            )
            .unwrap(),
        )
        .unwrap();

    let written = fs::read_to_string(&config).unwrap();
    let parsed = written.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(parsed["approval_policy"].as_str(), Some("on-request"));
    assert_eq!(
        parsed["projects"]["/work"]["trust_level"].as_str(),
        Some("trusted")
    );
    assert_eq!(parsed["model"].as_str(), Some("gpt-5-codex"));
    assert_eq!(parsed["model_provider"].as_str(), Some("grillforge"));
    assert_eq!(
        parsed["model_providers"]["grillforge"]["wire_api"].as_str(),
        Some("responses")
    );
    assert_eq!(
        parsed["model_providers"]["grillforge"]["experimental_bearer_token"].as_str(),
        Some("provider-secret")
    );
    assert_eq!(
        fs::read(&auth).unwrap(),
        b"{\"tokens\":\"must-stay-exact\"}\n"
    );
    assert_eq!(
        adapter.status().unwrap().takeover,
        CodexTakeoverStatus::Active
    );
}

#[test]
fn codex_disable_restores_exact_original_config() {
    let temp = tempdir().unwrap();
    let config = temp.path().join(".codex/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = b"model = \"native\"\n# keep formatting\n";
    fs::write(&config, original).unwrap();
    let adapter = CodexAdapter::new(
        CodexPaths::new(config.clone()),
        temp.path().join("grillforge"),
    );
    adapter
        .apply(CodexRequest::new("http://127.0.0.1:8080/v1", "token", "coder").unwrap())
        .unwrap();
    adapter.disable().unwrap();
    assert_eq!(fs::read(config).unwrap(), original);
    assert_eq!(
        adapter.status().unwrap().takeover,
        CodexTakeoverStatus::Inactive
    );
}

#[test]
fn codex_apply_configures_real_main_default_subagent_and_custom_codex_agent_models() {
    let temp = tempdir().unwrap();
    let codex = temp.path().join("home/.codex");
    let agents = codex.join("agents");
    fs::create_dir_all(&agents).unwrap();
    let config = codex.join("config.toml");
    let reviewer = agents.join("reviewer.toml");
    let original_config = b"approval_policy = \"on-request\"\n";
    let original_reviewer = b"name = \"reviewer\"\ndescription = \"Review changes\"\ndeveloper_instructions = \"Review only\"\n";
    fs::write(&config, original_config).unwrap();
    fs::write(&reviewer, original_reviewer).unwrap();
    let adapter = CodexAdapter::new(
        CodexPaths::new(config.clone()),
        temp.path().join("grillforge"),
    );
    let deepseek = CodexProviderRequest::new(
        "deepseek",
        "DeepSeek",
        "https://api.deepseek.example/v1",
        "deepseek-secret",
    )
    .unwrap();
    let openrouter = CodexProviderRequest::new(
        "openrouter",
        "OpenRouter",
        "https://openrouter.example/api/v1",
        "openrouter-secret",
    )
    .unwrap();
    let mut custom_agents = BTreeMap::new();
    custom_agents.insert(
        "reviewer".to_string(),
        CodexModelSelection::managed(openrouter, "review-model").unwrap(),
    );
    let request = CodexRequest::from_selections(
        CodexModelSelection::managed(deepseek.clone(), "main-model").unwrap(),
        Some(CodexModelSelection::managed(deepseek, "worker-model").unwrap()),
        custom_agents,
    )
    .unwrap();

    adapter.apply(request).unwrap();

    let written = fs::read_to_string(&config)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert_eq!(written["model"].as_str(), Some("main-model"));
    assert_eq!(
        written["model_provider"].as_str(),
        Some("grillforge_deepseek")
    );
    assert_eq!(
        written["agents"]["default_subagent_model"].as_str(),
        Some("worker-model")
    );
    assert_eq!(
        written["model_providers"]["grillforge_openrouter"]["base_url"].as_str(),
        Some("https://openrouter.example/api/v1")
    );
    let written_reviewer = fs::read_to_string(&reviewer)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert_eq!(written_reviewer["model"].as_str(), Some("review-model"));
    assert_eq!(
        written_reviewer["model_provider"].as_str(),
        Some("grillforge_openrouter")
    );

    adapter.disable().unwrap();
    assert_eq!(fs::read(config).unwrap(), original_config);
    assert_eq!(fs::read(reviewer).unwrap(), original_reviewer);
}

#[cfg(unix)]
#[test]
fn bundled_codex_model_catalog_exposes_only_visible_native_models() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir().unwrap();
    let executable = temp.path().join("codex");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' '{\"models\":[{\"slug\":\"gpt-visible\",\"display_name\":\"GPT Visible\",\"visibility\":\"list\"},{\"slug\":\"gpt-hidden\",\"display_name\":\"GPT Hidden\",\"visibility\":\"hide\"}]}'\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

    let models = inspect_codex_native_models(&executable).unwrap();

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-visible");
    assert_eq!(models[0].name, "GPT Visible");
}

#[cfg(unix)]
#[test]
fn bundled_codex_model_catalog_drains_output_larger_than_a_process_pipe() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir().unwrap();
    let executable = temp.path().join("codex");
    let mut catalog = (0..5_000)
        .map(|index| {
            json!({
                "slug": format!("hidden-{index}"),
                "display_name": format!("Hidden model {index}"),
                "visibility": "hide"
            })
        })
        .collect::<Vec<_>>();
    catalog.push(json!({
        "slug": "gpt-visible",
        "display_name": "GPT Visible",
        "visibility": "list"
    }));
    let output = serde_json::to_string(&json!({"models": catalog})).unwrap();
    assert!(output.len() > 64 * 1024);
    fs::write(&executable, format!("#!/bin/sh\nprintf '%s' '{output}'\n")).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

    let models = inspect_codex_native_models(&executable).unwrap();

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-visible");
}

#[test]
fn native_codex_selection_uses_the_builtin_openai_provider() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.codex/config.toml");
    let adapter = CodexAdapter::new(
        CodexPaths::new(config.clone()),
        temp.path().join("grillforge"),
    );
    let request = CodexRequest::from_selections(
        CodexModelSelection::native("gpt-main").unwrap(),
        Some(CodexModelSelection::native("gpt-worker").unwrap()),
        BTreeMap::new(),
    )
    .unwrap();

    adapter.apply(request).unwrap();

    let written = fs::read_to_string(config)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert_eq!(written["model"].as_str(), Some("gpt-main"));
    assert_eq!(written["model_provider"].as_str(), Some("openai"));
    assert_eq!(
        written["agents"]["default_subagent_model"].as_str(),
        Some("gpt-worker")
    );
}

#[test]
fn malformed_custom_agent_returns_an_actionable_error() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.codex/config.toml");
    let agents = config.parent().unwrap().join("agents");
    fs::create_dir_all(&agents).unwrap();
    fs::write(
        agents.join("reviewer.toml"),
        "name = \"reviewer\"\ndescription = \"Review changes\"\n",
    )
    .unwrap();
    let adapter = CodexAdapter::new(CodexPaths::new(config), temp.path().join("grillforge"));

    let error = adapter.custom_agents().unwrap_err();

    assert_eq!(
        error.to_string(),
        "Codex Agent file reviewer.toml must define developer_instructions"
    );
}
