use grillforge_lib::adapters::claude_code::{
    ClaudeCodeAdapter, ClaudeCodeOperation, ClaudeCodeTakeoverStatus, EnableRequest, WorkerModel,
    WorkerStrategy, inspect_claude_cli,
};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn worker(id: &str, route_alias: &str) -> WorkerModel {
    WorkerModel::new(id, route_alias)
}

fn adapter(claude_dir: &tempfile::TempDir) -> ClaudeCodeAdapter {
    ClaudeCodeAdapter::new(claude_dir.path(), claude_dir.path().join("grillforge"))
}

#[test]
fn selectable_workers_generate_stable_native_agent_definitions() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let adapter = adapter(&claude_dir);

    let plan = adapter
        .plan_enable(EnableRequest::native_main(
            "http://127.0.0.1:15721",
            vec![
                worker("review", "grillforge/worker-review"),
                worker("tests", "grillforge/worker-tests"),
            ],
            WorkerStrategy::SelectablePool,
        ))
        .expect("valid integration plan");

    let writes: Vec<_> = plan
        .operations()
        .iter()
        .filter_map(|operation| match operation {
            ClaudeCodeOperation::WriteFile { path, contents } => {
                Some((path.as_path(), contents.as_str()))
            }
            _ => None,
        })
        .collect();

    assert_eq!(writes.len(), 2);
    assert!(writes[0].0.ends_with("agents/grillforge-worker-review.md"));
    assert_eq!(
        writes[0].1,
        "---\nname: grillforge-worker-review\ndescription: GrillForge worker review\nmodel: grillforge/worker-review\n---\n<!-- Managed by GrillForge. -->\nExecute the delegated task and return a concise result.\n"
    );
    assert!(writes[1].0.ends_with("agents/grillforge-worker-tests.md"));
    assert_eq!(
        writes[1].1,
        "---\nname: grillforge-worker-tests\ndescription: GrillForge worker tests\nmodel: grillforge/worker-tests\n---\n<!-- Managed by GrillForge. -->\nExecute the delegated task and return a concise result.\n"
    );
    assert!(!plan.operations().iter().any(|operation| matches!(
        operation,
        ClaudeCodeOperation::SetEnvironment { key, .. }
            if key == "CLAUDE_CODE_SUBAGENT_MODEL"
    )));
}

#[test]
fn forced_worker_uses_only_the_global_subagent_override() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let adapter = adapter(&claude_dir);

    let plan = adapter
        .plan_enable(EnableRequest::native_main(
            "http://localhost:15721",
            vec![worker("review", "grillforge/worker-review")],
            WorkerStrategy::ForcedSingle,
        ))
        .expect("valid forced Worker plan");

    assert!(plan.operations().iter().any(|operation| matches!(
        operation,
        ClaudeCodeOperation::SetEnvironment { key, value }
            if key == "CLAUDE_CODE_SUBAGENT_MODEL"
                && value == "grillforge/worker-review"
    )));
    assert!(
        !plan
            .operations()
            .iter()
            .any(|operation| matches!(operation, ClaudeCodeOperation::WriteFile { .. }))
    );
}

#[test]
fn forced_worker_removes_selectable_agent_definitions_from_the_active_plan() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let old_agent = claude_dir.path().join("agents/grillforge-worker-old.md");
    fs::create_dir_all(old_agent.parent().expect("agent directory")).expect("create agents");
    fs::write(
        &old_agent,
        b"---\nname: grillforge-worker-old\n---\n<!-- Managed by GrillForge. -->\nold\n",
    )
    .expect("seed managed agent");
    let adapter = adapter(&claude_dir);

    let plan = adapter
        .plan_enable(EnableRequest::native_main(
            "http://127.0.0.1:15721",
            vec![worker("review", "grillforge/worker-review")],
            WorkerStrategy::ForcedSingle,
        ))
        .expect("valid forced Worker plan");

    assert!(plan.operations().iter().any(|operation| matches!(
        operation,
        ClaudeCodeOperation::RemoveFile { path } if path == &old_agent
    )));
    assert!(
        !plan
            .operations()
            .iter()
            .any(|operation| matches!(operation, ClaudeCodeOperation::WriteFile { .. }))
    );
}

#[test]
fn native_main_with_workers_never_masks_subscription_authentication() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let adapter = adapter(&claude_dir);

    let plan = adapter
        .plan_enable(EnableRequest::native_main(
            "http://127.0.0.1:15721",
            vec![worker("review", "grillforge/worker-review")],
            WorkerStrategy::ForcedSingle,
        ))
        .expect("valid Native-main plan");

    assert!(plan.operations().iter().any(|operation| matches!(
        operation,
        ClaudeCodeOperation::SetEnvironment { key, value }
            if key == "ANTHROPIC_BASE_URL" && value == "http://127.0.0.1:15721"
    )));
    assert!(!plan.operations().iter().any(|operation| match operation {
        ClaudeCodeOperation::SetEnvironment { key, .. }
        | ClaudeCodeOperation::RemoveEnvironment { key } => {
            matches!(key.as_str(), "ANTHROPIC_AUTH_TOKEN" | "ANTHROPIC_API_KEY")
        }
        _ => false,
    }));
}

#[test]
fn managed_main_can_be_enabled_without_workers() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let adapter = adapter(&claude_dir);

    let plan = adapter
        .plan_enable(EnableRequest::managed_main_only(
            "http://127.0.0.1:15721",
            "grillforge/main-review",
        ))
        .expect("valid managed-main plan");

    assert!(plan.operations().iter().any(|operation| matches!(
        operation,
        ClaudeCodeOperation::SetEnvironment { key, value }
            if key == "ANTHROPIC_MODEL" && value == "grillforge/main-review"
    )));
    assert!(!plan.operations().iter().any(|operation| matches!(
        operation,
        ClaudeCodeOperation::SetEnvironment { key, .. }
            if key == "CLAUDE_CODE_SUBAGENT_MODEL"
    ) || matches!(
        operation,
        ClaudeCodeOperation::WriteFile { .. }
    )));
}

#[test]
fn fixed_model_slots_use_cc_switch_environment_keys() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let adapter = adapter(&claude_dir);
    let routes = BTreeMap::from([
        ("sonnet".to_string(), "grillforge/reviewer".to_string()),
        ("haiku".to_string(), "grillforge/fast".to_string()),
    ]);

    let plan = adapter
        .plan_enable(
            EnableRequest::native_main_without_workers()
                .with_model_routes("http://127.0.0.1:15721", routes),
        )
        .expect("valid slot plan");

    assert!(plan.operations().iter().any(|operation| matches!(
        operation,
        ClaudeCodeOperation::SetEnvironment { key, value }
            if key == "ANTHROPIC_DEFAULT_SONNET_MODEL" && value == "grillforge/reviewer"
    )));
    assert!(plan.operations().iter().any(|operation| matches!(
        operation,
        ClaudeCodeOperation::SetEnvironment { key, value }
            if key == "ANTHROPIC_DEFAULT_HAIKU_MODEL" && value == "grillforge/fast"
    )));
}

#[test]
fn native_main_without_workers_does_not_take_over_claude_code() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let grillforge_dir = tempdir().expect("temporary GrillForge config");
    let adapter = ClaudeCodeAdapter::new(claude_dir.path(), grillforge_dir.path());

    adapter
        .enable(EnableRequest::native_main_without_workers())
        .expect("no-op native configuration");

    assert!(!claude_dir.path().join("settings.json").exists());
    assert!(!adapter.snapshot_path().exists());
}

#[test]
fn main_and_worker_modes_can_be_changed_independently() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let grillforge_dir = tempdir().expect("temporary GrillForge config");
    let settings_path = claude_dir.path().join("settings.json");
    fs::write(
        &settings_path,
        br#"{"env":{"ANTHROPIC_BASE_URL":"https://original.example","ANTHROPIC_MODEL":"claude-main","CLAUDE_CODE_SUBAGENT_MODEL":"claude-worker"}}"#,
    )
    .expect("seed settings");
    let adapter = ClaudeCodeAdapter::new(claude_dir.path(), grillforge_dir.path());
    adapter
        .enable(EnableRequest::managed_main(
            "http://127.0.0.1:15721",
            "grillforge/main-review",
            vec![
                worker("review", "grillforge/worker-review"),
                worker("tests", "grillforge/worker-tests"),
            ],
            WorkerStrategy::SelectablePool,
        ))
        .expect("enable main and workers");

    adapter
        .enable(EnableRequest::managed_main_only(
            "http://127.0.0.1:15721",
            "grillforge/main-review",
        ))
        .expect("turn workers off");

    let main_only: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings_path).expect("main-only settings"))
            .expect("main-only settings JSON");
    assert_eq!(
        main_only["env"]["ANTHROPIC_BASE_URL"],
        "http://127.0.0.1:15721"
    );
    assert_eq!(
        main_only["env"]["ANTHROPIC_MODEL"],
        "grillforge/main-review"
    );
    assert_eq!(
        main_only["env"]["CLAUDE_CODE_SUBAGENT_MODEL"],
        "claude-worker"
    );
    assert!(
        !claude_dir
            .path()
            .join("agents/grillforge-worker-review.md")
            .exists()
    );
    assert!(adapter.snapshot_path().exists());

    adapter
        .enable(EnableRequest::native_main_without_workers())
        .expect("turn GrillForge integration off");

    let restored: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings_path).expect("restored settings"))
            .expect("restored settings JSON");
    assert_eq!(
        restored["env"]["ANTHROPIC_BASE_URL"],
        "https://original.example"
    );
    assert_eq!(restored["env"]["ANTHROPIC_MODEL"], "claude-main");
    assert_eq!(
        restored["env"]["CLAUDE_CODE_SUBAGENT_MODEL"],
        "claude-worker"
    );
    assert!(!adapter.snapshot_path().exists());
}

#[test]
fn disable_plan_restores_managed_environment_and_exact_agent_files() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let settings_path = claude_dir.path().join("settings.json");
    let agent_path = claude_dir.path().join("agents/grillforge-worker-review.md");
    fs::create_dir_all(agent_path.parent().expect("agent directory")).expect("create agents");
    fs::write(
        &settings_path,
        br#"{"theme":"dark","env":{"ANTHROPIC_BASE_URL":"https://api.anthropic.com"}}"#,
    )
    .expect("seed settings");
    fs::write(
        &agent_path,
        b"---\nname: grillforge-worker-review\n---\n<!-- Managed by GrillForge. -->\nold\n",
    )
    .expect("seed agent");
    let adapter = adapter(&claude_dir);

    let enable = adapter
        .plan_enable(EnableRequest::native_main(
            "http://127.0.0.1:15721",
            vec![
                worker("review", "grillforge/worker-review"),
                worker("tests", "grillforge/worker-tests"),
            ],
            WorkerStrategy::SelectablePool,
        ))
        .expect("valid enable plan");
    let disable = adapter.plan_disable(enable.snapshot().expect("recovery snapshot"));

    assert!(disable.operations().iter().any(|operation| matches!(
        operation,
        ClaudeCodeOperation::SetEnvironment { key, value }
            if key == "ANTHROPIC_BASE_URL" && value == "https://api.anthropic.com"
    )));
    assert!(disable.operations().iter().any(|operation| matches!(
        operation,
        ClaudeCodeOperation::RestoreFile { path, contents: Some(contents) }
            if path == &agent_path
                && contents == b"---\nname: grillforge-worker-review\n---\n<!-- Managed by GrillForge. -->\nold\n"
    )));
    assert!(disable.operations().iter().any(|operation| matches!(
        operation,
        ClaudeCodeOperation::RestoreFile { path, contents: None }
            if path.ends_with("agents/grillforge-worker-tests.md")
    )));
}

#[test]
fn malformed_claude_settings_fail_before_any_operation_is_planned() {
    let claude_dir = tempdir().expect("temporary Claude config");
    fs::write(claude_dir.path().join("settings.json"), b"{").expect("seed malformed settings");
    let adapter = adapter(&claude_dir);

    let error = adapter
        .plan_enable(EnableRequest::native_main(
            "http://127.0.0.1:15721",
            vec![worker("review", "grillforge/worker-review")],
            WorkerStrategy::ForcedSingle,
        ))
        .expect_err("malformed settings must fail");

    assert_eq!(
        error.to_string(),
        format!(
            "Claude Code settings must be a valid JSON object: {}",
            claude_dir.path().join("settings.json").display()
        )
    );
}

#[test]
fn unrelated_agent_file_is_never_silently_overwritten() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let agent_path = claude_dir.path().join("agents/grillforge-worker-review.md");
    fs::create_dir_all(agent_path.parent().expect("agent directory")).expect("create agents");
    fs::write(&agent_path, b"user-owned agent\n").expect("seed collision");
    let adapter = adapter(&claude_dir);

    let error = adapter
        .plan_enable(EnableRequest::native_main(
            "http://127.0.0.1:15721",
            vec![
                worker("review", "grillforge/worker-review"),
                worker("tests", "grillforge/worker-tests"),
            ],
            WorkerStrategy::SelectablePool,
        ))
        .expect_err("user-owned file collision must fail");

    assert_eq!(
        error.to_string(),
        format!(
            "refusing to replace a non-GrillForge Claude Agent: {}",
            agent_path.display()
        )
    );
}

#[test]
fn validation_returns_the_first_boundary_error() {
    let claude_dir = tempdir().expect("temporary Claude config");
    fs::write(claude_dir.path().join("settings.json"), b"{").expect("seed malformed settings");
    let adapter = adapter(&claude_dir);

    let error = adapter
        .plan_enable(EnableRequest::native_main(
            "https://gateway.example.com",
            vec![worker("Invalid Worker", "bad alias")],
            WorkerStrategy::ForcedSingle,
        ))
        .expect_err("invalid request must fail");

    assert_eq!(
        error.to_string(),
        "Claude Code gateway must be an HTTP loopback URL: https://gateway.example.com"
    );
}

#[test]
fn enable_applies_the_plan_without_copying_authentication_into_the_snapshot() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let grillforge_dir = tempdir().expect("temporary GrillForge config");
    let settings_path = claude_dir.path().join("settings.json");
    fs::write(
        &settings_path,
        br#"{
            "theme": "dark",
            "env": {
                "KEEP_ME": "yes",
                "ANTHROPIC_AUTH_TOKEN": "subscription-secret",
                "ANTHROPIC_BASE_URL": "https://api.anthropic.com"
            }
        }"#,
    )
    .expect("seed settings");
    let adapter = ClaudeCodeAdapter::new(claude_dir.path(), grillforge_dir.path());

    adapter
        .enable(EnableRequest::native_main(
            "http://127.0.0.1:15721",
            vec![
                worker("review", "grillforge/worker-review"),
                worker("tests", "grillforge/worker-tests"),
            ],
            WorkerStrategy::SelectablePool,
        ))
        .expect("apply Claude integration");

    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings_path).expect("read applied settings"))
            .expect("valid settings JSON");
    assert_eq!(settings["theme"], "dark");
    assert_eq!(settings["env"]["KEEP_ME"], "yes");
    assert_eq!(
        settings["env"]["ANTHROPIC_AUTH_TOKEN"],
        "subscription-secret"
    );
    assert_eq!(
        settings["env"]["ANTHROPIC_BASE_URL"],
        "http://127.0.0.1:15721"
    );
    assert!(settings["env"].get("CLAUDE_CODE_SUBAGENT_MODEL").is_none());
    assert!(
        claude_dir
            .path()
            .join("agents/grillforge-worker-review.md")
            .is_file()
    );

    let snapshot = fs::read_to_string(adapter.snapshot_path()).expect("persisted snapshot");
    assert!(!snapshot.contains("subscription-secret"));
    assert!(!snapshot.contains("ANTHROPIC_AUTH_TOKEN"));
    assert!(snapshot.contains("https://api.anthropic.com"));
}

#[test]
fn disable_restores_only_managed_values_and_exact_agent_files() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let grillforge_dir = tempdir().expect("temporary GrillForge config");
    let settings_path = claude_dir.path().join("settings.json");
    let review_path = claude_dir.path().join("agents/grillforge-worker-review.md");
    let tests_path = claude_dir.path().join("agents/grillforge-worker-tests.md");
    let original_agent =
        "---\nname: grillforge-worker-review\n---\n<!-- Managed by GrillForge. -->\noriginal\n";
    fs::create_dir_all(review_path.parent().expect("agent directory")).expect("create agents");
    fs::write(
        &settings_path,
        br#"{"theme":"dark","env":{"KEEP_ME":"yes","ANTHROPIC_AUTH_TOKEN":"secret","ANTHROPIC_BASE_URL":"https://api.anthropic.com","ANTHROPIC_MODEL":"claude-main","CLAUDE_CODE_SUBAGENT_MODEL":"claude-worker"}}"#,
    )
    .expect("seed settings");
    fs::write(&review_path, original_agent).expect("seed managed agent");
    let adapter = ClaudeCodeAdapter::new(claude_dir.path(), grillforge_dir.path());

    adapter
        .enable(EnableRequest::managed_main(
            "http://127.0.0.1:15721",
            "grillforge/main-review",
            vec![
                worker("review", "grillforge/worker-review"),
                worker("tests", "grillforge/worker-tests"),
            ],
            WorkerStrategy::SelectablePool,
        ))
        .expect("enable integration");
    let mut active: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings_path).expect("read active settings"))
            .expect("active settings JSON");
    active["theme"] = serde_json::json!("light");
    active["env"]["KEEP_ME"] = serde_json::json!("changed");
    active["env"]["ANTHROPIC_AUTH_TOKEN"] = serde_json::json!("new-secret");
    fs::write(
        &settings_path,
        serde_json::to_vec_pretty(&active).expect("serialize edit"),
    )
    .expect("edit unrelated settings");

    adapter.disable().expect("restore integration");

    let restored: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings_path).expect("read restored settings"))
            .expect("restored settings JSON");
    assert_eq!(restored["theme"], "light");
    assert_eq!(restored["env"]["KEEP_ME"], "changed");
    assert_eq!(restored["env"]["ANTHROPIC_AUTH_TOKEN"], "new-secret");
    assert_eq!(
        restored["env"]["ANTHROPIC_BASE_URL"],
        "https://api.anthropic.com"
    );
    assert_eq!(restored["env"]["ANTHROPIC_MODEL"], "claude-main");
    assert_eq!(
        restored["env"]["CLAUDE_CODE_SUBAGENT_MODEL"],
        "claude-worker"
    );
    assert_eq!(
        fs::read_to_string(&review_path).expect("restored agent"),
        original_agent
    );
    assert!(!tests_path.exists());
    assert!(!adapter.snapshot_path().exists());
}

#[test]
fn repeated_enable_keeps_the_original_snapshot_and_tracks_new_agents() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let grillforge_dir = tempdir().expect("temporary GrillForge config");
    let settings_path = claude_dir.path().join("settings.json");
    fs::write(
        &settings_path,
        br#"{"env":{"ANTHROPIC_BASE_URL":"https://original.example"}}"#,
    )
    .expect("seed settings");
    let adapter = ClaudeCodeAdapter::new(claude_dir.path(), grillforge_dir.path());
    let first = EnableRequest::native_main(
        "http://127.0.0.1:15721",
        vec![
            worker("review", "grillforge/worker-review"),
            worker("tests", "grillforge/worker-tests"),
        ],
        WorkerStrategy::SelectablePool,
    );

    adapter.enable(first.clone()).expect("first enable");
    let first_snapshot = fs::read(adapter.snapshot_path()).expect("first snapshot");
    adapter.enable(first).expect("idempotent enable");
    assert_eq!(
        fs::read(adapter.snapshot_path()).expect("second snapshot"),
        first_snapshot
    );
    adapter
        .enable(EnableRequest::native_main(
            "http://127.0.0.1:15721",
            vec![
                worker("lint", "grillforge/worker-lint"),
                worker("docs", "grillforge/worker-docs"),
            ],
            WorkerStrategy::SelectablePool,
        ))
        .expect("update workers");

    adapter.disable().expect("restore original state");

    let restored: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings_path).expect("read restored settings"))
            .expect("restored settings JSON");
    assert_eq!(
        restored["env"]["ANTHROPIC_BASE_URL"],
        "https://original.example"
    );
    for id in ["review", "tests", "lint", "docs"] {
        assert!(
            !claude_dir
                .path()
                .join(format!("agents/grillforge-worker-{id}.md"))
                .exists()
        );
    }
}

#[test]
fn disable_preflight_refuses_user_owned_agent_without_changing_settings() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let grillforge_dir = tempdir().expect("temporary GrillForge config");
    let adapter = ClaudeCodeAdapter::new(claude_dir.path(), grillforge_dir.path());
    adapter
        .enable(EnableRequest::native_main(
            "http://127.0.0.1:15721",
            vec![
                worker("review", "grillforge/worker-review"),
                worker("tests", "grillforge/worker-tests"),
            ],
            WorkerStrategy::SelectablePool,
        ))
        .expect("enable integration");
    let settings_path = claude_dir.path().join("settings.json");
    let active_settings = fs::read(&settings_path).expect("active settings");
    let review_path = claude_dir.path().join("agents/grillforge-worker-review.md");
    fs::write(&review_path, "user-owned replacement\n").expect("replace agent");

    let error = adapter
        .disable()
        .expect_err("user-owned agent must stop restore");

    assert_eq!(
        error.to_string(),
        format!(
            "refusing to replace a non-GrillForge Claude Agent: {}",
            review_path.display()
        )
    );
    assert_eq!(
        fs::read(&settings_path).expect("unchanged settings"),
        active_settings
    );
    assert_eq!(
        fs::read_to_string(&review_path).expect("unchanged user agent"),
        "user-owned replacement\n"
    );
    assert!(adapter.snapshot_path().exists());
}

#[test]
fn status_reports_only_managed_aliases_and_agent_names() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let grillforge_dir = tempdir().expect("temporary GrillForge config");
    let settings_path = claude_dir.path().join("settings.json");
    fs::write(
        &settings_path,
        br#"{"env":{"ANTHROPIC_AUTH_TOKEN":"must-not-leak"}}"#,
    )
    .expect("seed settings");
    let adapter = ClaudeCodeAdapter::new(claude_dir.path(), grillforge_dir.path());
    adapter
        .enable(EnableRequest::managed_main(
            "http://127.0.0.1:15721",
            "grillforge/main-review",
            vec![worker("review", "grillforge/worker-review")],
            WorkerStrategy::ForcedSingle,
        ))
        .expect("enable integration");

    let status = adapter.status().expect("read status");

    assert!(status.snapshot_present);
    assert_eq!(status.takeover, ClaudeCodeTakeoverStatus::Active);
    assert_eq!(
        status.managed_main_alias.as_deref(),
        Some("grillforge/main-review")
    );
    assert_eq!(
        status.forced_worker_alias.as_deref(),
        Some("grillforge/worker-review")
    );
    assert!(status.generated_agent_names.is_empty());
    assert!(!format!("{status:?}").contains("must-not-leak"));
}

#[test]
fn status_reports_drift_without_repairing_configuration() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let grillforge_dir = tempdir().expect("temporary GrillForge config");
    let settings_path = claude_dir.path().join("settings.json");
    let adapter = ClaudeCodeAdapter::new(claude_dir.path(), grillforge_dir.path());
    adapter
        .enable(EnableRequest::native_main(
            "http://127.0.0.1:15721",
            vec![
                worker("review", "grillforge/worker-review"),
                worker("tests", "grillforge/worker-tests"),
            ],
            WorkerStrategy::SelectablePool,
        ))
        .expect("enable integration");
    let mut settings: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings_path).expect("active settings"))
            .expect("active settings JSON");
    settings["env"]["ANTHROPIC_BASE_URL"] = serde_json::json!("http://127.0.0.1:9999");
    let drifted = serde_json::to_vec_pretty(&settings).expect("serialize drift");
    fs::write(&settings_path, &drifted).expect("introduce drift");

    let status = adapter.status().expect("read drifted status");

    assert!(status.snapshot_present);
    assert_eq!(status.takeover, ClaudeCodeTakeoverStatus::Drifted);
    assert_eq!(status.differences, ["ANTHROPIC_BASE_URL"]);
    assert_eq!(
        status.generated_agent_names,
        [
            "grillforge-worker-review".to_string(),
            "grillforge-worker-tests".to_string()
        ]
    );
    assert_eq!(
        fs::read(&settings_path).expect("status is read-only"),
        drifted
    );
    assert!(adapter.snapshot_path().exists());
}

#[test]
fn clean_status_is_inactive_without_creating_files() {
    let claude_dir = tempdir().expect("temporary Claude config");
    let grillforge_dir = tempdir().expect("temporary GrillForge config");
    let adapter = ClaudeCodeAdapter::new(claude_dir.path(), grillforge_dir.path());

    let status = adapter.status().expect("read clean status");

    assert!(!status.snapshot_present);
    assert_eq!(status.takeover, ClaudeCodeTakeoverStatus::Inactive);
    assert!(status.differences.is_empty());
    assert!(!claude_dir.path().join("settings.json").exists());
    assert!(
        fs::read_dir(grillforge_dir.path())
            .expect("read GrillForge config")
            .next()
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn cli_inspection_reports_path_and_version_without_installation_changes() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempdir().expect("temporary CLI directory");
    let executable = directory.path().join("claude");
    fs::write(&executable, "#!/bin/sh\nprintf '2.1.7 (Claude Code)\\n'\n").expect("write fake CLI");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
        .expect("make fake CLI executable");

    let detection = inspect_claude_cli(&executable).expect("inspect fake CLI");

    assert_eq!(detection.path, executable);
    assert_eq!(detection.version, "2.1.7 (Claude Code)");
}

#[cfg(unix)]
#[test]
fn cli_inspection_has_a_hard_timeout() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempdir().expect("temporary CLI directory");
    let executable = directory.path().join("claude");
    fs::write(&executable, "#!/bin/sh\nexec sleep 10\n").expect("write hanging CLI");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
        .expect("make hanging CLI executable");
    let started = Instant::now();

    let error = inspect_claude_cli(&executable).expect_err("hanging CLI must time out");

    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(
        error.to_string(),
        format!(
            "Claude Code CLI version check timed out: {}",
            executable.display()
        )
    );
}

#[test]
#[ignore = "requires an installed Claude Code CLI; uses only loopback and dummy credentials"]
fn installed_claude_cli_routes_main_agent_worker_and_back() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback mock");
    listener
        .set_nonblocking(true)
        .expect("make mock listener nonblocking");
    let endpoint = format!(
        "http://{}",
        listener.local_addr().expect("mock listener address")
    );
    let running = Arc::new(AtomicBool::new(true));
    let models = Arc::new(Mutex::new(Vec::new()));
    let server_running = Arc::clone(&running);
    let server_models = Arc::clone(&models);
    let server = thread::spawn(move || {
        while server_running.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let payload = read_anthropic_request(&mut stream).expect("read mock request");
                    let model = payload["model"]
                        .as_str()
                        .expect("Claude request model")
                        .to_string();
                    server_models
                        .lock()
                        .expect("capture model")
                        .push(model.clone());
                    let has_tool_result = payload["messages"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|message| message["content"].as_array())
                        .flatten()
                        .any(|block| block["type"] == "tool_result");
                    let (content, stop_reason) = if model == "main-loopback" && !has_tool_result {
                        (
                            serde_json::json!([{
                                "type": "tool_use",
                                "id": "toolu_grillforge_e2e",
                                "name": "Agent",
                                "input": {
                                    "description": "Run loopback Worker",
                                    "prompt": "Return the loopback result",
                                    "subagent_type": "grillforge-worker-reviewer",
                                    "run_in_background": false
                                }
                            }]),
                            "tool_use",
                        )
                    } else {
                        (
                            serde_json::json!([{
                                "type": "text",
                                "text": format!("mock response from {model}")
                            }]),
                            "end_turn",
                        )
                    };
                    write_anthropic_response(&mut stream, &model, content, stop_reason)
                        .expect("write mock response");
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("loopback mock failed: {error}"),
            }
        }
    });

    let claude_dir = tempdir().expect("temporary Claude config");
    let adapter = adapter(&claude_dir);
    let plan = adapter
        .plan_enable(EnableRequest::native_main(
            &endpoint,
            vec![
                worker("reviewer", "grillforge/worker-loopback"),
                worker("spare", "grillforge/worker-spare"),
            ],
            WorkerStrategy::SelectablePool,
        ))
        .expect("plan loopback agents");
    for operation in plan.operations() {
        if let ClaudeCodeOperation::WriteFile { path, contents } = operation {
            fs::create_dir_all(path.parent().expect("agent directory")).expect("create agents");
            fs::write(path, contents).expect("install temporary Agent definition");
        }
    }
    let mut child = Command::new("claude")
        .args([
            "--print",
            "--no-session-persistence",
            "--output-format",
            "json",
            "delegate to reviewer",
        ])
        .current_dir(claude_dir.path())
        .env("CLAUDE_CONFIG_DIR", claude_dir.path())
        .env("ANTHROPIC_BASE_URL", &endpoint)
        .env("ANTHROPIC_MODEL", "main-loopback")
        .env("ANTHROPIC_API_KEY", "local-dummy-key")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start installed Claude Code CLI");

    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll Claude CLI") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill timed-out Claude CLI");
            child.wait().expect("reap timed-out Claude CLI");
            running.store(false, Ordering::Release);
            server.join().expect("stop loopback mock");
            panic!("Claude CLI loopback E2E exceeded 15 seconds");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let output = child.wait_with_output().expect("collect Claude CLI output");
    running.store(false, Ordering::Release);
    server.join().expect("stop loopback mock");

    assert!(
        status.success(),
        "Claude CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let captured = models.lock().expect("captured models");
    let sequence: Vec<_> = captured.iter().fold(Vec::new(), |mut sequence, model| {
        if sequence.last() != Some(model) {
            sequence.push(model.clone());
        }
        sequence
    });
    assert_eq!(
        sequence,
        [
            "main-loopback",
            "grillforge/worker-loopback",
            "main-loopback"
        ]
    );
}

fn read_anthropic_request(stream: &mut TcpStream) -> io::Result<serde_json::Value> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut received = Vec::new();
    let mut buffer = [0_u8; 8192];
    let (header_end, content_length) = loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request ended before headers",
            ));
        }
        received.extend_from_slice(&buffer[..read]);
        if let Some(header_end) = find_bytes(&received, b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&received[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                })
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "missing content-length")
                })?;
            break (header_end + 4, content_length);
        }
    };
    while received.len() < header_end + content_length {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request ended before body",
            ));
        }
        received.extend_from_slice(&buffer[..read]);
    }
    serde_json::from_slice(&received[header_end..header_end + content_length])
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn write_anthropic_response(
    stream: &mut TcpStream,
    model: &str,
    content: serde_json::Value,
    stop_reason: &str,
) -> io::Result<()> {
    let body = serde_json::to_vec(&serde_json::json!({
        "id": "msg_grillforge_e2e",
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {"input_tokens": 1, "output_tokens": 1}
    }))?;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
