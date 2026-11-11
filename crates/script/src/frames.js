// Nested browsing contexts as seen from one realm: WindowProxy objects for
// other frames (`parent`, `top`, `frames[i]`, `iframe.contentWindow`,
// `event.source`), cross-realm `postMessage` with structured cloning.
// Evaluated once per realm; `post(targetId, data, targetOrigin)` and
// `info(id, what, index)` are natives implemented by the engine.
(function (post, info) {
  'use strict';
  const SELF = info(-1, 'self', 0);
  const ids = new WeakMap();
  const proxies = new Map();
  // ECMAScript globals a same-origin frame's WindowProxy exposes (served
  // from this realm, see windowFor).
  const BUILTINS = new Set(['JSON', 'Math', 'Reflect', 'Atomics', 'Intl', 'Object', 'Function', 'Array', 'String', 'Number', 'Boolean',
    'Symbol', 'BigInt', 'Date', 'RegExp', 'Error', 'TypeError', 'RangeError', 'SyntaxError', 'ReferenceError', 'EvalError', 'URIError',
    'AggregateError', 'Map', 'Set', 'WeakMap', 'WeakSet', 'WeakRef', 'FinalizationRegistry', 'Promise', 'Proxy', 'ArrayBuffer',
    'SharedArrayBuffer', 'DataView', 'Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array', 'Int32Array',
    'Uint32Array', 'Float32Array', 'Float64Array', 'BigInt64Array', 'BigUint64Array', 'parseInt', 'parseFloat', 'isNaN', 'isFinite',
    'encodeURI', 'encodeURIComponent', 'decodeURI', 'decodeURIComponent', 'escape', 'unescape', 'NaN', 'Infinity', 'undefined',
    'Iterator']);
  const sameOrigin = (id) => info(id, 'document', 0) !== null;

  function cloneError(what) {
    const msg = "Failed to execute 'postMessage': " + what + ' could not be cloned.';
    return typeof DOMException === 'function' ? new DOMException(msg, 'DataCloneError') : new TypeError(msg);
  }

  // ---- structured clone (serialize to a JSON string, rebuild in the target realm)
  function serialize(value) {
    const seen = new Map();
    let next = 0;
    function enc(v) {
      switch (typeof v) {
        case 'undefined': return { t: 'u' };
        case 'boolean': case 'string': return v;
        case 'number':
          if (Number.isFinite(v) && !Object.is(v, -0)) return v;
          return { t: 'n', v: String(Object.is(v, -0) ? '-0' : v) };
        case 'bigint': return { t: 'b', v: v.toString() };
        case 'symbol': throw cloneError('Symbol()');
        case 'function': throw cloneError(String(v).slice(0, 40));
      }
      if (v === null) return null;
      if (ids.has(v)) throw cloneError('#<Window>');
      if (seen.has(v)) return { t: 'ref', id: seen.get(v) };
      const id = next++;
      seen.set(v, id);
      const tag = Object.prototype.toString.call(v).slice(8, -1);
      switch (tag) {
        case 'Array': return { t: 'a', id, v: Array.prototype.map.call(v, enc), l: v.length };
        case 'Date': return { t: 'd', id, v: v.getTime() };
        case 'RegExp': return { t: 'r', id, s: v.source, f: v.flags };
        case 'Boolean': case 'Number': case 'String': return { t: 'w', id, k: tag, v: enc(v.valueOf()) };
        case 'Map': { const e = []; v.forEach((val, key) => e.push([enc(key), enc(val)])); return { t: 'm', id, v: e }; }
        case 'Set': { const e = []; v.forEach((val) => e.push(enc(val))); return { t: 's', id, v: e }; }
        case 'ArrayBuffer': return { t: 'ab', id, v: Array.from(new Uint8Array(v)) };
        case 'Error': return { t: 'e', id, n: String(v.name), m: String(v.message) };
      }
      if (typeof ArrayBuffer === 'function' && ArrayBuffer.isView && ArrayBuffer.isView(v)) {
        const bytes = new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
        return { t: 'ta', id, k: tag, v: Array.from(bytes) };
      }
      if (typeof Node === 'function' && v instanceof Node) throw cloneError(tag);
      if (typeof Blob === 'function' && v instanceof Blob) throw cloneError(tag);
      const props = [];
      for (const k of Object.keys(v)) props.push([k, enc(v[k])]);
      return { t: 'o', id, v: props };
    }
    return JSON.stringify(enc(value));
  }

  function deserialize(text) {
    const objs = new Map();
    function dec(x) {
      if (x === null || typeof x !== 'object') return x;
      let out;
      switch (x.t) {
        case 'u': return undefined;
        case 'n': return x.v === '-0' ? -0 : Number(x.v);
        case 'b': return BigInt(x.v);
        case 'ref': return objs.get(x.id);
        case 'a':
          out = new Array(x.l || 0); objs.set(x.id, out);
          x.v.forEach((e, i) => { out[i] = dec(e); });
          return out;
        case 'd': out = new Date(x.v); break;
        case 'r': out = new RegExp(x.s, x.f); break;
        case 'w': out = Object(dec(x.v)); break;
        case 'm': out = new Map(); objs.set(x.id, out); for (const [k, v] of x.v) out.set(dec(k), dec(v)); return out;
        case 's': out = new Set(); objs.set(x.id, out); for (const v of x.v) out.add(dec(v)); return out;
        case 'ab': out = new Uint8Array(x.v).buffer; break;
        case 'ta': {
          const C = globalThis[x.k] || Uint8Array;
          const buf = new Uint8Array(x.v).buffer;
          out = x.k === 'DataView' ? new DataView(buf) : new C(buf);
          break;
        }
        case 'e': {
          const C = globalThis[x.n];
          out = typeof C === 'function' && C.prototype instanceof Error || C === Error ? new C(x.m) : new Error(x.m);
          break;
        }
        case 'o':
          out = {}; objs.set(x.id, out);
          for (const [k, v] of x.v) out[k] = dec(v);
          return out;
        default: return undefined;
      }
      objs.set(x.id, out);
      return out;
    }
    return dec(JSON.parse(text));
  }

  // ---- WindowProxy for another frame
  function targetOriginOf(arg) {
    if (arg && typeof arg === 'object') return arg.targetOrigin === undefined ? '/' : String(arg.targetOrigin);
    return arg === undefined ? '/' : String(arg);
  }

  function windowFor(id) {
    if (id === undefined || id === null || id < 0) return null;
    if (id === SELF) return globalThis;
    let p = proxies.get(id);
    if (p) return p;
    const location = {
      get href() { return info(id, 'href', 0); },
      set href(v) { info(id, 'navigate', String(v)); },
      assign(v) { info(id, 'navigate', String(v)); },
      replace(v) { info(id, 'navigate', String(v)); },
      toString() { return info(id, 'href', 0); },
    };
    const target = {
      postMessage(message, targetOrigin, transfer) {
        const list = transfer || (targetOrigin && typeof targetOrigin === 'object' ? targetOrigin.transfer : null);
        const ports = Array.isArray(list) ? list.filter((t) => typeof MessagePort === 'function' && t instanceof MessagePort) : [];
        const data = serialize(message);
        post(id, JSON.stringify({ t: 'msg', d: data, ports: ports.map((port) => bridgePort(port, id)) }), targetOriginOf(targetOrigin));
      },
      focus() {}, blur() {}, close() {},
      get window() { return p; },
      get self() { return p; },
      get frames() { return p; },
      get parent() { const i = info(id, 'parent', 0); return i < 0 ? p : windowFor(i); },
      get top() { return windowFor(info(id, 'top', 0)); },
      get opener() { return null; },
      get length() { return info(id, 'length', 0); },
      get closed() { return info(id, 'closed', 0); },
      get location() { return location; },
      get document() { return info(id, 'document', 0); },
      // Same-origin frames: style queries of the frame's document.
      getComputedStyle(el, pseudo) { return g.getComputedStyle(el, pseudo); },
      set location(v) { info(id, 'navigate', String(v)); },
    };
    Object.defineProperty(target, Symbol.toStringTag, { value: 'Window' });
    p = new Proxy(target, {
      get(t, key) {
        if (typeof key === 'string' && key !== '' && String(Number(key) >>> 0) === key) {
          const child = info(id, 'child', Number(key));
          return child < 0 ? undefined : windowFor(child);
        }
        if (key in t) return t[key];
        // A same-origin frame's own globals (JSON, Array, ...): the frame
        // runs in another realm, so this realm's pristine built-ins stand
        // in for them (scripts take JSON / Array from a fresh about:blank
        // iframe to avoid a page's patched ones).
        if (BUILTINS.has(key) && sameOrigin(id)) return g[key];
        if (typeof key === 'string' && key !== '') {
          // Named access to a child frame (`parent.frames['name']`).
          const child = info(id, 'named', key);
          if (child >= 0) return windowFor(child);
        }
        return undefined;
      },
      has(t, key) {
        return key in t || (BUILTINS.has(key) && sameOrigin(id));
      },
      set(t, key, v) {
        if (key === 'location') { info(id, 'navigate', String(v)); return true; }
        return true;
      },
    });
    ids.set(p, id);
    proxies.set(id, p);
    return p;
  }

  // ---- this window's frame accessors
  const g = globalThis;
  const own = {
    parent: () => { const i = info(SELF, 'parent', 0); return i < 0 ? g : windowFor(i); },
    top: () => windowFor(info(SELF, 'top', 0)) || g,
    length: () => info(SELF, 'length', 0),
  };
  for (const k of Object.keys(own)) {
    Object.defineProperty(g, k, { get: own[k], set(v) { Object.defineProperty(g, k, { value: v, writable: true, configurable: true }); }, configurable: true });
  }
  // Named access to child frames (`window.frames['name']`, `window.name`
  // of a child): getters on the global for the current children's names,
  // refreshed whenever `frames` / `length` is read.
  const named = new Set();
  function defineNamed() {
    const n = info(SELF, 'length', 0);
    for (let i = 0; i < n; i++) {
      const name = info(info(SELF, 'child', i), 'name', 0);
      if (typeof name !== 'string' || name === '' || named.has(name) || name in g) continue;
      named.add(name);
      Object.defineProperty(g, name, {
        get() { const c = info(SELF, 'named', name); return c < 0 ? undefined : windowFor(c); },
        set(v) { named.delete(name); Object.defineProperty(g, name, { value: v, writable: true, configurable: true, enumerable: true }); },
        configurable: true,
      });
    }
  }
  const framesDesc = Object.getOwnPropertyDescriptor(g, 'frames');
  if (!framesDesc || framesDesc.configurable) {
    Object.defineProperty(g, 'frames', {
      get() { defineNamed(); return g; },
      set(v) { Object.defineProperty(g, 'frames', { value: v, writable: true, configurable: true }); },
      configurable: true,
    });
  }
  const lengthGet = own.length;
  Object.defineProperty(g, 'length', {
    get() { defineNamed(); return lengthGet(); },
    set(v) { Object.defineProperty(g, 'length', { value: v, writable: true, configurable: true }); },
    configurable: true,
  });
  for (let i = 0; i < 16; i++) {
    Object.defineProperty(g, String(i), {
      get() { const c = info(SELF, 'child', i); return c < 0 ? undefined : windowFor(c); },
      configurable: true,
    });
  }

  // The serializer, for the worker message routing (messaging.js,
  // worker.js; removed from the global after startup).
  Object.defineProperty(g, '__rhClone', { value: { serialize, deserialize }, configurable: true });

  // structuredClone with the same algorithm as postMessage.
  Object.defineProperty(g, 'structuredClone', {
    value: function structuredClone(value) { return deserialize(serialize(value)); },
    writable: true, configurable: true,
  });

  // ---- MessagePorts transferred to another frame: each end keeps a local
  // port and the engine carries messages between the two realms.
  const bridges = new Map();
  let nextBridge = 0;
  function forward(port, bridge, frame) {
    port.onmessage = (ev) => post(frame, JSON.stringify({ t: 'port', id: bridge, d: serialize(ev.data) }), '*');
  }
  // `port` (ours) goes to frame `frame`: messages arriving on it are sent
  // there; messages from there are posted into it.
  function bridgePort(port, frame) {
    const bridge = SELF + ':' + nextBridge++;
    bridges.set(bridge, port);
    forward(port, bridge, frame);
    return bridge;
  }

  function deliver(text, origin, source) {
    let env;
    try { env = JSON.parse(text); } catch (e) { return; }
    if (env && env.t === 'port') {
      const port = bridges.get(env.id);
      if (port) port.postMessage(deserialize(env.d));
      return;
    }
    let value;
    try { value = deserialize(env.d); } catch (e) { value = null; }
    const ports = (env.ports || []).map((bridge) => {
      const ch = new MessageChannel();
      bridges.set(bridge, ch.port2);
      forward(ch.port2, bridge, source);
      return ch.port1;
    });
    const ev = new MessageEvent('message', { data: value, origin, source: windowFor(source), ports });
    g.dispatchEvent(ev);
  }

  return { serialize, deserialize, windowFor, deliver };
})
