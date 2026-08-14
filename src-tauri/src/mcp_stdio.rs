use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;
use std::io::{self, BufRead, Read, Write};
use std::time::Duration;

const URL_ENV: &str = "GRILLFORGE_MCP_URL";
const TOKEN_ENV: &str = "GRILLFORGE_MCP_TOKEN";
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const FORWARD_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60 + 60);

pub fn run_from_env() -> Result<(), String> {
    let url = std::env::var(URL_ENV).map_err(|_| format!("{URL_ENV} is required"))?;
    let token = std::env::var(TOKEN_ENV).map_err(|_| format!("{TOKEN_ENV} is required"))?;
    crate::mcp_mount::validate_mcp_url(&url)?;
    crate::mcp_mount::validate_token(&token)?;
    forward(io::stdin().lock(), io::stdout().lock(), &url, &token)
}

fn forward(input: impl BufRead, output: impl Write, url: &str, token: &str) -> Result<(), String> {
    forward_with_timeout(input, output, url, token, FORWARD_TIMEOUT)
}

fn forward_with_timeout(
    input: impl BufRead,
    mut output: impl Write,
    url: &str,
    token: &str,
    timeout: Duration,
) -> Result<(), String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(timeout)
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
            .header(ACCEPT, "application/json, text/event-stream")
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
        if response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"))
        {
            relay_event_stream(response, &mut output)?;
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
        write_message(&mut output, &response)?;
    }
    Ok(())
}

fn relay_event_stream(response: impl Read, output: &mut impl Write) -> Result<(), String> {
    let mut reader = io::BufReader::new(response);
    let mut data = String::new();
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| format!("could not read GrillForge MCP event stream: {error}"))?;
        if read == 0 {
            if !data.is_empty() {
                write_event_data(output, &data)?;
            }
            return Ok(());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if !data.is_empty() {
                write_event_data(output, &data)?;
                data.clear();
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
            if data.len() > MAX_MESSAGE_BYTES {
                return Err("MCP event exceeds 4 MiB".into());
            }
        }
    }
}

fn write_event_data(output: &mut impl Write, data: &str) -> Result<(), String> {
    let message: Value = serde_json::from_str(data)
        .map_err(|error| format!("invalid JSON from GrillForge MCP event stream: {error}"))?;
    write_message(output, &message)
}

fn write_message(output: &mut impl Write, message: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *output, message)
        .map_err(|error| format!("could not write MCP stdout: {error}"))?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|error| format!("could not write MCP stdout: {error}"))
}

#[cfg(test)]
mod tests {
    use super::forward_with_timeout;
    use std::io::{BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    fn delayed_server(delay: Duration) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("connection");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).expect("request");
            thread::sleep(delay);
            let body = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("response");
        });
        format!("http://{address}/mcp")
    }

    #[test]
    fn stdio_forward_timeout_is_an_explicit_boundary_not_a_fixed_minute() {
        let request = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
        let short_url = delayed_server(Duration::from_millis(80));
        let short = forward_with_timeout(
            BufReader::new(request.as_slice()),
            Vec::new(),
            &short_url,
            "token",
            Duration::from_millis(20),
        );
        assert!(
            short
                .unwrap_err()
                .contains("could not reach GrillForge MCP service")
        );

        let long_url = delayed_server(Duration::from_millis(40));
        let mut output = Vec::new();
        forward_with_timeout(
            BufReader::new(request.as_slice()),
            &mut output,
            &long_url,
            "token",
            Duration::from_millis(500),
        )
        .expect("long request stays open");
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "{\"id\":1,\"jsonrpc\":\"2.0\",\"result\":{}}\n"
        );
    }
}
