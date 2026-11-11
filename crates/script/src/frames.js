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
      postMessage(message, targetOrigin) {
        post(id, serialize(message), targetOriginOf(targetOrigin));
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
      set location(v) { info(id, 'navigate', String(v)); },
    };
    Object.defineProperty(target, Symbol.toStringTag, { value: 'Window' });
    p = new Proxy(target, {
      get(t, key) {
        if (typeof key === 'string' && /^(0|[1-9][0-9]*)$/.test(key)) {
          const child = info(id, 'child', Number(key));
          return child < 0 ? undefined : windowFor(child);
        }
        const v = t[key];
        return v;
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
  for (let i = 0; i < 16; i++) {
    Object.defineProperty(g, String(i), {
      get() { const c = info(SELF, 'child', i); return c < 0 ? undefined : windowFor(c); },
      configurable: true,
    });
  }

  // ---- MessageChannel / MessagePort / BroadcastChannel (this realm)
  if (typeof MessageChannel !== 'function' && typeof EventTarget === 'function') {
    const KEY = {};
    const portState = new WeakMap();
    const pending = [];
    let flushScheduled = false;
    function flush() {
      flushScheduled = false;
      const batch = pending.splice(0, pending.length);
      for (const [target, ev] of batch) {
        try { target.dispatchEvent(ev); } catch (e) { setTimeout(() => { throw e; }, 0); }
      }
    }
    function enqueue(target, ev) {
      pending.push([target, ev]);
      if (!flushScheduled) { flushScheduled = true; setTimeout(flush, 0); }
    }
    function handlerProp(proto, name, onSet) {
      const handlers = new WeakMap();
      Object.defineProperty(proto, 'on' + name, {
        configurable: true, enumerable: true,
        get() { const h = handlers.get(this); return h ? h.fn : null; },
        set(fn) {
          const old = handlers.get(this);
          if (old) this.removeEventListener(name, old.wrapper);
          if (typeof fn === 'function') {
            const wrapper = (ev) => fn.call(this, ev);
            handlers.set(this, { fn, wrapper });
            this.addEventListener(name, wrapper);
            if (onSet) onSet(this);
          } else handlers.delete(this);
        },
      });
    }
    class MessagePort extends EventTarget {
      constructor(key) {
        if (key !== KEY) throw new TypeError('Illegal constructor');
        super();
        portState.set(this, { other: null, started: false, closed: false, queue: [] });
      }
      postMessage(message, transfer) {
        const me = portState.get(this);
        if (!me || me.closed || !me.other) return;
        const list = Array.isArray(transfer) ? transfer : (transfer && Array.isArray(transfer.transfer) ? transfer.transfer : []);
        const ports = list.filter((t) => t instanceof MessagePort);
        const data = deserialize(serialize(message));
        const ev = new MessageEvent('message', { data, ports, origin: '' });
        const other = portState.get(me.other);
        if (!other || other.closed) return;
        if (other.started) enqueue(me.other, ev); else other.queue.push(ev);
      }
      start() {
        const me = portState.get(this);
        if (!me || me.started) return;
        me.started = true;
        for (const ev of me.queue.splice(0, me.queue.length)) enqueue(this, ev);
      }
      close() {
        const me = portState.get(this);
        if (me) { me.closed = true; me.queue.length = 0; }
      }
    }
    handlerProp(MessagePort.prototype, 'message', (port) => port.start());
    handlerProp(MessagePort.prototype, 'messageerror');
    Object.defineProperty(MessagePort.prototype, Symbol.toStringTag, { value: 'MessagePort', configurable: true });
    class MessageChannel {
      constructor() {
        const a = new MessagePort(KEY), b = new MessagePort(KEY);
        portState.get(a).other = b;
        portState.get(b).other = a;
        Object.defineProperty(this, 'port1', { value: a, enumerable: true });
        Object.defineProperty(this, 'port2', { value: b, enumerable: true });
      }
    }
    const channels = new Map();
    class BroadcastChannel extends EventTarget {
      constructor(name) {
        super();
        Object.defineProperty(this, 'name', { value: String(name), enumerable: true });
        this._closed = false;
        Object.defineProperty(this, '_closed', { enumerable: false, writable: true });
        if (!channels.has(this.name)) channels.set(this.name, new Set());
        channels.get(this.name).add(this);
      }
      postMessage(message) {
        if (this._closed) throw new DOMException('BroadcastChannel is closed', 'InvalidStateError');
        const text = serialize(message);
        for (const other of channels.get(this.name) || []) {
          if (other !== this && !other._closed) enqueue(other, new MessageEvent('message', { data: deserialize(text), origin: location.origin }));
        }
      }
      close() { this._closed = true; const set = channels.get(this.name); if (set) set.delete(this); }
    }
    handlerProp(BroadcastChannel.prototype, 'message');
    handlerProp(BroadcastChannel.prototype, 'messageerror');
    for (const C of [MessagePort, MessageChannel, BroadcastChannel]) {
      Object.defineProperty(g, C.name, { value: C, writable: true, configurable: true });
    }
  }

  // structuredClone with the same algorithm as postMessage.
  Object.defineProperty(g, 'structuredClone', {
    value: function structuredClone(value) { return deserialize(serialize(value)); },
    writable: true, configurable: true,
  });

  function deliver(data, origin, source) {
    let value;
    try { value = deserialize(data); } catch (e) { value = null; }
    const ev = new MessageEvent('message', { data: value, origin, source: windowFor(source), ports: [] });
    g.dispatchEvent(ev);
  }

  return { serialize, deserialize, windowFor, deliver };
})
