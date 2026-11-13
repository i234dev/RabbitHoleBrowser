// A real service worker global, installed only in its own Worker realm.
(function () {
  'use strict';
  const g = globalThis, N = g.__rhNative, api = g.__rhSW;
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
  N.installHandlers(g, ['install', 'activate', 'message', 'messageerror']);
  N.registerProto('__sw_dispatch', function (type) {
    const e = new ExtendableEvent(type), s = lifetime.get(e);
    s.dispatching = true; s.report = true;
    dispatch.call(g, e);
    s.dispatching = false;
    queueMicrotask(() => finish(e));
  });
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
