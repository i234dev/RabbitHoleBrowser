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
  Object.defineProperty(g, 'clients', { value: {
    claim() { try { N.swClaim(); return Promise.resolve(); } catch (e) { return Promise.reject(e); } },
    matchAll() { return Promise.reject(new DOMException('Client enumeration is not implemented yet.', 'NotSupportedError')); },
    get() { return Promise.reject(new DOMException('Client enumeration is not implemented yet.', 'NotSupportedError')); },
    openWindow() { return Promise.reject(new DOMException('Opening windows from service workers is not implemented yet.', 'NotSupportedError')); },
  }, configurable: true });
})();
