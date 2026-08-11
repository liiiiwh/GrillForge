use grillforge_lib::adapters::claude_desktop::{
    ClaudeDesktopAdapter, ClaudeDesktopRequest, ClaudeDesktopRouteSpec,
    ClaudeDesktopTakeoverStatus, PROFILE_ID, detect_macos_claude_client_in,
    detect_windows_claude_client_in, is_claude_desktop_route_id, macos_paths_from_home,
    windows_paths_from_local_app_data,
};
use serde_json::{Value, json};
use std::fs;
use tempfile::tempdir;

fn request() -> ClaudeDesktopRequest {
    ClaudeDesktopRequest::new(
        "http://127.0.0.1:15721/claude-desktop",
        "local-secret-token",
        vec![
            ClaudeDesktopRouteSpec::new("claude-sonnet-4-6", Some("DeepSeek V3"), true),
            ClaudeDesktopRouteSpec::new("claude-fable-5", None::<String>, false),
        ],
    )
}

#[test]
fn platform_paths_match_claude_desktop_3p_layout() {
    let mac = macos_paths_from_home("/Users/example");
    assert_eq!(
        mac.normal_config_path,
        std::path::Path::new(
            "/Users/example/Library/Application Support/Claude/claude_desktop_config.json"
        )
    );
    assert_eq!(
        mac.profile_path,
        std::path::PathBuf::from(format!(
            "/Users/example/Library/Application Support/Claude-3p/configLibrary/{PROFILE_ID}.json"
        ))
    );

    let windows = windows_paths_from_local_app_data(r"C:\Users\example\AppData\Local");
    assert_eq!(
        windows.threep_config_path,
        std::path::Path::new(
            r"C:\Users\example\AppData\Local/Claude-3p/claude_desktop_config.json"
        )
    );
}

#[test]
fn route_ids_are_limited_to_claude_desktop_safe_roles() {
    for route in [
        "claude-sonnet-4-6",
        "claude-opus-4-1",
        "claude-haiku-4-5",
        "claude-fable-5",
        "anthropic/claude-sonnet-4-6",
    ] {
        assert!(is_claude_desktop_route_id(route), "expected safe: {route}");
    }
    for route in [
        "gpt-5",
        "claude-mythos-1",
        "claude-sonnet-",
        "claude-sonnet-4[1m]",
        "Claude-sonnet-4",
    ] {
        assert!(
            !is_claude_desktop_route_id(route),
            "expected unsafe: {route}"
        );
    }
}

#[test]
fn detection_checks_the_native_client_executable() {
    let root = tempdir().unwrap();
    let mac_executable = root
        .path()
        .join("Applications/Claude.app/Contents/MacOS/Claude");
    fs::create_dir_all(mac_executable.parent().unwrap()).unwrap();
    fs::write(&mac_executable, b"binary").unwrap();
    assert_eq!(
        detect_macos_claude_client_in(&[root.path().join("Applications")])
            .unwrap()
            .executable_path,
        mac_executable
    );

    let windows_executable = root.path().join("Local/Programs/Claude/Claude.exe");
    fs::create_dir_all(windows_executable.parent().unwrap()).unwrap();
    fs::write(&windows_executable, b"binary").unwrap();
    assert_eq!(
        detect_windows_claude_client_in(&[root.path().join("Local")])
            .unwrap()
            .executable_path,
        windows_executable
    );
}

#[test]
fn apply_writes_both_deployment_modes_gateway_profile_and_meta() {
    let root = tempdir().expect("temporary root");
    let paths = macos_paths_from_home(root.path());
    fs::create_dir_all(paths.normal_config_path.parent().unwrap()).unwrap();
    fs::write(&paths.normal_config_path, br#"{"keep":"normal"}"#).unwrap();
    fs::create_dir_all(paths.threep_config_path.parent().unwrap()).unwrap();
    fs::write(&paths.threep_config_path, br#"{"keep":"3p"}"#).unwrap();
    let adapter = ClaudeDesktopAdapter::new(paths.clone(), root.path().join("grillforge"));

    adapter
        .apply(request())
        .expect("apply Claude Client profile");

    let normal: Value =
        serde_json::from_slice(&fs::read(&paths.normal_config_path).unwrap()).unwrap();
    let threep: Value =
        serde_json::from_slice(&fs::read(&paths.threep_config_path).unwrap()).unwrap();
    let profile: Value = serde_json::from_slice(&fs::read(&paths.profile_path).unwrap()).unwrap();
    let meta: Value = serde_json::from_slice(&fs::read(&paths.meta_path).unwrap()).unwrap();
    assert_eq!(normal, json!({"keep": "normal", "deploymentMode": "3p"}));
    assert_eq!(threep, json!({"keep": "3p", "deploymentMode": "3p"}));
    assert_eq!(
        profile["inferenceGatewayBaseUrl"],
        json!("http://127.0.0.1:15721/claude-desktop")
    );
    assert_eq!(
        profile["inferenceGatewayApiKey"],
        json!("local-secret-token")
    );
    assert_eq!(profile["inferenceGatewayAuthScheme"], json!("bearer"));
    assert_eq!(profile["inferenceProvider"], json!("gateway"));
    assert_eq!(profile["disableDeploymentModeChooser"], json!(true));
    assert_eq!(profile["coworkEgressAllowedHosts"], json!(["*"]));
    assert_eq!(
        profile["inferenceModels"],
        json!([
            {"name": "claude-sonnet-4-6", "labelOverride": "DeepSeek V3", "supports1m": true},
            "claude-fable-5"
        ])
    );
    assert_eq!(meta["appliedId"], json!(PROFILE_ID));
    assert_eq!(
        meta["entries"],
        json!([{
            "id": PROFILE_ID,
            "name": "GrillForge"
        }])
    );
    assert!(adapter.snapshot_path().is_file());
    assert_eq!(
        adapter.status().unwrap().takeover,
        ClaudeDesktopTakeoverStatus::Active
    );
}

#[test]
fn invalid_input_fails_before_writing_and_never_discloses_the_token() {
    let root = tempdir().unwrap();
    let paths = macos_paths_from_home(root.path());
    let adapter = ClaudeDesktopAdapter::new(paths.clone(), root.path().join("grillforge"));
    let token = "token-that-must-stay-secret";
    let invalid = ClaudeDesktopRequest::new(
        "https://gateway.example.com",
        token,
        vec![ClaudeDesktopRouteSpec::new("gpt-5", Some("GPT"), false)],
    );
    assert!(!format!("{invalid:?}").contains(token));
    let error = adapter
        .apply(invalid)
        .expect_err("non-loopback gateway fails");
    assert!(!error.to_string().contains(token));
    assert!(!paths.normal_config_path.exists());
    assert!(!adapter.snapshot_path().exists());
}

#[test]
fn disable_restores_all_four_files_exactly_and_removes_the_single_snapshot() {
    let root = tempdir().unwrap();
    let paths = macos_paths_from_home(root.path());
    let originals = [
        (
            &paths.normal_config_path,
            br#"{"deploymentMode":"1p","normal":1}"#.as_slice(),
        ),
        (
            &paths.threep_config_path,
            br#"{"deploymentMode":"1p","threep":2}"#.as_slice(),
        ),
    ];
    for (path, bytes) in originals {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let adapter = ClaudeDesktopAdapter::new(paths.clone(), root.path().join("grillforge"));
    adapter.apply(request()).unwrap();
    adapter
        .disable()
        .expect("restore official Claude Client configuration");

    for (path, expected) in originals {
        assert_eq!(fs::read(path).unwrap(), expected);
    }
    assert!(!paths.profile_path.exists());
    assert!(!paths.meta_path.exists());
    assert!(!adapter.snapshot_path().exists());
    assert_eq!(
        adapter.status().unwrap().takeover,
        ClaudeDesktopTakeoverStatus::Inactive
    );
}

#[test]
fn status_distinguishes_inactive_active_and_drifted() {
    let root = tempdir().unwrap();
    let paths = macos_paths_from_home(root.path());
    let adapter = ClaudeDesktopAdapter::new(paths.clone(), root.path().join("grillforge"));
    assert_eq!(
        adapter.status().unwrap().takeover,
        ClaudeDesktopTakeoverStatus::Inactive
    );

    adapter.apply(request()).unwrap();
    assert_eq!(
        adapter.status().unwrap().takeover,
        ClaudeDesktopTakeoverStatus::Active
    );
    fs::write(&paths.profile_path, br#"{"changed":true}"#).unwrap();
    let status = adapter.status().unwrap();
    assert_eq!(status.takeover, ClaudeDesktopTakeoverStatus::Drifted);
    assert_eq!(
        status.differences,
        ["Claude-3p/configLibrary/GrillForge.json"]
    );
}

#[test]
fn reapply_updates_the_profile_but_keeps_the_one_original_snapshot() {
    let root = tempdir().unwrap();
    let paths = macos_paths_from_home(root.path());
    fs::create_dir_all(paths.normal_config_path.parent().unwrap()).unwrap();
    fs::write(&paths.normal_config_path, br#"{"original":true}"#).unwrap();
    let adapter = ClaudeDesktopAdapter::new(paths.clone(), root.path().join("grillforge"));
    adapter.apply(request()).unwrap();
    adapter
        .apply(ClaudeDesktopRequest::new(
            "http://localhost:15721/claude-desktop",
            "replacement-token",
            vec![ClaudeDesktopRouteSpec::new(
                "claude-haiku-4-5",
                Some("Fast"),
                false,
            )],
        ))
        .unwrap();
    let profile: Value = serde_json::from_slice(&fs::read(&paths.profile_path).unwrap()).unwrap();
    assert_eq!(
        profile["inferenceGatewayApiKey"],
        json!("replacement-token")
    );
    assert_eq!(
        adapter.status().unwrap().takeover,
        ClaudeDesktopTakeoverStatus::Active
    );

    adapter.disable().unwrap();
    assert_eq!(
        fs::read(&paths.normal_config_path).unwrap(),
        br#"{"original":true}"#
    );
    assert!(!paths.threep_config_path.exists());
    assert!(!paths.profile_path.exists());
    assert!(!paths.meta_path.exists());
}

#[cfg(unix)]
#[test]
fn failed_apply_rolls_back_earlier_files_and_the_snapshot() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().unwrap();
    let paths = macos_paths_from_home(root.path());
    let normal = br#"{"deploymentMode":"1p","normal":true}"#;
    let threep = br#"{"deploymentMode":"1p","threep":true}"#;
    fs::create_dir_all(paths.normal_config_path.parent().unwrap()).unwrap();
    fs::write(&paths.normal_config_path, normal).unwrap();
    let threep_dir = paths.threep_config_path.parent().unwrap();
    fs::create_dir_all(threep_dir).unwrap();
    fs::write(&paths.threep_config_path, threep).unwrap();
    fs::set_permissions(threep_dir, fs::Permissions::from_mode(0o500)).unwrap();
    let adapter = ClaudeDesktopAdapter::new(paths.clone(), root.path().join("grillforge"));

    let error = adapter
        .apply(request())
        .expect_err("read-only 3P directory must fail");
    fs::set_permissions(threep_dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(!error.to_string().contains("local-secret-token"));
    assert_eq!(fs::read(&paths.normal_config_path).unwrap(), normal);
    assert_eq!(fs::read(&paths.threep_config_path).unwrap(), threep);
    assert!(!paths.profile_path.exists());
    assert!(!paths.meta_path.exists());
    assert!(!adapter.snapshot_path().exists());
}
