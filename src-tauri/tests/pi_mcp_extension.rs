use grillforge_lib::pi_mcp_extension::{
    PI_MCP_EXTENSION_SOURCE, ensure_pi_mcp_progress_adapter, install_pi_mcp_extension_with,
    install_pi_mcp_extension_with_timeout, pi_mcp_extension_status_at,
};
use std::fs;
use std::time::Duration;

fn upstream_fixture(settings: &std::path::Path) {
    let agent_root = settings.parent().unwrap();
    for path in [
        agent_root.join("npm/node_modules/pi-mcp-extension/src/index.ts"),
        agent_root.join("npm/node_modules/@modelcontextprotocol/sdk/dist/esm/client/index.js"),
    ] {
        fs::create_dir_all(path.parent().unwrap()).expect("fixture parent");
        fs::write(path, "export default () => {};\n").expect("fixture source");
    }
}

#[test]
fn progress_adapter_disables_the_upstream_entrypoint_without_modifying_node_modules() {
    let root = tempfile::tempdir().expect("root");
    let settings = root.path().join("settings.json");
    upstream_fixture(&settings);
    fs::write(
        &settings,
        format!(r#"{{"theme":"dark","packages":["npm:other","{PI_MCP_EXTENSION_SOURCE}"]}}"#),
    )
    .expect("settings");
    let upstream_before = fs::read(
        root.path()
            .join("npm/node_modules/pi-mcp-extension/src/index.ts"),
    )
    .expect("upstream");

    ensure_pi_mcp_progress_adapter(&settings).expect("adapter");
    ensure_pi_mcp_progress_adapter(&settings).expect("idempotent adapter");

    let active: serde_json::Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(active["theme"], "dark");
    assert_eq!(active["packages"][0], "npm:other");
    assert_eq!(
        active["packages"][1],
        serde_json::json!({"source": PI_MCP_EXTENSION_SOURCE, "autoload": false})
    );
    let adapter_root = root.path().join("extensions/grillforge-mcp-progress");
    assert!(adapter_root.join("index.ts").is_file());
    assert!(adapter_root.join("progress-adapter.mjs").is_file());
    assert_eq!(
        fs::read(
            root.path()
                .join("npm/node_modules/pi-mcp-extension/src/index.ts")
        )
        .unwrap(),
        upstream_before
    );
}

#[test]
fn status_only_reports_the_exact_reviewed_pi_mcp_package() {
    let root = tempfile::tempdir().expect("root");
    let settings = root.path().join("settings.json");
    fs::write(&settings, r#"{"packages":["npm:other"]}"#).expect("settings");
    assert!(!pi_mcp_extension_status_at(&settings).unwrap().installed);

    fs::write(
        &settings,
        format!(r#"{{"packages":["{PI_MCP_EXTENSION_SOURCE}"]}}"#),
    )
    .expect("settings");
    let status = pi_mcp_extension_status_at(&settings).unwrap();
    assert!(status.installed);
    assert_eq!(status.package_source, PI_MCP_EXTENSION_SOURCE);
}

#[cfg(unix)]
#[test]
fn one_click_install_uses_the_selected_working_pi_cli_and_rechecks_settings() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().expect("root");
    let settings = root.path().join("settings.json");
    upstream_fixture(&settings);
    let args = root.path().join("args.txt");
    let cli = root.path().join("pi");
    fs::write(
        &cli,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nmkdir -p '{}'\nprintf '{{\"packages\":[\"{}\"]}}' > '{}'\n",
            args.display(),
            settings.parent().unwrap().display(),
            PI_MCP_EXTENSION_SOURCE,
            settings.display(),
        ),
    )
    .expect("fake pi");
    let mut permissions = fs::metadata(&cli).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&cli, permissions).unwrap();

    let status = install_pi_mcp_extension_with(&cli, &settings).expect("install");
    assert!(status.installed);
    assert_eq!(
        fs::read_to_string(args).unwrap(),
        format!("install\n{PI_MCP_EXTENSION_SOURCE}\n--approve\n")
    );
}

#[cfg(unix)]
#[test]
fn one_click_install_exposes_the_selected_pi_runtime_node_to_env_shebangs() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().expect("root");
    let bin = root.path().join("runtime/bin");
    fs::create_dir_all(&bin).expect("bin");
    let settings = root.path().join("settings.json");
    upstream_fixture(&settings);
    let cli = bin.join("pi");
    let node = bin.join("node");
    fs::write(&cli, "#!/usr/bin/env node\n").expect("pi entrypoint");
    fs::write(
        &node,
        format!(
            "#!/bin/sh\nshift\nmkdir -p '{}'\nprintf '{{\"packages\":[\"{}\"]}}' > '{}'\n",
            settings.parent().unwrap().display(),
            PI_MCP_EXTENSION_SOURCE,
            settings.display(),
        ),
    )
    .expect("node shim");
    for executable in [&cli, &node] {
        fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let status = install_pi_mcp_extension_with(&cli, &settings).expect("install through node");
    assert!(status.installed);
}

#[cfg(unix)]
#[test]
fn install_surfaces_nonzero_exit_and_success_without_registration() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().expect("root");
    let settings = root.path().join("settings.json");
    let cli = root.path().join("pi");
    fs::write(
        &cli,
        "#!/bin/sh\nprintf 'registry unavailable\\n' >&2\nexit 7\n",
    )
    .expect("fake pi");
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
    let error = install_pi_mcp_extension_with(&cli, &settings).expect_err("nonzero exit");
    assert!(error.contains("registry unavailable"));

    fs::write(&cli, "#!/bin/sh\nexit 0\n").expect("fake pi");
    let error = install_pi_mcp_extension_with(&cli, &settings).expect_err("missing registration");
    assert!(error.contains("reported a successful installation"));
}

#[cfg(unix)]
#[test]
fn install_timeout_kills_the_stuck_pi_process() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().expect("root");
    let settings = root.path().join("settings.json");
    let cli = root.path().join("pi");
    fs::write(&cli, "#!/bin/sh\nsleep 30\n").expect("fake pi");
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();

    let error = install_pi_mcp_extension_with_timeout(&cli, &settings, Duration::from_millis(50))
        .expect_err("timeout");

    assert_eq!(error, "Pi MCP extension installation timed out");
}
