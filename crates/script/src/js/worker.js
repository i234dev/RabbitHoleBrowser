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
  WorkerGlobalScope.prototype = Object.create(base);
  DedicatedWorkerGlobalScope.prototype = Object.create(WorkerGlobalScope.prototype);
  SharedWorkerGlobalScope.prototype = Object.create(WorkerGlobalScope.prototype);
  for (const [K, n] of [[WorkerGlobalScope, 'WorkerGlobalScope'], [DedicatedWorkerGlobalScope, 'DedicatedWorkerGlobalScope'], [SharedWorkerGlobalScope, 'SharedWorkerGlobalScope']]) {
    Object.defineProperty(K.prototype, 'constructor', { value: K, writable: true, configurable: true });
    tag(K, n);
    Object.defineProperty(g, n, { value: K, writable: true, configurable: true });
  }
  Object.setPrototypeOf(g, info.shared ? SharedWorkerGlobalScope.prototype : DedicatedWorkerGlobalScope.prototype);

  // ---- no DOM in workers
  const gone = ['window', 'document', 'Window', 'Document', 'HTMLDocument', 'parent', 'top', 'frames', 'opener', 'frameElement',
    'alert', 'confirm', 'prompt', 'print', 'history', 'localStorage', 'sessionStorage', 'customElements', 'external', 'screen'];
  for (const k of gone) {
    try { if (!delete g[k]) Object.defineProperty(g, k, { value: undefined, configurable: true }); } catch (e) { /* non-configurable: keep */ }
  }
  Object.defineProperty(g, 'self', { get() { return g; }, set(v) { Object.defineProperty(g, 'self', { value: v, writable: true, configurable: true }); }, configurable: true });
  Object.defineProperty(g, 'name', { value: info.name, writable: true, configurable: true });
  Object.defineProperty(g, 'isSecureContext', { value: true, configurable: true });

  // ---- message routing (ids: see workers.rs)
  const bridges = new Map();
  let nextBridge = 0;
  const post = (msg) => N.workerPost(JSON.stringify(msg));
  // A local MessagePort bridged to the owner's realm under id `b`.
  function bridge(port, b) {
    bridges.set(b, port);
    port.onmessage = (ev) => post({ t: 'port', b, d: C.serialize(ev.data) });
  }
  function portsOf(transfer) {
    const list = Array.isArray(transfer) ? transfer : transfer && transfer.transfer ? Array.from(transfer.transfer) : [];
    return list.filter((t) => typeof g.MessagePort === 'function' && t instanceof g.MessagePort);
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
      const d = C.serialize(message);
      const ports = portsOf(transfer).map((p) => { const b = 'w' + info.id + '.' + nextBridge++; bridge(p, b); return b; });
      post({ t: 'msg', d, ports });
    },
    close() { N.workerClose(); },
    importScripts(...urls) {
      for (const u of urls) {
        const href = new URL(String(u), g.location.href).href;
        const src = N.loadScriptSync(href);
        if (src === null) throw domError(`Failed to execute 'importScripts' on 'WorkerGlobalScope': The script at '${href}' failed to load.`, 'NetworkError');
        (0, eval)(src);
      }
    },
  };
  const own = info.shared ? ['close', 'importScripts'] : ['postMessage', 'close', 'importScripts'];
  for (const k of own) Object.defineProperty(g, k, { value: scope[k], writable: true, configurable: true });
  N.installHandlers(g, info.shared ? ['connect', 'error'] : ['message', 'messageerror', 'error']);

  N.registerProto('__worker_receive', function receive(text) {
    let m;
    try { m = JSON.parse(text); } catch (e) { return; }
    if (m.t === 'port') {
      const p = bridges.get(m.b);
      if (p) p.postMessage(C.deserialize(m.d));
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
      try { data = C.deserialize(m.d); } catch (e) { g.dispatchEvent(new g.MessageEvent('messageerror', {})); return; }
      g.dispatchEvent(new g.MessageEvent('message', { data, ports: incomingPorts(m.ports) }));
    }
  });
})();
