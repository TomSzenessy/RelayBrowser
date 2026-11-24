//! End-to-end: a real MCP client talks to RelayMcp over an in-memory pipe,
//! and a simulated page answers bridge commands exactly as bridge.js does.

use relay_bridge::{AgentPolicy, Browser, Owner, SharedHub, TabId};
use relay_extensions::store::Store;
use relay_mcp::RelayMcp;
use rmcp::{model::CallToolRequestParams, ServiceExt};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Plays the role of WKWebView + bridge.js: loads post `hello` + a DOM
/// snapshot; evaluated commands post a `result`.
struct SimPage {
    hub: SharedHub,
    log: Mutex<Vec<String>>,
}

impl SimPage {
    fn load(&self, tab: TabId, url: &str) {
        let hub = self.hub.clone();
        let url = url.to_string();
        tokio::spawn(async move {
            hub.ingest_json(tab, &json!({"kind":"hello","url":url,"version":1}).to_string()).unwrap();
            hub.ingest_json(tab, &json!({"kind":"page","url":url,"title":"Checkout"}).to_string()).unwrap();
            hub.ingest_json(tab, &json!({"kind":"dom_snapshot","root":{"id":1,"tag":"body","children":[
                {"id":2,"tag":"h1","text":"Your cart"},
                {"id":3,"tag":"input","attrs":{"type":"email","placeholder":"Email"},"interactive":true},
                {"id":4,"tag":"button","text":"Pay now","attrs":{"id":"pay"},"interactive":true}
            ]}}).to_string()).unwrap();
            hub.ingest_json(tab, r#"{"kind":"console","level":"error","args":["TypeError: cart.total is undefined"],"source":"app.js:10:3"}"#).unwrap();
            hub.ingest_json(tab, r#"{"kind":"network","id":"f1","api":"fetch","method":"POST","url":"https://shop.test/api/checkout","status":500,"duration_ms":41.27,"response_body":"{\"error\":\"card declined\"}"}"#).unwrap();
            hub.ingest_json(tab, r#"{"kind":"network","id":"r1","api":"resource","method":"GET","url":"https://shop.test/app.js","status":200}"#).unwrap();
        });
    }
}

impl Browser for SimPage {
    fn open(&self, tab: TabId, url: &str) { self.load(tab, url) }
    fn navigate(&self, tab: TabId, url: &str) { self.load(tab, url) }
    fn go_back(&self, tab: TabId) { self.load(tab, "https://shop.test/prev") }
    fn go_forward(&self, tab: TabId) { self.load(tab, "https://shop.test/next") }
    fn reload(&self, tab: TabId) { self.load(tab, "https://shop.test/cart") }
    fn close(&self, _: TabId) {}
    fn activate(&self, _: TabId) {}
    fn refresh_cookies(&self, tab: TabId) {
        self.hub.update(|h| h.set_cookies(tab, vec![]));
    }
    fn open_web_inspector(&self, _: TabId) {}
    fn eval(&self, tab: TabId, script: String) {
        let payload = script.trim_start_matches("window.__relay && window.__relay.exec(").trim_end_matches("), void 0");
        let cmd: Value = serde_json::from_str(payload).unwrap();
        self.log.lock().unwrap().push(cmd["op"].as_str().unwrap().to_string());
        let value = match cmd["op"].as_str().unwrap() {
            "click" => json!({"id": cmd["target"], "tag": "button", "text": "Pay now"}),
            "type" => json!({"id": cmd["target"], "tag": "input", "text": cmd["text"]}),
            "read_text" => json!("Your cart\nPay now"),
            "markdown" => json!("# Your cart\n\nPay now"),
            "press_key" => json!({"key": cmd["key"], "tag": "input", "prevented": false}),
            "eval_js" => json!({"result": {"sum": 3, "script_len": cmd["script"].as_str().unwrap_or("").len()}}),
            "set_cookie" => json!({"name": cmd["name"], "set": true}),
            "storage" => json!({
                "local": {"values": {"cart": "1"}, "truncated": false, "redacted": 0},
                "session": {"values": {}, "truncated": false, "redacted": 0},
                "url": "https://shop.test/cart"
            }),
            "page_info" => json!({"url": "https://shop.test/cart", "title": "Checkout", "scroll_x": 0, "scroll_y": 120}),
            _ => Value::Null,
        };
        let ok = cmd["target"].as_u64() != Some(99);
        let reply = json!({"kind":"result","id":cmd["id"],"ok":ok,"value":value,
            "error": if ok { Value::Null } else { "element 99 not found (re-read the DOM)".into() }});
        let hub = self.hub.clone();
        tokio::spawn(async move { hub.ingest_json(tab, &reply.to_string()).unwrap() });
    }
    fn screenshot(&self, _: TabId, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Ok(vec![0xff, 0xd8, 0xff, 0xd9]));
    }
}

/// Same page, but leaves `screenshot` at the trait default so the error path
/// (a backend without macOS WebKit) is exercised.
struct NoShotPage(Arc<SimPage>);

impl Browser for NoShotPage {
    fn open(&self, tab: TabId, url: &str) { self.0.open(tab, url) }
    fn navigate(&self, tab: TabId, url: &str) { self.0.navigate(tab, url) }
    fn go_back(&self, tab: TabId) { self.0.go_back(tab) }
    fn go_forward(&self, tab: TabId) { self.0.go_forward(tab) }
    fn reload(&self, tab: TabId) { self.0.reload(tab) }
    fn close(&self, tab: TabId) { self.0.close(tab) }
    fn activate(&self, tab: TabId) { self.0.activate(tab) }
    fn refresh_cookies(&self, tab: TabId) { self.0.refresh_cookies(tab) }
    fn open_web_inspector(&self, tab: TabId) { self.0.open_web_inspector(tab) }
    fn eval(&self, tab: TabId, script: String) { self.0.eval(tab, script) }
}

fn text(r: &rmcp::model::CallToolResult) -> String {
    r.content.iter().filter_map(|c| c.as_text()).map(|t| t.text.clone()).collect::<Vec<_>>().join("\n")
}

async fn call_message(client: &rmcp::service::RunningService<rmcp::RoleClient, ()>, name: &'static str, args: Value) -> String {
    match client.call_tool(CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone())).await {
        Ok(r) => text(&r),
        Err(e) => e.to_string(),
    }
}

async fn call(client: &rmcp::service::RunningService<rmcp::RoleClient, ()>, name: &'static str, args: Value) -> (bool, String) {
    let r = client
        .call_tool(CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone()))
        .await
        .unwrap();
    (r.is_error.unwrap_or(false), text(&r))
}

#[tokio::test]
async fn agent_debugs_a_failing_checkout_without_screenshots() {
    let hub = SharedHub::new();
    let page = Arc::new(SimPage { hub: hub.clone(), log: Mutex::new(vec![]) });
    let audit = Arc::new(Mutex::new(Store::in_memory().unwrap()));
    audit.lock().unwrap().set_setting("blocking", "on").unwrap();
    audit.lock().unwrap().set_setting("shell_control", "on").unwrap();
    audit.lock().unwrap().set_setting("start_sections", "frequent,recent").unwrap();
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = RelayMcp::new(hub.clone(), page.clone()).with_audit(audit.clone());
    tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await.unwrap() });
    let client = ().serve(client_io).await.unwrap();

    let tools: Vec<String> = client.list_all_tools().await.unwrap().into_iter().map(|t| t.name.to_string()).collect();
    for t in [
        "navigate", "click_element", "type_text", "read_dom", "get_console_logs", "get_network_requests", "list_tabs", "open_tab", "wait_for",
        "get_page_info", "press_key", "get_storage", "batch_actions", "eval_js", "set_cookie", "screenshot", "shell_settings",
        "shell_shortcuts", "shell_start_page", "shell_apps", "shell_version",
    ] {
        assert!(tools.contains(&t.to_string()), "missing tool {t}: {tools:?}");
    }

    // No active tab yet: a clear error, not a crash.
    let (err, msg) = call(&client, "read_dom", json!({})).await;
    assert!(err && msg.contains("no active tab"), "{msg}");

    // Agents may not open non-web schemes.
    let (err, msg) = call(&client, "open_tab", json!({"url": "file:///etc/passwd"})).await;
    assert!(err && msg.contains("only http(s)"), "{msg}");

    // User Stop: every tool fails fast and nothing reaches the page.
    hub.set_agents_paused(true);
    let tabs_before = hub.lock().tabs().count();
    for tool in ["list_tabs", "read_dom", "get_page_info", "click_element", "wait_for"] {
        let args = if tool == "click_element" { json!({"element_id": 4}) } else if tool == "wait_for" { json!({"text": "x"}) } else { json!({}) };
        let msg = call_message(&client, tool, args).await;
        assert!(msg.contains("paused by the user"), "{tool}: {msg}");
    }
    let msg = call_message(&client, "open_tab", json!({"url": "https://shop.test/cart"})).await;
    assert!(msg.contains("paused"), "{msg}");
    assert_eq!(hub.lock().tabs().count(), tabs_before, "a paused open_tab creates nothing");
    hub.set_agents_paused(false);

    let (err, msg) = call(&client, "open_tab", json!({"url": "https://shop.test/cart"})).await;
    assert!(!err, "{msg}");
    assert!(msg.contains("[4] button \"Pay now\" #pay"), "{msg}");
    let tab = hub.lock().tabs().next().unwrap().id;
    assert_eq!(hub.lock().tab(tab).unwrap().owner, Owner::Agent);
    hub.update(|h| h.set_active(tab));

    // Read-only site: reads work, mutations and navigation are refused.
    hub.set_agent_policy("shop.test", AgentPolicy::ReadOnly);
    let (err, msg) = call(&client, "click_element", json!({"element_id": 4})).await;
    assert!(err && msg.contains("read-only"), "{msg}");
    let msg = call_message(&client, "navigate", json!({"url": "https://shop.test/other"})).await;
    assert!(msg.contains("read-only"), "{msg}");
    let (err, _) = call(&client, "read_dom", json!({})).await;
    assert!(!err, "read-only still allows reads");

    // Blocked site: even reads fail and opening a blocked tab creates nothing.
    hub.set_agent_policy("shop.test", AgentPolicy::Block);
    for tool in ["read_dom", "get_page_info", "get_console_logs"] {
        let msg = call_message(&client, tool, json!({})).await;
        assert!(msg.contains("blocked"), "{tool}: {msg}");
    }
    let count = hub.lock().tabs().count();
    let msg = call_message(&client, "open_tab", json!({"url": "https://shop.test/cart"})).await;
    assert!(msg.contains("blocked"), "{msg}");
    assert_eq!(hub.lock().tabs().count(), count);
    hub.set_agent_policy("shop.test", AgentPolicy::Allow);

    let (_, dom) = call(&client, "read_dom", json!({})).await;
    assert!(dom.contains("[2] h1 \"Your cart\"") && dom.contains("[3] input[type=email] \"Email\""), "{dom}");

    let (err, msg) = call(&client, "type_text", json!({"element_id": 3, "text": "a@b.test"})).await;
    assert!(!err && msg.contains("Typed") && msg.contains("a@b.test"), "{msg}");
    let (err, msg) = call(&client, "click_element", json!({"element_id": 4})).await;
    assert!(!err && msg.contains("Clicked button") && msg.contains("Pay now"), "{msg}");
    let (err, msg) = call(&client, "click_element", json!({"element_id": 99})).await;
    assert!(err && msg.contains("re-read the DOM"), "{msg}");

    // Agent wait: resolves from streamed state, no page round-trip.
    let (err, msg) = call(&client, "wait_for", json!({"text": "pay now"})).await;
    assert!(!err && msg.contains("Found after") && msg.contains("[4] button"), "{msg}");
    let (err, msg) = call(&client, "wait_for", json!({"selector": "#pay", "timeout_ms": 500})).await;
    assert!(!err && msg.contains("[4] button"), "{msg}");
    let started = std::time::Instant::now();
    let (err, msg) = call(&client, "wait_for", json!({"text": "coupon applied", "timeout_ms": 120})).await;
    assert!(err && msg.contains("did not appear"), "{msg}");
    assert!(started.elapsed() >= std::time::Duration::from_millis(100), "timeout returned early");

    let (err, info) = call(&client, "get_page_info", json!({})).await;
    assert!(!err, "{info}");
    let info: Value = serde_json::from_str(&info).unwrap();
    assert_eq!(info["title"], "Checkout");
    assert_eq!(info["scroll_y"], 120);

    let (_, logs) = call(&client, "get_console_logs", json!({"min_level": "warn"})).await;
    assert!(logs.contains("[Error] TypeError: cart.total is undefined (app.js:10:3)"), "{logs}");

    let (_, net) = call(&client, "get_network_requests", json!({"failed_only": true, "include_bodies": true})).await;
    let net: Value = serde_json::from_str(&net).unwrap();
    assert_eq!(net.as_array().unwrap().len(), 1);
    assert_eq!(net[0]["status"], 500);
    assert_eq!(net[0]["duration_ms"], 41.3);
    assert!(net[0]["response_body"].as_str().unwrap().contains("card declined"));

    // A CAPTCHA pauses the agent's mutating actions but not reads.
    hub.ingest_json(tab, r#"{"kind":"challenge","challenge":"captcha"}"#).unwrap();
    let (err, msg) = call(&client, "click_element", json!({"element_id": 4})).await;
    assert!(err && msg.contains("waiting for a human"), "{msg}");
    let (err, _) = call(&client, "read_text", json!({})).await;
    assert!(!err);

    // Agents cannot close user tabs.
    let user_tab = hub.update(|h| h.create_tab("https://mail.test", Owner::User));
    let (err, msg) = call(&client, "close_tab", json!({"tab_id": user_tab})).await;
    assert!(err && msg.contains("belongs to the user"), "{msg}");
    let (err, _) = call(&client, "close_tab", json!({"tab_id": tab})).await;
    assert!(!err);

    assert_eq!(*page.log.lock().unwrap(), vec!["type", "click", "click", "page_info", "read_text"]);
    client.cancel().await.unwrap();
}

/// A tab put to sleep by the shell wakes when an agent uses it.
#[tokio::test]
async fn agents_wake_sleeping_tabs() {
    let hub = SharedHub::new();
    let page = Arc::new(SimPage { hub: hub.clone(), log: Mutex::new(vec![]) });
    struct Waker(Arc<SimPage>);
    impl Browser for Waker {
        fn open(&self, t: TabId, u: &str) { self.0.open(t, u) }
        fn navigate(&self, t: TabId, u: &str) { self.0.navigate(t, u) }
        fn go_back(&self, _: TabId) {}
        fn go_forward(&self, _: TabId) {}
        fn reload(&self, _: TabId) {}
        fn close(&self, _: TabId) {}
        fn refresh_cookies(&self, _: TabId) {}
        fn open_web_inspector(&self, _: TabId) {}
        fn eval(&self, t: TabId, s: String) { self.0.eval(t, s) }
        // Like Engine::wake: recreate the view and reload the page.
        fn activate(&self, t: TabId) {
            self.0.hub.update(|h| h.tab_mut(t).unwrap().sleeping = false);
            self.0.load(t, "https://shop.test/cart");
        }
    }
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = RelayMcp::new(hub.clone(), Arc::new(Waker(page.clone())));
    tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await.unwrap() });
    let client = ().serve(client_io).await.unwrap();
    let (err, _) = call(&client, "open_tab", json!({"url": "https://shop.test/cart"})).await;
    assert!(!err);
    let tab = hub.lock().tabs().next().unwrap().id;
    hub.update(|h| {
        let t = h.tab_mut(tab).unwrap();
        t.sleeping = true;
        t.dom = Default::default();
    });
    let (err, msg) = call(&client, "click_element", json!({"tab_id": tab, "element_id": 4})).await;
    assert!(!err, "{msg}");
    let (err, dom) = call(&client, "read_dom", json!({"tab_id": tab})).await;
    assert!(!err && dom.contains("Pay now"), "{dom}");
    client.cancel().await.unwrap();
}

/// A real MCP client plans and applies shell state only after the user grants
/// it; revoking in Settings/Agent sidebar takes effect on the next call. The
/// multi-thread runtime lets the apply timeout fire even if a blocking store
/// lock regresses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_plan_apply_round_trip_with_a_live_client_grant() {
    let hub = SharedHub::new();
    let page = Arc::new(SimPage { hub: hub.clone(), log: Mutex::new(vec![]) });
    let store = Arc::new(Mutex::new(Store::in_memory().unwrap()));
    store.lock().unwrap().set_setting("theme", "light").unwrap();
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = RelayMcp::new(hub.clone(), page.clone()).with_audit(store.clone());
    tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await.unwrap() });
    let client = ().serve(client_io).await.unwrap();

    let tools = client.list_all_tools().await.unwrap();
    for name in ["shell_settings", "shell_shortcuts", "shell_start_page", "shell_sidebar", "shell_apps", "shell_version"] {
        let tool = tools.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("missing {name}"));
        let description = tool.description.as_deref().unwrap_or_default();
        assert!(description.contains("Read-only"), "{name}: {description}");
    }
    for forbidden in ["shell_eval", "shell_exec", "shell_script", "shell_html"] {
        assert!(!tools.iter().any(|t| t.name == forbidden), "{forbidden} must not exist");
    }

    // Off by default: no global switch and no grant.
    let msg = call_message(&client, "shell_plan_change", json!({"theme": "dark"})).await;
    assert!(msg.contains("shell writes are off"), "{msg}");

    let client_name = store.lock().unwrap().mcp_clients().unwrap().first().expect("initialize records the client").name.clone();
    assert!(!client_name.is_empty(), "the client advertises a name");
    store.lock().unwrap().set_mcp_client_granted(&client_name, true, 1).unwrap();
    store.lock().unwrap().set_setting("search", &format!("https://search.test/?q=%s&pad={}", "x".repeat(2000))).unwrap();

    // Every shell read tool answers through the real client with bounded data.
    let (err, settings) = call(&client, "shell_settings", json!({})).await;
    assert!(!err, "{settings}");
    let settings: Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(settings["search"].as_str().unwrap().chars().count(), 1024, "settings values are capped");
    let (err, shortcuts) = call(&client, "shell_shortcuts", json!({})).await;
    assert!(!err, "{shortcuts}");
    let shortcuts: Value = serde_json::from_str(&shortcuts).unwrap();
    assert_eq!(shortcuts["defaults"]["palette"], "secondary-k");
    let (err, start) = call(&client, "shell_start_page", json!({})).await;
    assert!(!err, "{start}");
    assert!(serde_json::from_str::<Value>(&start).unwrap()["sections"].is_string());
    let (err, apps) = call(&client, "shell_apps", json!({})).await;
    assert!(!err, "{apps}");
    assert!(serde_json::from_str::<Value>(&apps).unwrap()["apps"].is_array());
    let (err, version) = call(&client, "shell_version", json!({})).await;
    assert!(!err, "{version}");
    assert_eq!(serde_json::from_str::<Value>(&version).unwrap()["name"], "relay-mcp");
    let (err, sidebar) = call(&client, "shell_sidebar", json!({})).await;
    assert!(!err, "{sidebar}");
    assert_eq!(serde_json::from_str::<Value>(&sidebar).unwrap()["open"], false);

    let (err, planned) = call(&client, "shell_plan_change", json!({
        "theme": "dark",
        "accent": "purple",
        "start_sections": "apps,recent",
        "settings": {"blocking": "off", "keybindings": "find=ctrl-alt-f"},
        "sidebar": "agents",
        "widget": {"title": "Build note", "body": "All checks passed"}
    })).await;
    assert!(!err, "{planned}");
    let planned: Value = serde_json::from_str(&planned).unwrap();
    let id = planned["plan_id"].as_u64().unwrap();
    assert!(planned["diff"].as_str().unwrap().contains("theme: light -> dark"));
    assert_eq!(store.lock().unwrap().setting("theme").unwrap().as_deref(), Some("light"), "planning writes nothing");

    // A deadlocked apply would hang here; the timeout turns that into a failure.
    let (err, applied) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        call(&client, "shell_apply_change", json!({"plan_id": id})),
    )
    .await
    .expect("shell_apply_change timed out; the profile-store lock is probably still held");
    assert!(!err, "{applied}");
    let s = store.lock().unwrap();
    assert_eq!(s.setting("theme").unwrap().as_deref(), Some("dark"));
    assert_eq!(s.setting("accent").unwrap().as_deref(), Some("purple"));
    assert_eq!(s.setting("start_sections").unwrap().as_deref(), Some("apps,recent"));
    assert_eq!(s.setting("blocking").unwrap().as_deref(), Some("off"));
    assert_eq!(s.setting("keybindings").unwrap().as_deref(), Some("find=ctrl-alt-f"));
    let rows = s.agent_audit(20).unwrap();
    drop(s);
    let audit = rows.iter().find(|r| r.tool == "shell_apply" && r.outcome.starts_with("ok ")).expect("apply row");
    assert_eq!(rows.iter().filter(|r| r.tool == "shell_apply" && r.outcome.starts_with("ok ")).count(), 1, "exactly one apply row");
    assert!(audit.outcome.contains(&format!("client={client_name}")), "{}", audit.outcome);
    assert!(audit.outcome.contains(&format!("plan={id}")), "{}", audit.outcome);
    for key in ["theme=", "accent=", "start_sections=", "blocking=", "keybindings=", "sidebar=", "widget="] {
        assert!(audit.outcome.contains(key), "missing {key}: {}", audit.outcome);
    }
    assert!(!audit.outcome.contains("dark") && !audit.outcome.contains("All checks passed"), "{}", audit.outcome);
    assert!(hub.sidebar().open && hub.sidebar().tab == "agents");

    let (err, sidebar) = call(&client, "shell_sidebar", json!({})).await;
    assert!(!err, "{sidebar}");
    let sidebar: Value = serde_json::from_str(&sidebar).unwrap();
    assert_eq!(sidebar["open"], true);
    assert_eq!(sidebar["tab"], "agents");

    // The id is single-use; a retry is unknown and writes no second row.
    let msg = call_message(&client, "shell_apply_change", json!({"plan_id": id})).await;
    assert!(msg.contains("unknown or already applied"), "{msg}");
    let rows = store.lock().unwrap().agent_audit(20).unwrap();
    assert_eq!(rows.iter().filter(|r| r.tool == "shell_apply" && r.outcome.starts_with("ok ")).count(), 1);

    // Unknown ids are stable errors and write no apply result.
    let msg = call_message(&client, "shell_apply_change", json!({"plan_id": 9999})).await;
    assert!(msg.contains("unknown or already applied"), "{msg}");

    // No free-form fields: HTML/JS/URL knobs fail deserialization.
    let msg = call_message(&client, "shell_plan_change", json!({"theme": "dark", "script": "alert(1)"})).await;
    assert!(msg.contains("unknown field") || msg.contains("invalid"), "{msg}");
    let msg = call_message(&client, "shell_plan_change", json!({"theme": "dark", "start_url": "https://evil.test"})).await;
    assert!(msg.contains("unknown field") || msg.contains("invalid"), "{msg}");

    // Revoke is read on the next call, with no restart.
    store.lock().unwrap().set_mcp_client_granted(&client_name, false, 2).unwrap();
    let msg = call_message(&client, "shell_plan_change", json!({"theme": "light"})).await;
    assert!(msg.contains("shell writes are off"), "{msg}");
    let rows = store.lock().unwrap().agent_audit(20).unwrap();
    assert!(rows.iter().any(|r| r.tool == "shell_plan" && r.outcome == "denied"), "{rows:?}");
    client.cancel().await.unwrap();
}

/// The full grant matrix: global switch and per-client grant are independent,
/// and a revoke is read on the next call.
#[tokio::test]
async fn shell_grant_matrix_global_switch_and_client_grant() {
    let hub = SharedHub::new();
    let page = Arc::new(SimPage { hub: hub.clone(), log: Mutex::new(vec![]) });
    let store = Arc::new(Mutex::new(Store::in_memory().unwrap()));
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = RelayMcp::new(hub.clone(), page.clone()).with_audit(store.clone());
    tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await.unwrap() });
    let client = ().serve(client_io).await.unwrap();
    let name = {
        client.list_all_tools().await.unwrap();
        store.lock().unwrap().mcp_clients().unwrap().first().expect("client recorded on initialize").name.clone()
    };

    // Global off, no grant: denied.
    let msg = call_message(&client, "shell_plan_change", json!({"theme": "dark"})).await;
    assert!(msg.contains("shell writes are off"), "{msg}");

    // Global off, granted: allowed.
    store.lock().unwrap().set_mcp_client_granted(&name, true, 1).unwrap();
    let (err, planned) = call(&client, "shell_plan_change", json!({"theme": "dark"})).await;
    assert!(!err, "{planned}");
    assert!(serde_json::from_str::<Value>(&planned).unwrap()["plan_id"].is_u64());

    // Global on, ungranted: allowed by the global switch.
    store.lock().unwrap().set_setting("shell_control", "on").unwrap();
    store.lock().unwrap().set_mcp_client_granted(&name, false, 2).unwrap();
    let (err, planned) = call(&client, "shell_plan_change", json!({"theme": "light"})).await;
    assert!(!err, "{planned}");

    // Global off, revoked: denied again.
    store.lock().unwrap().set_setting("shell_control", "off").unwrap();
    let msg = call_message(&client, "shell_plan_change", json!({"theme": "light"})).await;
    assert!(msg.contains("shell writes are off"), "{msg}");
    client.cancel().await.unwrap();
}

/// Plan ids expire end to end; paused tokio time makes the 60 s TTL instant.
#[tokio::test(start_paused = true)]
async fn expired_plan_is_refused_end_to_end() {
    let hub = SharedHub::new();
    let page = Arc::new(SimPage { hub: hub.clone(), log: Mutex::new(vec![]) });
    let store = Arc::new(Mutex::new(Store::in_memory().unwrap()));
    store.lock().unwrap().set_setting("shell_control", "on").unwrap();
    store.lock().unwrap().set_setting("theme", "light").unwrap();
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = RelayMcp::new(hub.clone(), page.clone()).with_audit(store.clone());
    tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await.unwrap() });
    let client = ().serve(client_io).await.unwrap();

    let (_, planned) = call(&client, "shell_plan_change", json!({"theme": "dark"})).await;
    let id = serde_json::from_str::<Value>(&planned).unwrap()["plan_id"].as_u64().unwrap();
    tokio::time::advance(std::time::Duration::from_secs(61)).await;
    let msg = call_message(&client, "shell_apply_change", json!({"plan_id": id})).await;
    assert!(msg.contains("expired"), "{msg}");
    assert_eq!(store.lock().unwrap().setting("theme").unwrap().as_deref(), Some("light"));
    let rows = store.lock().unwrap().agent_audit(10).unwrap();
    assert!(!rows.iter().any(|r| r.tool == "shell_apply"), "an expired apply writes no audit row: {rows:?}");
    client.cancel().await.unwrap();
}

/// Shell tools fail cleanly when the server was built without the profile
/// store (the read/plan path a test or CLI-only server takes).
#[tokio::test]
async fn shell_tools_without_a_store_fail_cleanly() {
    let hub = SharedHub::new();
    let page: Arc<dyn Browser> = Arc::new(SimPage { hub: hub.clone(), log: Mutex::new(vec![]) });
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = RelayMcp::new(hub, page);
    tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await.unwrap() });
    let client = ().serve(client_io).await.unwrap();
    for tool in ["shell_settings", "shell_shortcuts", "shell_start_page", "shell_apps"] {
        let msg = call_message(&client, tool, json!({})).await;
        assert!(msg.contains("shell store is attached only while the browser is running"), "{tool}: {msg}");
    }
    let msg = call_message(&client, "shell_plan_change", json!({"theme": "dark"})).await;
    assert!(msg.contains("shell store is attached only while the browser is running"), "plan: {msg}");
    client.cancel().await.unwrap();
}

/// A fake Browser returns JPEG bytes; the same tool explains the no-WebKit
/// default instead of returning an image.
#[tokio::test]
async fn screenshot_returns_image_content_and_default_error_is_clear() {
    let hub = SharedHub::new();
    hub.update(|h| {
        let tab = h.create_tab("https://shot.test/", Owner::User);
        h.set_active(tab);
    });
    let page = Arc::new(SimPage { hub: hub.clone(), log: Mutex::new(vec![]) });
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = RelayMcp::new(hub.clone(), page.clone());
    tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await.unwrap() });
    let client = ().serve(client_io).await.unwrap();
    let r = client.call_tool(CallToolRequestParams::new("screenshot")).await.unwrap();
    assert!(!r.is_error.unwrap_or(false), "{:?}", r.content);
    let image = r.content.iter().find_map(|c| c.as_image()).expect("image content");
    assert_eq!(image.mime_type, "image/jpeg");
    assert_eq!(image.data, "/9j/2Q==");
    client.cancel().await.unwrap();

    let no_shot: Arc<dyn Browser> = Arc::new(NoShotPage(page.clone()));
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = RelayMcp::new(hub, no_shot);
    tokio::spawn(async move { server.serve(server_io).await.unwrap().waiting().await.unwrap() });
    let client = ().serve(client_io).await.unwrap();
    let msg = call_message(&client, "screenshot", json!({})).await;
    assert!(msg.contains("screenshots need macOS WebKit"), "{msg}");
    client.cancel().await.unwrap();
}

/// The socket is user-only, and a second instance must not take it over.
#[cfg(unix)]
#[tokio::test]
async fn socket_is_private_and_not_stolen() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay").join("mcp.sock");
    let hub = SharedHub::new();
    let page: Arc<dyn Browser> = Arc::new(SimPage { hub: hub.clone(), log: Mutex::new(vec![]) });
    let p = path.clone();
    let (h2, b2) = (hub.clone(), page.clone());
    tokio::spawn(async move { relay_mcp::serve_socket(&p, h2, b2, None).await });
    for _ in 0..100 {
        if path.exists() { break; }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    let err = relay_mcp::serve_socket(&path, hub, page, None).await.unwrap_err();
    assert!(err.to_string().contains("another Relay instance"), "{err}");
}

#[tokio::test]
async fn read_markdown_and_nav_tools_obey_policy_and_stop() {
    let hub = SharedHub::new();
    let page = Arc::new(SimPage { hub: hub.clone(), log: Mutex::new(vec![]) });
    let audit = Arc::new(Mutex::new(Store::in_memory().unwrap()));
    let server = RelayMcp::new(hub.clone(), page.clone()).with_audit(audit.clone());
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        let running = server.serve(server_io).await.unwrap();
        let _ = running.waiting().await;
    });
    let client = ().serve(client_io).await.unwrap();

    let (err, _) = call(&client, "open_tab", json!({"url": "https://shop.test/cart"})).await;
    assert!(!err);
    let tab = hub.lock().tab_ids()[0];

    let (err, md) = call(&client, "read_markdown", json!({"tab_id": tab})).await;
    assert!(!err, "{md}");
    assert!(md.contains("# Your cart"), "{md}");

    let (err, sc) = call(&client, "scroll_into_view", json!({"tab_id": tab, "element_id": 4})).await;
    assert!(!err, "{sc}");

    let (err, fw) = call(&client, "go_forward", json!({"tab_id": tab})).await;
    assert!(!err, "{fw}");
    assert!(fw.contains("https://shop.test/next"), "{fw}");

    let (err, rl) = call(&client, "reload", json!({"tab_id": tab})).await;
    assert!(!err, "{rl}");
    assert!(rl.contains("https://shop.test/cart"), "{rl}");

    // ReadOnly allows read_markdown, refuses mutating go_forward/reload.
    hub.set_agent_policy("shop.test", AgentPolicy::ReadOnly);
    let (err, md_ro) = call(&client, "read_markdown", json!({"tab_id": tab})).await;
    assert!(!err, "{md_ro}");
    let (err, fw_ro) = call(&client, "go_forward", json!({"tab_id": tab})).await;
    assert!(err, "go_forward must fail under ReadOnly: {fw_ro}");
    let (err, rl_ro) = call(&client, "reload", json!({"tab_id": tab})).await;
    assert!(err, "reload must fail under ReadOnly: {rl_ro}");

    // Block refuses read_markdown too.
    hub.set_agent_policy("shop.test", AgentPolicy::Block);
    let (err, md_blk) = call(&client, "read_markdown", json!({"tab_id": tab})).await;
    assert!(err, "read_markdown must fail under Block: {md_blk}");

    // Stop refuses everything fast.
    hub.set_agent_policy("shop.test", AgentPolicy::Allow);
    hub.set_agents_paused(true);
    let msg = call_message(&client, "read_markdown", json!({"tab_id": tab})).await;
    assert!(msg.contains("paused"), "{msg}");

    let rows = audit.lock().unwrap().agent_audit(50).unwrap();
    assert!(rows.iter().any(|r| r.tool == "read_markdown" && r.outcome == "ok"));
    assert!(rows.iter().any(|r| r.tool == "go_forward" && r.outcome == "ok"));
    assert!(rows.iter().any(|r| r.tool == "reload" && r.outcome == "ok"));
    assert!(rows.iter().any(|r| r.tool == "scroll_into_view" && r.outcome == "ok"));
    assert!(rows.iter().any(|r| r.tool == "go_forward" && r.outcome == "error"), "refused go_forward must audit as error");
    assert!(rows.iter().any(|r| r.tool == "reload" && r.outcome == "error"), "refused reload must audit as error");

    let _ = client.cancel().await;
    server_task.abort();
}

