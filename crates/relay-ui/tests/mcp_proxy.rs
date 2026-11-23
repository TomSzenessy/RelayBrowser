//! The exact workflow an agent uses: `relay mcp` on stdio, proxied over the
//! user-only socket to the running browser's shared hub. This test spawns the
//! real binary and speaks MCP to it, so the transport (not just the tool
//! router) is covered.

use relay_bridge::{Browser, Owner, SharedHub, TabId};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct NullBrowser;

impl Browser for NullBrowser {
    fn open(&self, _: TabId, _: &str) {}
    fn navigate(&self, _: TabId, _: &str) {}
    fn go_back(&self, _: TabId) {}
    fn go_forward(&self, _: TabId) {}
    fn reload(&self, _: TabId) {}
    fn close(&self, _: TabId) {}
    fn activate(&self, _: TabId) {}
    fn eval(&self, _: TabId, _: String) {}
    fn refresh_cookies(&self, _: TabId) {}
    fn open_web_inspector(&self, _: TabId) {}
}

fn send(child: &mut std::process::Child, msg: serde_json::Value) {
    let stdin = child.stdin.as_mut().expect("stdin");
    writeln!(stdin, "{msg}").expect("write MCP message");
    stdin.flush().expect("flush MCP message");
}

fn recv(reader: &mut BufReader<std::process::ChildStdout>, id: u64) -> serde_json::Value {
    let mut line = String::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "no response for id {id}");
        line.clear();
        let n = reader.read_line(&mut line).expect("read MCP response");
        assert!(n > 0, "relay mcp exited before answering id {id}");
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
            return v;
        }
    }
}

#[test]
fn relay_mcp_stdio_proxy_reaches_a_running_browser() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("mcp.sock");
    let hub = SharedHub::new();
    hub.update(|h| {
        let tab = h.create_tab("https://shop.test/cart", Owner::User);
        h.set_active(tab);
        h.tab_mut(tab).unwrap().title = "Checkout".into();
    });

    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let server_socket = socket.clone();
    let server_hub = hub.clone();
    runtime.spawn(async move {
        let _ = relay_mcp::serve_socket(&server_socket, server_hub, Arc::new(NullBrowser), None).await;
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "the MCP socket never appeared");
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_relay"))
        .arg("mcp")
        .env("RELAY_HOME", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn relay mcp");
    let mut reader = BufReader::new(child.stdout.take().expect("stdout"));

    send(
        &mut child,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "relay-test", "version": "0"}
            }
        }),
    );
    let init = recv(&mut reader, 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "relay", "{init}");
    assert!(init["result"]["instructions"].as_str().unwrap_or_default().contains("untrusted"), "{init}");

    send(&mut child, serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    send(&mut child, serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let tools = recv(&mut reader, 2);
    let names: Vec<&str> = tools["result"]["tools"].as_array().expect("tools").iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in ["list_tabs", "read_dom", "wait_for", "get_console_logs", "get_network_requests"] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }

    send(
        &mut child,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "list_tabs", "arguments": {}}
        }),
    );
    let tabs = recv(&mut reader, 3);
    let text = tabs["result"]["content"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("Checkout") && text.contains("shop.test"), "{tabs}");

    let _ = child.kill();
    let _ = child.wait();
    drop(runtime);
}
