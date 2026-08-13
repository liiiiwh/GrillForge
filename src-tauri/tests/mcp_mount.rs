use grillforge_lib::mcp_mount::{McpClientFormat, McpMountManager, McpMountTarget};
use serde_json::Value;
use std::fs;

#[test]
fn claude_mount_is_isolated_updated_and_removed_without_touching_other_servers() {
    let root = tempfile::tempdir().expect("root");
    let config = root.path().join(".claude.json");
    fs::write(
        &config,
        r#"{"theme":"dark","mcpServers":{"keep":{"command":"keep"}}}"#,
    )
    .expect("fixture");
    let manager = McpMountManager::new(
        root.path().join("snapshots"),
        [McpMountTarget::new(
            "claude_code",
            &config,
            McpClientFormat::ClaudeJson,
        )],
    )
    .expect("manager");

    manager
        .mount(
            "claude_code",
            "http://127.0.0.1:15721/mcp/claude_code",
            "first-token",
        )
        .expect("mount");
    manager
        .mount(
            "claude_code",
            "http://127.0.0.1:15721/mcp/claude_code",
            "second-token",
        )
        .expect("live update");

    let active: Value =
        serde_json::from_slice(&fs::read(&config).expect("active config")).expect("active JSON");
    assert_eq!(active["theme"], "dark");
    assert_eq!(active["mcpServers"]["keep"]["command"], "keep");
    assert_eq!(
        active["mcpServers"]["grillforge-claude-code"]["url"],
        "http://127.0.0.1:15721/mcp/claude_code"
    );
    assert_eq!(
        active["mcpServers"]["grillforge-claude-code"]["headers"]["Authorization"],
        "Bearer second-token"
    );
    assert_eq!(
        active["mcpServers"]["grillforge-claude-code"]["alwaysLoad"],
        true
    );
    let mut changed = active;
    changed["theme"] = "light".into();
    changed["mcpServers"]["later"] = serde_json::json!({"command":"later"});
    fs::write(&config, serde_json::to_vec_pretty(&changed).unwrap()).expect("later edits");

    manager.unmount("claude_code").expect("unmount");
    let restored: Value = serde_json::from_slice(&fs::read(&config).expect("restored config"))
        .expect("restored JSON");
    assert_eq!(restored["theme"], "light");
    assert_eq!(restored["mcpServers"]["keep"]["command"], "keep");
    assert_eq!(restored["mcpServers"]["later"]["command"], "later");
    assert!(
        restored["mcpServers"]
            .get("grillforge-claude-code")
            .is_none()
    );
}

#[test]
fn claude_desktop_mount_uses_a_local_stdio_server_that_forwards_to_grillforge() {
    let root = tempfile::tempdir().expect("root");
    let config = root.path().join("claude_desktop_config.json");
    fs::write(
        &config,
        r#"{"deploymentMode":"1p","mcpServers":{"keep":{"command":"keep"}}}"#,
    )
    .expect("fixture");
    let manager = McpMountManager::new(
        root.path().join("snapshots"),
        [McpMountTarget::new(
            "claude_desktop",
            &config,
            McpClientFormat::ClaudeDesktopJson,
        )
        .with_stdio_command("/Applications/GrillForge.app/Contents/MacOS/grillforge")],
    )
    .expect("manager");

    manager
        .mount(
            "claude_desktop",
            "http://127.0.0.1:15721/mcp/claude_desktop",
            "desktop-token",
        )
        .expect("mount");
    let active: Value =
        serde_json::from_slice(&fs::read(&config).expect("active config")).expect("JSON");
    assert_eq!(active["deploymentMode"], "1p");
    assert_eq!(active["mcpServers"]["keep"]["command"], "keep");
    let server = &active["mcpServers"]["grillforge-claude-desktop"];
    assert_eq!(
        server["command"],
        "/Applications/GrillForge.app/Contents/MacOS/grillforge"
    );
    assert_eq!(server["args"], serde_json::json!(["mcp-stdio"]));
    assert_eq!(
        server["env"]["GRILLFORGE_MCP_URL"],
        "http://127.0.0.1:15721/mcp/claude_desktop"
    );
    assert_eq!(server["env"]["GRILLFORGE_MCP_TOKEN"], "desktop-token");
    assert!(server.get("transport").is_none());
    assert!(server.get("url").is_none());

    let mut changed = active;
    changed["deploymentMode"] = "3p".into();
    fs::write(&config, serde_json::to_vec_pretty(&changed).unwrap()).expect("switch to 3p");

    manager.unmount("claude_desktop").expect("unmount");
    let restored: Value =
        serde_json::from_slice(&fs::read(&config).expect("restored config")).expect("JSON");
    assert_eq!(restored["deploymentMode"], "3p");
    assert_eq!(restored["mcpServers"]["keep"]["command"], "keep");
    assert!(
        restored["mcpServers"]
            .get("grillforge-claude-desktop")
            .is_none()
    );
}

#[test]
fn claude_desktop_replaces_the_invalid_legacy_http_entry_and_never_restores_it() {
    let root = tempfile::tempdir().expect("root");
    let config = root.path().join("claude_desktop_config.json");
    fs::write(
        &config,
        r#"{"deploymentMode":"1p","mcpServers":{"grillforge-claude-desktop":{"transport":"http","url":"http://127.0.0.1:15721/mcp/claude_desktop","headers":{"Authorization":"Bearer old"}}}}"#,
    )
    .expect("legacy fixture");
    let manager = McpMountManager::new(
        root.path().join("snapshots"),
        [McpMountTarget::new(
            "claude_desktop",
            &config,
            McpClientFormat::ClaudeDesktopJson,
        )
        .with_stdio_command("/Applications/GrillForge.app/Contents/MacOS/grillforge")],
    )
    .expect("manager");

    manager
        .mount(
            "claude_desktop",
            "http://127.0.0.1:15721/mcp/claude_desktop",
            "new-token",
        )
        .expect("replace legacy mount");
    manager.unmount("claude_desktop").expect("unmount");

    let restored: Value =
        serde_json::from_slice(&fs::read(&config).expect("config")).expect("JSON");
    assert_eq!(restored["deploymentMode"], "1p");
    assert!(
        restored["mcpServers"]
            .as_object()
            .expect("servers")
            .is_empty()
    );
}

#[test]
fn codex_mount_uses_the_real_http_mcp_toml_shape() {
    let root = tempfile::tempdir().expect("root");
    let config = root.path().join("config.toml");
    fs::write(&config, "model = \"gpt-5\"\n").expect("fixture");
    let manager = McpMountManager::new(
        root.path().join("snapshots"),
        [McpMountTarget::new(
            "codex",
            &config,
            McpClientFormat::CodexToml,
        )],
    )
    .expect("manager");

    manager
        .mount("codex", "http://127.0.0.1:15721/mcp/codex", "codex-token")
        .expect("mount");
    let active = fs::read_to_string(&config).expect("active config");
    let parsed = active.parse::<toml_edit::DocumentMut>().expect("TOML");
    assert_eq!(parsed["model"].as_str(), Some("gpt-5"));
    assert_eq!(
        parsed["mcp_servers"]["grillforge-codex"]["url"].as_str(),
        Some("http://127.0.0.1:15721/mcp/codex")
    );
    assert_eq!(
        parsed["mcp_servers"]["grillforge-codex"]["http_headers"]["Authorization"].as_str(),
        Some("Bearer codex-token")
    );
    let server = &parsed["mcp_servers"]["grillforge-codex"];
    assert_eq!(server["enabled"].as_bool(), Some(true));
    assert_eq!(server["required"].as_bool(), Some(true));
    assert_eq!(
        server["enabled_tools"]
            .as_array()
            .expect("enabled tools")
            .iter()
            .filter_map(toml_edit::Value::as_str)
            .collect::<Vec<_>>(),
        vec!["list_agents", "run_agent"]
    );
    assert_eq!(
        server["omit_tools_from"]
            .as_array()
            .expect("omitted tool surfaces")
            .iter()
            .filter_map(toml_edit::Value::as_str)
            .collect::<Vec<_>>(),
        vec!["deferred", "code_mode"]
    );
    assert_eq!(
        server["default_tools_approval_mode"].as_str(),
        Some("approve")
    );
    let changed = active.replace("model = \"gpt-5\"", "model = \"gpt-5.1\"")
        + "\n[mcp_servers.later]\nurl = \"http://127.0.0.1:9999/mcp\"\n";
    fs::write(&config, changed).expect("later edits");

    manager.unmount("codex").expect("unmount");
    let restored = fs::read_to_string(&config).expect("restored");
    let parsed = restored.parse::<toml_edit::DocumentMut>().expect("TOML");
    assert_eq!(parsed["model"].as_str(), Some("gpt-5.1"));
    assert_eq!(
        parsed["mcp_servers"]["later"]["url"].as_str(),
        Some("http://127.0.0.1:9999/mcp")
    );
    assert!(parsed["mcp_servers"].get("grillforge-codex").is_none());
}

#[test]
fn unsupported_or_non_loopback_mounts_fail_before_writing() {
    let root = tempfile::tempdir().expect("root");
    let config = root.path().join("pi.json");
    let manager = McpMountManager::new(
        root.path().join("snapshots"),
        [McpMountTarget::new(
            "pi",
            &config,
            McpClientFormat::Unsupported,
        )],
    )
    .expect("manager");

    assert_eq!(
        manager
            .mount("pi", "http://127.0.0.1:15721/mcp/pi", "token")
            .expect_err("Pi cannot be advertised as MCP-capable"),
        "client pi does not provide a verified MCP configuration format"
    );
    assert!(
        manager
            .mount("pi", "https://example.com/mcp/pi", "token")
            .is_err()
    );
    assert!(!config.exists());
}

#[test]
fn json_mcp_clients_use_their_native_remote_server_shapes() {
    let cases = [
        (
            "gemini",
            McpClientFormat::GeminiJson,
            "mcpServers",
            "httpUrl",
        ),
        ("opencode", McpClientFormat::OpenCodeJson, "mcp", "url"),
        ("kimi_code", McpClientFormat::KimiJson, "mcpServers", "url"),
        ("pi", McpClientFormat::PiExtensionJson, "mcpServers", "url"),
    ];
    for (client, format, section, url_key) in cases {
        let root = tempfile::tempdir().expect("root");
        let config = root.path().join("config.json");
        fs::write(&config, r#"{"keep":true}"#).expect("fixture");
        let mut target = McpMountTarget::new(client, &config, format);
        if format == McpClientFormat::ClaudeDesktopJson {
            target =
                target.with_stdio_command("/Applications/GrillForge.app/Contents/MacOS/grillforge");
        }
        let manager =
            McpMountManager::new(root.path().join("snapshots"), [target]).expect("manager");
        let url = format!("http://127.0.0.1:15721/mcp/{client}");
        manager.mount(client, &url, "token").expect("mount");
        let active: Value =
            serde_json::from_slice(&fs::read(&config).expect("active")).expect("JSON");
        assert_eq!(active["keep"], true);
        assert_eq!(
            active[section][format!("grillforge-{}", client.replace('_', "-"))][url_key],
            url
        );
        let mut changed = active;
        changed["keep"] = false.into();
        changed[section]["later"] = serde_json::json!({url_key:"later"});
        fs::write(&config, serde_json::to_vec_pretty(&changed).unwrap()).expect("later edits");
        manager.unmount(client).expect("unmount");
        let restored: Value =
            serde_json::from_slice(&fs::read(&config).expect("restored")).expect("JSON");
        assert_eq!(restored["keep"], false);
        assert_eq!(restored[section]["later"][url_key], "later");
        assert!(
            restored[section]
                .get(format!("grillforge-{}", client.replace('_', "-")))
                .is_none()
        );
    }
}

#[test]
fn unmount_preserves_user_removal_of_the_mcp_section_for_every_client_format() {
    let json_cases = [
        ("claude_code", McpClientFormat::ClaudeJson),
        ("claude_desktop", McpClientFormat::ClaudeDesktopJson),
        ("gemini", McpClientFormat::GeminiJson),
        ("opencode", McpClientFormat::OpenCodeJson),
        ("kimi_code", McpClientFormat::KimiJson),
        ("pi", McpClientFormat::PiExtensionJson),
    ];
    for (client, format) in json_cases {
        let root = tempfile::tempdir().expect("root");
        let config = root.path().join("config.json");
        fs::write(&config, r#"{"model":"before"}"#).expect("fixture");
        let mut target = McpMountTarget::new(client, &config, format);
        if format == McpClientFormat::ClaudeDesktopJson {
            target =
                target.with_stdio_command("/Applications/GrillForge.app/Contents/MacOS/grillforge");
        }
        let manager =
            McpMountManager::new(root.path().join("snapshots"), [target]).expect("manager");
        manager
            .mount(
                client,
                &format!("http://127.0.0.1:15721/mcp/{client}"),
                "token",
            )
            .expect("mount");
        fs::write(&config, r#"{"model":"after"}"#).expect("user edit");

        manager.unmount(client).expect("unmount");

        let restored: Value =
            serde_json::from_slice(&fs::read(&config).expect("config")).expect("JSON");
        assert_eq!(restored, serde_json::json!({"model":"after"}));
    }

    let root = tempfile::tempdir().expect("root");
    let config = root.path().join("config.toml");
    fs::write(&config, "model = \"before\"\n").expect("fixture");
    let manager = McpMountManager::new(
        root.path().join("snapshots"),
        [McpMountTarget::new(
            "codex",
            &config,
            McpClientFormat::CodexToml,
        )],
    )
    .expect("manager");
    manager
        .mount("codex", "http://127.0.0.1:15721/mcp/codex", "token")
        .expect("mount");
    fs::write(&config, "model = \"after\"\n").expect("user edit");

    manager.unmount("codex").expect("unmount");

    assert_eq!(
        fs::read_to_string(&config).expect("config"),
        "model = \"after\"\n"
    );
}

#[test]
fn unmount_preserves_a_user_replacement_of_the_reserved_server_name() {
    let json_cases = [
        (
            "claude_code",
            McpClientFormat::ClaudeJson,
            "mcpServers",
            "url",
        ),
        (
            "claude_desktop",
            McpClientFormat::ClaudeDesktopJson,
            "mcpServers",
            "url",
        ),
        (
            "gemini",
            McpClientFormat::GeminiJson,
            "mcpServers",
            "httpUrl",
        ),
        ("opencode", McpClientFormat::OpenCodeJson, "mcp", "url"),
        ("kimi_code", McpClientFormat::KimiJson, "mcpServers", "url"),
        ("pi", McpClientFormat::PiExtensionJson, "mcpServers", "url"),
    ];
    for (client, format, section, url_key) in json_cases {
        let root = tempfile::tempdir().expect("root");
        let config = root.path().join("config.json");
        fs::write(&config, r#"{"setting":"before"}"#).expect("fixture");
        let mut target = McpMountTarget::new(client, &config, format);
        if format == McpClientFormat::ClaudeDesktopJson {
            target =
                target.with_stdio_command("/Applications/GrillForge.app/Contents/MacOS/grillforge");
        }
        let manager =
            McpMountManager::new(root.path().join("snapshots"), [target]).expect("manager");
        manager
            .mount(
                client,
                &format!("http://127.0.0.1:15721/mcp/{client}"),
                "token",
            )
            .expect("mount");
        let mut changed: Value =
            serde_json::from_slice(&fs::read(&config).expect("config")).expect("JSON");
        changed["setting"] = "after".into();
        changed[section][format!("grillforge-{}", client.replace('_', "-"))] =
            serde_json::json!({url_key: "http://127.0.0.1:9999/user-owned"});
        fs::write(&config, serde_json::to_vec_pretty(&changed).unwrap()).expect("user edit");

        manager.unmount(client).expect("unmount");

        let restored: Value =
            serde_json::from_slice(&fs::read(&config).expect("config")).expect("JSON");
        assert_eq!(restored["setting"], "after");
        assert_eq!(
            restored[section][format!("grillforge-{}", client.replace('_', "-"))][url_key],
            "http://127.0.0.1:9999/user-owned"
        );
    }

    let root = tempfile::tempdir().expect("root");
    let config = root.path().join("config.toml");
    fs::write(&config, "model = \"before\"\n").expect("fixture");
    let manager = McpMountManager::new(
        root.path().join("snapshots"),
        [McpMountTarget::new(
            "codex",
            &config,
            McpClientFormat::CodexToml,
        )],
    )
    .expect("manager");
    manager
        .mount("codex", "http://127.0.0.1:15721/mcp/codex", "token")
        .expect("mount");
    fs::write(
        &config,
        "model = \"after\"\n\n[mcp_servers.grillforge-codex]\nurl = \"http://127.0.0.1:9999/user-owned\"\n",
    )
    .expect("user edit");

    manager.unmount("codex").expect("unmount");

    let restored = fs::read_to_string(&config).expect("config");
    let restored = restored.parse::<toml_edit::DocumentMut>().expect("TOML");
    assert_eq!(restored["model"].as_str(), Some("after"));
    assert_eq!(
        restored["mcp_servers"]["grillforge-codex"]["url"].as_str(),
        Some("http://127.0.0.1:9999/user-owned")
    );
}

#[test]
fn pi_mcp_is_only_available_when_the_user_installed_the_extension() {
    let root = tempfile::tempdir().expect("root");
    let settings = root.path().join("settings.json");
    fs::write(&settings, r#"{"packages":["npm:other"]}"#).expect("settings");
    assert!(
        !grillforge_lib::mcp_mount::pi_mcp_extension_installed(&settings)
            .expect("inspect settings")
    );
    fs::write(&settings, r#"{"packages":["npm:pi-mcp-extension"]}"#).expect("settings");
    assert!(
        grillforge_lib::mcp_mount::pi_mcp_extension_installed(&settings).expect("inspect settings")
    );
}
