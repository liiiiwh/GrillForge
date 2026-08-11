use grillforge_lib::adapters::openclaw::{
    OpenClawAdapter, OpenClawModelSpec, OpenClawPaths, OpenClawRequest, OpenClawTakeoverStatus,
    detect_openclaw_cli_in, inspect_openclaw_cli, paths_from_home,
};
use serde_json::{Value, json};
use std::fs;

fn model(id: &str, name: &str) -> OpenClawModelSpec {
    OpenClawModelSpec::new(
        id,
        name,
        true,
        vec!["text".into(), "image".into()],
        200_000,
        32_000,
    )
    .expect("model")
}

fn request(models: Vec<OpenClawModelSpec>, primary: &str, fallbacks: Vec<&str>) -> OpenClawRequest {
    OpenClawRequest::new(
        "http://127.0.0.1:15721",
        "local-openclaw-token",
        models,
        primary,
        fallbacks.into_iter().map(str::to_string).collect(),
    )
    .expect("request")
}

fn parse(path: &std::path::Path) -> Value {
    json5::from_str(&fs::read_to_string(path).expect("configuration")).expect("valid JSON5")
}

#[test]
fn openclaw_path_matches_the_real_client_layout() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        paths_from_home(home.path()).config_path,
        home.path().join(".openclaw/openclaw.json")
    );
}

#[cfg(unix)]
#[test]
fn cli_detection_reads_openclaw_version_and_fails_on_an_invalid_executable() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("openclaw");
    fs::write(&executable, "#!/bin/sh\nprintf 'OpenClaw 2026.8.1\\n'\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let detection = inspect_openclaw_cli(&executable).expect("detect CLI");
    assert_eq!(detection.path, executable);
    assert_eq!(detection.version, "OpenClaw 2026.8.1");

    let invalid = directory.path().join("invalid-openclaw");
    fs::write(&invalid, "#!/bin/sh\nexit 3\n").unwrap();
    fs::set_permissions(&invalid, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        inspect_openclaw_cli(&invalid)
            .expect_err("invalid CLI")
            .to_string()
            .contains("did not return a version")
    );
}

#[cfg(unix)]
#[test]
fn openclaw_detector_checks_the_official_local_prefix() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join(".openclaw/bin/openclaw");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, "#!/bin/sh\nprintf 'OpenClaw local-prefix\n'\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

    let detection = detect_openclaw_cli_in([
        directory.path().join("missing/openclaw"),
        executable.clone(),
    ])
    .unwrap()
    .expect("local-prefix OpenClaw CLI");

    assert_eq!(detection.path, executable);
    assert_eq!(detection.version, "OpenClaw local-prefix");
}

#[test]
fn request_validation_fails_fast_on_invalid_routes_or_pool_membership() {
    assert!(
        OpenClawModelSpec::new("not-managed", "Bad", false, vec!["text".into()], 100, 10,)
            .expect_err("route alias")
            .to_string()
            .contains("GrillForge route alias")
    );
    let models = vec![model("grillforge/main", "Main")];
    assert!(
        OpenClawRequest::new(
            "http://127.0.0.1:15721",
            "token",
            models.clone(),
            "grillforge/missing",
            vec![],
        )
        .expect_err("unknown primary")
        .to_string()
        .contains("primary model")
    );
    assert!(
        OpenClawRequest::new(
            "http://127.0.0.1:15721",
            "token",
            models,
            "grillforge/main",
            vec!["grillforge/main".into()],
        )
        .expect_err("primary fallback")
        .to_string()
        .contains("fallback")
    );
}

#[test]
fn apply_projects_owned_provider_primary_and_fallback_pool_without_losing_other_config() {
    let directory = tempfile::tempdir().unwrap();
    let grillforge = tempfile::tempdir().unwrap();
    let config = directory.path().join("openclaw.json");
    let original = br#"{
      // OpenClaw accepts JSON5 and existing settings must survive.
      ui: { theme: 'dark' },
      models: {
        mode: 'merge',
        providers: {
          existing: { baseUrl: 'https://existing.invalid', api: 'anthropic-messages', models: [{ id: 'native' }] },
        },
      },
      agents: {
        defaults: {
          timeoutSeconds: 45,
          model: { primary: 'existing/native', fallbacks: ['existing/backup'], custom: true },
          models: { 'existing/native': { alias: 'Existing' } },
        },
      },
    }"#;
    fs::write(&config, original).unwrap();
    let adapter = OpenClawAdapter::new(OpenClawPaths::new(&config), grillforge.path());
    let request = request(
        vec![
            model("grillforge/main", "Main Worker"),
            model("grillforge/review", "Review Worker"),
        ],
        "grillforge/main",
        vec!["grillforge/review"],
    );

    let status = adapter.apply(request.clone()).expect("apply");
    assert_eq!(status.takeover, OpenClawTakeoverStatus::Active);
    assert!(status.snapshot_present);
    let written = parse(&config);
    assert_eq!(written["ui"]["theme"], "dark");
    assert_eq!(written["models"]["mode"], "merge");
    assert_eq!(
        written["models"]["providers"]["existing"]["baseUrl"],
        "https://existing.invalid"
    );
    assert_eq!(
        written["models"]["providers"]["grillforge"],
        json!({
            "baseUrl": "http://127.0.0.1:15721",
            "apiKey": "local-openclaw-token",
            "api": "anthropic-messages",
            "models": [
                {
                    "id": "grillforge/main",
                    "name": "Main Worker",
                    "reasoning": true,
                    "input": ["text", "image"],
                    "contextWindow": 200000,
                    "maxTokens": 32000
                },
                {
                    "id": "grillforge/review",
                    "name": "Review Worker",
                    "reasoning": true,
                    "input": ["text", "image"],
                    "contextWindow": 200000,
                    "maxTokens": 32000
                }
            ]
        })
    );
    assert_eq!(
        written["agents"]["defaults"]["model"],
        json!({
            "primary": "grillforge/grillforge/main",
            "fallbacks": ["grillforge/grillforge/review"],
            "custom": true
        })
    );
    assert_eq!(written["agents"]["defaults"]["timeoutSeconds"], 45);
    assert_eq!(
        written["agents"]["defaults"]["models"],
        json!({
            "existing/native": {"alias": "Existing"},
            "grillforge/grillforge/main": {"alias": "Main Worker"},
            "grillforge/grillforge/review": {"alias": "Review Worker"}
        })
    );
    assert!(!format!("{request:?}").contains("local-openclaw-token"));

    adapter.disable().expect("restore");
    assert_eq!(fs::read(&config).unwrap(), original);
    assert!(!adapter.snapshot_path().exists());
}

#[test]
fn reapply_replaces_only_grillforge_owned_models_and_keeps_the_first_original_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let grillforge = tempfile::tempdir().unwrap();
    let config = directory.path().join("openclaw.json");
    let original = br#"{"models":{"providers":{}},"agents":{"defaults":{"models":{"other/model":{"alias":"Other"}}}}}"#;
    fs::write(&config, original).unwrap();
    let adapter = OpenClawAdapter::new(OpenClawPaths::new(&config), grillforge.path());
    adapter
        .apply(request(
            vec![
                model("grillforge/old", "Old"),
                model("grillforge/main", "Main"),
            ],
            "grillforge/main",
            vec!["grillforge/old"],
        ))
        .unwrap();
    adapter
        .apply(request(
            vec![model("grillforge/new", "New")],
            "grillforge/new",
            vec![],
        ))
        .expect("reapply");

    let written = parse(&config);
    let catalog = written["agents"]["defaults"]["models"].as_object().unwrap();
    assert!(catalog.contains_key("other/model"));
    assert!(catalog.contains_key("grillforge/grillforge/new"));
    assert!(!catalog.contains_key("grillforge/grillforge/old"));
    assert!(!catalog.contains_key("grillforge/grillforge/main"));
    adapter.disable().unwrap();
    assert_eq!(fs::read(&config).unwrap(), original);
}

#[test]
fn drift_blocks_apply_and_restore_while_retaining_the_recovery_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let grillforge = tempfile::tempdir().unwrap();
    let config = directory.path().join("openclaw.json");
    fs::write(&config, br#"{"keep":true}"#).unwrap();
    let adapter = OpenClawAdapter::new(OpenClawPaths::new(&config), grillforge.path());
    adapter
        .apply(request(
            vec![model("grillforge/main", "Main")],
            "grillforge/main",
            vec![],
        ))
        .unwrap();
    fs::write(&config, br#"{"external":"edit"}"#).unwrap();

    assert_eq!(
        adapter.status().unwrap().takeover,
        OpenClawTakeoverStatus::Drifted
    );
    assert!(
        adapter
            .disable()
            .expect_err("drift")
            .to_string()
            .contains("differs")
    );
    assert!(
        adapter
            .apply(request(
                vec![model("grillforge/next", "Next")],
                "grillforge/next",
                vec![],
            ))
            .expect_err("drift")
            .to_string()
            .contains("differs")
    );
    assert!(adapter.snapshot_path().is_file());
    assert_eq!(fs::read(&config).unwrap(), br#"{"external":"edit"}"#);
}

#[test]
fn absent_config_is_removed_on_restore_and_invalid_json5_is_never_modified() {
    let directory = tempfile::tempdir().unwrap();
    let grillforge = tempfile::tempdir().unwrap();
    let config = directory.path().join("openclaw.json");
    let adapter = OpenClawAdapter::new(OpenClawPaths::new(&config), grillforge.path());
    adapter
        .apply(request(
            vec![model("grillforge/main", "Main")],
            "grillforge/main",
            vec![],
        ))
        .expect("create config");
    assert!(config.is_file());
    adapter.disable().expect("remove generated config");
    assert!(!config.exists());

    fs::write(&config, "{ invalid JSON5").unwrap();
    let error = adapter
        .apply(request(
            vec![model("grillforge/main", "Main")],
            "grillforge/main",
            vec![],
        ))
        .expect_err("invalid config");
    assert!(error.to_string().contains("invalid JSON5"));
    assert_eq!(fs::read_to_string(&config).unwrap(), "{ invalid JSON5");
    assert!(!adapter.snapshot_path().exists());
}
