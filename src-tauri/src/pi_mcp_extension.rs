use serde::Serialize;
use serde_json::{Map, Value};
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

pub const PI_MCP_EXTENSION_SOURCE: &str = "npm:pi-mcp-extension@1.5.0";
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);
const PROGRESS_ADAPTER_SOURCE: &str = include_str!("../pi-extension/progress-adapter.js");
const PROGRESS_ADAPTER_DIR: &str = "grillforge-mcp-progress";

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

pub fn ensure_pi_mcp_progress_adapter(settings_path: &Path) -> Result<(), String> {
    let agent_root = settings_path.parent().ok_or_else(|| {
        format!(
            "Pi settings path has no parent directory: {}",
            settings_path.display()
        )
    })?;
    let upstream_entry = agent_root.join("npm/node_modules/pi-mcp-extension/src/index.ts");
    let sdk_entry =
        agent_root.join("npm/node_modules/@modelcontextprotocol/sdk/dist/esm/client/index.js");
    for (label, path) in [
        ("pi-mcp-extension entrypoint", &upstream_entry),
        ("MCP SDK client", &sdk_entry),
    ] {
        if !path.is_file() {
            return Err(format!("{label} does not exist: {}", path.display()));
        }
    }

    let settings_bytes = fs::read(settings_path)
        .map_err(|error| format!("could not read {}: {error}", settings_path.display()))?;
    let mut settings: Value = serde_json::from_slice(&settings_bytes)
        .map_err(|error| format!("invalid Pi settings {}: {error}", settings_path.display()))?;
    let packages = settings
        .as_object_mut()
        .and_then(|root| root.get_mut("packages"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            format!(
                "{} does not contain a Pi packages array",
                settings_path.display()
            )
        })?;
    let package = packages
        .iter_mut()
        .find(|package| match package {
            Value::String(source) => source == PI_MCP_EXTENSION_SOURCE,
            Value::Object(package) => package
                .get("source")
                .and_then(Value::as_str)
                .is_some_and(|source| source == PI_MCP_EXTENSION_SOURCE),
            _ => false,
        })
        .ok_or_else(|| format!("{PI_MCP_EXTENSION_SOURCE} is not installed"))?;
    let mut disabled = match package {
        Value::Object(existing) => existing.clone(),
        _ => Map::new(),
    };
    disabled.insert(
        "source".into(),
        Value::String(PI_MCP_EXTENSION_SOURCE.into()),
    );
    disabled.insert("autoload".into(), Value::Bool(false));
    disabled.remove("extensions");
    *package = Value::Object(disabled);

    let adapter_root = agent_root.join("extensions").join(PROGRESS_ADAPTER_DIR);
    fs::create_dir_all(&adapter_root).map_err(|error| {
        format!(
            "could not create Pi progress adapter directory {}: {error}",
            adapter_root.display()
        )
    })?;
    crate::storage::atomic_replace(
        &adapter_root.join("progress-adapter.mjs"),
        PROGRESS_ADAPTER_SOURCE.as_bytes(),
    )
    .map_err(|error| format!("could not install Pi progress adapter: {error}"))?;
    let upstream_url = file_url(&upstream_entry)?;
    let sdk_url = file_url(&sdk_entry)?;
    let loader = format!(
        "import upstreamExtension from {};\nimport {{ Client }} from {};\nimport {{ wrapPiMcpExtension }} from \"./progress-adapter.mjs\";\n\nexport default wrapPiMcpExtension(upstreamExtension, Client);\n",
        serde_json::to_string(&upstream_url).expect("URL serialization cannot fail"),
        serde_json::to_string(&sdk_url).expect("URL serialization cannot fail"),
    );
    crate::storage::atomic_replace(&adapter_root.join("index.ts"), loader.as_bytes())
        .map_err(|error| format!("could not install Pi progress adapter loader: {error}"))?;
    let settings = serde_json::to_vec_pretty(&settings)
        .map_err(|error| format!("could not serialize Pi settings: {error}"))?;
    crate::storage::atomic_replace(settings_path, &settings)
        .map_err(|error| format!("could not update {}: {error}", settings_path.display()))?;
    Ok(())
}

fn file_url(path: &Path) -> Result<String, String> {
    Url::from_file_path(path)
        .map(|url| url.into())
        .map_err(|()| format!("could not convert path to file URL: {}", path.display()))
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
            if !crate::mcp_mount::pi_mcp_extension_installed(settings_path)? {
                return Err(format!(
                    "Pi reported a successful installation but {} does not contain {}",
                    settings_path.display(),
                    PI_MCP_EXTENSION_SOURCE
                ));
            }
            ensure_pi_mcp_progress_adapter(settings_path)?;
            return pi_mcp_extension_status_at(settings_path);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Pi MCP extension installation timed out".into());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[tauri::command(async)]
pub fn pi_mcp_extension_status() -> Result<PiMcpExtensionStatus, String> {
    let settings = crate::adapters::pi::current_pi_paths()
        .map_err(|error| error.to_string())?
        .settings_path;
    pi_mcp_extension_status_at(&settings)
}

#[tauri::command(async)]
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
