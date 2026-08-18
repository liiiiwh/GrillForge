use grillforge_lib::adapters::pi::{
    PiAdapter, PiModelSpec, PiPaths, PiRequest, PiTakeoverStatus, detect_pi_cli_in, inspect_pi_cli,
    paths_from_home, pi_cli_candidates_from_home,
};
use serde_json::{Value, json};
use std::fs;
use tempfile::tempdir;

fn model(id: &str) -> PiModelSpec {
    PiModelSpec::new(id, id, true, vec!["text".into()], 128_000, 16_384).unwrap()
}

#[test]
fn apply_projects_models_and_preserves_unrelated_pi_configuration() {
    let temp = tempdir().unwrap();
    let agent = temp.path().join("home/.pi/agent");
    fs::create_dir_all(&agent).unwrap();
    let models_path = agent.join("models.json");
    let settings_path = agent.join("settings.json");
    fs::write(
        &models_path,
        serde_json::to_vec_pretty(&json!({
            "providers": {"personal": {"baseUrl": "http://example.test", "models": []}},
            "unrelated": true
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &settings_path,
        serde_json::to_vec_pretty(&json!({"theme": "dark", "defaultThinkingLevel": "high"}))
            .unwrap(),
    )
    .unwrap();

    let adapter = PiAdapter::new(
        PiPaths::new(models_path.clone(), settings_path.clone()),
        temp.path().join("grillforge"),
    );
    let request = PiRequest::new(
        "http://127.0.0.1:15721",
        "gateway-secret",
        vec![model("grillforge/deepseek-chat"), model("grillforge/coder")],
        Some("grillforge/deepseek-chat".into()),
    )
    .unwrap();
    adapter.apply(request).unwrap();

    let models: Value = serde_json::from_slice(&fs::read(&models_path).unwrap()).unwrap();
    assert_eq!(models["unrelated"], true);
    assert!(models["providers"]["personal"].is_object());
    assert_eq!(
        models["providers"]["grillforge"]["baseUrl"],
        "http://127.0.0.1:15721"
    );
    assert_eq!(
        models["providers"]["grillforge"]["api"],
        "anthropic-messages"
    );
    assert_eq!(
        models["providers"]["grillforge"]["apiKey"],
        "gateway-secret"
    );
    assert_eq!(
        models["providers"]["grillforge"]["models"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let settings: Value = serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
    assert_eq!(settings["theme"], "dark");
    assert_eq!(settings["defaultThinkingLevel"], "high");
    assert_eq!(settings["defaultProvider"], "grillforge");
    assert_eq!(settings["defaultModel"], "grillforge/deepseek-chat");
    assert_eq!(settings["enabledModels"].as_array().unwrap().len(), 2);
    assert_eq!(adapter.status().unwrap().takeover, PiTakeoverStatus::Active);
}

#[test]
fn disable_restores_exact_original_files_and_apply_is_idempotent() {
    let temp = tempdir().unwrap();
    let paths = PiPaths::new(
        temp.path().join("pi/models.json"),
        temp.path().join("pi/settings.json"),
    );
    fs::create_dir_all(paths.models_path.parent().unwrap()).unwrap();
    let original_models = b"{\n  \"providers\": {}\n}\n";
    fs::write(&paths.models_path, original_models).unwrap();
    let adapter = PiAdapter::new(paths.clone(), temp.path().join("grillforge"));
    let request = PiRequest::new(
        "http://127.0.0.1:15721",
        "gateway-secret",
        vec![model("grillforge/coder")],
        None,
    )
    .unwrap();

    adapter.apply(request.clone()).unwrap();
    let first = fs::read(&paths.models_path).unwrap();
    adapter.apply(request).unwrap();
    assert_eq!(fs::read(&paths.models_path).unwrap(), first);
    adapter.disable().unwrap();

    assert_eq!(fs::read(&paths.models_path).unwrap(), original_models);
    assert!(!paths.settings_path.exists());
    assert!(!adapter.snapshot_path().exists());
    assert_eq!(
        adapter.status().unwrap().takeover,
        PiTakeoverStatus::Inactive
    );
}

#[test]
fn pi_json_reformatting_does_not_create_false_drift() {
    let temp = tempdir().unwrap();
    let paths = PiPaths::new(
        temp.path().join("pi/models.json"),
        temp.path().join("pi/settings.json"),
    );
    let adapter = PiAdapter::new(paths.clone(), temp.path().join("grillforge"));
    adapter
        .apply(
            PiRequest::new(
                "http://127.0.0.1:15721/pi",
                "gateway-secret",
                vec![model("grillforge/coder")],
                Some("grillforge/coder".into()),
            )
            .unwrap(),
        )
        .unwrap();

    let settings: Value = serde_json::from_slice(&fs::read(&paths.settings_path).unwrap()).unwrap();
    fs::write(&paths.settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();

    assert_eq!(adapter.status().unwrap().takeover, PiTakeoverStatus::Active);
    adapter.disable().unwrap();
    assert!(!paths.models_path.exists());
    assert!(!paths.settings_path.exists());
}

#[test]
fn unrelated_pi_changes_do_not_create_drift_and_survive_disable() {
    let temp = tempdir().unwrap();
    let paths = PiPaths::new(
        temp.path().join("pi/models.json"),
        temp.path().join("pi/settings.json"),
    );
    fs::create_dir_all(paths.models_path.parent().unwrap()).unwrap();
    fs::write(
        &paths.settings_path,
        serde_json::to_vec_pretty(&json!({"theme": "light"})).unwrap(),
    )
    .unwrap();
    let adapter = PiAdapter::new(paths.clone(), temp.path().join("grillforge"));
    adapter
        .apply(
            PiRequest::new(
                "http://127.0.0.1:15721/pi",
                "gateway-secret",
                vec![model("grillforge/coder")],
                Some("grillforge/coder".into()),
            )
            .unwrap(),
        )
        .unwrap();

    let mut settings: Value =
        serde_json::from_slice(&fs::read(&paths.settings_path).unwrap()).unwrap();
    settings["packages"] = json!([{
        "source": "npm:pi-mcp-extension@1.5.0",
        "autoload": false
    }]);
    fs::write(
        &paths.settings_path,
        serde_json::to_vec_pretty(&settings).unwrap(),
    )
    .unwrap();

    let mut models: Value = serde_json::from_slice(&fs::read(&paths.models_path).unwrap()).unwrap();
    models["unrelated"] = json!(true);
    fs::write(
        &paths.models_path,
        serde_json::to_vec_pretty(&models).unwrap(),
    )
    .unwrap();

    assert_eq!(adapter.status().unwrap().takeover, PiTakeoverStatus::Active);
    adapter.disable().unwrap();

    let settings: Value = serde_json::from_slice(&fs::read(&paths.settings_path).unwrap()).unwrap();
    assert_eq!(settings["theme"], "light");
    assert_eq!(
        settings["packages"][0]["source"],
        "npm:pi-mcp-extension@1.5.0"
    );
    assert!(settings.get("defaultProvider").is_none());
    assert!(settings.get("defaultModel").is_none());
    assert!(settings.get("enabledModels").is_none());

    let models: Value = serde_json::from_slice(&fs::read(&paths.models_path).unwrap()).unwrap();
    assert_eq!(models["unrelated"], true);
    assert!(models["providers"].get("grillforge").is_none());
}

#[test]
fn request_rejects_non_loopback_gateway_and_unknown_default() {
    let bad_gateway = PiRequest::new(
        "https://gateway.example.com",
        "token",
        vec![model("grillforge/coder")],
        None,
    )
    .unwrap_err();
    assert!(bad_gateway.to_string().contains("loopback"));

    let unknown_default = PiRequest::new(
        "http://127.0.0.1:15721",
        "token",
        vec![model("grillforge/coder")],
        Some("grillforge/missing".into()),
    )
    .unwrap_err();
    assert!(unknown_default.to_string().contains("default"));
}

#[test]
fn paths_follow_the_official_pi_agent_directory() {
    let paths = paths_from_home("/tmp/example-home");
    assert_eq!(
        paths.models_path,
        std::path::Path::new("/tmp/example-home/.pi/agent/models.json")
    );
    assert_eq!(
        paths.settings_path,
        std::path::Path::new("/tmp/example-home/.pi/agent/settings.json")
    );
}

#[cfg(unix)]
#[test]
fn pi_cli_version_is_inspected_without_running_a_session() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let executable = temp.path().join("pi");
    fs::write(
        &executable,
        "#!/bin/sh\n[ \"$PI_OFFLINE\" = 1 ] || exit 41\nprintf 'pi 0.42.0\\n'\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

    let detected = inspect_pi_cli(&executable).unwrap();
    assert_eq!(detected.path, executable);
    assert_eq!(detected.version, "pi 0.42.0");
}

#[cfg(unix)]
#[test]
fn pi_detector_checks_a_user_install_candidate_outside_path() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let executable = temp.path().join(".local/bin/pi");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, "#!/bin/sh\nprintf 'pi user-install\n'\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

    let detected = detect_pi_cli_in([temp.path().join("missing/pi"), executable.clone()])
        .unwrap()
        .expect("user-installed Pi CLI");

    assert_eq!(detected.path, executable);
    assert_eq!(detected.version, "pi user-install");
}

#[cfg(unix)]
#[test]
fn pi_detector_discovers_an_nvm_global_install_used_by_desktop_apps() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let executable = temp.path().join(".nvm/versions/node/v22.22.0/bin/pi");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, "#!/bin/sh\nprintf '0.84.1\n'\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

    let detected = detect_pi_cli_in(pi_cli_candidates_from_home(temp.path(), "pi"))
        .unwrap()
        .expect("NVM-installed Pi CLI");

    assert_eq!(detected.path, executable);
    assert_eq!(detected.version, "0.84.1");
}

#[test]
#[ignore = "requires a locally installed Pi CLI"]
fn live_pi_detection_works_outside_the_terminal_path() {
    let detection = grillforge_lib::adapters::pi::detect_pi_cli()
        .unwrap()
        .expect("installed Pi CLI must be discovered");
    assert!(detection.path.is_absolute());
    assert!(!detection.version.is_empty());
}
