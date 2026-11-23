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
  var SKIP = { SCRIPT: 1, STYLE: 1, NOSCRIPT: 1, TEMPLATE: 1, META: 1, LINK: 1, TITLE: 1 };
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
      try { el.setAttribute("data-relay-id", String(id)); } catch (e) { /* svg quirks */ }
    }
    // A node can be detached and reattached; its id survives, and the byId
    // map must point at it again for click/type/scroll commands.
    if (byId.get(id) !== el) byId.set(id, el);
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

  // A field whose declared purpose is a secret. Passwords, card fields,
  // one-time codes, and an explicit data-relay-private opt-in keep their
  // value out of every snapshot, outline, and command reply. The element
  // still exposes id/tag/type/name/label so agents can click or type it.
  function isSecret(el) {
    if (!el || el.tagName !== "INPUT") return false;
    if (String(el.type || "").toLowerCase() === "password") return true;
    if (el.hasAttribute && el.hasAttribute("data-relay-private")) return true;
    var ac = String(el.getAttribute("autocomplete") || "").toLowerCase();
    return ac.indexOf("cc-") === 0 || ac === "one-time-code";
  }

  function attrsOf(el) {
    var attrs = {};
    var secret = isSecret(el);
    for (var i = 0; i < KEEP_ATTRS.length; i++) {
      var a = KEEP_ATTRS[i];
      var v = a === "value" && "value" in el && !secret ? el.value : el.getAttribute(a);
      if (a === "value" && secret) v = null;
      if (v !== null && v !== undefined && v !== "") attrs[a] = String(v).slice(0, MAX_TEXT);
    }
    return attrs;
  }

  function serialize(el, budget) {
    if (SKIP[el.tagName]) return null;
    if (budget.n <= 0) {
      budget.dropped = true;
      return null;
    }
    budget.n--;
    var attrs = attrsOf(el);
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
    // The document element is the root so html/head attribute changes can be
    // patched by id instead of forcing a snapshot; HEAD is a node with attrs
    // but its title/meta/link children are skipped.
    var root = document.documentElement;
    if (!root) return;
    post({ kind: "dom_snapshot", root: serialize(root, { n: MAX_NODES }) });
  }

  // Mutation diffs: re-serialize the nearest known ancestor of each changed
  // subtree, coalesced per timer tick. Attribute-only changes travel as a
  // tiny `dom_attrs` message, so toggling a body class does not resend the
  // whole document. Rust replaces subtrees by id and attrs by id.
  var dirty = new Set();
  var attrDirty = new Set();
  var removed = [];
  var scheduled = false;
  function schedule() {
    if (scheduled) return;
    scheduled = true;
    // setTimeout, not requestAnimationFrame: WebKit suspends rAF in pages
    // that are not on screen (background and agent tabs), which would stall
    // every patch. Timers are only throttled there.
    setTimeout(flush, 16);
  }
  function flush() {
    scheduled = false;
    if (!document.body) return;
    var attrs = [];
    attrDirty.forEach(function (el) {
      if (!el.isConnected) return;
      var id = idOf.get(el);
      if (!id) {
        // Never serialized: its parent needs a subtree patch to introduce it.
        dirty.add(el.parentElement || el);
        return;
      }
      attrs.push({ id: id, attrs: attrsOf(el) });
    });
    attrDirty.clear();

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
    var dropped = false;
    roots.forEach(function (el) {
      if (!idOf.get(el)) {
        // A newly inserted root with no id cannot be attached to a parent by
        // id alone; a coherent snapshot is the only safe answer.
        dropped = true;
        return;
      }
      var n = serialize(el, budget);
      if (n) nodes.push(n);
      else dropped = true;
      if (budget.dropped) dropped = true;
    });
    var gone = removed;
    removed = [];
    if (dropped) {
      snapshot();
      return;
    }
    if (attrs.length) post({ kind: "dom_attrs", nodes: attrs });
    if (nodes.length || gone.length) post({ kind: "dom_patch", nodes: nodes, removed: gone });
  }
  function markDirty(el) {
    if (!el || el.nodeType !== 1) return;
    dirty.add(el);
    schedule();
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
      if (r.type === "attributes") {
        if (r.attributeName === "data-relay-id") continue;
        if (r.target && r.target.nodeType === 1) {
          attrDirty.add(r.target);
          schedule();
        }
        continue;
      }
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

  // ----------------------------------------------------------- Markdown
  var MD_CAP = 200 * 1024;
  var MD_SKIP = { SCRIPT: 1, STYLE: 1, NOSCRIPT: 1, TEMPLATE: 1, HEAD: 1, NAV: 1, ASIDE: 1, FOOTER: 1,
    IFRAME: 1, SVG: 1, CANVAS: 1, OBJECT: 1, EMBED: 1, SELECT: 1, BUTTON: 1, INPUT: 1, TEXTAREA: 1, META: 1, LINK: 1 };
  var MD_BLOCK = { P: 1, DIV: 1, SECTION: 1, MAIN: 1, ARTICLE: 1, HEADER: 1, FIGURE: 1, FIGCAPTION: 1,
    DETAILS: 1, SUMMARY: 1, ADDRESS: 1, DL: 1, DT: 1, DD: 1, FORM: 1, FIELDSET: 1, CENTER: 1 };

  function mdHidden(el) {
    if (el.hidden || el.getAttribute("aria-hidden") === "true") return true;
    if (el.hasAttribute("data-relay-overlay") || el.hasAttribute("data-relay-hints")) return true;
    var cs = getComputedStyle(el);
    return cs.display === "none" || cs.visibility === "hidden";
  }
  function mdInline(el) { return mdChildren(el).replace(/\s*\n\s*/g, " ").replace(/ {2,}/g, " ").trim(); }
  function mdCell(el) { return mdInline(el).replace(/\|/g, "\\|"); }
  function mdTable(el) {
    var rows = [];
    el.querySelectorAll("tr").forEach(function (tr) {
      if (tr.closest("table") !== el) return;
      var cells = [];
      for (var c = tr.firstElementChild; c; c = c.nextElementSibling) {
        if (c.tagName === "TH" || c.tagName === "TD") cells.push(mdCell(c));
      }
      if (cells.length) rows.push(cells);
    });
    if (!rows.length) return "";
    var width = Math.max.apply(null, rows.map(function (r) { return r.length; }));
    var lines = rows.map(function (r) {
      while (r.length < width) r.push("");
      return "| " + r.join(" | ") + " |";
    });
    var sep = "|" + new Array(width + 1).join(" --- |");
    lines.splice(1, 0, sep);
    return lines.join("\n");
  }
  function mdList(el, ordered) {
    var out = [], n = 0;
    for (var li = el.firstElementChild; li; li = li.nextElementSibling) {
      if (li.tagName !== "LI" || mdHidden(li)) continue;
      n++;
      var body = mdChildren(li).trim().replace(/\n{2,}/g, "\n").replace(/\n/g, "\n  ");
      out.push((ordered ? n + ". " : "- ") + body);
    }
    return out.join("\n");
  }
  function mdPre(el) {
    var code = el.querySelector("code");
    var text = (code || el).textContent.replace(/\n+$/, "");
    var m = /(?:^|\s)language-([\w+#-]+)/.exec((code && code.className) || el.className || "");
    var fence = "```";
    while (text.indexOf(fence) !== -1) fence += "`";
    return fence + (m ? m[1] : "") + "\n" + text + "\n" + fence;
  }
  function mdNode(n) {
    if (n.nodeType === 3) return n.nodeValue.replace(/\s+/g, " ");
    if (n.nodeType !== 1) return "";
    var tag = n.tagName;
    if (MD_SKIP[tag] || mdHidden(n)) return "";
    var inner;
    switch (tag) {
      case "H1": case "H2": case "H3": case "H4": case "H5": case "H6":
        inner = mdInline(n);
        return inner ? "\n\n" + new Array(Number(tag[1]) + 1).join("#") + " " + inner + "\n\n" : "";
      case "BR": return "  \n";
      case "HR": return "\n\n---\n\n";
      case "STRONG": case "B": inner = mdChildren(n); return inner.trim() ? "**" + inner.trim() + "**" : inner;
      case "EM": case "I": inner = mdChildren(n); return inner.trim() ? "*" + inner.trim() + "*" : inner;
      case "CODE": inner = n.textContent.replace(/\s+/g, " ").trim(); return inner ? "`" + inner + "`" : "";
      case "PRE": return "\n\n" + mdPre(n) + "\n\n";
      case "A":
        inner = mdInline(n);
        if (!inner) return "";
        return n.href && !/^javascript:/i.test(n.href) ? "[" + inner + "](" + n.href + ")" : inner;
      case "IMG": return n.getAttribute("alt") && n.src ? "![" + n.getAttribute("alt") + "](" + n.src + ")" : "";
      case "UL": case "OL": inner = mdList(n, tag === "OL"); return inner ? "\n\n" + inner + "\n\n" : "";
      case "BLOCKQUOTE":
        inner = mdChildren(n).trim();
        return inner ? "\n\n" + inner.split("\n").map(function (l) { return l ? "> " + l : ">"; }).join("\n") + "\n\n" : "";
      case "TABLE": inner = mdTable(n); return inner ? "\n\n" + inner + "\n\n" : "";
      case "P": inner = mdChildren(n).trim(); return inner ? "\n\n" + inner + "\n\n" : "";
      default:
        inner = mdChildren(n);
        return MD_BLOCK[tag] ? "\n\n" + inner + "\n\n" : inner;
    }
  }
  function mdChildren(el) {
    var s = "";
    for (var c = el.firstChild; c; c = c.nextSibling) s += mdNode(c);
    return s;
  }
  function toMarkdown() {
    var root = document.querySelector("article") || document.querySelector("main") || document.body;
    var md = mdChildren(root).replace(/[ \t]+\n/g, "\n").replace(/\n[ \t]+/g, "\n").replace(/\n{3,}/g, "\n\n").trim();
    if (md) md += "\n";
    var enc = new TextEncoder();
    if (md.length > MD_CAP) md = md.slice(0, MD_CAP);
    while (enc.encode(md).length > MD_CAP) md = md.slice(0, Math.floor(md.length * 0.95));
    return md;
  }

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
    var text = isSecret(el) ? "" : (el.innerText || el.value || "");
    return { id: relayId(el), tag: el.tagName.toLowerCase(), text: String(text).slice(0, MAX_TEXT) };
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
    markdown: function () {
      var text = toMarkdown();
      post({ kind: "markdown", text: text });
      return text;
    },
    page_info: function () {
      return { url: location.href, title: document.title, scroll_x: window.scrollX, scroll_y: window.scrollY };
    },
    web_app: webApp,
    favicon: favicon,
    press_key: function (c) {
      var key = String(c.key || "");
      if (!key || key.length > 40) throw new Error("key is empty or too long");
      var mods = c.modifiers || [];
      if (!Array.isArray(mods)) throw new Error("modifiers must be an array");
      var opts = {
        key: key, bubbles: true, cancelable: true,
        ctrlKey: mods.indexOf("ctrl") !== -1, altKey: mods.indexOf("alt") !== -1,
        shiftKey: mods.indexOf("shift") !== -1, metaKey: mods.indexOf("meta") !== -1
      };
      var target = document.activeElement || document.body;
      var down = new KeyboardEvent("keydown", opts);
      target.dispatchEvent(down);
      if (key === "Enter" && target.form && !down.defaultPrevented) {
        if (target.form.requestSubmit) target.form.requestSubmit(); else target.form.submit();
      }
      target.dispatchEvent(new KeyboardEvent("keyup", opts));
      return { key: key, tag: target.tagName ? target.tagName.toLowerCase() : "", prevented: down.defaultPrevented };
    },
    storage: storageSummary,
    set_cookie: function (c) {
      var name = String(c.name || "");
      var value = String(c.value || "");
      if (!/^[A-Za-z0-9_\-.]{1,64}$/.test(name)) throw new Error("cookie name is not valid");
      if (value.length > 4096 || /[\r\n]/.test(value)) throw new Error("cookie value is too long or has a newline");
      var cookie = name + "=" + encodeURIComponent(value) + "; Path=/; SameSite=Lax";
      if (location.protocol === "https:") cookie += "; Secure";
      document.cookie = cookie;
      return { name: name, set: true };
    },
    eval_js: function (c) {
      var script = typeof c.script === "string" ? c.script.trim() : "";
      if (!script) throw new Error("script is empty");
      if (script.length > 50000) throw new Error("script is too long");
      var value = (new Function('"use strict"; return (' + script + ');'))();
      var seen = [];
      var text = JSON.stringify(value, function (k, v) {
        if (typeof v === "function") return "[function]";
        if (typeof v === "bigint") return String(v);
        if (v && typeof v === "object") {
          if (seen.indexOf(v) !== -1) return "[cycle]";
          seen.push(v);
        }
        return v;
      });
      if (typeof text !== "string") text = "null";
      if (text.length > 100000) throw new Error("result is too large");
      return { result: JSON.parse(text) };
    }
  };

  var maxIconBytes = 2 * 1024 * 1024;
  function absoluteUrl(value, base) {
    if (value === undefined || value === null || value === "") return null;
    try {
      var u = new URL(String(value), base || location.href);
      return /^https?:$/i.test(u.protocol) ? u.href : null;
    } catch (e) {
      return null;
    }
  }
  function manifestText(value, max) {
    return typeof value === "string" ? value.slice(0, max) : "";
  }
  function fetchWithTimeout(url, ms) {
    var ctrl = typeof AbortController === "function" ? new AbortController() : null;
    var timer = ctrl ? setTimeout(function () { ctrl.abort(); }, ms) : null;
    function done() { if (timer) clearTimeout(timer); }
    return fetch(url, { credentials: "same-origin", redirect: "follow", signal: ctrl ? ctrl.signal : undefined })
      .then(function (r) {
        done();
        if (!r.ok) throw new Error("HTTP " + r.status);
        return r;
      }, function (e) {
        done();
        throw e;
      });
  }
  function blobToDataUrl(blob) {
    return new Promise(function (resolve, reject) {
      if (blob.size > maxIconBytes) return reject(new Error("icon is too large"));
      if (blob.type && blob.type !== "image/png") return reject(new Error("icon is not a PNG"));
      var reader = new FileReader();
      reader.onload = function () {
        var url = String(reader.result || "");
        if (url.indexOf("data:image/png;base64,") === 0) resolve(url);
        else reject(new Error("icon is not a PNG"));
      };
      reader.onerror = function () { reject(reader.error || new Error("could not read the icon")); };
      reader.readAsDataURL(blob);
    });
  }
  function iconArea(icon) {
    var sizes = String(icon && icon.sizes || "").toLowerCase();
    if (sizes === "any") return 0;
    var m = /(\d+)x(\d+)/.exec(sizes);
    return m ? Number(m[1]) * Number(m[2]) : 0;
  }
  function pickIcon(manifest, manifestUrl) {
    var icons = Array.isArray(manifest.icons) ? manifest.icons : [];
    var usable = [];
    for (var i = 0; i < icons.length; i++) {
      var icon = icons[i];
      if (!icon || typeof icon.src !== "string" || !icon.src) continue;
      var type = manifestText(icon.type, 80).toLowerCase();
      if (type && type !== "image/png") continue;
      var src = /^data:/i.test(icon.src) ? icon.src : absoluteUrl(icon.src, manifestUrl);
      if (!src) continue;
      if (/^data:/i.test(src) && src.indexOf("data:image/png;base64,") !== 0) continue;
      usable.push({ src: src, area: iconArea(icon) });
    }
    usable.sort(function (a, b) { return b.area - a.area; });
    if (!usable.length) return Promise.resolve(null);
    var best = usable[0].src;
    if (/^data:/i.test(best)) return Promise.resolve(best);
    return fetchWithTimeout(best, 5000)
      .then(function (r) { return r.blob(); })
      .then(blobToDataUrl)
      .catch(function () { return null; });
  }
  function webApp() {
    var link = document.querySelector('link[rel~="manifest"]');
    var href = link && link.getAttribute("href");
    if (!href) return { available: false, reason: "This page does not declare a web app manifest." };
    var manifestUrl = absoluteUrl(href);
    if (!manifestUrl) return { available: false, reason: "The page's web app manifest is not an http(s) URL." };
    return fetchWithTimeout(manifestUrl, 6000)
      .then(function (r) { return r.json(); })
      .then(function (m) {
        if (!m || typeof m !== "object" || Array.isArray(m)) throw new Error("the manifest is not an object");
        var start = absoluteUrl(m.start_url || ".", manifestUrl) || location.href;
        return pickIcon(m, manifestUrl).then(function (icon) {
          return {
            available: true,
            page_url: location.href,
            name: manifestText(m.name, 200) || manifestText(m.short_name, 80) || document.title.slice(0, 200),
            short_name: manifestText(m.short_name, 80),
            start_url: start,
            scope: absoluteUrl(m.scope, manifestUrl) || "",
            display: manifestText(m.display, 40) || "standalone",
            theme_color: manifestText(m.theme_color, 40),
            background_color: manifestText(m.background_color, 40),
            manifest_url: manifestUrl,
            icon: icon
          };
        });
      })
      .catch(function (e) {
        return { available: false, reason: "Could not read the web app manifest: " + String(e && e.message || e) };
      });
  }

  function storageSummary() {
    function dump(area) {
      var values = {}, count = 0, total = 0, redacted = 0, truncated = false;
      var secret = /pass|token|secret|auth|key|credential/i;
      try {
        for (var i = 0; i < area.length; i++) {
          var k = area.key(i);
          var v = area.getItem(k) || "";
          if (count >= 200 || total + v.length > 16384) { truncated = true; break; }
          if (secret.test(k)) { v = "[redacted]"; redacted++; }
          values[k] = v;
          count++;
          total += v.length;
        }
      } catch (e) { return { values: {}, truncated: true, redacted: 0, error: String(e && e.message || e) }; }
      return { values: values, truncated: truncated, redacted: redacted };
    }
    return { local: dump(localStorage), session: dump(sessionStorage), url: location.href };
  }

  var maxFaviconBytes = 256 * 1024;
  function blobToIcon(blob) {
    return new Promise(function (resolve, reject) {
      if (blob.size > maxFaviconBytes) return reject(new Error("icon is too large"));
      var type = String(blob.type || "").toLowerCase();
      if (!/^image\/(png|jpeg|webp|gif)$/.test(type)) return reject(new Error("icon format is not supported"));
      var reader = new FileReader();
      reader.onload = function () {
        var url = String(reader.result || "");
        if (url.indexOf("data:" + type + ";base64,") !== 0) return reject(new Error("icon could not be encoded"));
        resolve({ data: url, mime: type });
      };
      reader.onerror = function () { reject(reader.error || new Error("could not read the icon")); };
      reader.readAsDataURL(blob);
    });
  }
  function faviconData(url) {
    var data = /^data:(image\/(?:png|jpeg|webp|gif));base64,([A-Za-z0-9+/=]+)$/i.exec(url);
    if (data) {
      if (data[2].length > maxFaviconBytes * 4 / 3) return Promise.reject(new Error("icon is too large"));
      return Promise.resolve({ data: url, mime: data[1].toLowerCase() });
    }
    var abs = absoluteUrl(url);
    if (!abs) return Promise.reject(new Error("icon is not an http(s) URL"));
    return fetchWithTimeout(abs, 3000)
      .then(function (r) { return r.blob(); })
      .then(blobToIcon);
  }
  function favicon() {
    var seen = Object.create(null);
    var urls = [];
    document.querySelectorAll('link[rel~="icon"], link[rel="apple-touch-icon"]').forEach(function (link) {
      if (!link.href || seen[link.href]) return;
      seen[link.href] = true;
      urls.push(link.href);
    });
    try {
      var fallback = new URL("/favicon.ico", location.href).href;
      if (!seen[fallback]) urls.push(fallback);
    } catch (e) {}
    urls = urls.slice(0, 5);
    var chain = Promise.reject(new Error("no icon candidates"));
    urls.forEach(function (url) {
      chain = chain.catch(function () { return faviconData(url); });
    });
    return chain.then(
      function (icon) { return { available: true, data: icon.data, mime: icon.mime }; },
      function () { return { available: false }; }
    );
  }

  function exec(cmd) {
    var reply = { kind: "result", id: cmd.id, ok: true, value: null, error: null };
    function fail(e) {
      reply.ok = false;
      reply.error = String(e && e.message || e);
      post(reply);
    }
    try {
      var fn = ops[cmd.op];
      if (!fn) throw new Error("unknown op " + cmd.op);
      var value = fn(cmd);
      if (value && typeof value.then === "function") {
        // Async ops (web_app reads a manifest and an icon) post their own
        // result when they settle; the synchronous reply is a placeholder.
        value.then(function (v) { reply.value = v; post(reply); }, fail);
        return reply;
      }
      reply.value = value;
    } catch (e) {
      fail(e);
      return reply;
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
