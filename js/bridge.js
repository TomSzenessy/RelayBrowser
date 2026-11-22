// Relay "One Bridge": injected at document start into every page (main frame).
// Streams DOM, network, and console state to Rust and executes commands sent
// back from Rust. The Inspector UI and the MCP server both read the state this
// script produces; nothing else talks to the page.
//
// Wire contract (owner: crates/relay-bridge/src/protocol.rs):
//   page -> Rust: post({kind: "...", ...})  via webkit.messageHandlers.relay
//   Rust -> page: window.__relay.exec({id, op, ...})  result posted as kind "result"
(function () {
  "use strict";
  if (window.__relay) return;

  var MAX_NODES = 4000;
  var MAX_TEXT = 160;
  var MAX_BODY = 4096;
  var MAX_ARG = 2000;
  var SKIP = { SCRIPT: 1, STYLE: 1, NOSCRIPT: 1, TEMPLATE: 1, META: 1, LINK: 1, HEAD: 1 };
  var KEEP_ATTRS = ["id", "class", "name", "type", "href", "src", "role", "aria-label",
    "placeholder", "title", "alt", "value", "for", "action", "method", "autocomplete"];
  var INTERACTIVE = { A: 1, BUTTON: 1, INPUT: 1, SELECT: 1, TEXTAREA: 1, SUMMARY: 1, LABEL: 1, OPTION: 1 };

  var sink = (window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.relay) || null;
  var queue = [];
  function post(msg) {
    msg.t = Date.now();
    if (sink) {
      try { sink.postMessage(JSON.stringify(msg)); } catch (e) { /* page tore down */ }
    } else {
      queue.push(msg); // test harnesses drain window.__relay.drain()
    }
  }

  // ---------------------------------------------------------------- DOM
  var nextId = 1;
  var byId = new Map();
  var idOf = new WeakMap();

  function relayId(el) {
    var id = idOf.get(el);
    if (!id) {
      id = nextId++;
      idOf.set(el, id);
      byId.set(id, el);
      try { el.setAttribute("data-relay-id", String(id)); } catch (e) { /* svg quirks */ }
    }
    return id;
  }

  function isInteractive(el) {
    if (INTERACTIVE[el.tagName]) return true;
    if (el.hasAttribute("onclick") || el.hasAttribute("contenteditable")) return true;
    var role = el.getAttribute("role");
    if (role && /button|link|checkbox|tab|menuitem|switch|textbox|combobox|option|radio/.test(role)) return true;
    var ti = el.getAttribute("tabindex");
    return ti !== null && ti !== "-1";
  }

  function directText(el) {
    var s = "";
    for (var c = el.firstChild; c; c = c.nextSibling) {
      if (c.nodeType === 3) s += c.nodeValue;
    }
    s = s.replace(/\s+/g, " ").trim();
    return s.length > MAX_TEXT ? s.slice(0, MAX_TEXT) + "…" : s;
  }

  function serialize(el, budget) {
    if (budget.n <= 0 || SKIP[el.tagName]) return null;
    budget.n--;
    var attrs = {};
    for (var i = 0; i < KEEP_ATTRS.length; i++) {
      var a = KEEP_ATTRS[i];
      var v = a === "value" && "value" in el && el.type !== "password" ? el.value : el.getAttribute(a);
      if (a === "value" && el.type === "password") v = null;
      if (v !== null && v !== undefined && v !== "") attrs[a] = String(v).slice(0, MAX_TEXT);
    }
    var node = {
      id: relayId(el),
      tag: el.tagName.toLowerCase(),
      attrs: attrs,
      text: directText(el),
      interactive: isInteractive(el),
      children: []
    };
    for (var c = el.firstElementChild; c; c = c.nextElementSibling) {
      var child = serialize(c, budget);
      if (child) node.children.push(child);
    }
    return node;
  }

  function snapshot() {
    var root = document.documentElement;
    if (!root) return;
    var body = document.body || root;
    post({ kind: "dom_snapshot", root: serialize(body, { n: MAX_NODES }) });
  }

  // Mutation diffs: re-serialize the nearest known ancestor of each change,
  // coalesced per animation frame. Rust replaces those subtrees by id.
  var dirty = new Set();
  var removed = [];
  var scheduled = false;
  function flush() {
    scheduled = false;
    if (!document.body) return;
    var roots = [];
    dirty.forEach(function (el) {
      if (!el.isConnected) return;
      for (var p = el.parentElement; p; p = p.parentElement) {
        if (dirty.has(p)) return; // an ancestor is re-sent anyway
      }
      roots.push(el);
    });
    dirty.clear();
    var nodes = [];
    var budget = { n: MAX_NODES };
    roots.forEach(function (el) {
      var n = el === document.body || !idOf.get(el) ? null : serialize(el, budget);
      if (n) nodes.push(n);
    });
    var gone = removed;
    removed = [];
    if (roots.some(function (el) { return el === document.body || !idOf.get(el); })) {
      snapshot();
    } else if (nodes.length || gone.length) {
      post({ kind: "dom_patch", nodes: nodes, removed: gone });
    }
  }
  function markDirty(el) {
    if (!el || el.nodeType !== 1) return;
    dirty.add(el);
    if (!scheduled) {
      scheduled = true;
      (window.requestAnimationFrame || setTimeout)(flush, 16);
    }
  }
  function collectRemoved(node) {
    if (node.nodeType !== 1) return;
    var id = idOf.get(node);
    if (id) { removed.push(id); byId.delete(id); }
    for (var c = node.firstElementChild; c; c = c.nextElementSibling) collectRemoved(c);
  }

  var observer = new MutationObserver(function (records) {
    for (var i = 0; i < records.length; i++) {
      var r = records[i];
      if (r.type === "attributes" && r.attributeName === "data-relay-id") continue;
      if (r.type === "childList") r.removedNodes.forEach(collectRemoved);
      markDirty(r.type === "characterData" ? r.target.parentElement : r.target);
    }
    detectChallenge();
  });

  // ------------------------------------------------------------ Console
  var levels = ["log", "info", "warn", "error", "debug"];
  function fmt(v) {
    if (typeof v === "string") return v.slice(0, MAX_ARG);
    if (v instanceof Error) return (v.name + ": " + v.message + (v.stack ? "\n" + v.stack : "")).slice(0, MAX_ARG);
    try { return JSON.stringify(v).slice(0, MAX_ARG); } catch (e) { return String(v).slice(0, MAX_ARG); }
  }
  levels.forEach(function (level) {
    var orig = console[level];
    if (typeof orig !== "function") return;
    console[level] = function () {
      try {
        post({ kind: "console", level: level, args: Array.prototype.map.call(arguments, fmt) });
      } catch (e) { /* never break the page */ }
      return orig.apply(this, arguments);
    };
  });
  window.addEventListener("error", function (ev) {
    post({ kind: "console", level: "error", args: [fmt(ev.error || ev.message)],
      source: ev.filename ? ev.filename + ":" + ev.lineno + ":" + ev.colno : null });
  });
  window.addEventListener("unhandledrejection", function (ev) {
    post({ kind: "console", level: "error", args: ["Unhandled rejection: " + fmt(ev.reason)] });
  });

  // ------------------------------------------------------------ Network
  var reqSeq = 0;
  function bodyText(text, contentType) {
    if (typeof text !== "string") return null;
    if (contentType && !/json|text|xml|javascript|form/.test(contentType)) return null;
    return text.length > MAX_BODY ? text.slice(0, MAX_BODY) + "…" : text;
  }
  function absUrl(u) { try { return new URL(u, location.href).href; } catch (e) { return String(u); } }

  var origFetch = window.fetch;
  if (typeof origFetch === "function") {
    window.fetch = function (input, init) {
      var id = "f" + (++reqSeq);
      var method = ((init && init.method) || (input && input.method) || "GET").toUpperCase();
      var url = absUrl(typeof input === "string" || input instanceof URL ? input : input.url);
      var start = performance.now();
      var reqBody = init && typeof init.body === "string" ? bodyText(init.body, "text") : null;
      return origFetch.apply(this, arguments).then(function (res) {
        var entry = { kind: "network", id: id, api: "fetch", method: method, url: url, status: res.status,
          status_text: res.statusText, duration_ms: performance.now() - start, request_body: reqBody,
          content_type: res.headers.get("content-type"), response_body: null, error: null };
        var ct = entry.content_type || "";
        if (/json|text/.test(ct)) {
          res.clone().text().then(function (t) { entry.response_body = bodyText(t, ct); post(entry); },
            function () { post(entry); });
        } else {
          post(entry);
        }
        return res;
      }, function (err) {
        post({ kind: "network", id: id, api: "fetch", method: method, url: url, status: 0,
          duration_ms: performance.now() - start, request_body: reqBody, error: fmt(err) });
        throw err;
      });
    };
  }

  var XHR = window.XMLHttpRequest;
  if (XHR) {
    var open = XHR.prototype.open, send = XHR.prototype.send;
    XHR.prototype.open = function (method, url) {
      this.__relay = { id: "x" + (++reqSeq), method: String(method || "GET").toUpperCase(), url: absUrl(url) };
      return open.apply(this, arguments);
    };
    XHR.prototype.send = function (body) {
      var meta = this.__relay, xhr = this;
      if (meta) {
        meta.start = performance.now();
        meta.body = typeof body === "string" ? bodyText(body, "text") : null;
        xhr.addEventListener("loadend", function () {
          var ct = xhr.getResponseHeader("content-type");
          var text = null;
          try { text = xhr.responseType === "" || xhr.responseType === "text" ? xhr.responseText : null; } catch (e) { /* */ }
          post({ kind: "network", id: meta.id, api: "xhr", method: meta.method, url: meta.url, status: xhr.status,
            status_text: xhr.statusText, duration_ms: performance.now() - meta.start, request_body: meta.body,
            content_type: ct, response_body: bodyText(text, ct), error: xhr.status === 0 ? "network error" : null });
        });
      }
      return send.apply(this, arguments);
    };
  }

  if (window.PerformanceObserver) {
    try {
      new PerformanceObserver(function (list) {
        list.getEntries().forEach(function (e) {
          if (e.initiatorType === "fetch" || e.initiatorType === "xmlhttprequest") return; // wrapped above
          post({ kind: "network", id: "r" + (++reqSeq), api: "resource", method: "GET", url: e.name,
            status: e.responseStatus || null, duration_ms: e.duration, resource_type: e.initiatorType,
            transfer_size: e.transferSize || null });
        });
      }).observe({ type: "resource", buffered: true });
    } catch (e) { /* older engines */ }
  }

  // ----------------------------------------------- Challenge detection
  // Attached agents pause when a page asks for a human (2FA, CAPTCHA).
  var lastChallenge = null;
  function detectChallenge() {
    var kind = null;
    if (document.querySelector('iframe[src*="recaptcha"], iframe[src*="hcaptcha"], iframe[src*="challenges.cloudflare.com"], .g-recaptcha, .h-captcha, .cf-turnstile')) {
      kind = "captcha";
    } else if (document.querySelector('input[autocomplete="one-time-code"], input[name*="otp" i], input[name*="totp" i], input[id*="2fa" i]')) {
      kind = "two_factor";
    }
    if (kind !== lastChallenge) {
      lastChallenge = kind;
      post({ kind: "challenge", challenge: kind });
    }
  }

  // -------------------------------------------------------------- Media
  // Tabs playing audio never sleep.
  var playing = false;
  function mediaState() {
    var now = Array.prototype.some.call(document.querySelectorAll("audio, video"), function (m) {
      return !m.paused && !m.muted && m.volume > 0;
    });
    if (now !== playing) { playing = now; post({ kind: "media", playing: now }); }
  }
  ["play", "pause", "ended", "volumechange", "emptied"].forEach(function (ev) {
    document.addEventListener(ev, mediaState, true);
  });

  // ------------------------------------------------------------ Actions
  var overlay = null;
  function highlight(el) {
    if (!overlay) {
      overlay = document.createElement("div");
      overlay.setAttribute("data-relay-overlay", "");
      overlay.style.cssText = "position:fixed;pointer-events:none;z-index:2147483647;" +
        "background:rgba(80,140,255,.25);outline:2px solid rgb(80,140,255);border-radius:2px;transition:all .06s";
    }
    var r = el.getBoundingClientRect();
    overlay.style.left = r.left + "px"; overlay.style.top = r.top + "px";
    overlay.style.width = r.width + "px"; overlay.style.height = r.height + "px";
    if (!overlay.isConnected) document.documentElement.appendChild(overlay);
  }
  function setNativeValue(el, value) {
    var proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    var desc = Object.getOwnPropertyDescriptor(proto, "value");
    if (desc && desc.set && (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) desc.set.call(el, value);
    else if (el.isContentEditable) el.textContent = value;
    else el.value = value;
  }
  function describe(el) {
    return { id: relayId(el), tag: el.tagName.toLowerCase(), text: (el.innerText || el.value || "").slice(0, MAX_TEXT) };
  }
  function need(id) {
    var el = byId.get(id);
    if (!el || !el.isConnected) throw new Error("element " + id + " not found (re-read the DOM)");
    return el;
  }

  var ops = {
    snapshot: function () { snapshot(); return true; },
    highlight: function (c) { highlight(need(c.target)); return true; },
    unhighlight: function () { if (overlay) overlay.remove(); return true; },
    edit_style: function (c) { var el = need(c.target); el.style.cssText += ";" + c.css; return el.style.cssText; },
    scroll_into_view: function (c) { need(c.target).scrollIntoView({ block: "center" }); return true; },
    click: function (c) {
      var el = need(c.target);
      el.scrollIntoView({ block: "center" });
      var r = el.getBoundingClientRect();
      var opts = { bubbles: true, cancelable: true, view: window, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 };
      el.dispatchEvent(new PointerEvent("pointerdown", opts));
      el.dispatchEvent(new MouseEvent("mousedown", opts));
      el.dispatchEvent(new PointerEvent("pointerup", opts));
      el.dispatchEvent(new MouseEvent("mouseup", opts));
      if (typeof el.focus === "function") el.focus();
      el.click();
      return describe(el);
    },
    type: function (c) {
      var el = need(c.target);
      el.focus();
      setNativeValue(el, c.clear === false ? (el.value || "") + c.text : c.text);
      el.dispatchEvent(new InputEvent("input", { bubbles: true, data: c.text, inputType: "insertText" }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      if (c.submit) {
        el.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", code: "Enter", bubbles: true }));
        if (el.form) { if (el.form.requestSubmit) el.form.requestSubmit(); else el.form.submit(); }
      }
      return describe(el);
    },
    query: function (c) {
      var out = [];
      document.querySelectorAll(c.selector).forEach(function (el) { if (out.length < 50) out.push(describe(el)); });
      return out;
    },
    find_text: function (c) {
      var needle = String(c.text).toLowerCase(), out = [];
      var walker = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT);
      for (var el = walker.nextNode(); el && out.length < 50; el = walker.nextNode()) {
        if (SKIP[el.tagName]) continue;
        if (directText(el).toLowerCase().indexOf(needle) !== -1 ||
            (el.getAttribute("aria-label") || "").toLowerCase().indexOf(needle) !== -1) out.push(describe(el));
      }
      return out;
    },
    read_text: function (c) {
      var el = c.target ? need(c.target) : document.body;
      var t = (el.innerText || "").replace(/\n{3,}/g, "\n\n");
      return t.length > 20000 ? t.slice(0, 20000) + "…" : t;
    },
    scroll: function (c) { window.scrollBy(c.x || 0, c.y || 0); return { x: window.scrollX, y: window.scrollY }; },
    scroll_to: function (c) { window.scrollTo(c.x || 0, c.y || 0); return { x: window.scrollX, y: window.scrollY }; },
    show_hints: function (c) {
      ops.hide_hints();
      var layer = document.createElement("div");
      layer.setAttribute("data-relay-hints", "");
      layer.style.cssText = "position:fixed;inset:0;pointer-events:none;z-index:2147483647";
      var shown = 0;
      c.hints.forEach(function (pair) {
        var el = byId.get(pair[0]);
        if (!el || !el.isConnected) return;
        var r = el.getBoundingClientRect();
        if (r.width === 0 || r.height === 0 || r.bottom < 0 || r.top > innerHeight || r.right < 0 || r.left > innerWidth) return;
        var tag = document.createElement("span");
        tag.textContent = pair[1].toUpperCase();
        tag.style.cssText = "position:fixed;left:" + Math.max(0, r.left) + "px;top:" + Math.max(0, r.top) + "px;" +
          "font:bold 11px/1.2 ui-monospace,monospace;padding:1px 3px;border-radius:3px;" +
          "background:#ffd84d;color:#1b1c1f;box-shadow:0 1px 3px rgba(0,0,0,.4)";
        layer.appendChild(tag);
        shown++;
      });
      document.documentElement.appendChild(layer);
      return shown;
    },
    hide_hints: function () {
      var old = document.querySelector("[data-relay-hints]");
      if (old) old.remove();
      return true;
    },
    page_info: function () {
      return { url: location.href, title: document.title, scroll_x: window.scrollX, scroll_y: window.scrollY };
    }
  };

  function exec(cmd) {
    var reply = { kind: "result", id: cmd.id, ok: true, value: null, error: null };
    try {
      var fn = ops[cmd.op];
      if (!fn) throw new Error("unknown op " + cmd.op);
      reply.value = fn(cmd);
    } catch (e) {
      reply.ok = false;
      reply.error = String(e && e.message || e);
    }
    post(reply);
    return reply;
  }

  // ------------------------------------------------------------ Hotkeys
  // With the page focused, WebKit receives keys; forward Relay's bare-key
  // shortcuts unless the user is typing.
  window.addEventListener("keydown", function (ev) {
    if (ev.metaKey || ev.ctrlKey || ev.altKey || ev.isComposing || ev.key !== "f") return;
    var a = document.activeElement;
    if (a && (a.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(a.tagName))) return;
    ev.preventDefault();
    ev.stopPropagation();
    post({ kind: "hotkey", key: ev.key });
  }, true);

  // ---------------------------------------------------------- Lifecycle
  function announce() { post({ kind: "page", url: location.href, title: document.title }); }
  ["pushState", "replaceState"].forEach(function (m) {
    var orig = history[m];
    history[m] = function () { var r = orig.apply(this, arguments); announce(); return r; };
  });
  window.addEventListener("popstate", announce);
  window.addEventListener("hashchange", announce);

  function start() {
    announce();
    snapshot();
    detectChallenge();
    observer.observe(document.documentElement, { subtree: true, childList: true, attributes: true, characterData: true });
    var titleEl = document.querySelector("title");
    if (titleEl) new MutationObserver(announce).observe(titleEl, { childList: true, characterData: true, subtree: true });
  }

  Object.defineProperty(window, "__relay", {
    value: Object.freeze({
      exec: exec,
      drain: function () { var q = queue; queue = []; return q; },
      version: 1
    }),
    enumerable: false
  });

  post({ kind: "hello", url: location.href, version: 1 });
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start, { once: true });
  else start();
})();
