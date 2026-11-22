//! Real-WebKit smoke test for the macOS engine and the injected bridge.
//!
//! Runs on Apple Silicon CI (`cargo run -p relay-webkit --example smoke`). It
//! serves a page from a loopback HTTP server, loads it in a real `WKWebView`
//! through [`relay_webkit::mac::Engine`], then asserts on what arrives in the
//! shared hub: the bridge hello, DOM, console, a failed `fetch`, and a click
//! that changes the page. This is off-Mac evidence that the macOS-only engine
//! works; the GPUI window is the only part still left to a human.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("smoke: macOS only (WKWebView is a system framework)");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    mac::run();
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{NSDate, NSRunLoop};
    use relay_bridge::{DomNode, Op, Owner, SharedHub, TabId};
    use relay_webkit::mac::Engine;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};
    use tokio::sync::oneshot::error::TryRecvError;

    const PAGE: &str = r#"<!doctype html>
<html><head><title>Relay smoke</title></head><body>
  <h1>Relay smoke</h1>
  <button id="pay" onclick="document.getElementById('out').textContent='paid'">Pay now</button>
  <div id="out">unpaid</div>
  <script>
    console.log("boot", {n: 1});
    fetch("/api/checkout", {method: "POST", body: JSON.stringify({card: "4242"})})
      .catch(function () {});
  </script>
</body></html>"#;

    fn serve() -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let handle = std::thread::spawn(move || {
            for _ in 0..16 {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let (status, content_type, body) = if path.starts_with("/api/checkout") {
                    ("500 Internal Server Error", "application/json", r#"{"error":"card declined"}"#)
                } else if path.starts_with("/favicon.ico") {
                    ("204 No Content", "text/plain", "")
                } else {
                    ("200 OK", "text/html; charset=utf-8", PAGE)
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://{addr}/"), handle)
    }

    fn pump(seconds: f64) {
        let date = NSDate::dateWithTimeIntervalSinceNow(seconds);
        NSRunLoop::currentRunLoop().runUntilDate(&date);
    }

    /// Spin the AppKit run loop until `f` is true; returns whether it became true.
    fn until(mut f: impl FnMut() -> bool, seconds: f64) -> bool {
        let deadline = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < deadline {
            if f() {
                return true;
            }
            pump(0.02);
        }
        f()
    }

    fn find<'a>(node: &'a DomNode, pred: &impl Fn(&DomNode) -> bool) -> Option<&'a DomNode> {
        if pred(node) {
            return Some(node);
        }
        node.children.iter().find_map(|c| find(c, pred))
    }

    /// Register one bridge op, evaluate it, and wait for the page's reply.
    fn op(engine: &Engine, hub: &SharedHub, tab: TabId, op: Op) -> serde_json::Value {
        let (script, mut rx) = hub.begin_command(tab, op).expect("register command");
        engine.eval(tab, &script);
        let mut reply = None;
        let delivered = until(
            || match rx.try_recv() {
                Ok(r) => {
                    reply = Some(r);
                    true
                }
                Err(TryRecvError::Empty) => false,
                Err(TryRecvError::Closed) => true,
            },
            5.0,
        );
        assert!(delivered, "the page never answered the command");
        let r = reply.expect("reply appeared without a result");
        assert!(r.ok, "bridge refused the command: {:?}", r.error);
        r.value
    }

    pub fn run() {
        let mtm = MainThreadMarker::new().expect("smoke must run on the main thread");
        // AppKit must be initialized before WebKit; no window is needed.
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();

        let (url, server) = serve();
        let hub = SharedHub::new();
        let tab = hub.update(|h| h.create_tab(&url, Owner::User));
        let mut engine = Engine::new(mtm, hub.clone(), |e| eprintln!("smoke: engine event {e:?}"));
        engine.create(tab, &url);

        assert!(
            until(|| hub.lock().tab(tab).is_some_and(|t| t.dom.root().is_some()), 20.0),
            "bridge never delivered a DOM"
        );
        assert!(until(|| hub.lock().tab(tab).is_some_and(|t| t.load_seq > 0), 5.0), "no hello");

        let (title, nodes) = {
            let h = hub.lock();
            let t = h.tab(tab).unwrap();
            (t.title.clone(), t.dom.root().and_then(|r| t.dom.tree(r, 100)))
        };
        assert_eq!(title, "Relay smoke");
        let root = nodes.expect("DOM tree");
        let pay = find(&root, &|n| n.attrs.get("id").is_some_and(|v| v == "pay")).expect("pay button").id;

        assert!(
            until(
                || hub.lock().tab(tab).is_some_and(|t| t.console.iter().any(|c| c.args.first().is_some_and(|a| a == "boot"))),
                5.0
            ),
            "console.log was not captured"
        );
        let failure = until(
            || {
                hub.lock().tab(tab).is_some_and(|t| {
                    t.network.iter().any(|n| {
                        n.api == "fetch" && n.status == Some(500) && n.response_body.as_deref() == Some(r#"{"error":"card declined"}"#)
                    })
                })
            },
            10.0,
        );
        assert!(failure, "fetch status/body were not captured");

        op(&engine, &hub, tab, Op::Click { target: pay });
        let paid = until(
            || {
                hub.lock().tab(tab).is_some_and(|t| {
                    t.dom
                        .root()
                        .and_then(|r| t.dom.tree(r, 100))
                        .is_some_and(|n| find(&n, &|x| x.text == "paid").is_some())
                })
            },
            5.0,
        );
        assert!(paid, "click did not run the page's own handler");

        let info = op(&engine, &hub, tab, Op::PageInfo);
        assert_eq!(info["title"], "Relay smoke");

        // A refused connection is surfaced as a readable load error (the shell
        // hides WebKit's blank error page and shows Retry).
        let dead = "http://127.0.0.1:1/";
        let failed_tab = hub.update(|h| h.create_tab(dead, Owner::User));
        engine.create(failed_tab, dead);
        assert!(
            until(|| hub.lock().tab(failed_tab).is_some_and(|t| t.load_error.is_some()), 15.0),
            "a refused navigation did not set load_error"
        );
        println!("smoke: load error = {:?}", hub.lock().tab(failed_tab).unwrap().load_error);

        // Sleep destroys the WebView after capturing a snapshot; wake recreates
        // it and the bridge delivers a fresh DOM. This is the RAM-killer path.
        engine.sleep(tab, (0.0, 0.0));
        assert!(hub.lock().tab(tab).unwrap().sleeping, "sleep did not mark the tab");
        assert!(
            until(|| engine.slept(tab).and_then(|s| s.snapshot_jpeg).is_some(), 10.0),
            "sleep did not capture a JPEG snapshot"
        );
        engine.wake(tab);
        assert!(!hub.lock().tab(tab).unwrap().sleeping, "wake left the tab asleep");
        assert!(
            until(|| hub.lock().tab(tab).is_some_and(|t| t.dom.root().is_some()), 20.0),
            "the woken tab did not reload through the bridge"
        );
        engine.after_load(tab);
        println!("smoke: sleep/wake returned a live DOM");
        server.join().ok();
        println!("SMOKE OK: hello, DOM, console, fetch(500), click, and page info all verified in WKWebView");
        std::process::exit(0);
    }
}
