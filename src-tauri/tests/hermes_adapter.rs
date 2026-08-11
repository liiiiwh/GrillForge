use grillforge_lib::adapters::hermes::{
    HermesAdapter, HermesAdapterError, HermesModel, HermesPaths, HermesRequest,
    HermesTakeoverStatus, detect_hermes_cli_in, paths_from_home,
};
use serde_yaml::Value;
use std::fs;
use tempfile::tempdir;

#[test]
fn hermes_paths_follow_the_upstream_default_location() {
    assert_eq!(
        paths_from_home("/home/tester").config_path,
        std::path::PathBuf::from("/home/tester/.hermes/config.yaml")
    );
}

#[cfg(unix)]
#[test]
fn hermes_detector_checks_the_official_per_user_install() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let cli = temp.path().join(".local/bin/hermes");
    fs::create_dir_all(cli.parent().unwrap()).unwrap();
    fs::write(&cli, "#!/bin/sh\nprintf 'hermes user-install\n'\n").unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();

    let detected = detect_hermes_cli_in([temp.path().join("missing/hermes"), cli.clone()])
        .unwrap()
        .expect("per-user Hermes CLI");

    assert_eq!(detected.path, cli);
    assert_eq!(detected.version, "hermes user-install");
}

fn request() -> HermesRequest {
    HermesRequest::new(
        "http://127.0.0.1:19191/v1",
        "gateway-token",
        vec![
            HermesModel::new("grillforge/coder", "Coder").unwrap(),
            HermesModel::new("grillforge/reviewer", "Reviewer").unwrap(),
        ],
        "grillforge/coder",
    )
    .unwrap()
}

#[test]
fn hermes_apply_preserves_unrelated_yaml_and_sets_real_provider_defaults() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.hermes/config.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        r#"# user comment must stay
agent:
  max_turns: 50
custom_providers:
  - name: native
    base_url: https://example.com/v1
    api_key: native-secret
    model: native/model
    models:
      native/model: {}
  - name: grillforge
    base_url: http://127.0.0.1:1/v1
    api_key: old-token
    request_timeout_seconds: 30
model:
  default: native/model
  provider: native
  reasoning_effort: high
"#,
    )
    .unwrap();
    let adapter = HermesAdapter::new(
        HermesPaths::new(config.clone()),
        temp.path().join("grillforge"),
    );

    let status = adapter.apply(request()).unwrap();
    let written = fs::read_to_string(&config).unwrap();
    let value: Value = serde_yaml::from_str(&written).unwrap();
    assert!(written.contains("# user comment must stay"));
    assert_eq!(value["agent"]["max_turns"], 50);
    assert_eq!(value["model"]["default"], "grillforge/coder");
    assert_eq!(value["model"]["provider"], "grillforge");
    assert_eq!(value["model"]["reasoning_effort"], "high");
    let providers = value["custom_providers"].as_sequence().unwrap();
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0]["name"], "native");
    let managed = providers
        .iter()
        .find(|provider| provider["name"] == "grillforge")
        .unwrap();
    assert_eq!(managed["base_url"], "http://127.0.0.1:19191/v1");
    assert_eq!(managed["api_key"], "gateway-token");
    assert_eq!(managed["api_mode"], "anthropic_messages");
    assert_eq!(managed["model"], "grillforge/coder");
    assert_eq!(managed["request_timeout_seconds"], 30);
    assert!(managed["models"]["grillforge/coder"].is_mapping());
    assert!(managed["models"]["grillforge/reviewer"].is_mapping());
    assert_eq!(status.takeover, HermesTakeoverStatus::Active);
}

#[test]
fn hermes_disable_restores_exact_original_yaml() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.hermes/config.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = b"# exact formatting\nagent: {max_turns: 7}\n";
    fs::write(&config, original).unwrap();
    let adapter = HermesAdapter::new(
        HermesPaths::new(config.clone()),
        temp.path().join("grillforge"),
    );

    adapter.apply(request()).unwrap();
    adapter.disable().unwrap();

    assert_eq!(fs::read(&config).unwrap(), original);
    assert_eq!(
        adapter.status().unwrap().takeover,
        HermesTakeoverStatus::Inactive
    );
}

#[test]
fn hermes_detects_drift_and_refuses_apply_or_restore() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.hermes/config.yaml");
    let adapter = HermesAdapter::new(
        HermesPaths::new(config.clone()),
        temp.path().join("grillforge"),
    );
    adapter.apply(request()).unwrap();
    fs::write(&config, b"agent:\n  changed: true\n").unwrap();

    assert_eq!(
        adapter.status().unwrap().takeover,
        HermesTakeoverStatus::Drifted
    );
    assert!(matches!(
        adapter.apply(request()),
        Err(HermesAdapterError::Drifted)
    ));
    assert!(matches!(
        adapter.disable(),
        Err(HermesAdapterError::Drifted)
    ));
}

#[test]
fn hermes_invalid_yaml_shapes_fail_without_overwriting_user_config() {
    for original in [
        b"model: [broken".as_slice(),
        b"- root\n- sequence\n".as_slice(),
        b"custom_providers: wrong\n".as_slice(),
        b"model: wrong\n".as_slice(),
        b"custom_providers:\n  - base_url: http://example.test\n".as_slice(),
    ] {
        let temp = tempdir().unwrap();
        let config = temp.path().join("home/.hermes/config.yaml");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, original).unwrap();
        let root = temp.path().join("grillforge");
        let adapter = HermesAdapter::new(HermesPaths::new(config.clone()), &root);

        assert!(adapter.apply(request()).is_err());
        assert_eq!(fs::read(&config).unwrap(), original);
        assert!(!root.join("hermes.snapshot.json").exists());
    }
}

#[test]
fn hermes_request_rejects_invalid_routes_and_redacts_the_token() {
    let model = HermesModel::new("grillforge/coder", "Coder").unwrap();
    assert!(HermesModel::new("model/coder", "Coder").is_err());
    assert!(
        HermesRequest::new(
            "https://example.com/v1",
            "token",
            vec![model.clone()],
            "grillforge/coder"
        )
        .is_err()
    );
    assert!(
        HermesRequest::new(
            "http://127.0.0.1:8080/v1",
            " token",
            vec![model.clone()],
            "grillforge/coder"
        )
        .is_err()
    );
    assert!(
        HermesRequest::new(
            "http://127.0.0.1:8080/v1",
            "token",
            vec![model.clone(), model.clone()],
            "grillforge/coder"
        )
        .is_err()
    );
    assert!(
        HermesRequest::new(
            "http://127.0.0.1:8080/v1",
            "token",
            vec![model],
            "grillforge/missing"
        )
        .is_err()
    );
    assert!(!format!("{:?}", request()).contains("gateway-token"));
}

#[cfg(unix)]
#[test]
fn hermes_cli_inspection_reads_the_real_version_command() {
    use grillforge_lib::adapters::hermes::inspect_hermes_cli;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir().unwrap();
    let executable = temp.path().join("hermes");
    fs::write(&executable, b"#!/bin/sh\nprintf '0.9.0\\n'\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

    let detected = inspect_hermes_cli(&executable).unwrap();
    assert_eq!(detected.path, executable);
    assert_eq!(detected.version, "0.9.0");
}
