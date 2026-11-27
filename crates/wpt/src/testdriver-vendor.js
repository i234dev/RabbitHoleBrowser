// RabbitHole WPT runner: test_driver automation. Requests go to the runner
// as console lines ("@@TD@@{json}"); it performs them with the engine's
// input pipeline (trusted pointer and key events) and settles the promise
// through __rhTestDriverDone(id, error).
(function () {
  "use strict";
  const internal = window.test_driver_internal;
  if (!internal) return;
  const pending = new Map();
  let seq = 0;
  // A browsing context can navigate while WebDriver has a pending request.
  const realm = Math.random().toString(36).slice(2);
  self.__rhTestDriverRealm = realm;
  function contextPath(win) {
    const path = [];
    while (win !== win.top) {
      const parent = win.parent;
      let i = 0;
      while (i < parent.length && parent.frames[i] !== win) ++i;
      if (i === parent.length) throw new Error("Browsing context was removed");
      path.unshift(i);
      win = parent;
    }
    return path;
  }

  // Same-origin WindowProxy reads in RabbitHole can copy data and call a
  // global function. They cannot copy test_driver, an object of functions.
  // Expose the caller's WebDriver client for documents that loaded their
  // own vendor script, with actions addressed to the requested context.
  document.addEventListener("load", function (event) {
    const frame = event.target;
    if (frame.localName !== "iframe" && frame.localName !== "frame") return;
    const win = frame.contentWindow;
    if (!win || !win.document || !win.__rhTestDriverRealm) return;
    const installed = win.__rhTestDriverRealm;
    const client = Object.create(self.test_driver);
    if (self.test_driver.Actions) {
      client.Actions = class extends self.test_driver.Actions {
        constructor(...args) { super(...args); this.setContext(win); }
      };
    }
    client.action_sequence = (actions, context) => internal.action_sequence(actions, context || win);
    Object.defineProperty(win, "test_driver", {
      configurable: true,
      get() { return win.document && win.__rhTestDriverRealm === installed ? client : undefined; },
    });
  }, true);
  self.__rhTestDriverDone = function (id, error) {
    const p = pending.get(id);
    if (!p) return;
    pending.delete(id);
    if (error) p.reject(new Error(error)); else p.resolve();
  };
  function request(action, data, target = self) {
    const id = ++seq;
    return new Promise(function (resolve, reject) {
      pending.set(id, { resolve: resolve, reject: reject });
      console.log("@@TD@@" + JSON.stringify(Object.assign({ id, action, sender: contextPath(self), realm, target: contextPath(target), targetRealm: target.__rhTestDriverRealm }, data)));
    });
  }
  function centerOf(el) {
    const r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  }
  internal.in_automation = true;
  internal.click = function (element, coords) {
    const c = coords || centerOf(element);
    return request("click", { x: c.x, y: c.y }, element.ownerDocument.defaultView);
  };
  internal.send_keys = function (element, keys) {
    try { element.focus(); } catch (e) {}
    return request("keys", { keys: String(keys) }, element.ownerDocument.defaultView);
  };
  // WebDriver actions: element origins become viewport coordinates here.
  internal.action_sequence = function (actions, context) {
    const sources = (actions || []).map(function (src) {
      return {
        type: src.type,
        actions: (src.actions || []).map(function (a) {
          const out = Object.assign({}, a);
          if (a.type === "pointerMove" || a.type === "scroll") {
            let x = a.x || 0, y = a.y || 0;
            if (a.origin && typeof a.origin === "object" && a.origin.getBoundingClientRect) {
              const c = centerOf(a.origin);
              x += c.x; y += c.y;
              out.origin = "viewport";
            } else if (a.origin === "pointer") {
              out.origin = "pointer";
            } else {
              out.origin = "viewport";
            }
            out.x = x; out.y = y;
          }
          return out;
        }),
      };
    });
    return request("actions", { sources: sources }, context || self);
  };
})();
