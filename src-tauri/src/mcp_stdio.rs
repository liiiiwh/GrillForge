use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;
use std::io::{self, BufRead, Read, Write};
use std::time::Duration;

const URL_ENV: &str = "GRILLFORGE_MCP_URL";
const TOKEN_ENV: &str = "GRILLFORGE_MCP_TOKEN";
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

pub fn run_from_env() -> Result<(), String> {
    let url = std::env::var(URL_ENV).map_err(|_| format!("{URL_ENV} is required"))?;
    let token = std::env::var(TOKEN_ENV).map_err(|_| format!("{TOKEN_ENV} is required"))?;
    crate::mcp_mount::validate_mcp_url(&url)?;
    crate::mcp_mount::validate_token(&token)?;
    forward(io::stdin().lock(), io::stdout().lock(), &url, &token)
}

fn forward(
    input: impl BufRead,
    mut output: impl Write,
    url: &str,
    token: &str,
) -> Result<(), String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("could not create local MCP client: {error}"))?;
    for line in input.lines() {
        let line = line.map_err(|error| format!("could not read MCP stdin: {error}"))?;
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_MESSAGE_BYTES {
            return Err("MCP request exceeds 4 MiB".into());
        }
        let request: Value = serde_json::from_str(&line)
            .map_err(|error| format!("invalid MCP JSON from Claude Client: {error}"))?;
        let expects_response = request.get("id").is_some();
        let response = client
            .post(url)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .map_err(|error| format!("could not reach GrillForge MCP service: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "GrillForge MCP service returned HTTP {}",
                response.status().as_u16()
            ));
        }
        if !expects_response {
            continue;
        }
        let mut bytes = Vec::new();
        response
            .take((MAX_MESSAGE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("could not read GrillForge MCP response: {error}"))?;
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err("MCP response exceeds 4 MiB".into());
        }
        let response: Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid JSON from GrillForge MCP service: {error}"))?;
        serde_json::to_writer(&mut output, &response)
            .map_err(|error| format!("could not write MCP stdout: {error}"))?;
        output
            .write_all(b"\n")
            .and_then(|()| output.flush())
            .map_err(|error| format!("could not write MCP stdout: {error}"))?;
    }
    Ok(())
}
