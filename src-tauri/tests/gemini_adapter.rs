use grillforge_lib::adapters::gemini::{
    GeminiAdapter, GeminiPaths, GeminiRequest, GeminiTakeoverStatus,
};
use serde_json::Value;
use std::fs;
use tempfile::tempdir;

#[cfg(unix)]
#[test]
fn gemini_cli_inspection_reads_version() {
    use grillforge_lib::adapters::gemini::inspect_gemini_cli;
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let cli = temp.path().join("gemini");
    fs::write(&cli, "#!/bin/sh\nprintf 'gemini 0.9.0\\n'\n").unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(inspect_gemini_cli(cli).unwrap().version, "gemini 0.9.0");
}

#[cfg(unix)]
#[test]
fn gemini_detector_checks_a_user_install_candidate_outside_path() {
    use grillforge_lib::adapters::gemini::detect_gemini_cli_in;
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let cli = temp.path().join("Library/pnpm/gemini");
    fs::create_dir_all(cli.parent().unwrap()).unwrap();
    fs::write(&cli, "#!/bin/sh\nprintf 'gemini user-install\n'\n").unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();

    let detected = detect_gemini_cli_in([temp.path().join("missing/gemini"), cli.clone()])
        .unwrap()
        .expect("user-installed Gemini CLI");

    assert_eq!(detected.path, cli);
    assert_eq!(detected.version, "gemini user-install");
}

#[test]
fn gemini_apply_projects_api_key_model_and_auth_while_preserving_unrelated_settings() {
    let temp = tempdir().unwrap();
    let root = temp.path().join(".gemini");
    fs::create_dir_all(&root).unwrap();
    let env_path = root.join(".env");
    let settings_path = root.join("settings.json");
    fs::write(&env_path, "KEEP_ME=yes\n").unwrap();
    fs::write(
        &settings_path,
        r#"{"theme":"dark","security":{"sandbox":true}}"#,
    )
    .unwrap();
    let adapter = GeminiAdapter::new(
        GeminiPaths::new(env_path.clone(), settings_path.clone()),
        temp.path().join(".grillforge"),
    );

    adapter
        .apply(
            GeminiRequest::new(
                "https://generativelanguage.googleapis.com",
                "gemini-secret",
                "gemini-2.5-pro",
            )
            .unwrap(),
        )
        .unwrap();

    let env = fs::read_to_string(&env_path).unwrap();
    assert!(env.contains("KEEP_ME=yes"));
    assert!(env.contains("GEMINI_API_KEY=gemini-secret"));
    assert!(env.contains("GEMINI_MODEL=gemini-2.5-pro"));
    assert!(env.contains("GOOGLE_GEMINI_BASE_URL=https://generativelanguage.googleapis.com"));
    let settings: Value = serde_json::from_slice(&fs::read(settings_path).unwrap()).unwrap();
    assert_eq!(settings["theme"], "dark");
    assert_eq!(settings["security"]["sandbox"], true);
    assert_eq!(
        settings["security"]["auth"]["selectedType"],
        "gemini-api-key"
    );
    assert_eq!(
        adapter.status().unwrap().takeover,
        GeminiTakeoverStatus::Active
    );
}

#[test]
fn gemini_disable_restores_both_files_exactly_and_rejects_invalid_env() {
    let temp = tempdir().unwrap();
    let root = temp.path().join(".gemini");
    fs::create_dir_all(&root).unwrap();
    let env_path = root.join(".env");
    let settings_path = root.join("settings.json");
    let original_env = b"# keep layout\nCUSTOM=value\n";
    let original_settings = b"{\n  \"theme\": \"ansi\"\n}\n";
    fs::write(&env_path, original_env).unwrap();
    fs::write(&settings_path, original_settings).unwrap();
    let adapter = GeminiAdapter::new(
        GeminiPaths::new(env_path.clone(), settings_path.clone()),
        temp.path().join(".grillforge"),
    );
    adapter
        .apply(GeminiRequest::new("http://127.0.0.1:8080", "key", "model").unwrap())
        .unwrap();
    adapter.disable().unwrap();
    assert_eq!(fs::read(env_path).unwrap(), original_env);
    assert_eq!(fs::read(settings_path).unwrap(), original_settings);

    let invalid_root = temp.path().join("invalid");
    fs::create_dir_all(&invalid_root).unwrap();
    let invalid_env = invalid_root.join(".env");
    fs::write(&invalid_env, "this is not valid\n").unwrap();
    let invalid = GeminiAdapter::new(
        GeminiPaths::new(invalid_env, invalid_root.join("settings.json")),
        temp.path().join("invalid-grillforge"),
    );
    assert!(
        invalid
            .apply(GeminiRequest::new("https://example.com", "key", "model").unwrap())
            .unwrap_err()
            .to_string()
            .contains("line 1")
    );
}

#[test]
fn gemini_request_rejects_unsafe_values_and_redacts_key() {
    let error =
        GeminiRequest::new("https://user:pass@example.com", "never-print", "model").unwrap_err();
    assert!(!error.to_string().contains("never-print"));
    assert!(GeminiRequest::new("https://example.com?q=1", "key", "model").is_err());
    assert!(GeminiRequest::new("https://example.com", "", "model").is_err());
}
