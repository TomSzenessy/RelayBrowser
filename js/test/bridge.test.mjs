// Runs js/bridge.js in Playwright's WebKit (the closest engine to WKWebView
// available off-Mac), injected at document start exactly as WKUserScript does,
// and checks every message against the Rust protocol (examples/validate.rs).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { webkit } from "playwright";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..");
const BRIDGE = readFileSync(join(here, "..", "bridge.js"), "utf8");
const ORIGIN = "https://shop.relay.test";

const PAGE = `<!doctype html><html><head><title>Checkout</title></head><body>
  <h1>Your cart</h1>
  <form id="f" onsubmit="event.preventDefault(); window.submitted = true">
    <input id="email" type="email" placeholder="Email">
    <input id="pw" type="password" value="hunter2">
    <input id="card" autocomplete="cc-number" value="4111111111111111">
    <input id="otp" autocomplete="one-time-code" value="123456">
    <input id="secret" data-relay-private value="private-value">
    <input id="q" type="search" value="hello">
    <button id="pay" type="button" onclick="window.paid = (window.paid||0)+1">Pay now</button>
  </form>
  <div id="list"></div>
  <script>
    document.getElementById("email").addEventListener("input", e => window.lastInput = e.target.value);
    console.log("boot", {n: 1});
    console.warn("careful");
  </script>
</body></html>`;

let browser, page, all = [];

async function drain() {
  const msgs = await page.evaluate(() => window.__relay.drain());
  all.push(...msgs);
  return msgs;
}
// Searches everything recorded so far (not just this call's drain), so a
// message drained by an earlier step is still found.
async function until(pred, ms = 3000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    await drain();
    const hit = all.find(pred);
    if (hit) return { hit, seen: all };
    await page.waitForTimeout(25);
  }
  throw new Error("timed out; saw kinds: " + all.map(m => m.kind).join(","));
}
function walk(node, f) { f(node); (node.children || []).forEach(c => walk(c, f)); }

before(async () => {
  browser = await webkit.launch();
  page = await browser.newPage();
  await page.addInitScript(BRIDGE);
  await page.route(`${ORIGIN}/**`, async route => {
    const url = new URL(route.request().url());
    if (url.pathname === "/") return route.fulfill({ contentType: "text/html", body: PAGE });
    if (url.pathname === "/api/checkout")
      return route.fulfill({ status: 500, contentType: "application/json", body: '{"error":"card declined"}' });
    if (url.pathname === "/api/cart") return route.fulfill({ status: 200, contentType: "application/json", body: '{"items":2}' });
    return route.fulfill({ status: 404, body: "" });
  });
  await page.goto(`${ORIGIN}/`);
});

after(async () => {
  await browser?.close();
  // Contract: Rust parses every message the real bridge produced.
  const targetDir = process.env.CARGO_TARGET_DIR || join(root, "target");
  const bin = join(targetDir, "debug", "examples", "validate");
  if (!existsSync(bin)) throw new Error("build it first: cargo build -p relay-bridge --example validate");
  const r = spawnSync(bin, { input: all.map(m => JSON.stringify(m)).join("\n") });
  assert.equal(r.status, 0, r.stderr.toString());
  assert.match(r.stdout.toString(), /\d+ messages parsed/);
});

test("load announces the page and streams a DOM with ids", async () => {
  const { seen } = await until(m => m.kind === "dom_snapshot");
  assert.ok(seen.some(m => m.kind === "hello" && m.url === `${ORIGIN}/`));
  const snap = seen.find(m => m.kind === "dom_snapshot");
  const nodes = [];
  walk(snap.root, n => nodes.push(n));
  const pay = nodes.find(n => n.attrs.id === "pay");
  assert.equal(pay.text, "Pay now");
  assert.equal(pay.interactive, true);
  assert.equal(await page.getAttribute("#pay", "data-relay-id"), String(pay.id));
  const pw = nodes.find(n => n.attrs.id === "pw");
  assert.equal(pw.attrs.value, undefined, "password values never leave the page");
  assert.equal(nodes.find(n => n.attrs.id === "card").attrs.value, undefined, "card numbers never leave the page");
  assert.equal(nodes.find(n => n.attrs.id === "otp").attrs.value, undefined, "one-time codes never leave the page");
  assert.equal(nodes.find(n => n.attrs.id === "secret").attrs.value, undefined, "data-relay-private values never leave the page");
  assert.equal(nodes.find(n => n.attrs.id === "q").attrs.value, "hello", "ordinary fields stay debuggable");
  assert.equal(nodes.find(n => n.attrs.id === "card").attrs.autocomplete, "cc-number", "purpose stays visible");
  assert.equal(nodes.find(n => n.attrs.id === "card").interactive, true);
  assert.ok(!nodes.some(n => n.tag === "script"), "scripts are skipped");
  const pageMsg = (await until(m => m.kind === "page" && m.title === "Checkout")).hit;
  assert.equal(pageMsg.url, `${ORIGIN}/`);
});

test("console, uncaught errors, and rejections are captured", async () => {
  // Boot-time logs ran before the test; they are already in the stream.
  assert.ok(all.some(m => m.kind === "console" && m.level === "log" && m.args[0] === "boot" && m.args[1] === '{"n":1}'));
  assert.ok(all.some(m => m.kind === "console" && m.level === "warn"));
  await page.evaluate(() => { setTimeout(() => { throw new TypeError("cart.total is undefined"); }); Promise.reject(new Error("nope")); });
  await until(m => m.kind === "console" && m.level === "error" && /cart\.total is undefined/.test(m.args[0]));
  await until(m => m.kind === "console" && m.level === "error" && /Unhandled rejection: Error: nope/.test(m.args[0]));
});

test("fetch and XHR are recorded with status, timing, and JSON bodies", async () => {
  await page.evaluate(async () => {
    await fetch("/api/checkout", { method: "POST", body: JSON.stringify({ card: "4242" }) });
    await new Promise(res => { const x = new XMLHttpRequest(); x.open("GET", "/api/cart"); x.onloadend = res; x.send(); });
  });
  const f = (await until(m => m.kind === "network" && m.api === "fetch")).hit;
  assert.equal(f.method, "POST");
  assert.equal(f.status, 500);
  assert.equal(f.url, `${ORIGIN}/api/checkout`);
  assert.equal(f.response_body, '{"error":"card declined"}');
  assert.equal(f.request_body, '{"card":"4242"}');
  assert.ok(f.duration_ms >= 0);
  const x = (await until(m => m.kind === "network" && m.api === "xhr")).hit;
  assert.equal(x.status, 200);
  assert.equal(x.response_body, '{"items":2}');
});

test("mutations arrive as subtree patches", async () => {
  await page.evaluate(() => { const b = document.createElement("button"); b.textContent = "Apply coupon"; document.getElementById("list").appendChild(b); });
  const patch = (await until(m => m.kind === "dom_patch")).hit;
  const nodes = [];
  patch.nodes.forEach(n => walk(n, x => nodes.push(x)));
  assert.ok(nodes.some(n => n.tag === "button" && n.text === "Apply coupon"));
  await page.evaluate(() => document.getElementById("list").replaceChildren());
  const p2 = (await until(m => m.kind === "dom_patch" && m.removed.length > 0 && m !== patch)).hit;
  assert.ok(p2.removed.length >= 1);
});

test("body attribute changes arrive as dom_attrs, not a full snapshot", async () => {
  const bodyId = Number(await page.getAttribute("body", "data-relay-id"));
  assert.ok(bodyId > 0, "the snapshot gave body an id");
  await drain();
  const before = all.filter(m => m.kind === "dom_snapshot").length;
  await page.evaluate(() => document.body.classList.add("relay-attr-test"));
  const { hit } = await until(m => m.kind === "dom_attrs" && m.nodes.some(n => n.id === bodyId));
  assert.equal(hit.nodes.find(n => n.id === bodyId).attrs.class, "relay-attr-test");
  await page.waitForTimeout(80);
  await drain();
  assert.equal(all.filter(m => m.kind === "dom_snapshot").length, before, "no full DOM resend for one class");
  assert.ok(!all.some(m => m.kind === "dom_patch" && m.nodes.some(n => n.id === bodyId)),
    "body is not re-sent as a patch subtree either");
});

test("html and head attribute changes also use dom_attrs", async () => {
  const htmlId = Number(await page.getAttribute("html", "data-relay-id"));
  const headId = Number(await page.getAttribute("head", "data-relay-id"));
  assert.ok(htmlId > 0 && headId > 0, "the snapshot includes html and head");
  await drain();
  const before = all.filter(m => m.kind === "dom_snapshot").length;
  await page.evaluate(() => {
    document.documentElement.classList.add("relay-html-attr");
    document.head.classList.add("relay-head-attr");
  });
  const { hit } = await until(m => m.kind === "dom_attrs" && m.nodes.some(n => n.id === htmlId) && m.nodes.some(n => n.id === headId));
  assert.equal(hit.nodes.find(n => n.id === htmlId).attrs.class, "relay-html-attr");
  assert.equal(hit.nodes.find(n => n.id === headId).attrs.class, "relay-head-attr");
  await page.waitForTimeout(80);
  await drain();
  assert.equal(all.filter(m => m.kind === "dom_snapshot").length, before, "no full resend for html/head attributes");
});

test("a DOM patch past the node budget falls back to one coherent snapshot", async () => {
  const p = await browser.newPage();
  await p.addInitScript(BRIDGE);
  await p.route(`${ORIGIN}/**`, route => route.fulfill({ contentType: "text/html",
    body: "<html><body><div id='a'></div><div id='b'></div></body></html>" }));
  await p.goto(`${ORIGIN}/budget`);
  await p.evaluate(() => window.__relay.drain());
  await p.evaluate(() => {
    for (const id of ["a", "b"]) {
      const d = document.getElementById(id);
      for (let i = 0; i < 2500; i++) {
        const e = document.createElement("button");
        e.textContent = id + i;
        d.appendChild(e);
      }
    }
  });
  let sawSnapshot = false, patchNodes = 0;
  for (let i = 0; i < 200 && !sawSnapshot; i++) {
    const msgs = await p.evaluate(() => window.__relay.drain());
    all.push(...msgs);
    for (const m of msgs) {
      if (m.kind === "dom_snapshot") sawSnapshot = true;
      if (m.kind === "dom_patch") patchNodes = Math.max(patchNodes, m.nodes.length);
    }
    if (!sawSnapshot) await p.waitForTimeout(25);
  }
  assert.ok(sawSnapshot, "a partial patch was not posted; a snapshot replaced it");
  assert.ok(patchNodes <= 2, "only whole dirty roots are patched, never a truncated one");
  await p.close();
});

test("commands click, type, query, and report errors", async () => {
  const id = Number(await page.getAttribute("#pay", "data-relay-id"));
  const email = Number(await page.getAttribute("#email", "data-relay-id"));
  // Exactly the script shape Rust's Command::to_script produces.
  const run = cmd => page.evaluate(s => eval(s), `window.__relay && window.__relay.exec(${JSON.stringify(cmd)}), void 0`)
    .then(() => until(m => m.kind === "result" && m.id === cmd.id)).then(r => r.hit);
  const click = await run({ id: 1, op: "click", target: id });
  assert.equal(click.ok, true);
  assert.equal(await page.evaluate(() => window.paid), 1);
  const typed = await run({ id: 2, op: "type", target: email, text: "a@b.test", submit: true, clear: true });
  assert.equal(typed.ok, true);
  assert.equal(await page.inputValue("#email"), "a@b.test");
  assert.equal(await page.evaluate(() => window.lastInput), "a@b.test", "input listeners (React-style) fire");
  assert.equal(await page.evaluate(() => window.submitted), true);
  const found = await run({ id: 3, op: "find_text", text: "pay now" });
  assert.equal(found.value[0].id, id);
  const q = await run({ id: 4, op: "query", selector: "input" });
  assert.equal(q.value.length, 6);
  const card = Number(await page.getAttribute("#card", "data-relay-id"));
  const filled = await run({ id: 7, op: "type", target: card, text: "4242424242424242", submit: false, clear: true });
  assert.equal(filled.ok, true);
  assert.equal(filled.value.text, "", "a command reply does not echo the card value");
  assert.equal(await page.inputValue("#card"), "4242424242424242", "the page still receives the typed value");
  const search = Number(await page.getAttribute("#q", "data-relay-id"));
  const ordinary = await run({ id: 8, op: "type", target: search, text: "relay", submit: false, clear: true });
  assert.equal(ordinary.value.text, "relay", "ordinary fields still report their value");
  const missing = await run({ id: 5, op: "click", target: 999999 });
  assert.equal(missing.ok, false);
  assert.match(missing.error, /re-read the DOM/);
  const hl = await run({ id: 6, op: "highlight", target: id });
  assert.equal(hl.ok, true);
  assert.equal(await page.locator("[data-relay-overlay]").count(), 1);
});

test("2FA prompts are detected so attached agents pause", async () => {
  await page.evaluate(() => { const i = document.createElement("input"); i.autocomplete = "one-time-code"; document.body.appendChild(i); });
  const c = (await until(m => m.kind === "challenge" && m.challenge)).hit;
  assert.equal(c.challenge, "two_factor");
});

test("the page cannot replace the bridge", async () => {
  const replaced = await page.evaluate(() => { try { window.__relay = { exec() { return "evil"; } }; } catch (e) {} return window.__relay.version; });
  assert.equal(replaced, 1);
});

test("link hints label visible elements and 'f' is forwarded only outside inputs", async () => {
  const pay = Number(await page.getAttribute("#pay", "data-relay-id"));
  await page.evaluate(s => eval(s), `window.__relay.exec(${JSON.stringify({ id: 50, op: "show_hints", hints: [[pay, "a"], [987654, "s"]] })})`);
  const r = (await until(m => m.kind === "result" && m.id === 50)).hit;
  assert.equal(r.value, 1, "only existing, visible elements get a label");
  assert.equal(await page.locator("[data-relay-hints] span").innerText(), "A");
  await page.evaluate(() => window.__relay.exec({ id: 51, op: "hide_hints" }));
  assert.equal(await page.locator("[data-relay-hints]").count(), 0);

  await page.evaluate(() => document.activeElement && document.activeElement.blur());
  await page.keyboard.press("f");
  await until(m => m.kind === "hotkey" && m.key === "f");
  const before = all.filter(m => m.kind === "hotkey").length;
  await page.focus("#email");
  await page.keyboard.press("f");
  await page.waitForTimeout(100);
  await drain();
  assert.equal(all.filter(m => m.kind === "hotkey").length, before, "typing 'f' in an input is not a hotkey");
  assert.match(await page.inputValue("#email"), /f$/);
});

const MD_PAGE = `<!doctype html><html><head><title>Doc</title><style>.x{color:red}</style></head><body>
  <nav><a href="/home">NAVLINK</a></nav>
  <header>Site header</header>
  <article>
    <h1>Hello <em>World</em></h1>
    <p>Intro with <strong>bold</strong>, <a href="/rel">a link</a> and <code>inline()</code>.</p>
    <p hidden>HIDDEN-ATTR</p>
    <p style="display:none">HIDDEN-STYLE</p>
    <aside>ASIDE-TEXT</aside>
    <h2>List</h2>
    <ul><li>one</li><li>two</li></ul>
    <ol><li>first</li><li>second</li></ol>
    <blockquote>quoted text</blockquote>
    <pre><code>let x = 1;\nlet y = 2;</code></pre>
    <img src="/pic.png" alt="A picture">
    <table><thead><tr><th>Name</th><th>Qty</th></tr></thead>
      <tbody><tr><td>Apple</td><td>3</td></tr><tr><td>Pear</td><td>5</td></tr></tbody></table>
    <script>window.SCRIPT_TEXT = "SCRIPT-BODY"</script>
  </article>
  <footer>FOOTER-TEXT</footer>
</body></html>`;

test("markdown op converts main content and posts it", async () => {
  const mdPage = await browser.newPage();
  await mdPage.addInitScript(BRIDGE);
  await mdPage.route(`${ORIGIN}/**`, route => route.fulfill({ contentType: "text/html", body: MD_PAGE }));
  await mdPage.goto(`${ORIGIN}/doc`);
  await mdPage.evaluate(() => window.__relay.exec({ id: 90, op: "markdown" }));
  const msgs = await mdPage.evaluate(() => window.__relay.drain());
  all.push(...msgs); // the after-hook proves Rust parses them
  const result = msgs.find(m => m.kind === "result" && m.id === 90);
  const posted = msgs.find(m => m.kind === "markdown");
  assert.ok(result.ok);
  assert.equal(posted.text, result.value);
  const md = result.value;
  assert.match(md, /^# Hello \*World\*$/m);
  assert.match(md, /\*\*bold\*\*/);
  assert.ok(md.includes(`[a link](${ORIGIN}/rel)`), md);
  assert.ok(md.includes("`inline()`"));
  assert.match(md, /^- one$/m);
  assert.match(md, /^1\. first$/m);
  assert.match(md, /^2\. second$/m);
  assert.match(md, /^> quoted text$/m);
  assert.ok(md.includes("```\nlet x = 1;\nlet y = 2;\n```"), md);
  assert.ok(md.includes(`![A picture](${ORIGIN}/pic.png)`), md);
  assert.ok(md.includes("| Name | Qty |\n| --- | --- |\n| Apple | 3 |\n| Pear | 5 |"), md);
  for (const bad of ["NAVLINK", "Site header", "HIDDEN-ATTR", "HIDDEN-STYLE", "ASIDE-TEXT", "FOOTER-TEXT", "SCRIPT-BODY", "color:red"])
    assert.ok(!md.includes(bad), `${bad} must be skipped:\n${md}`);
  await mdPage.close();
});

test("markdown falls back to body and caps at 200 KB", async () => {
  const p = await browser.newPage();
  await p.addInitScript(BRIDGE);
  await p.route(`${ORIGIN}/**`, route => route.fulfill({ contentType: "text/html",
    body: `<html><body><nav>NAV</nav><h3>Plain</h3>${"<p>" + "x".repeat(1000) + "</p>".repeat(1)}`.repeat(1) + "<p>y</p>".repeat(1) + "</body></html>" }));
  await p.goto(`${ORIGIN}/plain`);
  const small = await p.evaluate(() => window.__relay.exec({ id: 91, op: "markdown" }).value);
  assert.match(small, /^### Plain$/m);
  assert.ok(!small.includes("NAV"));
  await p.evaluate(() => { const d = document.createElement("div"); for (let i = 0; i < 6000; i++) { const e = document.createElement("p"); e.textContent = "z".repeat(100); d.appendChild(e); } document.body.appendChild(d); });
  const big = await p.evaluate(() => window.__relay.exec({ id: 92, op: "markdown" }).value);
  assert.ok(new TextEncoder().encode(big).length <= 200 * 1024 + 8, "capped");
  assert.ok(big.length > 150000);
  await p.close();
});

const APP_PAGE = `<!doctype html><html><head><title>Shop</title>
  <link rel="manifest" href="/app/manifest.json">
</head><body><h1>Shop</h1></body></html>`;

test("web_app reads a manifest asynchronously and returns a PNG icon", async () => {
  const p = await browser.newPage();
  await p.addInitScript(BRIDGE);
  const png = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lU2l4wAAAABJRU5ErkJggg==",
    "base64");
  await p.route(`${ORIGIN}/**`, async route => {
    const url = new URL(route.request().url());
    if (url.pathname === "/app/") return route.fulfill({ contentType: "text/html", body: APP_PAGE });
    if (url.pathname === "/app/manifest.json")
      return route.fulfill({ contentType: "application/manifest+json",
        body: JSON.stringify({
          name: "Relay Shop",
          short_name: "Shop",
          start_url: "/app/start",
          display: "standalone",
          theme_color: "#101010",
          icons: [
            { src: "/icon-32.png", sizes: "32x32", type: "image/png" },
            { src: "/icon-512.png", sizes: "512x512", type: "image/png" },
          ],
        }) });
    if (url.pathname === "/icon-512.png") return route.fulfill({ contentType: "image/png", body: png });
    return route.fulfill({ status: 404, body: "" });
  });
  await p.goto(`${ORIGIN}/app/`);
  const placeholder = await p.evaluate(() => window.__relay.exec({ id: 120, op: "web_app" }));
  assert.equal(placeholder.value, null, "an async op replies when it settles");
  let result;
  for (let i = 0; i < 100 && !result; i++) {
    const msgs = await p.evaluate(() => window.__relay.drain());
    all.push(...msgs);
    result = msgs.find(m => m.kind === "result" && m.id === 120);
    if (!result) await p.waitForTimeout(25);
  }
  assert.ok(result, "the async result arrived");
  assert.equal(result.ok, true, result.error);
  assert.equal(result.value.available, true, result.value.reason);
  assert.equal(result.value.name, "Relay Shop");
  assert.equal(result.value.start_url, `${ORIGIN}/app/start`);
  assert.equal(result.value.display, "standalone");
  assert.match(result.value.icon, /^data:image\/png;base64,/);
  await p.close();
});

test("web_app explains a page without a manifest", async () => {
  const p = await browser.newPage();
  await p.addInitScript(BRIDGE);
  await p.route(`${ORIGIN}/**`, route => route.fulfill({ contentType: "text/html", body: "<html><body>plain</body></html>" }));
  await p.goto(`${ORIGIN}/plain-app`);
  const r = await p.evaluate(() => window.__relay.exec({ id: 121, op: "web_app" }).value);
  assert.equal(r.available, false);
  assert.match(r.reason, /does not declare/);
  await p.close();
});

test("favicon returns a supported image and refuses non-images", async () => {
  const p = await browser.newPage();
  await p.addInitScript(BRIDGE);
  const png = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lU2l4wAAAABJRU5ErkJggg==",
    "base64");
  await p.route(`${ORIGIN}/**`, async route => {
    const url = new URL(route.request().url());
    if (url.pathname === "/icon/") return route.fulfill({ contentType: "text/html", body: '<link rel="icon" href="/icon.png"><h1>Icon</h1>' });
    if (url.pathname === "/icon.png") return route.fulfill({ contentType: "image/png", body: png });
    if (url.pathname === "/bad/") return route.fulfill({ contentType: "text/html", body: '<link rel="icon" href="/icon.txt"><h1>Bad</h1>' });
    if (url.pathname === "/icon.txt") return route.fulfill({ contentType: "text/plain", body: "not an icon" });
    return route.fulfill({ status: 404, body: "" });
  });
  await p.goto(`${ORIGIN}/icon/`);
  let result;
  await p.evaluate(() => window.__relay.exec({ id: 130, op: "favicon" }));
  for (let i = 0; i < 100 && !result; i++) {
    const msgs = await p.evaluate(() => window.__relay.drain());
    result = msgs.find(m => m.kind === "result" && m.id === 130);
    if (!result) await p.waitForTimeout(25);
  }
  assert.ok(result, "favicon replied");
  assert.equal(result.ok, true, result.error);
  assert.equal(result.value.available, true, JSON.stringify(result.value));
  assert.equal(result.value.mime, "image/png");
  assert.match(result.value.data, /^data:image\/png;base64,/);

  await p.goto(`${ORIGIN}/bad/`);
  result = null;
  await p.evaluate(() => window.__relay.exec({ id: 131, op: "favicon" }));
  for (let i = 0; i < 100 && !result; i++) {
    const msgs = await p.evaluate(() => window.__relay.drain());
    result = msgs.find(m => m.kind === "result" && m.id === 131);
    if (!result) await p.waitForTimeout(25);
  }
  assert.ok(result, "bad favicon replied");
  assert.equal(result.ok, true, result.error);
  assert.equal(result.value.available, false, "text/plain and ICO are refused");
  await p.close();
});

test("press_key dispatches a key and storage redacts secret names", async () => {
  const p = await browser.newPage();
  await p.addInitScript(BRIDGE);
  await p.route(`${ORIGIN}/**`, route => {
    const url = new URL(route.request().url());
    if (url.pathname === "/keys/") return route.fulfill({ contentType: "text/html", body: '<body><input id="q"><script>window.keys=[];document.addEventListener("keydown",e=>window.keys.push(e.key))</script></body>' });
    return route.fulfill({ status: 404 });
  });
  await p.goto(`${ORIGIN}/keys/`);
  const key = await p.evaluate(() => window.__relay.exec({ id: 140, op: "press_key", key: "Enter", modifiers: ["shift"] }));
  assert.equal(key.ok, true);
  assert.equal(key.value.key, "Enter");
  assert.deepEqual(await p.evaluate(() => window.keys), ["Enter"]);
  await p.evaluate(() => { localStorage.setItem("theme", "dark"); localStorage.setItem("auth_token", "secret-value"); });
  const storage = await p.evaluate(() => window.__relay.exec({ id: 141, op: "storage" }));
  assert.equal(storage.ok, true);
  assert.equal(storage.value.local.values.theme, "dark");
  assert.equal(storage.value.local.values.auth_token, "[redacted]");
  assert.equal(storage.value.local.redacted, 1);
  await p.close();
});

test("eval_js returns a JSON expression and refuses empty/oversized input", async () => {
  const p = await browser.newPage();
  await p.addInitScript(BRIDGE);
  await p.route(`${ORIGIN}/**`, route => route.fulfill({ contentType: "text/html", body: "<body><h1>Eval</h1></body>" }));
  await p.goto(`${ORIGIN}/eval/`);
  const ok = await p.evaluate(() => window.__relay.exec({ id: 150, op: "eval_js", script: "({sum: 1 + 2, title: document.title})" }));
  assert.equal(ok.ok, true, ok.error);
  assert.deepEqual(ok.value.result, { sum: 3, title: "" });
  const empty = await p.evaluate(() => window.__relay.exec({ id: 151, op: "eval_js", script: "  " }));
  assert.equal(empty.ok, false);
  const big = await p.evaluate(() => window.__relay.exec({ id: 152, op: "eval_js", script: "x".repeat(50001) }));
  assert.equal(big.ok, false);
  await p.close();
});

test("set_cookie writes only a scoped cookie on the page's own origin", async () => {
  const p = await browser.newPage();
  await p.addInitScript(BRIDGE);
  await p.route(`${ORIGIN}/**`, route => route.fulfill({ contentType: "text/html", body: "<body>cookies</body>" }));
  await p.goto(`${ORIGIN}/cookies/`);
  const ok = await p.evaluate(() => window.__relay.exec({ id: 160, op: "set_cookie", name: "relay_flag", value: "a b" }));
  assert.equal(ok.ok, true, ok.error);
  assert.equal(ok.value.set, true);
  assert.match(await p.evaluate(() => document.cookie), /relay_flag=a%20b/);
  const bad = await p.evaluate(() => window.__relay.exec({ id: 161, op: "set_cookie", name: "bad name", value: "x" }));
  assert.equal(bad.ok, false);
  const newline = await p.evaluate(() => window.__relay.exec({ id: 162, op: "set_cookie", name: "ok", value: "a\nb" }));
  assert.equal(newline.ok, false);
  await p.close();
});

test("reader builds an inert article overlay and removes it", async () => {
  const p = await browser.newPage();
  await p.addInitScript(BRIDGE);
  const page = `<!doctype html><html><body>
    <nav id="nav">menu</nav>
    <article id="story"><h1>Story</h1><p onclick="window.pwned=1">Body</p>
      <img src="data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7">
      <script>window.scripted=1</script>
    </article>
    <footer>foot</footer>
  </body></html>`;
  await p.route(`${ORIGIN}/**`, route => route.fulfill({ contentType: "text/html", body: page }));
  await p.goto(`${ORIGIN}/reader/`);
  const on = await p.evaluate(() => window.__relay.exec({ id: 170, op: "reader", on: true }));
  assert.equal(on.ok, true, on.error);
  assert.equal(await p.evaluate(() => !!document.getElementById("relay-reader")), true);
  const overlay = await p.evaluate(() => {
    const r = document.getElementById("relay-reader");
    return { text: r.innerText, html: r.innerHTML, hasNav: !!r.querySelector("nav"), hasScript: !!r.querySelector("script"), onclick: /onclick/i.test(r.innerHTML) };
  });
  assert.match(overlay.text, /Story/);
  assert.match(overlay.text, /Body/);
  assert.equal(overlay.hasNav, false);
  assert.equal(overlay.hasScript, false);
  assert.equal(overlay.onclick, false);
  const off = await p.evaluate(() => window.__relay.exec({ id: 171, op: "reader", on: false }));
  assert.equal(off.ok, true);
  assert.equal(await p.evaluate(() => !!document.getElementById("relay-reader")), false);
  await p.close();
});

test("picture_in_picture toggles a video or reports no support", async () => {
  const p = await browser.newPage();
  await p.addInitScript(BRIDGE);
  await p.route(`${ORIGIN}/**`, route => route.fulfill({ contentType: "text/html", body: '<body><video id="v"></video></body>' }));
  await p.goto(`${ORIGIN}/video/`);
  await p.evaluate(() => {
    const v = document.getElementById("v");
    v.requestPictureInPicture = () => { window.pip = (window.pip || 0) + 1; return Promise.resolve(); };
    v.webkitSetPresentationMode = (mode) => { window.pipMode = mode; window.pip = (window.pip || 0) + 1; };
  });
  let on;
  await p.evaluate(() => window.__relay.exec({ id: 180, op: "picture_in_picture" }));
  for (let i = 0; i < 100 && !on; i++) {
    const msgs = await p.evaluate(() => window.__relay.drain());
    on = msgs.find(m => m.kind === "result" && m.id === 180);
    if (!on) await p.waitForTimeout(25);
  }
  assert.ok(on, "PiP replied");
  assert.equal(on.ok, true, on.error);
  assert.equal(on.value.available, true);
  assert.equal(on.value.active, true);
  assert.equal(await p.evaluate(() => window.pip), 1);
  await p.goto(`${ORIGIN}/novideo/`);
  let none;
  await p.evaluate(() => window.__relay.exec({ id: 181, op: "picture_in_picture" }));
  for (let i = 0; i < 100 && !none; i++) {
    const msgs = await p.evaluate(() => window.__relay.drain());
    none = msgs.find(m => m.kind === "result" && m.id === 181);
    if (!none) await p.waitForTimeout(25);
  }
  assert.ok(none, "no-video PiP replied");
  assert.equal(none.ok, true);
  assert.equal(none.value.available, false);
  await p.close();
});
