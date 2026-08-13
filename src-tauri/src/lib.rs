pub mod adapters;
pub mod application;
pub mod bridge;
pub mod claude_desktop_integration;
pub mod cli_discovery;
pub mod client_integrations;
pub mod codex_integration;
pub mod configuration;
pub mod core;
pub mod extension_integration;
pub mod gateway;
pub mod integration;
pub mod local_agents;
pub mod mcp_mount;
pub mod mcp_stdio;
pub mod model_discovery;
pub mod pi_integration;
pub mod pi_mcp_extension;
pub mod presets;
pub mod storage;
pub mod usage_query;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::Manager;

    tauri::Builder::default()
        .setup(|app| {
            let home = app.path().home_dir()?;
            let root = home.join(".grillforge");
            let executable = std::env::current_exe()?;
            let kimi_root = std::env::var_os("KIMI_CODE_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join(".kimi-code"));
            app.manage(application::ControlPlaneService::new(&root));
            app.manage(integration::IntegrationService::new(
                integration::default_claude_config_root(&home),
                &root,
            ));
            app.manage(
                claude_desktop_integration::ClaudeDesktopIntegrationService::new(
                    claude_desktop_integration::default_claude_desktop_paths(&home),
                    &root,
                )
                .with_native_catalog_paths(
                    home.join(".claude"),
                    home.join(".claude.json"),
                    Some(home.join("Library/Application Support/Claude/Local Storage/leveldb")),
                ),
            );
            app.manage(pi_integration::PiIntegrationService::new(
                adapters::pi::paths_from_home(&home),
                &root,
            ));
            app.manage(codex_integration::CodexIntegrationService::new(
                adapters::codex::paths_from_home(&home),
                &root,
            ));
            app.manage(client_integrations::GeminiIntegrationService::new(
                adapters::gemini::paths_from_home(&home),
                &root,
            ));
            app.manage(client_integrations::GrokBuildIntegrationService::new(
                adapters::grok_build::paths_from_home(&home),
                &root,
            ));
            app.manage(client_integrations::OpenCodeIntegrationService::new(
                adapters::opencode::paths_from_home(&home),
                &root,
            ));
            app.manage(client_integrations::HermesIntegrationService::new(
                adapters::hermes::paths_from_home(&home),
                &root,
            ));
            app.manage(client_integrations::KimiCodeIntegrationService::new(
                adapters::kimi_code::KimiCodePaths::new(kimi_root.join("config.toml")),
                &root,
            ));
            let mounts = mcp_mount::McpMountManager::new(
                root.join("mcp-snapshots"),
                [
                    mcp_mount::McpMountTarget::new(
                        "claude_code",
                        home.join(".claude.json"),
                        mcp_mount::McpClientFormat::ClaudeJson,
                    )
                    .with_stdio_command(&executable),
                    mcp_mount::McpMountTarget::new(
                        "claude_desktop",
                        claude_desktop_integration::default_claude_desktop_paths(&home)
                            .normal_config_path,
                        mcp_mount::McpClientFormat::ClaudeDesktopJson,
                    )
                    .with_stdio_command(&executable),
                    mcp_mount::McpMountTarget::new(
                        "codex",
                        adapters::codex::paths_from_home(&home).config_path,
                        mcp_mount::McpClientFormat::CodexToml,
                    ),
                    mcp_mount::McpMountTarget::new(
                        "gemini",
                        adapters::gemini::paths_from_home(&home).settings_path,
                        mcp_mount::McpClientFormat::GeminiJson,
                    ),
                    mcp_mount::McpMountTarget::new(
                        "opencode",
                        adapters::opencode::paths_from_home(&home).config_path,
                        mcp_mount::McpClientFormat::OpenCodeJson,
                    ),
                    mcp_mount::McpMountTarget::new(
                        "kimi_code",
                        kimi_root.join("mcp.json"),
                        mcp_mount::McpClientFormat::KimiJson,
                    ),
                    mcp_mount::McpMountTarget::new(
                        "pi",
                        home.join(".pi/agent/mcp.json"),
                        mcp_mount::McpClientFormat::PiExtensionJson,
                    ),
                ],
            )?;
            app.manage(
                extension_integration::ExtensionIntegrationService::new(
                    mounts,
                    home.join(".claude"),
                    None,
                    Some(home.join(".pi/agent/settings.json")),
                )
                .with_codex(home.join(".codex"), None)
                .with_gemini(home.join(".gemini"), None)
                .with_pi(home.join(".pi/agent"), None)
                .with_opencode(home.join(".config/opencode"), None)
                .with_kimi(kimi_root, None),
            );

            let listener = std::net::TcpListener::bind(gateway::DEFAULT_GATEWAY_ADDRESS)?;
            listener.set_nonblocking(true)?;
            let gateway = gateway::Gateway::new(root);
            let status = gateway.status(format!("http://{}", gateway::DEFAULT_GATEWAY_ADDRESS));
            let router = gateway.router();
            tauri::async_runtime::spawn(async move {
                let listener = tokio::net::TcpListener::from_std(listener)
                    .expect("validated non-blocking gateway listener");
                axum::serve(listener, router)
                    .await
                    .expect("GrillForge gateway stopped unexpectedly");
            });
            app.manage(status);

            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = restore_enabled_clients(&app_handle) {
                    eprintln!("GrillForge startup restore failed: {error}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            application::load_state,
            application::save_provider,
            application::update_provider,
            application::delete_provider,
            application::sync_provider_models,
            application::save_model_with_native_protocols,
            application::update_model_with_native_protocols,
            application::delete_model,
            application::set_main_model,
            application::set_claude_native_model,
            application::set_claude_desktop_model_slot,
            application::set_model_slot,
            application::set_pi_main_model,
            application::set_pi_model_enabled,
            application::set_codex_main_model,
            application::set_codex_native_main_model,
            application::set_codex_default_subagent_model,
            application::set_codex_native_default_subagent_model,
            application::set_codex_custom_agent_model,
            application::set_codex_native_custom_agent_model,
            application::set_client_main_model,
            application::set_client_model_enabled,
            application::save_extension_subagent,
            extension_integration::update_extension_subagent,
            application::delete_extension_subagent,
            extension_integration::set_client_extension_binding,
            extension_integration::mount_client_mcp,
            extension_integration::unmount_client_mcp,
            extension_integration::client_mcp_status,
            extension_integration::client_mcp_statuses,
            local_agents::discover_local_agents,
            application::test_model_connection,
            application::query_provider_usage,
            gateway::gateway_status,
            presets::provider_presets,
            integration::integration_status,
            integration::detect_claude_code,
            integration::apply_claude_code,
            integration::disable_claude_code,
            claude_desktop_integration::claude_desktop_status,
            claude_desktop_integration::apply_claude_desktop,
            claude_desktop_integration::disable_claude_desktop,
            claude_desktop_integration::restart_claude_client,
            pi_integration::pi_status,
            pi_integration::apply_pi,
            pi_integration::disable_pi,
            pi_mcp_extension::pi_mcp_extension_status,
            pi_mcp_extension::install_pi_mcp_extension,
            codex_integration::codex_status,
            codex_integration::apply_codex,
            codex_integration::disable_codex,
            client_integrations::gemini_status,
            client_integrations::apply_gemini,
            client_integrations::disable_gemini,
            client_integrations::grok_build_status,
            client_integrations::apply_grok_build,
            client_integrations::disable_grok_build,
            client_integrations::opencode_status,
            client_integrations::apply_opencode,
            client_integrations::disable_opencode,
            client_integrations::hermes_status,
            client_integrations::apply_hermes,
            client_integrations::disable_hermes,
            client_integrations::kimi_code_status,
            client_integrations::apply_kimi_code,
            client_integrations::disable_kimi_code,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                if let Err(error) = restore_live_configs_before_exit(app) {
                    eprintln!(
                        "GrillForge refused to exit before restoring client configs: {error}"
                    );
                    api.prevent_exit();
                }
            }
            tauri::RunEvent::Exit => {
                if let Err(error) = restore_live_configs_before_exit(app) {
                    eprintln!("GrillForge final exit restore failed: {error}");
                }
            }
            _ => {}
        });
}

fn restore_one<T>(
    name: &str,
    enabled: bool,
    configured: bool,
    snapshot_present: bool,
    resume: impl FnOnce() -> Result<bool, String>,
    apply: impl FnOnce() -> Result<T, String>,
) -> Result<(), String> {
    if !enabled || !configured {
        return Ok(());
    }
    if snapshot_present {
        if !resume()? {
            return Err(format!(
                "{name} has a recovery snapshot whose live configuration differs; use Reapply"
            ));
        }
        return Ok(());
    }
    apply().map(|_| ())
}

fn restore_enabled_clients(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;

    let control = app.state::<application::ControlPlaneService>();
    let gateway = app.state::<gateway::GatewayStatus>();
    let extensions = app.state::<extension_integration::ExtensionIntegrationService>();
    extensions.restore_clients_then_reconcile(&control, &gateway, || {
        restore_enabled_model_clients(app, &control, &gateway)
    })
}

fn restore_enabled_model_clients(
    app: &tauri::AppHandle,
    control: &application::ControlPlaneService,
    gateway: &gateway::GatewayStatus,
) -> Result<(), String> {
    use tauri::Manager;

    let state = control.state()?;
    let mut failures = Vec::new();

    let claude = app.state::<integration::IntegrationService>();
    if let Some(native_base_url) = claude.native_upstream_base_url()? {
        gateway.set_native_base_url(&native_base_url)?;
    } else {
        gateway.use_official_native_base_url();
    }
    remember_restore(
        &mut failures,
        "Claude Code",
        restore_one(
            "Claude Code",
            control.client_integration_enabled("claude_code")?,
            control.client_has_managed_configuration("claude_code")?,
            claude.recovery_pending(),
            || claude.resume_if_applied(&state, gateway),
            || {
                let status = claude.apply(&state, &gateway.base_url)?;
                if let Err(error) = gateway.activate(&state) {
                    let _ = claude.disable();
                    return Err(error);
                }
                Ok(status)
            },
        ),
    );

    let desktop = app.state::<claude_desktop_integration::ClaudeDesktopIntegrationService>();
    remember_restore(
        &mut failures,
        "Claude Client",
        restore_one(
            "Claude Client",
            control.client_integration_enabled("claude_desktop")?,
            control.client_has_managed_configuration("claude_desktop")?,
            desktop.recovery_pending(),
            || desktop.resume_if_applied(&state, gateway),
            || desktop.apply(&state, gateway),
        ),
    );

    let pi = app.state::<pi_integration::PiIntegrationService>();
    remember_restore(
        &mut failures,
        "Pi",
        restore_one(
            "Pi",
            control.client_integration_enabled("pi")?,
            control.client_has_managed_configuration("pi")?,
            pi.recovery_pending(),
            || pi.resume_if_applied(&state, gateway),
            || pi.apply(&state, gateway),
        ),
    );

    let codex = app.state::<codex_integration::CodexIntegrationService>();
    remember_restore(
        &mut failures,
        "Codex",
        restore_one(
            "Codex",
            control.client_integration_enabled("codex")?,
            control.client_has_managed_configuration("codex")?,
            codex.recovery_pending(),
            || codex.resume_if_applied(control, gateway),
            || codex.apply(control, gateway),
        ),
    );

    let gemini = app.state::<client_integrations::GeminiIntegrationService>();
    remember_restore(
        &mut failures,
        "Gemini",
        restore_one(
            "Gemini",
            control.client_integration_enabled("gemini")?,
            control.client_has_managed_configuration("gemini")?,
            gemini.recovery_pending(),
            || gemini.resume_if_applied(control, gateway),
            || gemini.apply(control, gateway),
        ),
    );

    let grok = app.state::<client_integrations::GrokBuildIntegrationService>();
    remember_restore(
        &mut failures,
        "Grok Build",
        restore_one(
            "Grok Build",
            control.client_integration_enabled("grok_build")?,
            control.client_has_managed_configuration("grok_build")?,
            grok.recovery_pending(),
            || grok.resume_if_applied(control, gateway),
            || grok.apply_with_gateway(control, gateway),
        ),
    );

    let opencode = app.state::<client_integrations::OpenCodeIntegrationService>();
    remember_restore(
        &mut failures,
        "OpenCode",
        restore_one(
            "OpenCode",
            control.client_integration_enabled("opencode")?,
            control.client_has_managed_configuration("opencode")?,
            opencode.recovery_pending(),
            || opencode.resume_if_applied(control, gateway),
            || opencode.apply(control, gateway),
        ),
    );

    let hermes = app.state::<client_integrations::HermesIntegrationService>();
    remember_restore(
        &mut failures,
        "Hermes",
        restore_one(
            "Hermes",
            control.client_integration_enabled("hermes")?,
            control.client_has_managed_configuration("hermes")?,
            hermes.recovery_pending(),
            || hermes.resume_if_applied(control, gateway),
            || hermes.apply(control, gateway),
        ),
    );

    let kimi = app.state::<client_integrations::KimiCodeIntegrationService>();
    remember_restore(
        &mut failures,
        "Kimi Code",
        restore_one(
            "Kimi Code",
            control.client_integration_enabled("kimi_code")?,
            control.client_has_managed_configuration("kimi_code")?,
            kimi.recovery_pending(),
            || kimi.resume_if_applied(control, gateway),
            || kimi.apply(control, gateway),
        ),
    );
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn remember_restore(failures: &mut Vec<String>, name: &str, result: Result<(), String>) {
    if let Err(error) = result {
        failures.push(format!("{name}: {error}"));
    }
}

fn restore_live_configs_before_exit(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;

    let gateway = app.state::<gateway::GatewayStatus>();
    // MCP is the top configuration layer. Every model mutation temporarily
    // suspends and then remounts it, so exit restores the layers in reverse.
    let extensions = app.state::<extension_integration::ExtensionIntegrationService>();
    extensions.restore_live_mounts(&gateway)?;
    let control = app.state::<application::ControlPlaneService>();
    let state = control.state()?;
    let claude = app.state::<integration::IntegrationService>();
    if claude.recovery_pending() {
        claude.disable()?;
    }
    let desktop = app.state::<claude_desktop_integration::ClaudeDesktopIntegrationService>();
    if desktop.recovery_pending() {
        desktop.disable(&gateway)?;
    }
    let pi = app.state::<pi_integration::PiIntegrationService>();
    if pi.recovery_pending() {
        pi.disable(&state, &gateway)?;
    }
    let codex = app.state::<codex_integration::CodexIntegrationService>();
    if codex.recovery_pending() {
        codex.disable(&control, &gateway)?;
    }
    let gemini = app.state::<client_integrations::GeminiIntegrationService>();
    if gemini.recovery_pending() {
        gemini.disable(&control, &gateway)?;
    }
    let grok = app.state::<client_integrations::GrokBuildIntegrationService>();
    if grok.recovery_pending() {
        grok.disable(&control, &gateway)?;
    }
    let opencode = app.state::<client_integrations::OpenCodeIntegrationService>();
    if opencode.recovery_pending() {
        opencode.disable(&control, &gateway)?;
    }
    let hermes = app.state::<client_integrations::HermesIntegrationService>();
    if hermes.recovery_pending() {
        hermes.disable(&control, &gateway)?;
    }
    let kimi = app.state::<client_integrations::KimiCodeIntegrationService>();
    if kimi.recovery_pending() {
        kimi.disable(&control, &gateway)?;
    }
    gateway.deactivate();
    Ok(())
}
