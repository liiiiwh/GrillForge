use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::thread;

#[test]
fn claude_client_can_start_the_bundled_stdio_bridge_and_initialize_mcp() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut content_length = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header");
            if line == "\r\n" {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("content-length:") {
                content_length = Some(value.trim().parse::<usize>().expect("length"));
            }
            if lower.starts_with("authorization:") {
                assert_eq!(line.trim(), "authorization: Bearer local-token");
            }
        }
        let mut body = vec![0; content_length.expect("content length")];
        reader.read_exact(&mut body).expect("body");
        let request: Value = serde_json::from_slice(&body).expect("request JSON");
        assert_eq!(request["method"], "initialize");

        let response = br#"{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"GrillForge"}}}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.len()
        )
        .expect("response headers");
        stream.write_all(response).expect("response body");
    });

    let mut child = Command::new(env!("CARGO_BIN_EXE_grillforge"))
        .arg("mcp-stdio")
        .env(
            "GRILLFORGE_MCP_URL",
            format!("http://{address}/mcp/claude_desktop"),
        )
        .env("GRILLFORGE_MCP_TOKEN", "local-token")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("stdio bridge");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin
        .write_all(br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
        .and_then(|()| stdin.write_all(b"\n"))
        .expect("request");
    // Closing stdin is the normal Claude Client shutdown signal for a stdio server.
    drop(stdin);
    let output = child.wait_with_output().expect("bridge output");
    server.join().expect("server");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).expect("stdout JSON");
    assert_eq!(response["result"]["serverInfo"]["name"], "GrillForge");
}

#[test]
fn stdio_bridge_relays_progress_notifications_before_the_final_result() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut content_length = None;
        let mut accepts_event_stream = false;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header");
            if line == "\r\n" {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("content-length:") {
                content_length = Some(value.trim().parse::<usize>().expect("length"));
            }
            if lower.starts_with("accept:") && lower.contains("text/event-stream") {
                accepts_event_stream = true;
            }
        }
        assert!(accepts_event_stream);
        let mut body = vec![0; content_length.expect("content length")];
        reader.read_exact(&mut body).expect("body");

        let first = r#"data: {"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"p","progress":1,"message":"running"}}

"#;
        let final_result = r#"data: {"jsonrpc":"2.0","id":9,"result":{"content":[{"type":"text","text":"done"}],"isError":false}}

"#;
        let length = first.len() + final_result.len();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
        )
        .expect("response headers");
        stream.write_all(first.as_bytes()).expect("progress");
        stream.flush().expect("flush progress");
        thread::sleep(std::time::Duration::from_millis(30));
        stream
            .write_all(final_result.as_bytes())
            .expect("final result");
    });

    let mut child = Command::new(env!("CARGO_BIN_EXE_grillforge"))
        .arg("mcp-stdio")
        .env(
            "GRILLFORGE_MCP_URL",
            format!("http://{address}/mcp/claude_desktop"),
        )
        .env("GRILLFORGE_MCP_TOKEN", "local-token")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("stdio bridge");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin
        .write_all(br#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"run_agent","arguments":{},"_meta":{"progressToken":"p"}}}"#)
        .and_then(|()| stdin.write_all(b"\n"))
        .expect("request");
    drop(stdin);
    let output = child.wait_with_output().expect("bridge output");
    server.join().expect("server");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let messages = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["method"], "notifications/progress");
    assert_eq!(messages[1]["result"]["content"][0]["text"], "done");
}
