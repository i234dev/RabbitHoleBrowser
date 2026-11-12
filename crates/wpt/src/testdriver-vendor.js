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
  self.__rhTestDriverDone = function (id, error) {
    const p = pending.get(id);
    if (!p) return;
    pending.delete(id);
    if (error) p.reject(new Error(error)); else p.resolve();
  };
  function request(action, data) {
    const id = ++seq;
    return new Promise(function (resolve, reject) {
      pending.set(id, { resolve: resolve, reject: reject });
      console.log("@@TD@@" + JSON.stringify(Object.assign({ id: id, action: action }, data)));
    });
  }
  function centerOf(el) {
    const r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  }
  internal.in_automation = true;
  internal.click = function (element, coords) {
    const c = coords || centerOf(element);
    return request("click", { x: c.x, y: c.y });
  };
  internal.send_keys = function (element, keys) {
    try { element.focus(); } catch (e) {}
    return request("keys", { keys: String(keys) });
  };
  // WebDriver actions: element origins become viewport coordinates here.
  internal.action_sequence = function (actions) {
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
    return request("actions", { sources: sources });
  };
})();
