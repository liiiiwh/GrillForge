use grillforge_lib::adapters::kimi_code::{
    KimiCodeAdapter, KimiCodeModel, KimiCodePaths, KimiCodeRequest, KimiCodeTakeoverStatus,
    detect_kimi_code_cli_in, discover_kimi_code_agents, paths_from_home,
};
use std::fs;
use tempfile::tempdir;
use toml_edit::DocumentMut;

fn request() -> KimiCodeRequest {
    KimiCodeRequest::new(
        "http://127.0.0.1:19191/clients/kimi-code",
        "gateway-token",
        vec![
            KimiCodeModel::new("grillforge/coder", ["thinking"]).unwrap(),
            KimiCodeModel::new("grillforge/reviewer", ["image_in"]).unwrap(),
        ],
        "grillforge/coder",
    )
    .unwrap()
}

#[test]
fn kimi_code_uses_the_current_share_directory() {
    let paths = paths_from_home("/Users/tester");

    assert_eq!(
        paths.config_path,
        std::path::Path::new("/Users/tester/.kimi/config.toml")
    );
}

#[test]
fn kimi_code_rejects_capabilities_outside_the_current_cli_schema() {
    let error = KimiCodeModel::new("grillforge/coder", ["tool_use"])
        .expect_err("Kimi Code 1.49 does not expose a tool_use model capability");

    assert!(error.to_string().contains("capabilities"));
}

#[test]
fn kimi_code_apply_preserves_user_toml_and_writes_the_default_model_pool() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.kimi/config.toml");
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
    let adapter = KimiCodeAdapter::new(KimiCodePaths::new(&config), temp.path().join("grillforge"));

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
    assert!(written.get("secondary_model").is_none());
    assert_eq!(
        written["models"]["grillforge/coder"]["provider"].as_str(),
        Some("grillforge")
    );
    assert!(
        written["models"]["grillforge/coder"]
            .get("display_name")
            .is_none()
    );
    assert_eq!(status.takeover, KimiCodeTakeoverStatus::Active);
}

#[test]
fn kimi_code_disable_restores_exact_original_bytes_and_drift_refuses_overwrite() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.kimi/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = b"# keep this comment\ndefault_permission_mode = \"manual\"\n";
    fs::write(&config, original).unwrap();
    let adapter = KimiCodeAdapter::new(KimiCodePaths::new(&config), temp.path().join("grillforge"));

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
fn kimi_code_exposes_only_the_two_selectable_builtin_agents() {
    let agents = discover_kimi_code_agents();

    assert_eq!(
        agents
            .iter()
            .map(|agent| agent.name.as_str())
            .collect::<Vec<_>>(),
        ["default", "okabe"]
    );
}
