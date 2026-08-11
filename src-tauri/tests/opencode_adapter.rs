use grillforge_lib::adapters::opencode::{
    OpenCodeAdapter, OpenCodeAdapterError, OpenCodeModel, OpenCodePaths, OpenCodeRequest,
    OpenCodeTakeoverStatus, detect_opencode_cli_in, paths_from_home,
};
use serde_json::Value;
use std::fs;
use tempfile::tempdir;

#[test]
fn opencode_paths_follow_the_upstream_xdg_location() {
    let paths = paths_from_home("/home/tester");
    assert_eq!(
        paths.config_path,
        std::path::PathBuf::from("/home/tester/.config/opencode/opencode.json")
    );
}

#[cfg(unix)]
#[test]
fn opencode_detector_accepts_the_desktop_bundled_cli() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let cli = temp.path().join("OpenCode.app/Contents/MacOS/opencode-cli");
    fs::create_dir_all(cli.parent().unwrap()).unwrap();
    fs::write(&cli, "#!/bin/sh\nprintf 'opencode bundled-test\n'\n").unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();

    let detected = detect_opencode_cli_in([temp.path().join("missing/opencode"), cli.clone()])
        .unwrap()
        .expect("OpenCode desktop sidecar");

    assert_eq!(detected.path, cli);
    assert_eq!(detected.version, "opencode bundled-test");
}

fn request() -> OpenCodeRequest {
    OpenCodeRequest::new(
        "http://127.0.0.1:19191/v1",
        "gateway-token",
        vec![
            OpenCodeModel::new("grillforge/coder", "Coder").unwrap(),
            OpenCodeModel::new("grillforge/reviewer", "Reviewer").unwrap(),
        ],
        "grillforge/coder",
    )
    .unwrap()
}

#[test]
fn opencode_apply_reads_json5_and_preserves_unrelated_configuration() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.config/opencode/opencode.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        r#"{
          // user-owned JSON5 fields
          "$schema": "https://opencode.ai/config.json",
          "theme": "system",
          "provider": {
            "native": { "npm": "@ai-sdk/anthropic", },
          },
        }"#,
    )
    .unwrap();
    let adapter = OpenCodeAdapter::new(
        OpenCodePaths::new(config.clone()),
        temp.path().join("grillforge"),
    );

    let status = adapter.apply(request()).unwrap();
    let written: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    assert_eq!(written["$schema"], "https://opencode.ai/config.json");
    assert_eq!(written["theme"], "system");
    assert_eq!(written["provider"]["native"]["npm"], "@ai-sdk/anthropic");
    assert_eq!(
        written["provider"]["grillforge"]["npm"],
        "@ai-sdk/anthropic"
    );
    assert_eq!(
        written["provider"]["grillforge"]["options"]["baseURL"],
        "http://127.0.0.1:19191/v1"
    );
    assert_eq!(
        written["provider"]["grillforge"]["options"]["apiKey"],
        "gateway-token"
    );
    assert_eq!(
        written["provider"]["grillforge"]["models"]["grillforge/coder"]["name"],
        "Coder"
    );
    assert_eq!(
        written["provider"]["grillforge"]["models"]["grillforge/reviewer"]["name"],
        "Reviewer"
    );
    assert_eq!(written["model"], "grillforge/grillforge/coder");
    assert_eq!(status.takeover, OpenCodeTakeoverStatus::Active);
}

#[test]
fn opencode_disable_restores_the_exact_original_json5_bytes() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.config/opencode/opencode.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = b"{\n  // exact user formatting\n  theme: 'system',\n}\n";
    fs::write(&config, original).unwrap();
    let adapter = OpenCodeAdapter::new(
        OpenCodePaths::new(config.clone()),
        temp.path().join("grillforge"),
    );

    adapter.apply(request()).unwrap();
    adapter.disable().unwrap();

    assert_eq!(fs::read(&config).unwrap(), original);
    let status = adapter.status().unwrap();
    assert!(!status.snapshot_present);
    assert_eq!(status.takeover, OpenCodeTakeoverStatus::Inactive);
}

#[test]
fn opencode_detects_drift_and_refuses_apply_or_restore() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.config/opencode/opencode.json");
    let adapter = OpenCodeAdapter::new(
        OpenCodePaths::new(config.clone()),
        temp.path().join("grillforge"),
    );
    adapter.apply(request()).unwrap();
    fs::write(&config, b"{\"theme\":\"changed-elsewhere\"}").unwrap();

    assert_eq!(
        adapter.status().unwrap().takeover,
        OpenCodeTakeoverStatus::Drifted
    );
    assert!(matches!(
        adapter.apply(request()),
        Err(OpenCodeAdapterError::Drifted)
    ));
    assert!(matches!(
        adapter.disable(),
        Err(OpenCodeAdapterError::Drifted)
    ));
}

#[test]
fn opencode_invalid_json5_shapes_fail_without_overwriting_user_config() {
    for original in [
        b"{ broken".as_slice(),
        b"['not', 'an', 'object']".as_slice(),
        b"{ provider: 'must-be-an-object' }".as_slice(),
    ] {
        let temp = tempdir().unwrap();
        let config = temp.path().join("home/.config/opencode/opencode.json");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, original).unwrap();
        let root = temp.path().join("grillforge");
        let adapter = OpenCodeAdapter::new(OpenCodePaths::new(config.clone()), &root);

        assert!(adapter.apply(request()).is_err());
        assert_eq!(fs::read(&config).unwrap(), original);
        assert!(!root.join("opencode.snapshot.json").exists());
    }
}

#[test]
fn opencode_first_apply_creates_the_official_schema_and_default_model() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("home/.config/opencode/opencode.json");
    let adapter = OpenCodeAdapter::new(
        OpenCodePaths::new(config.clone()),
        temp.path().join("grillforge"),
    );

    adapter.apply(request()).unwrap();

    let value: Value = serde_json::from_slice(&fs::read(config).unwrap()).unwrap();
    assert_eq!(value["$schema"], "https://opencode.ai/config.json");
    assert_eq!(value["model"], "grillforge/grillforge/coder");
}

#[test]
fn opencode_request_rejects_invalid_or_ambiguous_model_pools_and_redacts_token() {
    let model = OpenCodeModel::new("grillforge/coder", "Coder").unwrap();
    assert!(
        OpenCodeRequest::new(
            "https://example.com/v1",
            "token",
            vec![model.clone()],
            "grillforge/coder"
        )
        .is_err()
    );
    assert!(
        OpenCodeRequest::new(
            "http://127.0.0.1:8080/v1",
            " token",
            vec![model.clone()],
            "grillforge/coder"
        )
        .is_err()
    );
    assert!(
        OpenCodeRequest::new(
            "http://127.0.0.1:8080/v1",
            "token",
            vec![model.clone(), model.clone()],
            "grillforge/coder"
        )
        .is_err()
    );
    assert!(
        OpenCodeRequest::new(
            "http://127.0.0.1:8080/v1",
            "token",
            vec![model],
            "grillforge/missing"
        )
        .is_err()
    );
    assert!(OpenCodeModel::new("raw-upstream-id", "Coder").is_err());
    assert!(!format!("{:?}", request()).contains("gateway-token"));
}

#[cfg(unix)]
#[test]
fn opencode_cli_inspection_reads_the_real_version_command() {
    use grillforge_lib::adapters::opencode::inspect_opencode_cli;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir().unwrap();
    let executable = temp.path().join("opencode");
    fs::write(&executable, b"#!/bin/sh\nprintf '1.2.3\\n'\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

    let detected = inspect_opencode_cli(&executable).unwrap();
    assert_eq!(detected.path, executable);
    assert_eq!(detected.version, "1.2.3");
}
