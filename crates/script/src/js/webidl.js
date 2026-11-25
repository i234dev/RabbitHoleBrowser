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
  // Operations whose implementations do not check their argument count:
  // calls with fewer than the required arguments throw a TypeError, their
  // `length` is that count, and a receiver not implementing the interface
  // is rejected (WebIDL overload resolution and receiver checks).
  const REQUIRED = {
    HTMLInputElement: { setCustomValidity: 1, setRangeText: 1, setSelectionRange: 2, stepUp: 0, stepDown: 0, showPicker: 0 },
    HTMLTextAreaElement: { setCustomValidity: 1, setRangeText: 1, setSelectionRange: 2 },
    HTMLSelectElement: { setCustomValidity: 1, add: 1, item: 1, namedItem: 1 },
    HTMLButtonElement: { setCustomValidity: 1 },
    HTMLObjectElement: { setCustomValidity: 1, checkValidity: 0, reportValidity: 0, getSVGDocument: 0 },
    HTMLOutputElement: { setCustomValidity: 1 },
    HTMLFieldSetElement: { setCustomValidity: 1 },
    HTMLIFrameElement: { getSVGDocument: 0 },
    HTMLEmbedElement: { getSVGDocument: 0 },
    HTMLMediaElement: { canPlayType: 1, fastSeek: 1, addTextTrack: 1, load: 0, pause: 0, getStartDate: 0 },
    HTMLTableElement: { deleteRow: 1 },
    HTMLTableSectionElement: { deleteRow: 1 },
    HTMLTableRowElement: { deleteCell: 1 },
    HTMLAllCollection: { namedItem: 1, item: 0 },
    HTMLScriptElement: {},
    Document: { getElementsByName: 1, execCommand: 1, queryCommandEnabled: 1, queryCommandIndeterm: 1, queryCommandState: 1, queryCommandSupported: 1, queryCommandValue: 1, hasFocus: 0 },
    Element: { insertAdjacentHTML: 2 },
    History: { pushState: 2, replaceState: 2, go: 0, back: 0, forward: 0 },
    Location: { assign: 1, replace: 1 },
    Navigator: { registerProtocolHandler: 2, unregisterProtocolHandler: 2, javaEnabled: 0 },
    TextTrackList: { getTrackById: 1 },
    TextTrackCueList: { getCueById: 1 },
    TimeRanges: { start: 1, end: 1 },
    MessageEvent: { initMessageEvent: 1 },
    StorageEvent: { initStorageEvent: 1 },
    CustomElementRegistry: { define: 2, get: 1, getName: 1, upgrade: 1 },
    XMLSerializer: { serializeToString: 1 },
    CanvasGradient: { addColorStop: 2 },
    Document: { getElementsByName: 1, execCommand: 1, queryCommandEnabled: 1, queryCommandIndeterm: 1, queryCommandState: 1, queryCommandSupported: 1, queryCommandValue: 1, hasFocus: 0, write: 0, writeln: 0 },
    MessagePort: { postMessage: 1 },
    Worker: { postMessage: 1 },
    ElementInternals: { setFormValue: 1 },
    DataTransfer: { setDragImage: 3, clearData: 0 },
    DataTransferItemList: { add: 1 },
    Navigation: { updateCurrentEntry: 1 },
    Sanitizer: { allowProcessingInstruction: 1, removeProcessingInstruction: 1, setJavascriptURLs: 1, setComments: 1, setDataAttributes: 1 },
    Origin: { isSameOrigin: 1, isSameSite: 1 },
    Element: { insertAdjacentHTML: 2, beforeHTML: 1, afterHTML: 1, replaceWithHTML: 1, beforeHTMLUnsafe: 1, afterHTMLUnsafe: 1, replaceWithHTMLUnsafe: 1 },
    CharacterData: { beforeHTML: 1, afterHTML: 1, replaceWithHTML: 1, beforeHTMLUnsafe: 1, afterHTMLUnsafe: 1, replaceWithHTMLUnsafe: 1 },
    HTMLOptionsCollection: { add: 1 },
    HTMLElement: { togglePopover: 0 },
    HTMLTableElement: { deleteRow: 1, insertRow: 0 },
    HTMLTableSectionElement: { deleteRow: 1, insertRow: 0 },
    HTMLTableRowElement: { deleteCell: 1, insertCell: 0 },
    HTMLFormElement: { requestSubmit: 0 },
    HTMLDialogElement: { close: 0, requestClose: 0 },
    HTMLCanvasElement: { toDataURL: 0, toBlob: 1, transferControlToOffscreen: 0 },
    Window: { postMessage: 1 },
  };
  for (const name of ['HTMLInputElement', 'HTMLButtonElement', 'HTMLSelectElement', 'HTMLTextAreaElement', 'HTMLOutputElement', 'HTMLFieldSetElement']) {
    REQUIRED[name] = Object.assign(REQUIRED[name] || {}, { checkValidity: 0, reportValidity: 0 });
  }
  REQUIRED.HTMLSelectElement.remove = 0;
  const statics = { HTMLScriptElement: { supports: 1 }, Document: { parseHTML: 1, parseHTMLUnsafe: 1 } };
  const globalChain = new Set();
  for (let o = Object.getPrototypeOf(g); o; o = Object.getPrototypeOf(o)) globalChain.add(o);
  const requireArgs = (holder, iface, name, n, C) => {
    const d = Object.getOwnPropertyDescriptor(holder, name);
    if (!d || typeof d.value !== 'function') return;
    const f = d.value;
    // (location, history and navigator objects may not be instances.)
    const brand = C && !globalChain.has(C.prototype) && !['Location', 'History', 'Navigator'].includes(iface);
    const w = { [name](...a) {
      if (brand && !(this instanceof C)) throw new TypeError('Illegal invocation');
      if (a.length < n) throw new TypeError(`Failed to execute '${name}' on '${iface}': ${n} argument${n === 1 ? '' : 's'} required, but only ${a.length} present.`);
      return f.apply(this, a);
    } }[name];
    Object.defineProperty(w, 'length', { value: n, configurable: true });
    d.value = w;
    try { Object.defineProperty(holder, name, d); } catch (e) { /* ignore */ }
  };
  for (const [iface, ops] of Object.entries(REQUIRED)) {
    const C = g[iface];
    if (typeof C !== 'function' || !C.prototype) continue;
    for (const [name, n] of Object.entries(ops)) requireArgs(C.prototype, iface, name, n, C);
  }
  for (const [iface, ops] of Object.entries(statics)) {
    const C = g[iface];
    if (typeof C !== 'function') continue;
    for (const [name, n] of Object.entries(ops)) requireArgs(C, iface, name, n, null);
  }
  const fix = (o) => {
    for (const k of Object.getOwnPropertyNames(o)) {
      if (k === 'constructor' || k === 'prototype' || ((k === 'length' || k === 'name') && typeof o === 'function') || k.startsWith('__')) continue;
      const d = Object.getOwnPropertyDescriptor(o, k);
      if (!d || d.enumerable || !d.configurable) continue;
      if (typeof d.value === 'function' || d.get || d.set) {
        d.enumerable = true;
        try { Object.defineProperty(o, k, d); } catch (e) { /* ignore */ }
      }
    }
  };
  // Regular operations of Window are own properties of the window.
  for (const k of ['close', 'stop', 'focus', 'blur', 'open', 'alert', 'confirm', 'prompt', 'print', 'postMessage', 'captureEvents', 'releaseEvents']) {
    const d = Object.getOwnPropertyDescriptor(g, k);
    if (d && d.configurable && !d.enumerable && typeof d.value === 'function' && typeof g.Window === 'function') { d.enumerable = true; Object.defineProperty(g, k, d); }
  }
  const N = g.__rhNative;
  if (!(N && N.webidlEnumerable && N.webidlEnumerable([...ES]))) for (const name of Object.getOwnPropertyNames(g)) {
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

  // The named properties object of Window: named child browsing contexts,
  // elements with an id, and embed/form/img/object elements with a name
  // are reachable as properties of window — and so as global variables —
  // after the window's own and inherited properties.
  const W = g.Window, doc = g.document;
  if (typeof W === 'function' && W.prototype && doc && typeof Proxy === 'function') {
    const base = Object.getPrototypeOf(W.prototype);
    const byId = Object.getOwnPropertyDescriptor(g.Document.prototype, 'getElementById');
    const byName = Object.getOwnPropertyDescriptor(g.Document.prototype, 'getElementsByName');
    const getById = byId && byId.value, getByName = byName && byName.value;
    const apply = Reflect.apply;
    const frameWindow = g.HTMLIFrameElement && Object.getOwnPropertyDescriptor(g.HTMLIFrameElement.prototype, 'contentWindow');
    const getFrameWindow = frameWindow && frameWindow.get;
    const legacyFrameWindow = g.HTMLFrameElement && Object.getOwnPropertyDescriptor(g.HTMLFrameElement.prototype, 'contentWindow');
    const getLegacyFrameWindow = legacyFrameWindow && legacyFrameWindow.get;
    const getLocalName = Object.getOwnPropertyDescriptor(g.Element.prototype, 'localName').get;
    const getNamespace = Object.getOwnPropertyDescriptor(g.Element.prototype, 'namespaceURI').get;
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
      const list = getByName ? apply(getByName, d, [k]) : null;
      // A named child navigable wins over a matching element id. Use the
      // native getter captured at startup, not a page-defined property,
      // and query the live document so rename/removal need no refresh.
      if (list && (getFrameWindow || getLegacyFrameWindow)) {
        for (let i = 0; i < list.length; i++) {
          const e = list[i];
          const tag = apply(getLocalName, e, []);
          const getter = tag === 'iframe' ? getFrameWindow : tag === 'frame' ? getLegacyFrameWindow : null;
          if (getter && apply(getNamespace, e, []) === 'http://www.w3.org/1999/xhtml') {
            const w = apply(getter, e, []);
            if (w !== null) return w;
          }
        }
      }
      const el = apply(getById, d, [k]);
      if (el) return el;
      if (list) {
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
      setPrototypeOf(t, p) { return Reflect.getPrototypeOf(t) === p; },
      preventExtensions() { return false; },
    });
    try {
      Object.setPrototypeOf(W.prototype, props);
      // Worker realms (set up after this) put it back.
      Object.defineProperty(g, '__rhWindowPropsBase', { value: base, configurable: true });
    } catch (e) { /* immutable */ }
  }
})();
