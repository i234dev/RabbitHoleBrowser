// The global scope of a worker realm (evaluated after the window APIs in a
// realm created for a Worker / SharedWorker): WorkerGlobalScope /
// DedicatedWorkerGlobalScope / SharedWorkerGlobalScope on the global's
// prototype chain, no DOM globals, postMessage / close / importScripts,
// and the receiving end of the host's message routing (see workers.rs).
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const C = g.__rhClone;
  const info = N.workerInfo();
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const tag = (K, name) => Object.defineProperty(K.prototype, Symbol.toStringTag, { value: name, configurable: true });

  // ---- global scope interfaces on the global's prototype chain
  function WorkerGlobalScope() { ILLEGAL(); }
  function DedicatedWorkerGlobalScope() { ILLEGAL(); }
  function SharedWorkerGlobalScope() { ILLEGAL(); }
  const base = Object.getPrototypeOf(g);
  // No named properties object (elements by id) without a document.
  if (g.__rhWindowPropsBase !== undefined) {
    Object.setPrototypeOf(base, g.__rhWindowPropsBase);
    delete g.__rhWindowPropsBase;
  }
  WorkerGlobalScope.prototype = Object.create(base);
  DedicatedWorkerGlobalScope.prototype = Object.create(WorkerGlobalScope.prototype);
  SharedWorkerGlobalScope.prototype = Object.create(WorkerGlobalScope.prototype);
  for (const [K, n] of [[WorkerGlobalScope, 'WorkerGlobalScope'], [DedicatedWorkerGlobalScope, 'DedicatedWorkerGlobalScope'], [SharedWorkerGlobalScope, 'SharedWorkerGlobalScope']]) {
    Object.defineProperty(K.prototype, 'constructor', { value: K, writable: true, configurable: true });
    tag(K, n);
    Object.defineProperty(g, n, { value: K, writable: true, configurable: true });
  }
  Object.setPrototypeOf(g, info.shared ? SharedWorkerGlobalScope.prototype : DedicatedWorkerGlobalScope.prototype);
  // Interface objects: EventTarget ← WorkerGlobalScope ← Dedicated/Shared,
  // with read-only prototype properties.
  if (typeof g.EventTarget === 'function') Object.setPrototypeOf(WorkerGlobalScope, g.EventTarget);
  Object.setPrototypeOf(DedicatedWorkerGlobalScope, WorkerGlobalScope);
  Object.setPrototypeOf(SharedWorkerGlobalScope, WorkerGlobalScope);
  for (const K of [WorkerGlobalScope, DedicatedWorkerGlobalScope, SharedWorkerGlobalScope]) Object.defineProperty(K, 'prototype', { writable: false });

  // ---- WorkerNavigator / WorkerLocation: the navigator and location
  // objects' interfaces in a worker.
  const workerInterface = (name, obj) => {
    const P = obj && Object.getPrototypeOf(obj);
    if (!P) return;
    const K = { [name]: function () { ILLEGAL(); } }[name];
    K.prototype = P;
    Object.defineProperty(K, 'prototype', { writable: false });
    Object.defineProperty(P, 'constructor', { value: K, writable: true, configurable: true });
    Object.defineProperty(P, Symbol.toStringTag, { value: name, configurable: true });
    Object.defineProperty(g, name, { value: K, writable: true, configurable: true });
  };
  workerInterface('WorkerNavigator', g.navigator);
  workerInterface('WorkerLocation', g.location);

  // ---- members of WorkerGlobalScope (and its WindowOrWorkerGlobalScope
  // mixin) live on WorkerGlobalScope.prototype; the global inherits them.
  const WGS_MEMBERS = ['self', 'location', 'navigator', 'importScripts', 'onerror', 'onlanguagechange', 'onoffline', 'ononline', 'onrejectionhandled',
    'onunhandledrejection', 'origin', 'isSecureContext', 'crossOriginIsolated', 'reportError', 'btoa', 'atob', 'setTimeout', 'clearTimeout',
    'setInterval', 'clearInterval', 'queueMicrotask', 'createImageBitmap', 'structuredClone'];
  // Operations: required argument counts; a receiver other than the
  // global (or null/undefined, which means it) is a TypeError.
  const WGS_OPS = { importScripts: 0, reportError: 1, btoa: 1, atob: 1, setTimeout: 1, clearTimeout: 0, setInterval: 1, clearInterval: 0,
    queueMicrotask: 1, createImageBitmap: 1, structuredClone: 1 };
  const scopeOperation = (name, f, n) => {
    const w = { [name](...a) {
      if (this != null && this !== g && !(this instanceof WorkerGlobalScope)) throw new TypeError('Illegal invocation');
      if (a.length < n) throw new TypeError(`Failed to execute '${name}' on 'WorkerGlobalScope': ${n} argument${n === 1 ? '' : 's'} required, but only ${a.length} present.`);
      return f.apply(g, a);
    } }[name];
    Object.defineProperty(w, 'length', { value: n, configurable: true });
    return w;
  };
  const moveToScope = () => {
    for (const k of WGS_MEMBERS) {
      let d = null;
      for (let o = g; o && o !== WorkerGlobalScope.prototype && o !== Object.prototype; o = o === g ? base : Object.getPrototypeOf(o)) {
        d = Object.getOwnPropertyDescriptor(o, k);
        if (d) break;
      }
      if (!d || !d.configurable) continue;
      d.enumerable = true;
      if (typeof d.value === 'function' && k in WGS_OPS) d.value = scopeOperation(k, d.value, WGS_OPS[k]);
      try {
        Object.defineProperty(WorkerGlobalScope.prototype, k, d);
        delete g[k];
      } catch (e) { /* keep it on the global */ }
    }
  };

  // ---- no DOM in workers
  const gone = ['window', 'document', 'Window', 'Document', 'HTMLDocument', 'parent', 'top', 'frames', 'opener', 'frameElement',
    'alert', 'confirm', 'prompt', 'print', 'history', 'localStorage', 'sessionStorage', 'customElements', 'external', 'screen',
    // Window-only interfaces of the HTML Standard.
    'HTMLAllCollection', 'HTMLFormControlsCollection', 'RadioNodeList', 'HTMLOptionsCollection', 'DOMStringMap', 'MediaError', 'AudioTrackList',
    'AudioTrack', 'VideoTrackList', 'VideoTrack', 'TextTrackList', 'TextTrack', 'TextTrackCueList', 'TextTrackCue', 'TimeRanges', 'TrackEvent',
    'ValidityState', 'SubmitEvent', 'FormDataEvent', 'CanvasRenderingContext2D', 'CustomElementRegistry', 'ElementInternals', 'CustomStateSet',
    'VisibilityStateEntry', 'UserActivation', 'ToggleEvent', 'CommandEvent', 'CloseWatcher', 'DataTransfer', 'DataTransferItemList',
    'DataTransferItem', 'DragEvent', 'BarProp', 'Location', 'History', 'Navigation', 'NavigationHistoryEntry', 'NavigationTransition',
    'NavigationActivation', 'NavigateEvent', 'NavigationPrecommitController', 'NavigationDestination', 'NavigationCurrentEntryChangeEvent',
    'PopStateEvent', 'HashChangeEvent', 'PageSwapEvent', 'PageRevealEvent', 'PageTransitionEvent', 'BeforeUnloadEvent', 'NotRestoredReasonDetails',
    'NotRestoredReasons', 'DOMParser', 'XMLSerializer', 'Sanitizer', 'Navigator', 'PluginArray', 'MimeTypeArray', 'Plugin', 'MimeType',
    'Worklet', 'Storage', 'StorageEvent', 'External',
    ...(info.shared ? [] : ['SharedWorkerGlobalScope']), 'SharedWorker',
    ...Object.getOwnPropertyNames(g).filter((k) => /^HTML[A-Za-z]*Element$/.test(k))];
  for (const k of gone) {
    try { if (!delete g[k]) Object.defineProperty(g, k, { value: undefined, configurable: true }); } catch (e) { /* non-configurable: keep */ }
  }
  Object.defineProperty(g, 'self', { get: { 'get self'() { return g; } }['get self'], set: { 'set self'(v) { Object.defineProperty(g, 'self', { value: v, writable: true, configurable: true }); } }['set self'], configurable: true });
  // [Replaceable] readonly attribute name.
  Object.defineProperty(g, 'name', {
    get: { 'get name'() { return info.name; } }['get name'],
    set: { 'set name'(v) { Object.defineProperty(g, 'name', { value: v, writable: true, configurable: true, enumerable: true }); } }['set name'],
    configurable: true, enumerable: true,
  });
  Object.defineProperty(g, 'isSecureContext', { get: { 'get isSecureContext'() { return true; } }['get isSecureContext'], configurable: true });

  // ---- message routing (ids: see workers.rs)
  const bridges = new Map();
  let nextBridge = 0;
  const post = (msg) => N.workerPost(JSON.stringify(msg));
  // A local MessagePort bridged to the owner's realm under id `b`.
  function bridge(port, b) {
    bridges.set(b, port);
    C.forwardPort(port, packet => post({ t: 'port', b, ...workerPacket(packet) }));
  }
  function workerPacket(packet) {
    const ports = packet.ports.map(port => {
      const b = 'w' + info.id + '.' + nextBridge++;
      bridge(port, b);
      return b;
    });
    return { d: packet.d, ports };
  }
  function incomingPorts(ids) {
    return (ids || []).map((b) => {
      const ch = new g.MessageChannel();
      bridge(ch.port2, b);
      return ch.port1;
    });
  }

  const scope = {
    postMessage(message, transfer) {
      post({ t: 'msg', ...workerPacket(C.pack(message, transfer)) });
    },
    close() { N.workerClose(); },
    importScripts(...urls) {
      for (const u of urls) {
        const href = new URL(String(u), g.location.href).href;
        const src = N.loadScriptSync(href);
        if (src === null) throw domError(`Failed to execute 'importScripts' on 'WorkerGlobalScope': The script at '${href}' failed to load.`, 'NetworkError');
        N.workerEvalScript(src, href);
      }
    },
  };
  // The global's own operations reject foreign receivers (null and
  // undefined mean the global) and missing arguments.
  const ownOperation = (name, f, n, iface) => {
    const w = { [name](...a) {
      if (this != null && this !== g) throw new TypeError('Illegal invocation');
      if (a.length < n) throw new TypeError(`Failed to execute '${name}' on '${iface}': ${n} argument${n === 1 ? '' : 's'} required, but only ${a.length} present.`);
      return f.apply(g, a);
    } }[name];
    Object.defineProperty(w, 'length', { value: n, configurable: true });
    return w;
  };
  const scopeName = info.shared ? 'SharedWorkerGlobalScope' : 'DedicatedWorkerGlobalScope';
  if (!info.shared) scope.postMessage = ownOperation('postMessage', scope.postMessage, 1, scopeName);
  scope.close = ownOperation('close', scope.close, 0, scopeName);
  const own = info.shared ? ['close', 'importScripts'] : ['postMessage', 'close', 'importScripts'];
  for (const k of own) Object.defineProperty(g, k, { value: scope[k], writable: true, configurable: true, enumerable: k !== 'importScripts' });
  for (const k of ['requestAnimationFrame', 'cancelAnimationFrame']) {
    const d = Object.getOwnPropertyDescriptor(g, k);
    if (d && d.configurable) { d.enumerable = true; Object.defineProperty(g, k, d); }
  }
  N.installHandlers(g, info.shared ? ['connect', 'error'] : ['message', 'messageerror', 'error']);
  moveToScope();

  N.registerProto('__worker_receive', function receive(text) {
    let m;
    try { m = JSON.parse(text); } catch (e) { return; }
    if (m.t === 'port') {
      const p = bridges.get(m.b);
      if (p) {
        const ports = incomingPorts(m.ports);
        C.enqueuePeer(p, { d: m.d, ports });
      }
      return;
    }
    if (m.t === 'connect') {
      const ch = new g.MessageChannel();
      bridge(ch.port2, m.b);
      g.dispatchEvent(new g.MessageEvent('connect', { data: '', ports: [ch.port1], source: ch.port1 }));
      return;
    }
    if (m.t === 'msg') {
      let data;
      const ports = incomingPorts(m.ports);
      try { data = C.deserialize(m.d, ports); } catch (e) { g.dispatchEvent(new g.MessageEvent('messageerror', {})); return; }
      g.dispatchEvent(new g.MessageEvent('message', { data, ports }));
    }
  });
})();
