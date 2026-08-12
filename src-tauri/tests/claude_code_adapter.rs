use grillforge_lib::adapters::claude_code::{
    ClaudeCodeAdapter, ClaudeCodeAdapterError, ClaudeCodeOperation, ClaudeCodeTakeoverStatus,
    EnableRequest, inspect_claude_cli,
};
use std::collections::BTreeMap;
use std::fs;

fn adapter(root: &tempfile::TempDir) -> ClaudeCodeAdapter {
    ClaudeCodeAdapter::new(root.path().join("claude"), root.path().join("grillforge"))
}

#[test]
fn managed_main_changes_only_the_route_and_gateway() {
    let root = tempfile::tempdir().expect("root");
    let plan = adapter(&root)
        .plan_enable(EnableRequest::managed_main_only(
            "http://127.0.0.1:15721",
            "grillforge/coder",
        ))
        .expect("plan");

    assert!(
        plan.operations()
            .contains(&ClaudeCodeOperation::SetEnvironment {
                key: "ANTHROPIC_BASE_URL".into(),
                value: "http://127.0.0.1:15721".into(),
            })
    );
    assert!(
        plan.operations()
            .contains(&ClaudeCodeOperation::SetEnvironment {
                key: "ANTHROPIC_MODEL".into(),
                value: "grillforge/coder".into(),
            })
    );
    assert!(plan.operations().iter().all(|operation| match operation {
        ClaudeCodeOperation::SetModel { .. } | ClaudeCodeOperation::RemoveModel => true,
        ClaudeCodeOperation::SetEnvironment { key, .. }
        | ClaudeCodeOperation::RemoveEnvironment { key } => {
            key != "ANTHROPIC_AUTH_TOKEN" && key != "ANTHROPIC_API_KEY"
        }
    }));
}

#[test]
fn fixed_model_slots_use_the_real_claude_code_environment_keys() {
    let root = tempfile::tempdir().expect("root");
    let routes = BTreeMap::from([
        ("sonnet".into(), "grillforge/sonnet".into()),
        ("opus".into(), "grillforge/opus".into()),
        ("fable".into(), "grillforge/fable".into()),
        ("haiku".into(), "grillforge/haiku".into()),
        ("subagent_default".into(), "grillforge/subagent".into()),
    ]);
    let plan = adapter(&root)
        .plan_enable(EnableRequest::native().with_model_routes("http://127.0.0.1:15721", routes))
        .expect("plan");

    for (key, value) in [
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", "grillforge/sonnet"),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", "grillforge/opus"),
        ("ANTHROPIC_DEFAULT_FABLE_MODEL", "grillforge/fable"),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "grillforge/haiku"),
        ("CLAUDE_CODE_SUBAGENT_MODEL", "grillforge/subagent"),
    ] {
        assert!(
            plan.operations()
                .contains(&ClaudeCodeOperation::SetEnvironment {
                    key: key.into(),
                    value: value.into(),
                })
        );
    }
}

#[test]
fn native_configuration_is_a_noop() {
    let root = tempfile::tempdir().expect("root");
    let adapter = adapter(&root);
    let plan = adapter.plan_enable(EnableRequest::native()).expect("plan");
    assert!(plan.operations().is_empty());
    adapter.enable(EnableRequest::native()).expect("enable");
    assert!(!root.path().join("claude/settings.json").exists());
    assert!(!adapter.snapshot_path().exists());
}

#[test]
fn native_models_write_real_claude_settings_and_restore_exactly() {
    let root = tempfile::tempdir().expect("root");
    let claude = root.path().join("claude");
    fs::create_dir_all(&claude).expect("Claude root");
    let original = br#"{"model":"opus","theme":"dark","env":{"KEEP":"yes"}}"#;
    fs::write(claude.join("settings.json"), original).expect("settings");
    let adapter = adapter(&root);
    let native_slots = BTreeMap::from([
        ("sonnet".into(), "sonnet".into()),
        ("subagent_default".into(), "haiku".into()),
    ]);

    adapter
        .enable(EnableRequest::native().with_native_models(Some("fable".into()), native_slots))
        .expect("enable native models");

    let configured: serde_json::Value = serde_json::from_slice(
        &fs::read(claude.join("settings.json")).expect("configured settings"),
    )
    .expect("json");
    assert_eq!(configured["model"], "fable");
    assert_eq!(
        configured["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"],
        "sonnet"
    );
    assert_eq!(configured["env"]["CLAUDE_CODE_SUBAGENT_MODEL"], "haiku");
    assert!(configured["env"].get("ANTHROPIC_BASE_URL").is_none());

    adapter.disable().expect("disable");
    assert_eq!(
        fs::read(claude.join("settings.json")).expect("restored"),
        original
    );
}

#[test]
fn disable_restores_the_exact_original_settings() {
    let root = tempfile::tempdir().expect("root");
    let claude = root.path().join("claude");
    fs::create_dir_all(&claude).expect("Claude root");
    let original = br#"{"theme":"dark","env":{"KEEP":"yes","ANTHROPIC_MODEL":"native"}}"#;
    fs::write(claude.join("settings.json"), original).expect("settings");
    let adapter = adapter(&root);

    adapter
        .enable(EnableRequest::managed_main_only(
            "http://127.0.0.1:15721",
            "grillforge/coder",
        ))
        .expect("enable");
    adapter.disable().expect("disable");

    assert_eq!(
        fs::read(claude.join("settings.json")).expect("restored"),
        original
    );
    assert!(!adapter.snapshot_path().exists());
}

#[test]
fn repeated_enable_keeps_the_first_recovery_baseline() {
    let root = tempfile::tempdir().expect("root");
    let claude = root.path().join("claude");
    fs::create_dir_all(&claude).expect("Claude root");
    let original = br#"{"env":{"ANTHROPIC_MODEL":"native"}}"#;
    fs::write(claude.join("settings.json"), original).expect("settings");
    let adapter = adapter(&root);

    adapter
        .enable(EnableRequest::managed_main_only(
            "http://127.0.0.1:15721",
            "grillforge/first",
        ))
        .expect("first");
    adapter
        .enable(EnableRequest::managed_main_only(
            "http://127.0.0.1:15721",
            "grillforge/second",
        ))
        .expect("second");
    adapter.disable().expect("disable");

    assert_eq!(
        fs::read(claude.join("settings.json")).expect("restored"),
        original
    );
}

#[test]
fn malformed_settings_fail_before_writing_a_snapshot() {
    let root = tempfile::tempdir().expect("root");
    let claude = root.path().join("claude");
    fs::create_dir_all(&claude).expect("Claude root");
    fs::write(claude.join("settings.json"), "[]").expect("settings");
    let adapter = adapter(&root);

    let error = adapter
        .enable(EnableRequest::managed_main_only(
            "http://127.0.0.1:15721",
            "grillforge/coder",
        ))
        .expect_err("invalid settings");
    assert!(matches!(error, ClaudeCodeAdapterError::InvalidSettings(_)));
    assert!(!adapter.snapshot_path().exists());
}

#[test]
fn status_reports_the_exact_changed_setting_without_repairing_it() {
    let root = tempfile::tempdir().expect("root");
    let adapter = adapter(&root);
    adapter
        .enable(EnableRequest::managed_main_only(
            "http://127.0.0.1:15721",
            "grillforge/coder",
        ))
        .expect("enable");
    let path = root.path().join("claude/settings.json");
    let mut settings: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("settings")).expect("json");
    settings["env"]["ANTHROPIC_MODEL"] = "user-change".into();
    fs::write(&path, serde_json::to_vec_pretty(&settings).expect("json")).expect("drift");

    let status = adapter.status().expect("status");
    assert_eq!(status.takeover, ClaudeCodeTakeoverStatus::Drifted);
    assert_eq!(status.differences, ["ANTHROPIC_MODEL"]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(path).expect("unchanged")).unwrap()["env"]
            ["ANTHROPIC_MODEL"],
        "user-change"
    );
}

#[test]
fn status_reads_existing_native_model_choices_without_taking_over() {
    let root = tempfile::tempdir().expect("root");
    let claude = root.path().join("claude");
    fs::create_dir_all(&claude).expect("Claude root");
    fs::write(
        claude.join("settings.json"),
        r#"{"model":"claude-custom-full-id","env":{"ANTHROPIC_DEFAULT_OPUS_MODEL":"opus","CLAUDE_CODE_SUBAGENT_MODEL":"haiku"}}"#,
    )
    .expect("settings");

    let status = adapter(&root).status().expect("status");

    assert_eq!(status.takeover, ClaudeCodeTakeoverStatus::Inactive);
    assert_eq!(
        status.native_model_slots,
        BTreeMap::from([
            ("main".into(), "claude-custom-full-id".into()),
            ("opus".into(), "opus".into()),
            ("subagent_default".into(), "haiku".into()),
        ])
    );
    assert!(!status.snapshot_present);
}

#[cfg(unix)]
#[test]
fn cli_inspection_reports_a_real_version_and_enforces_a_timeout() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().expect("root");
    let cli = root.path().join("claude");
    fs::write(&cli, "#!/bin/sh\necho '2.1.226 (Claude Code)'\n").expect("cli");
    let mut permissions = fs::metadata(&cli).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&cli, permissions).unwrap();
    let detection = inspect_claude_cli(&cli).expect("detection");
    assert_eq!(detection.path, cli);
    assert_eq!(detection.version, "2.1.226 (Claude Code)");

    let slow = root.path().join("slow-claude");
    fs::write(&slow, "#!/bin/sh\nsleep 5\n").expect("slow cli");
    let mut permissions = fs::metadata(&slow).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&slow, permissions).unwrap();
    assert!(matches!(
        inspect_claude_cli(&slow),
        Err(ClaudeCodeAdapterError::CliTimedOut(path)) if path == slow
    ));
}
