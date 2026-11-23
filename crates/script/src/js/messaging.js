// Cross-context messaging: window.postMessage (as a task, with structured
// cloning and transferable ports), MessageChannel / MessagePort,
// BroadcastChannel, dedicated and shared workers (each in its own realm,
// see workers.rs / worker.js), Worklet.
(function () {
  'use strict';
  // The engine's own timers, bound now: internal tasks (port messages,
  // idle callbacks, observer deliveries, ...) must not go through a page's
  // replacement of these globals, which may wrap, delay or drop them.
  const { setTimeout, clearTimeout, setInterval, clearInterval, requestAnimationFrame, cancelAnimationFrame, queueMicrotask } = globalThis;
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('messaging');
  const C = g.__rhClone;
  const knownPorts = new WeakSet();
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const task = (f) => setTimeout(f, 0);
  const origin = () => g.location.origin;

  function cloneWithPorts(data, transfer) {
    const list = C.transferList(transfer);
    const ports = list.filter(C.isPort);
    // Clone the payload and its port list together to retain shared identities.
    const result = g.structuredClone([data, ports], { transfer: list });
    return { data: result[0], ports: result[1] };
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
  function newPort() { const p = Object.create(MessagePort.prototype); knownPorts.add(p); hidden(p, S, { entangled: null, queue: [], started: false, closed: false, scheduled: false }); return p; }
  C.isPort = value => knownPorts.has(value);
  C.portDetached = port => !!port[S].detached;
  C.movePort = port => {
    const old = port[S], copy = newPort(), state = copy[S];
    state.entangled = old.entangled;
    if (state.entangled) state.entangled[S].entangled = copy;
    state.queue = old.queue;
    state.closed = old.closed;
    old.queue = [];
    old.entangled = null;
    old.closed = old.detached = true;
    return copy;
  };
  function flush(port) {
    const st = port[S];
    if (st.scheduled || !st.queue.length) return;
    st.scheduled = true;
    // One task per message: microtasks queued by a handler (promise
    // reactions installing the next handler) run before the next message.
    task(() => {
      st.scheduled = false;
      if (!st.queue.length || !st.started) return;
      const { data, ports } = st.queue.shift();
      port.dispatchEvent(new g.MessageEvent('message', { data, ports, origin: '' }));
      flush(port);
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
  // Each worker runs in its own realm (see workers.rs): messages are
  // structured clones serialized to JSON; MessagePorts are bridged by id.
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
  const workerObjects = new Map();
  const workerBridges = new Map();
  let nextWorkerBridge = 0;
  const send = (id, msg) => N.workerSend(id, JSON.stringify(msg));
  function bridgeTo(id, port, b) {
    workerBridges.set(b, port);
    port.onmessage = (ev) => send(id, { t: 'port', b, d: C.serialize(ev.data), ports: outgoingWorkerPorts(id, ev.ports) });
  }
  function transferredPorts(transfer) {
    const list = C.transferList(transfer);
    const ports = list.filter((t) => t instanceof MessagePort);
    for (const p of ports) if (p[S].detached) throw domError('Port at index 0 is already neutered.', 'DataCloneError');
    return ports;
  }
  function outgoingWorkerPorts(id, transfer) {
    return transferredPorts(transfer).map((p) => {
      const b = 'o' + nextWorkerBridge++;
      bridgeTo(id, p, b);
      return b;
    });
  }
  function incomingWorkerPorts(id, bridges) {
    return (bridges || []).map((b) => {
      const ch = new MessageChannel();
      bridgeTo(id, ch.port2, b);
      return ch.port1;
    });
  }
  function baseURL() { return (g.document && g.document.baseURI) || g.location.href; }
  N.registerProto('__worker_deliver', function deliver(id, text) {
    let m;
    try { m = JSON.parse(text); } catch (e) { return; }
    if (m.t === 'port') {
      const p = workerBridges.get(m.b);
      if (p) p.postMessage(C.deserialize(m.d), incomingWorkerPorts(id, m.ports));
      return;
    }
    const w = workerObjects.get(id);
    if (!w || w[S].terminated) return;
    if (m.t === 'error') {
      const ev = new g.ErrorEvent('error', { message: m.message, filename: m.filename, cancelable: true });
      w.dispatchEvent(ev);
      return;
    }
    if (m.t === 'msg') {
      let data;
      try { data = C.deserialize(m.d); } catch (e) { w.dispatchEvent(new g.MessageEvent('messageerror', {})); return; }
      const ports = incomingWorkerPorts(id, m.ports);
      w.dispatchEvent(new g.MessageEvent('message', { data, ports }));
    }
  });
  class Worker extends g.EventTarget {
    constructor(url, options = {}) {
      super();
      let u;
      try { u = new URL(String(url), baseURL()); } catch (e) { throw domError(`Failed to construct 'Worker': The URL '${url}' is invalid.`, 'SyntaxError'); }
      options = options || {};
      const id = N.workerStart(u.href, options.name === undefined ? '' : String(options.name), null, options.type === 'module');
      hidden(this, S, { id, terminated: false });
      workerObjects.set(id, this);
    }
    postMessage(message, transfer) {
      const st = this[S];
      if (st.terminated) return;
      const d = C.serialize(message);
      const ports = outgoingWorkerPorts(st.id, transfer);
      send(st.id, { t: 'msg', d, ports });
    }
    terminate() {
      const st = this[S];
      if (st.terminated) return;
      st.terminated = true;
      workerObjects.delete(st.id);
      N.workerTerminate(st.id);
    }
  }
  tag(Worker, 'Worker');
  N.installHandlers(Worker.prototype, ['message', 'messageerror', 'error']);
  define('Worker', Worker);
  class SharedWorker extends g.EventTarget {
    constructor(url, options = {}) {
      super();
      if (typeof options === 'string') options = { name: options };
      options = options || {};
      let u;
      try { u = new URL(String(url), baseURL()).href; } catch (e) { throw domError(`Failed to construct 'SharedWorker': The URL '${url}' is invalid.`, 'SyntaxError'); }
      const name = options.name === undefined ? '' : String(options.name);
      const id = N.workerStart(u, name, u + '\n' + name, options.type === 'module');
      const ch = new MessageChannel();
      hidden(this, S, { port: ch.port1, id, terminated: false });
      workerObjects.set(id, this);
      const b = 'o' + nextWorkerBridge++;
      bridgeTo(id, ch.port2, b);
      send(id, { t: 'connect', b });
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
