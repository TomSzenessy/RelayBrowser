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
