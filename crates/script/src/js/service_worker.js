// A real service worker global, installed only in its own Worker realm.
(function () {
  'use strict';
  const g = globalThis, N = g.__rhNative, api = g.__rhSW, cloneCodec = g.__rhClone;
  const info = N.swSnapshot();
  class ServiceWorkerGlobalScope extends g.WorkerGlobalScope {}
  Object.defineProperty(ServiceWorkerGlobalScope.prototype, Symbol.toStringTag, { value: 'ServiceWorkerGlobalScope', configurable: true });
  Object.defineProperty(g, 'ServiceWorkerGlobalScope', { value: ServiceWorkerGlobalScope, configurable: true, writable: true });
  Object.setPrototypeOf(g, ServiceWorkerGlobalScope.prototype);
  for (const name of ['postMessage', 'close', 'Worker', 'SharedWorker']) { try { delete g[name]; } catch (e) {} }
  const lifetime = new WeakMap();
  function finish(e) {
    const s = lifetime.get(e);
    if (s.closed || s.dispatching || s.pending) return;
    s.closed = true;
    if (s.report) N.swEventDone(!s.failed);
    if (s.fetchId !== undefined) N.swFetchDone(s.fetchId);
  }
  class ExtendableEvent extends g.Event {
    constructor(type, init = {}) {
      super(type, init);
      lifetime.set(this, { dispatching: false, pending: 0, failed: false, closed: false, report: false });
    }
    waitUntil(value) {
      const s = lifetime.get(this);
      if (!s || s.closed || (!s.dispatching && !s.pending)) throw new DOMException('The event lifetime cannot be extended.', 'InvalidStateError');
      s.pending++;
      Promise.resolve(value).then(() => { s.pending--; finish(this); }, () => { s.failed = true; s.pending--; finish(this); });
    }
  }
  Object.defineProperty(ExtendableEvent.prototype, Symbol.toStringTag, { value: 'ExtendableEvent', configurable: true });
  Object.defineProperty(g, 'ExtendableEvent', { value: ExtendableEvent, configurable: true, writable: true });
  const dispatch = g.EventTarget.prototype.dispatchEvent;
  N.installHandlers(g, ['install', 'activate', 'fetch', 'message', 'messageerror']);
  N.registerProto('__sw_dispatch', function (type) {
    const e = new ExtendableEvent(type), s = lifetime.get(e);
    s.dispatching = true; s.report = true;
    dispatch.call(g, e);
    s.dispatching = false;
    queueMicrotask(() => finish(e));
  });
  const fetchState = new WeakMap(), fetchSignals = new Map();
  class FetchEvent extends ExtendableEvent {
    constructor(type, init = {}) {
      super(type, init);
      if (!(init.request instanceof g.Request)) throw new TypeError('FetchEvent requires a Request.');
      let resolve, reject;
      const handled = new Promise((a, b) => { resolve = a; reject = b; });
      handled.catch(() => {});
      fetchState.set(this, { request: init.request, clientId: init.clientId || '', resultingClientId: init.resultingClientId || '', handled, resolve, reject, entered: false });
    }
    get request() { return fetchState.get(this).request; }
    get clientId() { return fetchState.get(this).clientId; }
    get resultingClientId() { return fetchState.get(this).resultingClientId; }
    get replacesClientId() { return ''; }
    get preloadResponse() { return Promise.resolve(undefined); }
    get handled() { return fetchState.get(this).handled; }
    respondWith(value) {
      const state = fetchState.get(this), event = lifetime.get(this);
      if (!state || !event.dispatching || state.entered || event.fetchId === undefined) throw new DOMException('respondWith must be called once during fetch dispatch.', 'InvalidStateError');
      state.entered = true;
      g.Event.prototype.stopImmediatePropagation.call(this);
      const response = Promise.resolve(value).then(r => {
        N.swFetchResponse(event.fetchId, r);
        state.resolve();
      }, e => { throw e; }).catch(error => {
        N.swFetchFailed(event.fetchId);
        state.reject(new DOMException('Service worker response failed.', 'NetworkError'));
        throw error;
      });
      this.waitUntil(response);
    }
  }
  Object.defineProperty(FetchEvent.prototype, Symbol.toStringTag, { value: 'FetchEvent', configurable: true });
  Object.defineProperty(g, 'FetchEvent', { value: FetchEvent, configurable: true, writable: true });
  N.registerProto('__sw_fetch', function (id, request, clientId, resultingClientId) {
    const controller = new g.AbortController();
    Object.defineProperty(request, 'signal', { value: controller.signal, configurable: true, enumerable: true });
    fetchSignals.set(id, controller);
    const event = new FetchEvent('fetch', { request, clientId, resultingClientId, cancelable: true });
    const state = fetchState.get(event), life = lifetime.get(event);
    life.dispatching = true; life.fetchId = id;
    dispatch.call(g, event);
    life.dispatching = false;
    // respondWith is synchronous even if waitUntil still keeps this event alive.
    if (!state.entered) {
      if (event.defaultPrevented) { const error = new DOMException('Fetch event was canceled.', 'NetworkError'); N.swFetchFailed(id); state.reject(error); }
      else { N.swFetchFallback(id); state.resolve(); }
    }
    queueMicrotask(() => finish(event));
  });
  N.registerProto('__sw_fetch_abort', function (id, encoded) {
    const controller = fetchSignals.get(id);
    fetchSignals.delete(id);
    if (!controller) return;
    let reason = new DOMException('Fetch aborted', 'AbortError');
    try {
      const record = JSON.parse(encoded);
      const decoded = record.kind === 'exception' ? new DOMException(record.message, record.name) : cloneCodec.deserialize(record.value);
      if (decoded !== undefined) reason = decoded;
    } catch (_) {}
    controller.abort(reason);
  });
  N.registerProto('__sw_fetch_complete', function (id) { fetchSignals.delete(id); });
  Object.defineProperty(g, 'registration', { get() { return api.registration(info.registration); }, configurable: true });
  Object.defineProperty(g, 'serviceWorker', { get() { return api.worker(info.worker); }, configurable: true });
  Object.defineProperty(g, 'skipWaiting', { value: function skipWaiting() { N.swSkipWaiting(); return Promise.resolve(); }, configurable: true, writable: true });
  // ------------------------------------------------------------ messaging
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const define = (name, value) => Object.defineProperty(g, name, { value, configurable: true, writable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const selfOrigin = new URL(g.location.href).origin;
  const messageState = new WeakMap();
  class ExtendableMessageEvent extends ExtendableEvent {
    constructor(type, init = {}) {
      if (!arguments.length) throw new TypeError("Failed to construct 'ExtendableMessageEvent': 1 argument required, but only 0 present.");
      if (init === null || init === undefined) init = {};
      super(type, init);
      const source = init.source === undefined ? null : init.source;
      if (source !== null && !(source instanceof Client) && !(source instanceof g.ServiceWorker) && !(source instanceof g.MessagePort)) throw new TypeError("Failed to construct 'ExtendableMessageEvent': The provided value is not of type '(Client or ServiceWorker or MessagePort)'.");
      const ports = init.ports === undefined ? [] : Array.from(init.ports);
      if (ports.some(p => !(p instanceof g.MessagePort))) throw new TypeError("Failed to construct 'ExtendableMessageEvent': ports must be MessagePorts.");
      messageState.set(this, { data: init.data === undefined ? null : init.data, origin: init.origin === undefined ? '' : `${init.origin}`, lastEventId: init.lastEventId === undefined ? '' : `${init.lastEventId}`, source, ports: Object.freeze(ports) });
    }
    get data() { return messageState.get(this).data; }
    get origin() { return messageState.get(this).origin; }
    get lastEventId() { return messageState.get(this).lastEventId; }
    get source() { return messageState.get(this).source; }
    get ports() { return messageState.get(this).ports; }
  }
  tag(ExtendableMessageEvent, 'ExtendableMessageEvent');
  define('ExtendableMessageEvent', ExtendableMessageEvent);

  // Client objects are snapshots of the host's client records.
  const clientState = new WeakMap();
  class Client {
    constructor() { ILLEGAL(); }
    get url() { return clientState.get(this).url; }
    get frameType() { return clientState.get(this).frameType; }
    get id() { return clientState.get(this).id; }
    get type() { return clientState.get(this).type; }
    get lifecycleState() { return 'active'; }
    postMessage(message, transfer = undefined) {
      if (!arguments.length) throw new TypeError("Failed to execute 'postMessage' on 'Client': 1 argument required, but only 0 present.");
      api.send('client', Number(clientState.get(this).id), message, transfer);
    }
  }
  tag(Client, 'Client');
  class WindowClient extends Client {
    constructor() { super(); }
    get visibilityState() { return 'visible'; }
    get focused() { return false; }
    get ancestorOrigins() { return Object.freeze([]); }
    focus() { return Promise.reject(new DOMException('Focusing a client requires a user activation (notificationclick).', 'InvalidAccessError')); }
    navigate(url) {
      return Promise.resolve().then(() => {
        new URL(`${url}`, g.location.href);
        throw new TypeError('WindowClient.navigate() is not available to this service worker.');
      });
    }
  }
  tag(WindowClient, 'WindowClient');
  const makeClient = record => {
    const c = Object.create(record.type === 'window' ? WindowClient.prototype : Client.prototype);
    clientState.set(c, { id: record.id, url: record.url, type: record.type, frameType: record.frameType });
    return c;
  };
  const clientById = id => { const rows = N.swClients('all', true, `${id}`); return rows.length ? makeClient(rows[0]) : undefined; };
  class Clients {
    constructor() { ILLEGAL(); }
    get(id) {
      if (!arguments.length) return Promise.reject(new TypeError("Failed to execute 'get' on 'Clients': 1 argument required, but only 0 present."));
      const key = `${id}`;
      return Promise.resolve().then(() => key === '' ? undefined : clientById(key));
    }
    matchAll(options = {}) {
      return new Promise((resolve, reject) => {
        try {
          if (options === null || options === undefined) options = {};
          if (typeof options !== 'object' && typeof options !== 'function') throw new TypeError('ClientQueryOptions must be a dictionary.');
          const type = options.type === undefined ? 'window' : `${options.type}`;
          if (!['window', 'worker', 'sharedworker', 'all'].includes(type)) throw new TypeError(`The provided value '${type}' is not a valid enum value of type ClientType.`);
          resolve(N.swClients(type, !!options.includeUncontrolled, '').map(makeClient));
        } catch (e) { reject(e); }
      });
    }
    openWindow(url) {
      return Promise.resolve().then(() => {
        new URL(`${url}`, g.location.href);
        throw new DOMException('Opening a window requires a user activation (notificationclick).', 'InvalidAccessError');
      });
    }
    claim() { try { N.swClaim(); return Promise.resolve(); } catch (e) { return Promise.reject(e); } }
  }
  tag(Clients, 'Clients');
  for (const [n, C] of [['Client', Client], ['WindowClient', WindowClient], ['Clients', Clients]]) define(n, C);
  const clients = Object.create(Clients.prototype);
  Object.defineProperty(g, 'clients', { get() { return clients; }, configurable: true, enumerable: true });

  // A message from a client (source kind 'client') or another service worker.
  N.registerProto('__sw_message', function (text, kind, id) {
    let m;
    try { m = JSON.parse(text); } catch (_) { return; }
    if (m.t === 'port') { api.portMessage(m, kind, id); return; }
    const source = kind === 'client' ? (clientById(id) || null) : api.worker(id);
    let data, type = 'message', ports = [];
    try { data = api.clone.deserialize(m.d); ports = api.incoming(kind, id, m.ports); } catch (_) { type = 'messageerror'; data = null; }
    const event = new ExtendableMessageEvent(type, { data, origin: selfOrigin, lastEventId: '', source, ports });
    const life = lifetime.get(event);
    life.dispatching = true;
    dispatch.call(g, event);
    life.dispatching = false;
    queueMicrotask(() => finish(event));
  });
})();
