// Web IDL property attributes, applied once every interface is defined:
// regular operations and attributes of interfaces (on interface
// prototype objects and constructors) and of the global object are
// enumerable (Web IDL §3.7.4 / §3.7.6). Classes written in JavaScript
// define them non-enumerable; ECMAScript built-ins are left alone.
(function () {
  'use strict';
  const g = globalThis;
  const ES = new Set(['Object', 'Function', 'Array', 'Number', 'Boolean', 'String', 'Symbol', 'Date', 'Promise', 'RegExp', 'Error', 'AggregateError',
    'EvalError', 'RangeError', 'ReferenceError', 'SyntaxError', 'TypeError', 'URIError', 'JSON', 'Math', 'Intl', 'ArrayBuffer', 'SharedArrayBuffer',
    'Atomics', 'DataView', 'Map', 'Set', 'WeakMap', 'WeakSet', 'WeakRef', 'FinalizationRegistry', 'Proxy', 'Reflect', 'BigInt', 'Iterator',
    'AsyncIterator', 'Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array', 'Int32Array', 'Uint32Array', 'Float16Array',
    'Float32Array', 'Float64Array', 'BigInt64Array', 'BigUint64Array', 'WebAssembly', 'Temporal', 'DisposableStack', 'AsyncDisposableStack',
    'SuppressedError', 'ShadowRealm', 'globalThis', 'eval', 'isFinite', 'isNaN', 'parseFloat', 'parseInt', 'decodeURI', 'decodeURIComponent',
    'encodeURI', 'encodeURIComponent', 'escape', 'unescape', 'console']);
  const fix = (o) => {
    for (const k of Object.getOwnPropertyNames(o)) {
      if (k === 'constructor' || k === 'prototype' || k === 'length' || k === 'name' || k.startsWith('__')) continue;
      const d = Object.getOwnPropertyDescriptor(o, k);
      if (!d || d.enumerable || !d.configurable) continue;
      if (typeof d.value === 'function' || d.get || d.set) {
        d.enumerable = true;
        try { Object.defineProperty(o, k, d); } catch (e) { /* ignore */ }
      }
    }
  };
  for (const name of Object.getOwnPropertyNames(g)) {
    if (ES.has(name) || name.startsWith('__') || !/^[A-Z]/.test(name)) continue;
    const d = Object.getOwnPropertyDescriptor(g, name);
    const C = d && d.value;
    if (typeof C !== 'function') continue;
    const P = C.prototype;
    if (!P || typeof P !== 'object') continue;
    // Platform interfaces carry a @@toStringTag (natives and JS alike).
    if (!Object.prototype.hasOwnProperty.call(P, Symbol.toStringTag)) continue;
    fix(P);
    fix(C);
  }

  // The named properties object of Window (HTML §7.2.3.5): elements with an
  // id (and embed/form/img/object elements with a name) are reachable as
  // properties of window — and so as global variables — after the
  // window's own and inherited properties.
  const W = g.Window, doc = g.document;
  if (typeof W === 'function' && W.prototype && doc && typeof Proxy === 'function') {
    const base = Object.getPrototypeOf(W.prototype);
    const byId = Object.getOwnPropertyDescriptor(g.Document.prototype, 'getElementById');
    const byName = Object.getOwnPropertyDescriptor(g.Document.prototype, 'getElementsByName');
    const getById = byId && byId.value, getByName = byName && byName.value;
    const NAMED = new Set(['embed', 'form', 'img', 'object']);
    let busy = false;
    const named = (k) => {
      if (typeof k !== 'string' || k === '' || busy || !getById) return undefined;
      // Worker realms (set up after this) have no document.
      if (Object.prototype.hasOwnProperty.call(g, 'WorkerGlobalScope')) return undefined;
      busy = true;
      try { return lookup(k); } finally { busy = false; }
    };
    const lookup = (k) => {
      const d = doc;
      const el = getById.call(d, k);
      if (el) return el;
      if (getByName) {
        const list = getByName.call(d, k);
        for (let i = 0; i < list.length; i++) {
          const e = list[i];
          if (NAMED.has(e.localName) && e.namespaceURI === 'http://www.w3.org/1999/xhtml') return e;
        }
      }
      return undefined;
    };
    const props = new Proxy(base || Object.prototype, {
      get(t, k, r) {
        if (typeof k === 'string' && !Reflect.has(t, k)) {
          const v = named(k);
          if (v !== undefined) return v;
        }
        return Reflect.get(t, k, r);
      },
      has(t, k) { return Reflect.has(t, k) || named(k) !== undefined; },
      getOwnPropertyDescriptor(t, k) {
        const own = Reflect.getOwnPropertyDescriptor(t, k);
        if (own) return own;
        const v = named(k);
        return v === undefined ? undefined : { value: v, writable: true, enumerable: false, configurable: true };
      },
      defineProperty() { return false; },
      deleteProperty(t, k) { return named(k) === undefined && Reflect.deleteProperty(t, k); },
      setPrototypeOf() { return false; },
      preventExtensions() { return false; },
    });
    try {
      Object.setPrototypeOf(W.prototype, props);
      // Worker realms (set up after this) put it back.
      Object.defineProperty(g, '__rhWindowPropsBase', { value: base, configurable: true });
    } catch (e) { /* immutable */ }
  }
})();
