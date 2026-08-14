use serde::Serialize;
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const PI_MCP_EXTENSION_SOURCE: &str = "npm:pi-mcp-extension@1.5.0";
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiMcpExtensionStatus {
    pub installed: bool,
    pub package_source: &'static str,
}

pub fn pi_mcp_extension_status_at(settings_path: &Path) -> Result<PiMcpExtensionStatus, String> {
    Ok(PiMcpExtensionStatus {
        installed: crate::mcp_mount::pi_mcp_extension_installed(settings_path)?,
        package_source: PI_MCP_EXTENSION_SOURCE,
    })
}

pub fn install_pi_mcp_extension_with(
    cli_path: &Path,
    settings_path: &Path,
) -> Result<PiMcpExtensionStatus, String> {
    install_pi_mcp_extension_with_timeout(cli_path, settings_path, INSTALL_TIMEOUT)
}

pub fn install_pi_mcp_extension_with_timeout(
    cli_path: &Path,
    settings_path: &Path,
    timeout: Duration,
) -> Result<PiMcpExtensionStatus, String> {
    if !cli_path.is_file() {
        return Err(format!("Pi CLI does not exist: {}", cli_path.display()));
    }
    let mut command = Command::new(cli_path);
    command.args(["install", PI_MCP_EXTENSION_SOURCE, "--approve"]);
    if let Some(bin_dir) = cli_path.parent() {
        let mut path = OsString::from(bin_dir.as_os_str());
        if let Some(existing) = std::env::var_os("PATH").filter(|value| !value.is_empty()) {
            path.push(if cfg!(windows) { ";" } else { ":" });
            path.push(existing);
        }
        command.env("PATH", path);
    }
    let mut child = command
        .env_remove("PI_OFFLINE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start Pi MCP extension installation: {error}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        if child
            .try_wait()
            .map_err(|error| format!("could not inspect Pi MCP extension installation: {error}"))?
            .is_some()
        {
            let output = child.wait_with_output().map_err(|error| {
                format!("could not finish Pi MCP extension installation: {error}")
            })?;
            if !output.status.success() {
                let message = String::from_utf8_lossy(&output.stderr);
                let message = message.lines().next().unwrap_or("unknown Pi install error");
                return Err(format!("Pi MCP extension installation failed: {message}"));
            }
            let status = pi_mcp_extension_status_at(settings_path)?;
            if !status.installed {
                return Err(format!(
                    "Pi reported a successful installation but {} does not contain {}",
                    settings_path.display(),
                    PI_MCP_EXTENSION_SOURCE
                ));
            }
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Pi MCP extension installation timed out".into());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[tauri::command]
pub fn pi_mcp_extension_status() -> Result<PiMcpExtensionStatus, String> {
    let settings = crate::adapters::pi::current_pi_paths()
        .map_err(|error| error.to_string())?
        .settings_path;
    pi_mcp_extension_status_at(&settings)
}

#[tauri::command]
pub async fn install_pi_mcp_extension(
    control: tauri::State<'_, crate::application::ControlPlaneService>,
    integration: tauri::State<'_, crate::extension_integration::ExtensionIntegrationService>,
    gateway: tauri::State<'_, crate::gateway::GatewayStatus>,
) -> Result<PiMcpExtensionStatus, String> {
    let status = tauri::async_runtime::spawn_blocking(|| {
        let detection = crate::adapters::pi::detect_pi_cli()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "Pi CLI is required before installing its MCP extension".to_string())?;
        let settings = crate::adapters::pi::current_pi_paths()
            .map_err(|error| error.to_string())?
            .settings_path;
        install_pi_mcp_extension_with(&detection.path, &settings)
    })
    .await
    .map_err(|error| format!("Pi MCP extension installation task failed: {error}"))??;
    let state = control.state()?;
    integration.reconcile_client(&state, &gateway, "pi")?;
    Ok(status)
}
