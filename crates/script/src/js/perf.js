// High Resolution Time / Performance Timeline / User Timing / Navigation
// Timing / Resource Timing / Paint Timing / requestIdleCallback.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('perf');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const perf = g.performance;
  const nativeNow = perf.now.bind(perf);
  const origin = perf.timeOrigin;
  // Move the instance's own members onto Performance.prototype.
  for (const k of Object.getOwnPropertyNames(perf)) { try { delete perf[k]; } catch (e) {} }
  const entries = [];
  const observers = new Set();
  let nextId = 1;
  let navigationId = 1;

  class PerformanceEntry {
    constructor() { ILLEGAL(); }
    get name() { return this[S].name; }
    get entryType() { return this[S].entryType; }
    get startTime() { return this[S].startTime; }
    get duration() { return this[S].duration; }
    get id() { return this[S].id; }
    get navigationId() { return this[S].navigationId; }
    toJSON() { const o = {}; for (let p = Object.getPrototypeOf(this); p && p !== Object.prototype; p = Object.getPrototypeOf(p)) for (const k of Object.getOwnPropertyNames(p)) { const d = Object.getOwnPropertyDescriptor(p, k); if (d.get && !(k in o)) { const v = this[k]; if (typeof v !== 'function' && typeof v !== 'object' || v === null) o[k] = v; } } return o; }
  }
  tag(PerformanceEntry, 'PerformanceEntry');
  function entry(C, data) {
    const e = Object.create(C.prototype);
    hidden(e, S, Object.assign({ id: nextId++, navigationId, duration: 0 }, data));
    return e;
  }
  function record(e) {
    entries.push(e);
    for (const o of observers) {
      if (!o[S].types.has(e.entryType)) continue;
      o[S].queue.push(e);
      if (!o[S].scheduled) { o[S].scheduled = true; setTimeout(() => deliver(o), 0); }
    }
  }
  function deliver(o) {
    o[S].scheduled = false;
    const list = o[S].queue;
    o[S].queue = [];
    if (!list.length) return;
    const l = Object.create(PerformanceObserverEntryList.prototype);
    hidden(l, S, list);
    try { o[S].callback.call(o, l, o, { droppedEntriesCount: 0 }); } catch (e) { console.error('Uncaught ' + (e && e.stack || e)); }
  }

  class PerformanceMark extends PerformanceEntry {
    constructor(name, options = {}) {
      super();
      void 0;
      throw new Error('unreachable');
    }
    get detail() { return this[S].detail; }
  }
  // Constructible PerformanceMark (super() above never runs).
  const PM = function PerformanceMark(name, options = {}) {
    if (!new.target) throw new TypeError("Failed to construct 'PerformanceMark': Please use the 'new' operator.");
    if (arguments.length < 1) throw new TypeError("Failed to construct 'PerformanceMark': 1 argument required, but only 0 present.");
    const start = options && options.startTime !== undefined ? +options.startTime : nativeNow();
    if (start < 0) throw new TypeError("Failed to construct 'PerformanceMark': 'startTime' cannot be negative.");
    const detail = options && options.detail !== undefined ? g.structuredClone(options.detail) : null;
    const e = Object.create(new.target.prototype);
    hidden(e, S, { id: nextId++, navigationId, name: String(name), entryType: 'mark', startTime: start, duration: 0, detail });
    return e;
  };
  PM.prototype = PerformanceMark.prototype;
  Object.defineProperty(PerformanceMark.prototype, 'constructor', { value: PM, writable: true, configurable: true });
  Object.setPrototypeOf(PM, PerformanceEntry);
  tag(PerformanceMark, 'PerformanceMark');
  class PerformanceMeasure extends PerformanceEntry { get detail() { return this[S].detail; } }
  tag(PerformanceMeasure, 'PerformanceMeasure');
  class PerformanceNavigationTimingBase extends PerformanceEntry {}
  void PerformanceNavigationTimingBase;
  const RT_FIELDS = ['initiatorType', 'deliveryType', 'nextHopProtocol', 'workerStart', 'redirectStart', 'redirectEnd', 'fetchStart', 'domainLookupStart', 'domainLookupEnd', 'connectStart', 'connectEnd', 'secureConnectionStart', 'requestStart', 'finalResponseHeadersStart', 'firstInterimResponseStart', 'responseStart', 'responseEnd', 'transferSize', 'encodedBodySize', 'decodedBodySize', 'responseStatus', 'renderBlockingStatus', 'contentType', 'contentEncoding'];
  class PerformanceResourceTiming extends PerformanceEntry {
    get serverTiming() { return Object.freeze([]); }
  }
  for (const k of RT_FIELDS) Object.defineProperty(PerformanceResourceTiming.prototype, k, { get() { return this[S][k] ?? (typeof this[S].fetchStart === 'number' && /Start|End|Size|Status/.test(k) ? 0 : ''); }, configurable: true, enumerable: true });
  tag(PerformanceResourceTiming, 'PerformanceResourceTiming');
  class PerformanceNavigationTiming extends PerformanceResourceTiming {}
  const NT_FIELDS = ['unloadEventStart', 'unloadEventEnd', 'domInteractive', 'domContentLoadedEventStart', 'domContentLoadedEventEnd', 'domComplete', 'loadEventStart', 'loadEventEnd', 'type', 'redirectCount', 'criticalCHRestart', 'activationStart', 'notRestoredReasons', 'confidence'];
  for (const k of NT_FIELDS) Object.defineProperty(PerformanceNavigationTiming.prototype, k, { get() { return this[S][k] ?? (k === 'type' ? 'navigate' : k === 'notRestoredReasons' || k === 'confidence' ? null : 0); }, configurable: true, enumerable: true });
  tag(PerformanceNavigationTiming, 'PerformanceNavigationTiming');
  class PerformanceTimingConfidence { constructor() { ILLEGAL(); } get randomizedTriggerRate() { return 0; } get value() { return 'high'; } toJSON() { return { randomizedTriggerRate: 0, value: 'high' }; } }
  tag(PerformanceTimingConfidence, 'PerformanceTimingConfidence');
  class PerformancePaintTiming extends PerformanceEntry {
    get paintTime() { return this[S].startTime; }
    get presentationTime() { return this[S].startTime; }
  }
  tag(PerformancePaintTiming, 'PerformancePaintTiming');
  class VisibilityStateEntry extends PerformanceEntry {}
  tag(VisibilityStateEntry, 'VisibilityStateEntry');

  // Legacy Navigation Timing (performance.timing / performance.navigation).
  const navTimes = { start: 0, interactive: 0, dclStart: 0, dclEnd: 0, complete: 0, loadStart: 0, loadEnd: 0 };
  class PerformanceTiming { constructor() { ILLEGAL(); } toJSON() { const o = {}; for (const k of TIMING_FIELDS) o[k] = this[k]; return o; } }
  const TIMING_FIELDS = ['navigationStart', 'unloadEventStart', 'unloadEventEnd', 'redirectStart', 'redirectEnd', 'fetchStart', 'domainLookupStart', 'domainLookupEnd', 'connectStart', 'connectEnd', 'secureConnectionStart', 'requestStart', 'responseStart', 'responseEnd', 'domLoading', 'domInteractive', 'domContentLoadedEventStart', 'domContentLoadedEventEnd', 'domComplete', 'loadEventStart', 'loadEventEnd'];
  const abs = (rel) => Math.round(origin + rel);
  for (const k of TIMING_FIELDS) {
    Object.defineProperty(PerformanceTiming.prototype, k, {
      get() {
        switch (k) {
          case 'unloadEventStart': case 'unloadEventEnd': case 'redirectStart': case 'redirectEnd': case 'secureConnectionStart': return 0;
          case 'domInteractive': return navTimes.interactive ? abs(navTimes.interactive) : 0;
          case 'domContentLoadedEventStart': return navTimes.dclStart ? abs(navTimes.dclStart) : 0;
          case 'domContentLoadedEventEnd': return navTimes.dclEnd ? abs(navTimes.dclEnd) : 0;
          case 'domComplete': return navTimes.complete ? abs(navTimes.complete) : 0;
          case 'loadEventStart': return navTimes.loadStart ? abs(navTimes.loadStart) : 0;
          case 'loadEventEnd': return navTimes.loadEnd ? abs(navTimes.loadEnd) : 0;
          default: return Math.round(origin);
        }
      }, configurable: true, enumerable: true,
    });
  }
  tag(PerformanceTiming, 'PerformanceTiming');
  class PerformanceNavigation { constructor() { ILLEGAL(); } get type() { return 0; } get redirectCount() { return 0; } toJSON() { return { type: 0, redirectCount: 0 }; } }
  for (const [k, v] of Object.entries({ TYPE_NAVIGATE: 0, TYPE_RELOAD: 1, TYPE_BACK_FORWARD: 2, TYPE_RESERVED: 255 })) { Object.defineProperty(PerformanceNavigation, k, { value: v, enumerable: true }); Object.defineProperty(PerformanceNavigation.prototype, k, { value: v, enumerable: true }); }
  tag(PerformanceNavigation, 'PerformanceNavigation');
  const timingObj = Object.create(PerformanceTiming.prototype);
  const navigationObj = Object.create(PerformanceNavigation.prototype);

  const navEntry = entry(PerformanceNavigationTiming, { name: document.URL, entryType: 'navigation', startTime: 0, initiatorType: 'navigation', deliveryType: '', nextHopProtocol: 'http/1.1', fetchStart: 0, requestStart: 0, responseStart: 0, responseEnd: 0, transferSize: 0, encodedBodySize: 0, decodedBodySize: 0, responseStatus: 200, type: 'navigate' });
  entries.push(navEntry);
  const mark = (k) => () => { navTimes[k] = nativeNow(); };
  document.addEventListener('readystatechange', () => {
    const t = nativeNow();
    if (document.readyState === 'interactive') { navTimes.interactive = t; navEntry[S].domInteractive = t; }
    if (document.readyState === 'complete') { navTimes.complete = t; navEntry[S].domComplete = t; }
  });
  document.addEventListener('DOMContentLoaded', () => { navTimes.dclStart = nativeNow(); navEntry[S].domContentLoadedEventStart = navTimes.dclStart; setTimeout(() => {}, 0); navTimes.dclEnd = navTimes.dclStart; navEntry[S].domContentLoadedEventEnd = navTimes.dclStart; }, { capture: true });
  g.addEventListener('load', () => {
    navTimes.loadStart = nativeNow();
    navEntry[S].loadEventStart = navTimes.loadStart;
    setTimeout(() => {
      navTimes.loadEnd = nativeNow();
      navEntry[S].loadEventEnd = navTimes.loadEnd;
      navEntry[S].duration = navTimes.loadEnd;
      // Paint timing: the first rendering after load.
      const fp = Math.min(navTimes.interactive || navTimes.loadStart, navTimes.loadStart);
      record(entry(PerformancePaintTiming, { name: 'first-paint', entryType: 'paint', startTime: fp }));
      record(entry(PerformancePaintTiming, { name: 'first-contentful-paint', entryType: 'paint', startTime: fp }));
    }, 0);
  }, { capture: true });
  void mark;
  entries.push(entry(VisibilityStateEntry, { name: 'visible', entryType: 'visibility-state', startTime: 0 }));

  // ---------------------------------------------------------- Performance
  const P = g.Performance;
  const pm = (name, fn) => hidden(P.prototype, name, fn);
  pm('now', function now() { return nativeNow(); });
  Object.defineProperty(P.prototype, 'timeOrigin', { get() { return origin; }, configurable: true, enumerable: true });
  Object.defineProperty(P.prototype, 'timing', { get() { return timingObj; }, configurable: true, enumerable: true });
  Object.defineProperty(P.prototype, 'navigation', { get() { return navigationObj; }, configurable: true, enumerable: true });
  Object.defineProperty(P.prototype, 'eventCounts', { get() { return eventCounts; }, configurable: true, enumerable: true });
  Object.defineProperty(P.prototype, 'interactionCount', { get() { return 0; }, configurable: true, enumerable: true });
  const eventCounts = new Map();
  pm('toJSON', function toJSON() { return { timeOrigin: origin, timing: timingObj.toJSON(), navigation: navigationObj.toJSON(), eventCounts: {} }; });
  const visible = () => entries.filter((e) => e.entryType !== 'resource' || resourceBuffer.includes(e));
  pm('getEntries', function getEntries() { return visible().slice().sort((a, b) => a.startTime - b.startTime); });
  pm('getEntriesByType', function getEntriesByType(type) { return visible().filter((e) => e.entryType === String(type)); });
  pm('getEntriesByName', function getEntriesByName(name, type) { return visible().filter((e) => e.name === String(name) && (type === undefined || e.entryType === String(type))); });
  pm('mark', function mark(name, options) {
    if (TIMING_FIELDS.includes(String(name))) throw new SyntaxError(`Failed to execute 'mark' on 'Performance': '${name}' is part of the PerformanceTiming interface, and cannot be used as a mark name.`);
    const e = new PM(name, options);
    record(e);
    return e;
  });
  const markTime = (v, what) => {
    if (typeof v === 'number') { if (v < 0) throw new TypeError(`'${what}' cannot be negative.`); return v; }
    const s = String(v);
    for (let i = entries.length - 1; i >= 0; i--) if (entries[i].entryType === 'mark' && entries[i].name === s) return entries[i].startTime;
    if (TIMING_FIELDS.includes(s)) { const t = timingObj[s]; if (!t) throw domError(`Failed to execute 'measure' on 'Performance': '${s}' is empty: either the event hasn't happened yet, or it would provide cross-origin timing information.`, 'InvalidAccessError'); return t - origin; }
    throw new SyntaxError(`Failed to execute 'measure' on 'Performance': The mark '${s}' does not exist.`);
  };
  pm('measure', function measure(name, startOrOptions, endMark) {
    let start = 0, end = nativeNow(), detail = null;
    if (startOrOptions && typeof startOrOptions === 'object') {
      const o = startOrOptions;
      if (o.start !== undefined) start = markTime(o.start, 'start');
      if (o.end !== undefined) end = markTime(o.end, 'end');
      if (o.duration !== undefined) { if (o.end !== undefined) end = end; else end = start + +o.duration; if (o.start === undefined && o.end !== undefined) start = end - +o.duration; }
      if (o.detail !== undefined) detail = g.structuredClone(o.detail);
    } else {
      if (startOrOptions !== undefined) start = markTime(startOrOptions, 'start');
      if (endMark !== undefined) end = markTime(endMark, 'end');
    }
    const e = entry(PerformanceMeasure, { name: String(name), entryType: 'measure', startTime: start, duration: end - start, detail });
    record(e);
    return e;
  });
  const clear = (type) => function (name) { for (let i = entries.length - 1; i >= 0; i--) if (entries[i].entryType === type && (name === undefined || entries[i].name === String(name))) entries.splice(i, 1); };
  pm('clearMarks', clear('mark'));
  pm('clearMeasures', clear('measure'));
  let resourceBuffer = [];
  let bufferSize = 250;
  pm('clearResourceTimings', function clearResourceTimings() { resourceBuffer = []; });
  pm('setResourceTimingBufferSize', function setResourceTimingBufferSize(n) { bufferSize = n >>> 0; });
  pm('measureUserAgentSpecificMemory', function () { return Promise.resolve({ bytes: 0, breakdown: [] }); });
  N.installHandlers(P.prototype, ['resourcetimingbufferfull']);
  // Resource entries for fetch() / XMLHttpRequest.
  function addResource(name, initiatorType, start, status) {
    const end = nativeNow();
    const e = entry(PerformanceResourceTiming, { name, entryType: 'resource', startTime: start, duration: end - start, initiatorType, deliveryType: '', nextHopProtocol: 'http/1.1', fetchStart: start, requestStart: start, responseStart: end, responseEnd: end, transferSize: 0, encodedBodySize: 0, decodedBodySize: 0, responseStatus: status || 0, contentType: '', contentEncoding: '' });
    entries.push(e);
    if (resourceBuffer.length < bufferSize) resourceBuffer.push(e);
    else perf.dispatchEvent(new Event('resourcetimingbufferfull'));
    for (const o of observers) if (o[S].types.has('resource')) { o[S].queue.push(e); if (!o[S].scheduled) { o[S].scheduled = true; setTimeout(() => deliver(o), 0); } }
  }
  const nativeFetch = g.fetch;
  hidden(g, 'fetch', function fetch(input, init) {
    const start = nativeNow();
    let url;
    try { url = new URL(input instanceof g.Request ? input.url : String(input), document.baseURI).href; } catch (e) { url = String(input); }
    const p = nativeFetch.call(this, input, init);
    p.then((r) => addResource(url, 'fetch', start, r.status), () => addResource(url, 'fetch', start, 0));
    return p;
  });
  if (g.XMLHttpRequest) {
    const open = g.XMLHttpRequest.prototype.open, send = g.XMLHttpRequest.prototype.send;
    hidden(g.XMLHttpRequest.prototype, 'open', function (method, url) { hidden(this, S, { url: (() => { try { return new URL(url, document.baseURI).href; } catch (e) { return String(url); } })() }); return open.apply(this, arguments); });
    hidden(g.XMLHttpRequest.prototype, 'send', function () { const st = this[S]; if (st) { const start = nativeNow(); this.addEventListener('loadend', () => addResource(st.url, 'xmlhttprequest', start, this.status), { once: true }); } return send.apply(this, arguments); });
  }

  // ------------------------------------------------- PerformanceObserver
  const SUPPORTED = Object.freeze(['mark', 'measure', 'navigation', 'resource', 'paint', 'visibility-state']);
  class PerformanceObserverEntryList {
    constructor() { ILLEGAL(); }
    getEntries() { return this[S].slice(); }
    getEntriesByType(t) { return this[S].filter((e) => e.entryType === String(t)); }
    getEntriesByName(n, t) { return this[S].filter((e) => e.name === String(n) && (t === undefined || e.entryType === String(t))); }
  }
  tag(PerformanceObserverEntryList, 'PerformanceObserverEntryList');
  class PerformanceObserver {
    constructor(callback) {
      if (typeof callback !== 'function') throw new TypeError("Failed to construct 'PerformanceObserver': The callback provided as parameter 1 is not a function.");
      hidden(this, S, { callback, types: new Set(), queue: [], scheduled: false });
    }
    static get supportedEntryTypes() { return SUPPORTED; }
    observe(options = {}) {
      const types = options.entryTypes ? Array.from(options.entryTypes) : options.type !== undefined ? [options.type] : null;
      if (!types) throw new TypeError("Failed to execute 'observe' on 'PerformanceObserver': An observe() call must include either entryTypes or type arguments.");
      if (options.entryTypes && options.type !== undefined) throw new TypeError("Failed to execute 'observe' on 'PerformanceObserver': An observe() call must not include both entryTypes and type arguments.");
      if (options.entryTypes) this[S].types.clear();
      for (const t of types) if (SUPPORTED.includes(String(t))) this[S].types.add(String(t));
      if (!this[S].types.size) return;
      observers.add(this);
      if (options.buffered && options.type !== undefined) {
        const buffered = entries.filter((e) => e.entryType === String(options.type));
        if (buffered.length) { this[S].queue.push(...buffered); if (!this[S].scheduled) { this[S].scheduled = true; setTimeout(() => deliver(this), 0); } }
      }
    }
    disconnect() { observers.delete(this); this[S].queue = []; }
    takeRecords() { const q = this[S].queue; this[S].queue = []; return q; }
  }
  tag(PerformanceObserver, 'PerformanceObserver');

  // -------------------------------------------------- requestIdleCallback
  class IdleDeadline {
    constructor() { ILLEGAL(); }
    get didTimeout() { return this[S].didTimeout; }
    timeRemaining() { return Math.max(0, this[S].deadline - nativeNow()); }
  }
  tag(IdleDeadline, 'IdleDeadline');
  const idle = new Map();
  let idleId = 0;
  hidden(g, 'requestIdleCallback', function requestIdleCallback(cb, options) {
    if (typeof cb !== 'function') throw new TypeError("Failed to execute 'requestIdleCallback' on 'Window': The callback provided as parameter 1 is not a function.");
    const id = ++idleId;
    const timeout = options && options.timeout > 0 ? +options.timeout : 0;
    const start = nativeNow();
    // Idle periods: after pending work, ~1 frame later.
    const t = setTimeout(() => {
      if (!idle.has(id)) return;
      idle.delete(id);
      const d = Object.create(IdleDeadline.prototype);
      hidden(d, S, { didTimeout: timeout > 0 && nativeNow() - start >= timeout, deadline: nativeNow() + 49 });
      cb(d);
    }, 1);
    idle.set(id, t);
    return id;
  });
  hidden(g, 'cancelIdleCallback', function cancelIdleCallback(id) { const t = idle.get(id); if (t !== undefined) { clearTimeout(t); idle.delete(id); } });

  define('PerformanceEntry', PerformanceEntry);
  define('PerformanceMark', PM);
  define('PerformanceMeasure', PerformanceMeasure);
  define('PerformanceResourceTiming', PerformanceResourceTiming);
  define('PerformanceNavigationTiming', PerformanceNavigationTiming);
  define('PerformanceTimingConfidence', PerformanceTimingConfidence);
  define('PerformancePaintTiming', PerformancePaintTiming);
  define('VisibilityStateEntry', VisibilityStateEntry);
  define('PerformanceTiming', PerformanceTiming);
  define('PerformanceNavigation', PerformanceNavigation);
  define('PerformanceObserver', PerformanceObserver);
  define('PerformanceObserverEntryList', PerformanceObserverEntryList);
  define('IdleDeadline', IdleDeadline);
})();
