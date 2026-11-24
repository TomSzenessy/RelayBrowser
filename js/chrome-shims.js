// Chrome API shims for WKWebExtension contexts. WebKit implements the
// `browser.*` WebExtensions API; these cover the Chrome-only surfaces
// popular extensions call. Native side: relay-ui handles "relay.*" messages
// sent through browser.runtime.sendNativeMessage.
(function () {
  "use strict";
  var g = typeof globalThis !== "undefined" ? globalThis : self;
  var b = g.browser;
  if (!b) return;
  if (!g.chrome) g.chrome = b;
  var chrome = g.chrome;

  function native(type, payload) {
    try {
      return b.runtime.sendNativeMessage("relay", Object.assign({ type: type }, payload || {}));
    } catch (e) {
      return Promise.reject(e);
    }
  }
  function cb(promise, callback) {
    if (typeof callback === "function") promise.then(function (v) { callback(v); }, function () { callback(); });
    return promise;
  }

  // Chrome extensions call the callback form far more often than the promise
  // form (`chrome.storage.local.get(keys, cb)`). WebKit's `browser.*` is
  // promise-only, so adapt the common namespaces without touching listeners.
  var lastError = null;
  try {
    Object.defineProperty(b.runtime || {}, "lastError", {
      configurable: true,
      get: function () { return lastError; }
    });
  } catch (e) {}
  function callbackForm(target, name) {
    if (!target || typeof target[name] !== "function" || target[name].__relayCallback) return;
    var original = target[name];
    var wrapped = function () {
      var args = Array.prototype.slice.call(arguments);
      var callback = typeof args[args.length - 1] === "function" ? args.pop() : null;
      var result;
      try {
        result = original.apply(target, args);
      } catch (e) {
        if (!callback) throw e;
        lastError = { message: String(e && e.message || e) };
        callback(undefined);
        lastError = null;
        return;
      }
      if (!callback) return result;
      var promise = result && typeof result.then === "function" ? result : Promise.resolve(result);
      promise.then(function (value) {
        lastError = null;
        try { callback(value); } finally { lastError = null; }
      }, function (e) {
        lastError = { message: String(e && e.message || e) };
        try { callback(undefined); } finally { lastError = null; }
      });
      return undefined;
    };
    wrapped.__relayCallback = true;
    target[name] = wrapped;
  }
  function adaptNamespace(root, methods) {
    if (!root) return;
    for (var name in methods) {
      var target = root;
      var parts = name.split(".");
      for (var i = 0; i < parts.length - 1 && target; i++) target = target[parts[i]];
      if (target) callbackForm(target, parts[parts.length - 1]);
    }
  }
  adaptNamespace(b, {
    "storage.local.get": 1, "storage.local.set": 1, "storage.local.remove": 1, "storage.local.clear": 1,
    "storage.sync.get": 1, "storage.sync.set": 1, "storage.sync.remove": 1, "storage.sync.clear": 1,
    "storage.session.get": 1, "storage.session.set": 1, "storage.session.remove": 1,
    "runtime.sendMessage": 1,
    "tabs.query": 1, "tabs.create": 1, "tabs.update": 1, "tabs.remove": 1, "tabs.get": 1, "tabs.sendMessage": 1,
    "scripting.executeScript": 1, "scripting.insertCSS": 1, "scripting.removeCSS": 1,
    "permissions.request": 1, "permissions.contains": 1, "permissions.remove": 1,
    "windows.create": 1, "windows.get": 1, "windows.getAll": 1, "windows.update": 1, "windows.remove": 1
  });

  // chrome.sidePanel -> a GPUI pane beside the page.
  if (!chrome.sidePanel) {
    var behavior = { openPanelOnActionClick: false };
    var options = {};
    chrome.sidePanel = {
      setOptions: function (o, callback) {
        options = Object.assign(options, o);
        return cb(native("relay.sidePanel.setOptions", { options: options }), callback);
      },
      getOptions: function (o, callback) { return cb(Promise.resolve(Object.assign({}, options, o)), callback); },
      open: function (o, callback) { return cb(native("relay.sidePanel.open", { options: Object.assign({}, options, o) }), callback); },
      setPanelBehavior: function (bh, callback) {
        behavior = Object.assign(behavior, bh);
        return cb(native("relay.sidePanel.setPanelBehavior", { behavior: behavior }), callback);
      },
      getPanelBehavior: function (callback) { return cb(Promise.resolve(behavior), callback); }
    };
    if (b.action && b.action.onClicked) {
      b.action.onClicked.addListener(function (tab) {
        if (behavior.openPanelOnActionClick) native("relay.sidePanel.open", { options: options, tabId: tab && tab.id });
      });
    }
  }

  // chrome.offscreen -> a hidden iframe in the background document when one
  // exists (MV3 background pages in WebKit), else a native hidden webview.
  if (!chrome.offscreen) {
    var frame = null;
    chrome.offscreen = {
      Reason: { AUDIO_PLAYBACK: "AUDIO_PLAYBACK", CLIPBOARD: "CLIPBOARD", DOM_PARSER: "DOM_PARSER",
        DOM_SCRAPING: "DOM_SCRAPING", BLOBS: "BLOBS", LOCAL_STORAGE: "LOCAL_STORAGE", WORKERS: "WORKERS" },
      createDocument: function (params, callback) {
        if (frame) return cb(Promise.reject(new Error("Only a single offscreen document may be created.")), callback);
        var url = b.runtime.getURL(params.url);
        if (typeof document !== "undefined" && document.body) {
          frame = document.createElement("iframe");
          frame.style.display = "none";
          frame.src = url;
          document.body.appendChild(frame);
          return cb(Promise.resolve(), callback);
        }
        frame = { native: true };
        return cb(native("relay.offscreen.create", { url: url, reasons: params.reasons }), callback);
      },
      closeDocument: function (callback) {
        var f = frame;
        frame = null;
        if (f && f.remove) { f.remove(); return cb(Promise.resolve(), callback); }
        return cb(f ? native("relay.offscreen.close") : Promise.resolve(), callback);
      },
      hasDocument: function (callback) { return cb(Promise.resolve(!!frame), callback); }
    };
  }

  // Older extensions probe these; WebKit lacks them.
  if (chrome.runtime && !chrome.runtime.getPlatformInfo) {
    chrome.runtime.getPlatformInfo = function (callback) {
      return cb(Promise.resolve({ os: "mac", arch: "arm", nacl_arch: "arm" }), callback);
    };
  }
})();
