// Cross-context messaging: window.postMessage (as a task, with structured
// cloning and transferable ports), MessageChannel / MessagePort,
// BroadcastChannel, dedicated and shared workers (run in their own global
// scope object inside this realm), Worklet.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('messaging');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const task = (f) => setTimeout(f, 0);
  const origin = () => g.location.origin;

  function cloneWithPorts(data, transfer) {
    const list = Array.isArray(transfer) ? transfer : transfer && transfer.transfer ? Array.from(transfer.transfer) : [];
    const ports = list.filter((t) => t instanceof MessagePort);
    for (const p of ports) if (p[S].detached) throw domError('Port at index 0 is already neutered.', 'DataCloneError');
    return { data: g.structuredClone(data), ports };
  }

  // --------------------------------------------------------------- ports
  class MessagePort extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    postMessage(message, transfer) {
      const st = this[S];
      if (st.closed) return;
      const { data, ports } = cloneWithPorts(message, transfer);
      const target = st.entangled;
      if (!target) return;
      target[S].queue.push({ data, ports });
      if (target[S].started) flush(target);
    }
    start() { this[S].started = true; flush(this); }
    close() { const st = this[S]; st.closed = true; if (st.entangled) { st.entangled[S].entangled = null; st.entangled.dispatchEvent(new Event('close')); } st.entangled = null; }
  }
  tag(MessagePort, 'MessagePort');
  N.installHandlers(MessagePort.prototype, ['message', 'messageerror', 'close']);
  // Setting onmessage implicitly starts the port.
  const onmsg = Object.getOwnPropertyDescriptor(MessagePort.prototype, 'onmessage');
  Object.defineProperty(MessagePort.prototype, 'onmessage', { get: onmsg.get, set(v) { onmsg.set.call(this, v); this.start(); }, configurable: true, enumerable: true });
  function newPort() { const p = Object.create(MessagePort.prototype); hidden(p, S, { entangled: null, queue: [], started: false, closed: false, scheduled: false }); return p; }
  function flush(port) {
    const st = port[S];
    if (st.scheduled || !st.queue.length) return;
    st.scheduled = true;
    task(() => {
      st.scheduled = false;
      while (st.queue.length && st.started) {
        const { data, ports } = st.queue.shift();
        port.dispatchEvent(new g.MessageEvent('message', { data, ports, origin: '' }));
      }
    });
  }
  class MessageChannel {
    constructor() {
      const a = newPort(), b = newPort();
      a[S].entangled = b; b[S].entangled = a;
      hidden(this, S, { a, b });
    }
    get port1() { return this[S].a; }
    get port2() { return this[S].b; }
  }
  tag(MessageChannel, 'MessageChannel');
  define('MessagePort', MessagePort);
  define('MessageChannel', MessageChannel);

  // ------------------------------------------------------- window.postMessage
  hidden(g, 'postMessage', function postMessage(message, targetOrigin, transfer) {
    let options = {};
    if (targetOrigin && typeof targetOrigin === 'object') { options = targetOrigin; targetOrigin = options.targetOrigin; }
    targetOrigin = targetOrigin === undefined ? '/' : String(targetOrigin);
    if (targetOrigin !== '*' && targetOrigin !== '/') {
      let u;
      try { u = new URL(targetOrigin); } catch (e) { throw domError(`Failed to execute 'postMessage' on 'Window': Invalid target origin '${targetOrigin}' in a call to 'postMessage'.`, 'SyntaxError'); }
      if (u.origin !== origin()) return;
    }
    const { data, ports } = cloneWithPorts(message, transfer || options.transfer);
    task(() => g.dispatchEvent(new g.MessageEvent('message', { data, origin: origin(), source: g, ports })));
  });

  // ------------------------------------------------------- BroadcastChannel
  const channels = new Set();
  class BroadcastChannel extends g.EventTarget {
    constructor(name) {
      super();
      if (arguments.length < 1) throw new TypeError("Failed to construct 'BroadcastChannel': 1 argument required, but only 0 present.");
      hidden(this, S, { name: String(name), closed: false });
      channels.add(this);
    }
    get name() { return this[S].name; }
    postMessage(message) {
      if (this[S].closed) throw domError("Failed to execute 'postMessage' on 'BroadcastChannel': Channel is closed", 'InvalidStateError');
      const data = g.structuredClone(message);
      for (const c of channels) {
        if (c === this || c[S].closed || c[S].name !== this[S].name) continue;
        task(() => { if (!c[S].closed) c.dispatchEvent(new g.MessageEvent('message', { data: g.structuredClone(data), origin: origin() })); });
      }
    }
    close() { this[S].closed = true; channels.delete(this); }
  }
  tag(BroadcastChannel, 'BroadcastChannel');
  N.installHandlers(BroadcastChannel.prototype, ['message', 'messageerror']);
  define('BroadcastChannel', BroadcastChannel);

  // ---------------------------------------------------------------- workers
  // A worker runs its script (fetched synchronously) as a function whose
  // `self`, `postMessage`, `onmessage`, `importScripts`, `close`, ... come
  // from its own WorkerGlobalScope object; other globals are shared with the
  // page (no thread or realm isolation).
  function loadScript(url) {
    if (/^data:/i.test(url)) {
      const m = /^data:([^,]*?)(;base64)?,(.*)$/is.exec(url);
      if (!m) return null;
      return m[2] ? g.atob(m[3]) : decodeURIComponent(m[3]);
    }
    const xhr = new g.XMLHttpRequest();
    xhr.open('GET', url, false);
    try { xhr.send(); } catch (e) { return null; }
    return xhr.status >= 200 && xhr.status < 300 ? xhr.responseText : null;
  }
  const WORKER_NAMES = ['self', 'postMessage', 'close', 'importScripts', 'addEventListener', 'removeEventListener', 'dispatchEvent', 'name', 'location'];
  // Without `with`, assignments to the worker's global event handlers
  // (`onmessage = ...`) are redirected to `self.onmessage = ...`.
  const rewriteHandlers = (src) => src.replace(/(^|[^.\w$])(on(?:message|messageerror|error|connect))(\s*=)(?!=)/g, (m, pre, name, eq) => `${pre}self.${name}${eq}`);
  function runWorker(url, scope, onError) {
    const params = WORKER_NAMES.slice();
    const run = (src) => {
      try {
        const fn = new Function(...params, rewriteHandlers(src));
        const args = params.map((p) => (p === 'self' ? scope : typeof scope[p] === 'function' ? scope[p].bind(scope) : scope[p]));
        fn.apply(scope, args);
      } catch (e) {
        onError(new g.ErrorEvent('error', { message: String(e && e.message || e), error: e, filename: url }));
      }
      scope[S].ready = true;
      for (const m of scope[S].queue.splice(0)) scope.dispatchEvent(m());
    };
    const fail = () => onError(new g.ErrorEvent('error', { message: `Failed to load worker script at '${url}'` }));
    if (/^data:/i.test(url)) { const src = loadScript(url); if (src === null) task(fail); else task(() => run(src)); return; }
    g.fetch(url).then((r) => (r.ok ? r.text() : Promise.reject(new Error(String(r.status))))).then(run, fail);
  }
  // Deliver to a worker scope once its script ran.
  function toScope(scope, makeEvent) {
    if (scope[S].ready) task(() => { if (!scope[S].closed) scope.dispatchEvent(makeEvent()); });
    else scope[S].queue.push(makeEvent);
  }
  class WorkerGlobalScope extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get self() { return this; }
    get location() { return this[S].location; }
    get name() { return this[S].name; }
    importScripts(...urls) {
      for (const u of urls) {
        const src = loadScript(new URL(u, this[S].location).href);
        if (src === null) throw domError(`Failed to execute 'importScripts' on 'WorkerGlobalScope': The script at '${u}' failed to load.`, 'NetworkError');
        (0, eval)(src);
      }
    }
    close() { this[S].closed = true; }
  }
  N.installHandlers(WorkerGlobalScope.prototype, ['error']);
  class DedicatedWorkerGlobalScope extends WorkerGlobalScope {
    postMessage(message, transfer) {
      if (this[S].closed) return;
      const { data, ports } = cloneWithPorts(message, transfer);
      const w = this[S].worker;
      task(() => { if (!w[S].terminated) w.dispatchEvent(new g.MessageEvent('message', { data, ports })); });
    }
  }
  N.installHandlers(DedicatedWorkerGlobalScope.prototype, ['message', 'messageerror']);
  tag(DedicatedWorkerGlobalScope, 'DedicatedWorkerGlobalScope');
  class Worker extends g.EventTarget {
    constructor(url, options = {}) {
      super();
      let u;
      try { u = new URL(String(url), document.baseURI); } catch (e) { throw domError(`Failed to construct 'Worker': The URL '${url}' is invalid.`, 'SyntaxError'); }
      const scope = Object.create(DedicatedWorkerGlobalScope.prototype);
      g.EventTarget.call ? void 0 : void 0;
      hidden(scope, S, { location: u.href, name: options.name || '', worker: this, closed: false, ready: false, queue: [] });
      hidden(this, S, { scope, terminated: false });
      runWorker(u.href, scope, (ev) => { if (!this[S].terminated) this.dispatchEvent(ev); });
    }
    postMessage(message, transfer) {
      const st = this[S];
      if (st.terminated) return;
      const { data, ports } = cloneWithPorts(message, transfer);
      if (!st.scope[S].closed) toScope(st.scope, () => new g.MessageEvent('message', { data, ports }));
    }
    terminate() { this[S].terminated = true; this[S].scope[S].closed = true; }
  }
  tag(Worker, 'Worker');
  N.installHandlers(Worker.prototype, ['message', 'messageerror', 'error']);
  define('Worker', Worker);
  const shared = new Map();
  class SharedWorker extends g.EventTarget {
    constructor(url, options = {}) {
      super();
      if (typeof options === 'string') options = { name: options };
      const u = new URL(String(url), document.baseURI).href;
      const key = u + '\n' + (options.name || '');
      const ch = new MessageChannel();
      hidden(this, S, { port: ch.port1 });
      let scope = shared.get(key);
      if (!scope) {
        scope = Object.create(WorkerGlobalScope.prototype);
        hidden(scope, S, { location: u, name: options.name || '', closed: false, ready: false, queue: [] });
        N.installHandlers(scope, ['connect']);
        shared.set(key, scope);
        runWorker(u, scope, (ev) => this.dispatchEvent(ev));
      }
      toScope(scope, () => new g.MessageEvent('connect', { data: '', ports: [ch.port2], source: ch.port2 }));
    }
    get port() { return this[S].port; }
  }
  tag(SharedWorker, 'SharedWorker');
  N.installHandlers(SharedWorker.prototype, ['error']);
  define('SharedWorker', SharedWorker);

  // Worklets (CSS paint / audio): modules load but there is no rendering
  // pipeline to call them.
  class Worklet {
    constructor() { ILLEGAL(); }
    addModule(url) {
      const src = loadScript(new URL(String(url), document.baseURI).href);
      return src === null ? Promise.reject(domError('Unable to load a worklet\'s module.', 'AbortError')) : Promise.resolve();
    }
  }
  tag(Worklet, 'Worklet');
  define('Worklet', Worklet);
  if (g.CSS) for (const k of ['paintWorklet', 'layoutWorklet', 'animationWorklet']) hidden(g.CSS, k, Object.create(Worklet.prototype));
})();
