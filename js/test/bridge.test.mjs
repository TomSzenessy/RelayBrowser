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
  assert.equal(q.value.length, 2);
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
