//! Exercise the real binary/SDK transport without requiring WSL for normal CI.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

struct Client {
    child: Child,
    lines: Receiver<String>,
    next_id: u64,
}

impl Client {
    fn new(extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_wsltop"))
            .arg("mcp")
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            lines,
            next_id: 1,
        };
        let init = client.request("initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"wsltop-tests","version":"1"}}));
        assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(init["result"]["serverInfo"]["name"], "wsltop");
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }
    fn send(&mut self, value: Value) {
        let input = self.child.stdin.as_mut().unwrap();
        writeln!(input, "{value}").unwrap();
        input.flush().unwrap();
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let line = self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("MCP response timed out");
            let response: Value =
                serde_json::from_str(&line).expect("stdout must contain only JSON-RPC");
            assert_eq!(response["jsonrpc"], "2.0");
            if response.get("id") == Some(&json!(id)) {
                return response;
            }
            assert!(
                response.get("id").is_none(),
                "unexpected response ID: {response}"
            );
        }
    }
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name":name,"arguments":arguments}))
    }
    fn close(&mut self) {
        drop(self.child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "MCP did not exit on EOF");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn stdio_initialization_discovery_errors_and_eof() {
    let mut client = Client::new(&[]);
    let tools = client.request("tools/list", json!({}));
    let tools = tools["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 4);
    for tool in tools {
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    }
    let missing = client.call("get_system_summary", json!({"snapshot_id":"missing"}));
    assert_eq!(missing["result"]["isError"], true);
    assert_eq!(
        missing["result"]["structuredContent"]["error"]["code"],
        "snapshot_unavailable"
    );
    let text: Value =
        serde_json::from_str(missing["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, missing["result"]["structuredContent"]);
    for (name, args) in [
        ("unknown", json!({})),
        ("list_resources", json!({"limit":1001})),
        ("list_resources", json!({"limit":-1})),
        ("list_resources", json!({"environment":"bad"})),
        ("list_resources", json!({"parent":"id"})),
        ("list_resources", json!({"surprise":true})),
        (
            "get_system_summary",
            json!({"snapshot_id":"x","max_age_ms":0}),
        ),
        ("inspect_resource", json!({"resource_id":"id"})),
        ("list_children", json!({"snapshot_id":"x"})),
        ("get_system_summary", json!({"max_age_ms":null})),
    ] {
        let result = client.call(name, args);
        assert_eq!(result["error"]["code"], -32602, "{result}");
    }
    assert!(client.request("ping", json!({})).get("result").is_some());
    client.close();
}

#[test]
fn mcp_startup_rejects_display_flags_without_protocol_output() {
    for args in [
        &["mcp", "--json"][..],
        &["mcp", "--interactive"],
        &["mcp", "--interval-ms", "0"],
        &["mcp", "--interval-ms", "60001"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_wsltop"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
    let help = Command::new(env!("CARGO_BIN_EXE_wsltop"))
        .args(["mcp", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("read-only MCP"));
}

#[cfg(unix)]
#[test]
#[ignore = "requires live WSL2 collection; run explicitly"]
fn native_mcp_observation_round_trip() {
    let mut client = Client::new(&[
        "--wsl-only",
        "--no-docker",
        "--no-wslc",
        "--interval-ms",
        "100",
    ]);
    // Two concurrent latest requests share one collection through the service gate.
    for id in [1001, 1002] {
        client.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
            "params":{"name":"list_resources","arguments":{"limit":1}}}));
    }
    let mut ids = Vec::new();
    for _ in 0..2 {
        let line = client.lines.recv_timeout(Duration::from_secs(15)).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["result"]["isError"], false, "{reply}");
        ids.push(reply["result"]["structuredContent"]["snapshot"]["snapshot_id"].clone());
    }
    assert_eq!(ids[0], ids[1]);
    let first = client.call("list_resources", json!({"limit":1,"max_age_ms":0}));
    assert_eq!(first["result"]["isError"], false, "{first}");
    let data = &first["result"]["structuredContent"];
    let snapshot = data["snapshot"]["snapshot_id"].as_str().unwrap();
    let resource = data["data"][0]["resource_id"].as_str().unwrap();
    assert!(data["data"][0]["usage"]["memory_bytes"].is_u64());
    for name in ["inspect_resource", "list_children"] {
        let response = client.call(name, json!({"snapshot_id":snapshot,"resource_id":resource}));
        assert_eq!(response["result"]["isError"], false, "{response}");
        assert_eq!(
            response["result"]["structuredContent"]["snapshot"],
            data["snapshot"]
        );
    }
    let summary = client.call("get_system_summary", json!({"snapshot_id":snapshot}));
    assert_eq!(
        summary["result"]["structuredContent"]["snapshot"]["cpu_scope"],
        "wsl_visible"
    );
    assert!(
        summary["result"]["structuredContent"]["data"]["environments"]["wsl"]["memory_bytes"]
            .is_u64()
    );
    assert!(summary["result"]["structuredContent"]["data"]["environments"]["docker"].is_null());
    assert_eq!(
        summary["result"]["structuredContent"]["snapshot"],
        data["snapshot"]
    );
    let refreshed = client.call("get_system_summary", json!({"max_age_ms":0}));
    let next = &refreshed["result"]["structuredContent"]["snapshot"]["snapshot_id"];
    assert_ne!(next, snapshot);
    let wrong = client.call(
        "inspect_resource",
        json!({"snapshot_id":next,"resource_id":resource}),
    );
    assert_eq!(
        wrong["result"]["structuredContent"]["error"]["code"],
        "unknown_resource"
    );
    let old = client.call(
        "inspect_resource",
        json!({"snapshot_id":snapshot,"resource_id":resource}),
    );
    assert_eq!(old["result"]["isError"], false);
    client.close();
}
