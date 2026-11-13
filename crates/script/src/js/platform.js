// Window / Navigator level APIs: Permissions, Notifications, Storage
// manager, Geolocation, Screen orientation, Clipboard, Web Locks,
// Scheduling, Trusted Types, Fullscreen, VisualViewport, MediaQueryList
// change events, BarProp, External, plugins, Origin, UserActivation,
// CloseWatcher, DataTransfer, document.all, CSSOM View extras, console.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('platform');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const make = (C, state) => { const o = Object.create(C.prototype); hidden(o, S, state || {}); return o; };
  const nav = g.navigator;
  const setNav = (name, value) => { try { delete nav[name]; } catch (e) {} Object.defineProperty(g.Navigator.prototype, name, { get() { return value; }, configurable: true, enumerable: true }); };
  const method = (C, name, fn, force) => { if (C && (force || !Object.prototype.hasOwnProperty.call(C.prototype, name))) hidden(C.prototype, name, fn); };
  const acc = (C, name, get, set) => { if (C && !Object.prototype.hasOwnProperty.call(C.prototype, name)) Object.defineProperty(C.prototype, name, { get, set, configurable: true, enumerable: true }); };

  // ----------------------------------------------------------- Permissions
  const PERMS = { geolocation: 'denied', notifications: 'denied', push: 'denied', midi: 'denied', camera: 'denied', microphone: 'denied', 'background-fetch': 'denied', 'background-sync': 'granted', 'persistent-storage': 'granted', 'screen-wake-lock': 'denied', 'storage-access': 'granted', 'clipboard-read': 'denied', 'clipboard-write': 'granted', 'local-fonts': 'denied', 'idle-detection': 'denied', 'window-management': 'denied', accelerometer: 'granted', gyroscope: 'granted', magnetometer: 'granted', 'payment-handler': 'denied', 'display-capture': 'denied' };
  class PermissionStatus extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get state() { return this[S].state; }
    get name() { return this[S].name; }
  }
  tag(PermissionStatus, 'PermissionStatus');
  N.installHandlers(PermissionStatus.prototype, ['change']);
  class Permissions {
    constructor() { ILLEGAL(); }
    query(desc) {
      if (!desc || typeof desc.name !== 'string') return Promise.reject(new TypeError("Failed to execute 'query' on 'Permissions': Failed to read the 'name' property from 'PermissionDescriptor': Required member is undefined."));
      if (!(desc.name in PERMS)) return Promise.reject(new TypeError(`Failed to execute 'query' on 'Permissions': The provided value '${desc.name}' is not a valid enum value of type PermissionName.`));
      return Promise.resolve(make(PermissionStatus, { state: PERMS[desc.name], name: desc.name }));
    }
    request(desc) { return this.query(desc); }
    revoke(desc) { return this.query(desc); }
  }
  tag(Permissions, 'Permissions');
  define('PermissionStatus', PermissionStatus);
  define('Permissions', Permissions);
  setNav('permissions', make(Permissions));

  // ---------------------------------------------------------- Notification
  class Notification extends g.EventTarget {
    constructor(title, options = {}) {
      super();
      if (arguments.length < 1) throw new TypeError("Failed to construct 'Notification': 1 argument required, but only 0 present.");
      if (options.silent && options.vibrate) throw new TypeError("Failed to construct 'Notification': Silent notifications must not specify vibration patterns.");
      hidden(this, S, { title: String(title), dir: options.dir || 'auto', lang: options.lang || '', body: options.body || '', tag: options.tag || '', image: options.image || '', icon: options.icon || '', badge: options.badge || '', vibrate: Object.freeze(Array.from(options.vibrate || [])), timestamp: options.timestamp ?? Date.now(), renotify: !!options.renotify, silent: options.silent ?? null, requireInteraction: !!options.requireInteraction, data: options.data === undefined ? null : g.structuredClone(options.data), actions: Object.freeze(Array.from(options.actions || [])), navigate: options.navigate || '' });
      // Permission is denied: the notification fails to show.
      setTimeout(() => this.dispatchEvent(new Event('error')), 0);
    }
    static get permission() { return 'denied'; }
    static requestPermission(cb) { const p = Promise.resolve('denied'); if (typeof cb === 'function') p.then(cb); return p; }
    static get maxActions() { return 2; }
    close() {}
  }
  for (const k of ['title', 'dir', 'lang', 'body', 'tag', 'image', 'icon', 'badge', 'vibrate', 'timestamp', 'renotify', 'silent', 'requireInteraction', 'data', 'actions', 'navigate']) Object.defineProperty(Notification.prototype, k, { get() { return this[S][k]; }, configurable: true, enumerable: true });
  tag(Notification, 'Notification');
  N.installHandlers(Notification.prototype, ['click', 'show', 'error', 'close']);
  define('Notification', Notification);

  // -------------------------------------------------------- StorageManager
  class StorageManager {
    constructor() { ILLEGAL(); }
    estimate() { let usage = 0; try { for (let i = 0; i < localStorage.length; i++) { const k = localStorage.key(i); usage += (k.length + (localStorage.getItem(k) || '').length) * 2; } } catch (e) {} return Promise.resolve({ quota: 1073741824, usage, usageDetails: {} }); }
    persist() { return Promise.resolve(true); }
    persisted() { return Promise.resolve(true); }
    getDirectory() { return Promise.reject(domError('The Origin Private File System is not available.', 'SecurityError')); }
  }
  tag(StorageManager, 'StorageManager');
  define('StorageManager', StorageManager);
  setNav('storage', make(StorageManager));

  // ------------------------------------------------------------ Geolocation
  class GeolocationPositionError {
    constructor() { ILLEGAL(); }
    get code() { return this[S].code; }
    get message() { return this[S].message; }
  }
  for (const [k, v] of Object.entries({ PERMISSION_DENIED: 1, POSITION_UNAVAILABLE: 2, TIMEOUT: 3 })) { Object.defineProperty(GeolocationPositionError, k, { value: v, enumerable: true }); Object.defineProperty(GeolocationPositionError.prototype, k, { value: v, enumerable: true }); }
  tag(GeolocationPositionError, 'GeolocationPositionError');
  class GeolocationCoordinates { constructor() { ILLEGAL(); } toJSON() { const o = {}; for (const k of COORDS) o[k] = this[k]; return o; } }
  const COORDS = ['accuracy', 'latitude', 'longitude', 'altitude', 'altitudeAccuracy', 'heading', 'speed'];
  for (const k of COORDS) Object.defineProperty(GeolocationCoordinates.prototype, k, { get() { return this[S][k] ?? null; }, configurable: true, enumerable: true });
  tag(GeolocationCoordinates, 'GeolocationCoordinates');
  class GeolocationPosition { constructor() { ILLEGAL(); } get coords() { return this[S].coords; } get timestamp() { return this[S].timestamp; } toJSON() { return { coords: this.coords.toJSON(), timestamp: this.timestamp }; } }
  tag(GeolocationPosition, 'GeolocationPosition');
  let watchId = 0;
  class Geolocation {
    constructor() { ILLEGAL(); }
    getCurrentPosition(success, error) {
      if (typeof success !== 'function') throw new TypeError("Failed to execute 'getCurrentPosition' on 'Geolocation': The callback provided as parameter 1 is not a function.");
      if (typeof error === 'function') setTimeout(() => error(make(GeolocationPositionError, { code: 1, message: 'User denied Geolocation' })), 0);
    }
    watchPosition(success, error) { this.getCurrentPosition(success, error); return ++watchId; }
    clearWatch() {}
  }
  tag(Geolocation, 'Geolocation');
  define('Geolocation', Geolocation);
  define('GeolocationPosition', GeolocationPosition);
  define('GeolocationCoordinates', GeolocationCoordinates);
  define('GeolocationPositionError', GeolocationPositionError);
  setNav('geolocation', make(Geolocation));

  // ----------------------------------------------------- ScreenOrientation
  class ScreenOrientation extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get type() { return g.innerWidth >= g.innerHeight ? 'landscape-primary' : 'portrait-primary'; }
    get angle() { return 0; }
    lock() { return Promise.reject(domError('screen.orientation.lock() is not available on this device.', 'NotSupportedError')); }
    unlock() {}
  }
  tag(ScreenOrientation, 'ScreenOrientation');
  N.installHandlers(ScreenOrientation.prototype, ['change']);
  define('ScreenOrientation', ScreenOrientation);
  if (g.screen) { const so = make(ScreenOrientation); try { delete g.screen.orientation; } catch (e) {} Object.defineProperty(g.Screen.prototype, 'orientation', { get() { return so; }, configurable: true, enumerable: true }); }
  for (const k of ['isExtended']) acc(g.Screen, k, function () { return false; });
  N.installHandlers(g.Screen.prototype, ['change']);

  // -------------------------------------------------------------- Clipboard
  let clipboardItems = [];
  class ClipboardItem {
    constructor(items, options = {}) {
      if (!items || typeof items !== 'object' || !Object.keys(items).length) throw new TypeError("Failed to construct 'ClipboardItem': Empty dictionary argument");
      hidden(this, S, { items: Object.assign({}, items), style: options.presentationStyle || 'unspecified' });
    }
    static supports(type) { return ['text/plain', 'text/html', 'image/png', 'text/uri-list', 'image/svg+xml'].includes(String(type)) || String(type).startsWith('web '); }
    get types() { return Object.freeze(Object.keys(this[S].items)); }
    get presentationStyle() { return this[S].style; }
    getType(type) {
      if (!(type in this[S].items)) return Promise.reject(domError(`Failed to execute 'getType' on 'ClipboardItem': The type was not found`, 'NotFoundError'));
      return Promise.resolve(this[S].items[type]).then((v) => (v instanceof g.Blob ? v : new g.Blob([String(v)], { type })));
    }
  }
  tag(ClipboardItem, 'ClipboardItem');
  class Clipboard extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    read() { return Promise.resolve(clipboardItems.slice()); }
    readText() {
      const item = clipboardItems.find((i) => i.types.includes('text/plain'));
      return item ? item.getType('text/plain').then((b) => b.text()) : Promise.resolve('');
    }
    write(items) { clipboardItems = Array.from(items); document.dispatchEvent(new Event('clipboardchange')); return Promise.resolve(); }
    writeText(text) { clipboardItems = [new ClipboardItem({ 'text/plain': String(text) })]; return Promise.resolve(); }
  }
  tag(Clipboard, 'Clipboard');
  define('Clipboard', Clipboard);
  define('ClipboardItem', ClipboardItem);
  setNav('clipboard', make(Clipboard));
  if (g.ClipboardEvent) {
    // ClipboardEvent's clipboardData is a DataTransfer.
    Object.defineProperty(g.ClipboardEvent.prototype, 'clipboardData', { get() { return null; }, configurable: true, enumerable: true });
  }

  // -------------------------------------------------------------- Web Locks
  const held = [];
  const pending = [];
  class Lock { constructor() { ILLEGAL(); } get name() { return this[S].name; } get mode() { return this[S].mode; } }
  tag(Lock, 'Lock');
  const grantable = (name, mode) => !held.some((l) => l.name === name && (mode === 'exclusive' || l.mode === 'exclusive'));
  function processLocks() {
    for (let i = 0; i < pending.length; i++) {
      const r = pending[i];
      if (!grantable(r.name, r.mode) || pending.slice(0, i).some((p) => p.name === r.name)) continue;
      pending.splice(i--, 1);
      grant(r);
    }
  }
  function grant(r) {
    const lock = make(Lock, { name: r.name, mode: r.mode });
    const rec = { name: r.name, mode: r.mode, clientId: 'page' };
    held.push(rec);
    let result;
    try { result = Promise.resolve(r.callback(lock)); } catch (e) { result = Promise.reject(e); }
    result.then(r.resolve, r.reject).then(() => { held.splice(held.indexOf(rec), 1); processLocks(); });
  }
  class LockManager {
    constructor() { ILLEGAL(); }
    request(name, options, callback) {
      if (typeof options === 'function') { callback = options; options = {}; }
      if (typeof callback !== 'function') return Promise.reject(new TypeError("Failed to execute 'request' on 'LockManager': The callback provided as parameter 2 is not a function."));
      name = String(name);
      if (name.startsWith('-')) return Promise.reject(domError("Failed to execute 'request' on 'LockManager': Names cannot start with '-'.", 'NotSupportedError'));
      const mode = options.mode || 'exclusive';
      return new Promise((resolve, reject) => {
        const r = { name, mode, callback, resolve, reject };
        if (options.signal && options.signal.aborted) { reject(options.signal.reason); return; }
        if (options.steal) { held.splice(0, held.length, ...held.filter((l) => l.name !== name)); grant(r); return; }
        if (!grantable(name, mode) || pending.some((p) => p.name === name)) {
          if (options.ifAvailable) { Promise.resolve(callback(null)).then(resolve, reject); return; }
          pending.push(r);
          if (options.signal) options.signal.addEventListener('abort', () => { const i = pending.indexOf(r); if (i >= 0) { pending.splice(i, 1); reject(options.signal.reason); } });
          return;
        }
        grant(r);
      });
    }
    query() { return Promise.resolve({ held: held.map((l) => ({ name: l.name, mode: l.mode, clientId: l.clientId })), pending: pending.map((p) => ({ name: p.name, mode: p.mode, clientId: 'page' })) }); }
  }
  tag(LockManager, 'LockManager');
  define('Lock', Lock);
  define('LockManager', LockManager);
  setNav('locks', make(LockManager));

  // ------------------------------------------------------------- Scheduling
  const PRIORITIES = ['user-blocking', 'user-visible', 'background'];
  class TaskSignal extends g.AbortSignal {
    constructor() { super(); ILLEGAL(); }
    get priority() { return this[S] ? this[S].priority : 'user-visible'; }
    static any(signals, init = {}) { const s = g.AbortSignal.any(signals); Object.setPrototypeOf(s, TaskSignal.prototype); hidden(s, S, { priority: init.priority instanceof TaskSignal ? init.priority.priority : init.priority || 'user-visible' }); return s; }
  }
  tag(TaskSignal, 'TaskSignal');
  N.installHandlers(TaskSignal.prototype, ['prioritychange']);
  class TaskController extends g.AbortController {
    constructor(init = {}) {
      super();
      const p = init.priority || 'user-visible';
      if (!PRIORITIES.includes(p)) throw new TypeError(`Failed to construct 'TaskController': The provided value '${p}' is not a valid enum value of type TaskPriority.`);
      Object.setPrototypeOf(this.signal, TaskSignal.prototype);
      hidden(this.signal, S, { priority: p });
    }
    setPriority(p) {
      if (!PRIORITIES.includes(p)) throw new TypeError(`Failed to execute 'setPriority' on 'TaskController': The provided value '${p}' is not a valid enum value of type TaskPriority.`);
      const sig = this.signal;
      const prev = sig[S].priority;
      if (prev === p) return;
      sig[S].priority = p;
      sig.dispatchEvent(new g.TaskPriorityChangeEvent('prioritychange', { previousPriority: prev }));
    }
  }
  tag(TaskController, 'TaskController');
  class Scheduler {
    constructor() { ILLEGAL(); }
    postTask(cb, options = {}) {
      if (typeof cb !== 'function') return Promise.reject(new TypeError("Failed to execute 'postTask' on 'Scheduler': The callback provided as parameter 1 is not a function."));
      const signal = options.signal;
      if (signal && signal.aborted) return Promise.reject(signal.reason);
      const delay = Math.max(0, +options.delay || 0);
      const prio = options.priority || (signal && signal[S] ? signal[S].priority : 'user-visible');
      return new Promise((resolve, reject) => {
        const t = setTimeout(() => {
          if (signal && signal.aborted) { reject(signal.reason); return; }
          try { resolve(cb()); } catch (e) { reject(e); }
        }, delay + (prio === 'background' ? 1 : 0));
        if (signal) signal.addEventListener('abort', () => { clearTimeout(t); reject(signal.reason); });
      });
    }
    yield() { return new Promise((r) => setTimeout(r, 0)); }
  }
  tag(Scheduler, 'Scheduler');
  define('TaskSignal', TaskSignal);
  define('TaskController', TaskController);
  define('Scheduler', Scheduler);
  hidden(g, 'scheduler', make(Scheduler));

  // ----------------------------------------------------------- Trusted Types
  const TT = Symbol('tt');
  function trusted(C) { return class extends Object { constructor() { super(); ILLEGAL(); } toString() { return this[TT]; } toJSON() { return this[TT]; } }; }
  const TrustedHTML = trusted(); const TrustedScript = trusted(); const TrustedScriptURL = trusted();
  for (const [C, n] of [[TrustedHTML, 'TrustedHTML'], [TrustedScript, 'TrustedScript'], [TrustedScriptURL, 'TrustedScriptURL']]) { Object.setPrototypeOf(C.prototype, Object.prototype); Object.defineProperty(C, 'name', { value: n }); tag(C, n); }
  TrustedHTML.fromLiteral = (s) => makeTrusted(TrustedHTML, Array.isArray(s) && s.raw ? s.join('') : String(s));
  TrustedScript.fromLiteral = (s) => makeTrusted(TrustedScript, Array.isArray(s) && s.raw ? s.join('') : String(s));
  TrustedScriptURL.fromLiteral = (s) => makeTrusted(TrustedScriptURL, Array.isArray(s) && s.raw ? s.join('') : String(s));
  const makeTrusted = (C, v) => { const o = Object.create(C.prototype); hidden(o, TT, String(v)); return o; };
  class TrustedTypePolicy {
    constructor() { ILLEGAL(); }
    get name() { return this[S].name; }
    createHTML(input, ...args) { const f = this[S].rules.createHTML; if (!f) throw new TypeError(`Failed to execute 'createHTML' on 'TrustedTypePolicy': Policy ${this[S].name}'s TrustedTypePolicyOptions did not specify a 'createHTML' member.`); return makeTrusted(TrustedHTML, f(String(input), ...args) ?? ''); }
    createScript(input, ...args) { const f = this[S].rules.createScript; if (!f) throw new TypeError(`Failed to execute 'createScript' on 'TrustedTypePolicy': Policy ${this[S].name}'s TrustedTypePolicyOptions did not specify a 'createScript' member.`); return makeTrusted(TrustedScript, f(String(input), ...args) ?? ''); }
    createParserOptions(options = {}, ...args) { const f = this[S].rules.createParserOptions; const v = f ? f(options, ...args) : options; const o = Object.create(TrustedHTMLParserOptions.prototype); hidden(o, TT, Object.assign({}, v ?? {})); return o; }
    createScriptURL(input, ...args) { const f = this[S].rules.createScriptURL; if (!f) throw new TypeError(`Failed to execute 'createScriptURL' on 'TrustedTypePolicy': Policy ${this[S].name}'s TrustedTypePolicyOptions did not specify a 'createScriptURL' member.`); return makeTrusted(TrustedScriptURL, f(String(input), ...args) ?? ''); }
  }
  tag(TrustedTypePolicy, 'TrustedTypePolicy');
  class TrustedHTMLParserOptions { constructor() { ILLEGAL(); } }
  tag(TrustedHTMLParserOptions, 'TrustedHTMLParserOptions');
  define('TrustedHTMLParserOptions', TrustedHTMLParserOptions);
  class TrustedTypePolicyFactory {
    constructor() { ILLEGAL(); }
    createPolicy(name, rules = {}) { const p = make(TrustedTypePolicy, { name: String(name), rules }); if (name === 'default') this[S].default = p; return p; }
    isHTML(v) { return v instanceof TrustedHTML; }
    isScript(v) { return v instanceof TrustedScript; }
    isScriptURL(v) { return v instanceof TrustedScriptURL; }
    get emptyHTML() { return makeTrusted(TrustedHTML, ''); }
    get emptyScript() { return makeTrusted(TrustedScript, ''); }
    getAttributeType(tagName, attr) { attr = String(attr).toLowerCase(); tagName = String(tagName).toLowerCase(); if (attr.startsWith('on')) return 'TrustedScript'; if ((tagName === 'script' && attr === 'src') || (tagName === 'iframe' && attr === 'srcdoc')) return tagName === 'script' ? 'TrustedScriptURL' : 'TrustedHTML'; return null; }
    getPropertyType(tagName, prop) { prop = String(prop); if (prop === 'innerHTML' || prop === 'outerHTML' || prop === 'srcdoc') return 'TrustedHTML'; if (String(tagName).toLowerCase() === 'script') return prop === 'src' ? 'TrustedScriptURL' : prop === 'text' || prop === 'textContent' || prop === 'innerText' ? 'TrustedScript' : null; return null; }
    get defaultPolicy() { return this[S].default || null; }
  }
  tag(TrustedTypePolicyFactory, 'TrustedTypePolicyFactory');
  define('TrustedHTML', TrustedHTML);
  define('TrustedScript', TrustedScript);
  define('TrustedScriptURL', TrustedScriptURL);
  define('TrustedTypePolicy', TrustedTypePolicy);
  define('TrustedTypePolicyFactory', TrustedTypePolicyFactory);
  hidden(g, 'trustedTypes', make(TrustedTypePolicyFactory, { default: null }));
  // Trusted Types enforcement for the HTML parsing sinks (`*HTMLUnsafe`):
  // with `require-trusted-types-for 'script'` in a CSP <meta>, a string goes
  // through the default policy's createHTML (or the options through its
  // createParserOptions), unless it is a TrustedHTML / the options are
  // TrustedHTMLParserOptions. Returns { html, options } or throws TypeError.
  const ttEnforced = () => {
    const d = g.document;
    if (!d || typeof d.querySelectorAll !== 'function') return false;
    for (const m of d.querySelectorAll('meta[http-equiv]')) {
      if (String(m.getAttribute('http-equiv')).toLowerCase() !== 'content-security-policy') continue;
      if (/(^|;)\s*require-trusted-types-for\s+'script'/i.test(m.getAttribute('content') || '')) return true;
    }
    return false;
  };
  const unvetted = (o) => !!o && typeof o === 'object' && (o.sanitizer !== undefined || o.runScripts === true);
  // The options of a streaming sink (`stream*HTMLUnsafe`).
  hidden(g, '__rhTrustedOptionsSink', function (options, sink) {
    if (options instanceof TrustedHTMLParserOptions) return Object.assign({}, options[TT]);
    if (!ttEnforced()) return options;
    const policy = g.trustedTypes[S].default;
    const rules = policy ? policy[S].rules : null;
    if (rules && typeof rules.createParserOptions === 'function') {
      const r = rules.createParserOptions(options === undefined || options === null ? {} : options, 'TrustedHTMLParserOptions', sink);
      if (r === null || r === undefined) throw new TypeError(`Failed to execute '${sink}': the default policy's createParserOptions rejected the options.`);
      return r;
    }
    if (unvetted(options)) throw new TypeError(`Failed to execute '${sink}': the parser options need a TrustedHTMLParserOptions.`);
    return options;
  });
  hidden(g, '__rhTrustedHTMLSink', function (html, options, sink) {
    const vetted = options instanceof TrustedHTMLParserOptions;
    const opts = vetted ? Object.assign({}, options[TT]) : options;
    const isTrusted = html instanceof TrustedHTML;
    const text = isTrusted ? html[TT] : String(html);
    if (vetted || !ttEnforced()) return { html: text, options: opts };
    const policy = g.trustedTypes[S].default;
    const rules = policy ? policy[S].rules : null;
    if (rules && typeof rules.createParserOptions === 'function') {
      const r = rules.createParserOptions(opts === undefined || opts === null ? {} : opts, 'TrustedHTMLParserOptions', sink);
      if (r === null || r === undefined) throw new TypeError(`Failed to execute '${sink}': the default policy's createParserOptions rejected the options.`);
      return { html: text, options: r };
    }
    if (unvetted(opts)) throw new TypeError(`Failed to execute '${sink}': the parser options need a TrustedHTMLParserOptions.`);
    if (isTrusted) return { html: text, options: opts };
    if (!rules || typeof rules.createHTML !== 'function') throw new TypeError(`Failed to execute '${sink}': This document requires 'TrustedHTML' assignment.`);
    const r = rules.createHTML(text, 'TrustedHTML', sink);
    if (r === null || r === undefined) throw new TypeError(`Failed to execute '${sink}': the default policy's createHTML rejected the input.`);
    return { html: String(r), options: opts };
  });

  // ------------------------------------------------------------- Fullscreen
  acc(g.Document, 'fullscreenEnabled', function () { return false; });
  acc(g.Document, 'fullscreen', function () { return false; });
  acc(g.Document, 'webkitFullscreenEnabled', function () { return false; });
  acc(g.Document, 'webkitIsFullScreen', function () { return false; });
  method(g.Document, 'exitFullscreen', function () { return Promise.reject(new TypeError('Document not active')); }, true);
  method(g.Element, 'requestFullscreen', function () { const el = this; setTimeout(() => { el.dispatchEvent(new Event('fullscreenerror', { bubbles: true, composed: true })); }, 0); return Promise.reject(new TypeError('Permissions check failed')); }, true);
  N.installHandlers(g.Element.prototype, ['fullscreenchange', 'fullscreenerror'].filter((n) => !Object.prototype.hasOwnProperty.call(g.Element.prototype, 'on' + n)));
  N.installHandlers(g.Document.prototype, ['fullscreenchange', 'fullscreenerror'].filter((n) => !Object.prototype.hasOwnProperty.call(g.Document.prototype, 'on' + n)));

  // ---------------------------------------------------------- VisualViewport
  class VisualViewport extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get offsetLeft() { return 0; }
    get offsetTop() { return 0; }
    get pageLeft() { return g.scrollX; }
    get pageTop() { return g.scrollY; }
    get width() { return g.innerWidth; }
    get height() { return g.innerHeight; }
    get scale() { return 1; }
  }
  tag(VisualViewport, 'VisualViewport');
  N.installHandlers(VisualViewport.prototype, ['resize', 'scroll', 'scrollend']);
  define('VisualViewport', VisualViewport);
  const vv = make(VisualViewport);
  try { delete g.visualViewport; } catch (e) {}
  Object.defineProperty(g, 'visualViewport', { get() { return vv; }, configurable: true, enumerable: true });
  g.addEventListener('resize', () => vv.dispatchEvent(new Event('resize')));
  g.addEventListener('scroll', () => vv.dispatchEvent(new Event('scroll')));
  hidden(g, 'moveBy', function moveBy() {});
  hidden(g, 'resizeBy', function resizeBy() {});

  // --------------------------------------------------- MediaQueryList events
  const MQL = g.MediaQueryList;
  if (MQL) {
    const live = [];
    const nativeMatchMedia = g.matchMedia;
    // MediaQueryList is an EventTarget (the native class was set up before
    // EventTarget existed).
    if (g.EventTarget) {
      Object.setPrototypeOf(MQL.prototype, g.EventTarget.prototype);
      Object.setPrototypeOf(MQL, g.EventTarget);
    }
    // The native list's live `matches` getter evaluates `this.media`.
    const probe = nativeMatchMedia.call(g, 'all');
    const nativeMatches = Object.getOwnPropertyDescriptor(probe, 'matches').get;
    const brand = (m) => { if (!m || typeof m !== 'object' || !Object.prototype.hasOwnProperty.call(m, S)) throw new TypeError('Illegal invocation'); return m[S]; };
    Object.defineProperty(MQL.prototype, 'media', { get: Object.defineProperty(function () { return brand(this).media; }, 'name', { value: 'get media' }), configurable: true, enumerable: true });
    Object.defineProperty(MQL.prototype, 'matches', { get: Object.defineProperty(function () { brand(this); return nativeMatches.call(this); }, 'name', { value: 'get matches' }), configurable: true, enumerable: true });
    N.installHandlers(MQL.prototype, ['change']);
    method(MQL, 'addListener', function addListener(f) { brand(this); if (f !== null && f !== undefined) this.addEventListener('change', f); }, true);
    method(MQL, 'removeListener', function removeListener(f) { brand(this); if (f !== null && f !== undefined) this.removeEventListener('change', f); }, true);
    hidden(g, 'matchMedia', function matchMedia(q) {
      if (arguments.length < 1) throw new TypeError("Failed to execute 'matchMedia' on 'Window': 1 argument required, but only 0 present.");
      const m = nativeMatchMedia.call(g, q);
      const media = m.media;
      for (const k of ['media', 'matches', 'onchange']) { try { delete m[k]; } catch (e) { /* ignore */ } }
      hidden(m, S, { media, last: false });
      m[S].last = m.matches;
      live.push(m);
      return m;
    });
    if (g.MediaQueryListEvent) Object.defineProperty(g.MediaQueryListEvent, 'length', { value: 1, configurable: true });
    g.addEventListener('resize', () => {
      for (let i = live.length - 1; i >= 0; i--) {
        const m = live[i];
        const now = m.matches;
        if (now !== m[S].last) { m[S].last = now; m.dispatchEvent(new g.MediaQueryListEvent('change', { media: m.media, matches: now })); }
      }
    });
  }

  // ------------------------------------------------------------ BarProp etc.
  class BarProp { constructor() { ILLEGAL(); } get visible() { return true; } }
  tag(BarProp, 'BarProp');
  define('BarProp', BarProp);
  for (const k of ['locationbar', 'menubar', 'personalbar', 'scrollbars', 'statusbar', 'toolbar']) { const b = make(BarProp); Object.defineProperty(g, k, { get() { return b; }, configurable: true, enumerable: true }); }
  class External { constructor() { ILLEGAL(); } AddSearchProvider() {} IsSearchProviderInstalled() { return 0; } }
  tag(External, 'External');
  define('External', External);
  const external = make(External);
  Object.defineProperty(g, 'external', { get() { return external; }, configurable: true, enumerable: true });
  Object.defineProperty(g, 'originAgentCluster', { get() { return false; }, configurable: true, enumerable: true });
  Object.defineProperty(g, 'crossOriginIsolated', { get() { return false; }, configurable: true, enumerable: true });
  hidden(g, 'reportError', function reportError(e) {
    const ev = new g.ErrorEvent('error', { message: e && e.message ? String(e.message) : String(e), error: e, cancelable: true });
    if (g.dispatchEvent(ev)) console.error('Uncaught ' + (e && e.stack ? e.stack : e));
  });

  // Plugins / MIME types (the PDF viewer entries every desktop browser has).
  class MimeType { constructor() { ILLEGAL(); } get type() { return this[S].type; } get description() { return 'Portable Document Format'; } get suffixes() { return 'pdf'; } get enabledPlugin() { return this[S].plugin; } }
  class Plugin { constructor() { ILLEGAL(); } get name() { return this[S].name; } get description() { return 'Portable Document Format'; } get filename() { return 'internal-pdf-viewer'; } get length() { return this[S].mimes.length; } item(i) { return this[S].mimes[i] || null; } namedItem(n) { return this[S].mimes.find((m) => m.type === n) || null; } [Symbol.iterator]() { return this[S].mimes[Symbol.iterator](); } }
  class PluginArray { constructor() { ILLEGAL(); } get length() { return this[S].length; } item(i) { return this[S][i] || null; } namedItem(n) { return this[S].find((p) => p.name === n) || null; } refresh() {} [Symbol.iterator]() { return this[S][Symbol.iterator](); } }
  class MimeTypeArray { constructor() { ILLEGAL(); } get length() { return this[S].length; } item(i) { return this[S][i] || null; } namedItem(n) { return this[S].find((m) => m.type === n) || null; } [Symbol.iterator]() { return this[S][Symbol.iterator](); } }
  for (const [C, n] of [[MimeType, 'MimeType'], [Plugin, 'Plugin'], [PluginArray, 'PluginArray'], [MimeTypeArray, 'MimeTypeArray']]) { tag(C, n); define(n, C); }
  const plugins = [], mimes = [];
  for (const name of ['PDF Viewer', 'Chrome PDF Viewer', 'Chromium PDF Viewer', 'Microsoft Edge PDF Viewer', 'WebKit built-in PDF']) {
    const p = make(Plugin, { name, mimes: [] });
    for (const type of ['application/pdf', 'text/pdf']) { const m = make(MimeType, { type, plugin: p }); p[S].mimes.push(m); }
    plugins.push(p);
  }
  for (const type of ['application/pdf', 'text/pdf']) mimes.push(make(MimeType, { type, plugin: plugins[0] }));
  const index = (arr, C, key) => { const o = make(C); hidden(o, S, arr); arr.forEach((v, i) => Object.defineProperty(o, i, { value: v, enumerable: true })); arr.forEach((v) => { if (!(v[key] in o)) Object.defineProperty(o, v[key], { value: v }); }); return o; };
  setNav('plugins', index(plugins, PluginArray, 'name'));
  setNav('mimeTypes', index(mimes, MimeTypeArray, 'type'));
  setNav('pdfViewerEnabled', true);
  method(g.Navigator, 'javaEnabled', function () { return false; });
  method(g.Navigator, 'taintEnabled', function () { return false; });
  method(g.Navigator, 'registerProtocolHandler', function () {});
  method(g.Navigator, 'unregisterProtocolHandler', function () {});
  method(g.Navigator, 'vibrate', function () { return false; });
  method(g.Navigator, 'canShare', function (data) { return !!data && (typeof data.url === 'string' || typeof data.text === 'string' || typeof data.title === 'string'); });
  method(g.Navigator, 'share', function () { return Promise.reject(domError('Share is not available.', 'NotAllowedError')); });
  method(g.Navigator, 'getGamepads', function () { return []; });
  method(g.Navigator, 'requestMediaKeySystemAccess', function () { return Promise.reject(domError('Unsupported keySystem or supportedConfigurations.', 'NotSupportedError')); });
  acc(g.Navigator, 'oscpu', function () { return undefined; });

  class UserActivation { constructor() { ILLEGAL(); } get hasBeenActive() { return this[S].been; } get isActive() { return this[S].active; } }
  tag(UserActivation, 'UserActivation');
  define('UserActivation', UserActivation);
  const ua = make(UserActivation, { been: false, active: false });
  setNav('userActivation', ua);
  for (const t of ['keydown', 'mousedown', 'pointerdown', 'pointerup', 'touchend']) g.addEventListener(t, (e) => { if (e.isTrusted) { ua[S].been = true; ua[S].active = true; setTimeout(() => { ua[S].active = false; }, 5000); } }, true);

  // ------------------------------------------------------------------ Origin
  class Origin {
    constructor() { return Object.assign(Object.create(new.target ? new.target.prototype : Origin.prototype), { [S]: { opaque: true, tuple: null } }); }
    static from(value) {
      let u;
      if (value instanceof Origin) return value;
      try { u = new URL(String(value)); } catch (e) { throw new TypeError("Failed to execute 'from' on 'Origin': Invalid URL."); }
      const o = Object.create(Origin.prototype);
      hidden(o, S, u.origin === 'null' ? { opaque: true, tuple: null } : { opaque: false, tuple: u.origin });
      return o;
    }
    get opaque() { return this[S].opaque; }
    isSameOrigin(other) { return !this[S].opaque && !other[S].opaque && this[S].tuple === other[S].tuple; }
    isSameSite(other) { if (this[S].opaque || other[S].opaque) return false; const h = (t) => new URL(t).hostname.split('.').slice(-2).join('.'); return h(this[S].tuple) === h(other[S].tuple) && new URL(this[S].tuple).protocol === new URL(other[S].tuple).protocol; }
    toJSON() { return this[S].tuple || 'null'; }
  }
  tag(Origin, 'Origin');
  define('Origin', Origin);

  // ------------------------------------------------------------- CloseWatcher
  const watchers = [];
  class CloseWatcher extends g.EventTarget {
    constructor(options = {}) {
      super();
      if (options.signal && options.signal.aborted) { hidden(this, S, { active: false }); return; }
      hidden(this, S, { active: true });
      watchers.push(this);
      if (options.signal) options.signal.addEventListener('abort', () => this.destroy());
    }
    requestClose() { if (!this[S].active) return; if (this.dispatchEvent(new Event('cancel', { cancelable: true }))) this.close(); }
    close() { if (!this[S].active) return; this.destroy(); this.dispatchEvent(new Event('close')); }
    destroy() { this[S].active = false; const i = watchers.indexOf(this); if (i >= 0) watchers.splice(i, 1); }
  }
  tag(CloseWatcher, 'CloseWatcher');
  N.installHandlers(CloseWatcher.prototype, ['cancel', 'close']);
  define('CloseWatcher', CloseWatcher);
  g.addEventListener('keydown', (e) => { if (e.key === 'Escape' && watchers.length) watchers[watchers.length - 1].requestClose(); });

  // ------------------------------------------------------------ DataTransfer
  class DataTransferItem {
    constructor() { ILLEGAL(); }
    get kind() { return this[S].kind; }
    get type() { return this[S].type; }
    getAsString(cb) { if (this[S].kind === 'string' && typeof cb === 'function') setTimeout(() => cb(this[S].data), 0); }
    getAsFile() { return this[S].kind === 'file' ? this[S].data : null; }
    webkitGetAsEntry() { return null; }
  }
  tag(DataTransferItem, 'DataTransferItem');
  class DataTransferItemList {
    constructor() { ILLEGAL(); }
    get length() { return this[S].items.length; }
    add(data, type) {
      if (data instanceof g.File) { const it = make(DataTransferItem, { kind: 'file', type: data.type, data }); this[S].items.push(it); return it; }
      type = String(type).toLowerCase();
      if (this[S].items.some((i) => i.kind === 'string' && i.type === type)) throw domError("Failed to execute 'add' on 'DataTransferItemList': An item already exists for type '" + type + "'.", 'NotSupportedError');
      const it = make(DataTransferItem, { kind: 'string', type, data: String(data) });
      this[S].items.push(it);
      return it;
    }
    remove(i) { this[S].items.splice(i, 1); }
    clear() { this[S].items.length = 0; }
    [Symbol.iterator]() { return this[S].items[Symbol.iterator](); }
  }
  tag(DataTransferItemList, 'DataTransferItemList');
  class DataTransfer {
    constructor() {
      const d = Object.create(new.target ? new.target.prototype : DataTransfer.prototype);
      const items = [];
      const list = make(DataTransferItemList, { items });
      hidden(d, S, { dropEffect: 'none', effectAllowed: 'uninitialized', items, list });
      return d;
    }
    get dropEffect() { return this[S].dropEffect; }
    set dropEffect(v) { if (['none', 'copy', 'link', 'move'].includes(v)) this[S].dropEffect = v; }
    get effectAllowed() { return this[S].effectAllowed; }
    set effectAllowed(v) { if (['none', 'copy', 'copyLink', 'copyMove', 'link', 'linkMove', 'move', 'all', 'uninitialized'].includes(v)) this[S].effectAllowed = v; }
    get items() { return this[S].list; }
    get types() { const t = []; for (const i of this[S].items) if (i.kind === 'string') t.push(i.type); if (this[S].items.some((i) => i.kind === 'file')) t.push('Files'); return Object.freeze(t); }
    get files() { return g.__rhFileList ? g.__rhFileList(this[S].items.filter((i) => i.kind === 'file').map((i) => i.getAsFile())) : []; }
    setDragImage() {}
    getData(format) { format = String(format).toLowerCase(); if (format === 'text') format = 'text/plain'; if (format === 'url') format = 'text/uri-list'; const it = this[S].items.find((i) => i.kind === 'string' && i.type === format); return it ? it[S].data : ''; }
    setData(format, data) { format = String(format).toLowerCase(); if (format === 'text') format = 'text/plain'; if (format === 'url') format = 'text/uri-list'; const i = this[S].items.findIndex((x) => x.kind === 'string' && x.type === format); if (i >= 0) this[S].items.splice(i, 1); this[S].list.add(String(data), format); }
    clearData(format) { if (format === undefined) { for (let i = this[S].items.length - 1; i >= 0; i--) if (this[S].items[i].kind === 'string') this[S].items.splice(i, 1); } else { format = String(format).toLowerCase(); const i = this[S].items.findIndex((x) => x.kind === 'string' && x.type === format); if (i >= 0) this[S].items.splice(i, 1); } }
  }
  tag(DataTransfer, 'DataTransfer');
  define('DataTransfer', DataTransfer);
  define('DataTransferItem', DataTransferItem);
  define('DataTransferItemList', DataTransferItemList);

  // -------------------------------------------------------- document.all
  class HTMLAllCollection {
    constructor() { ILLEGAL(); }
    get length() { return document.getElementsByTagName('*').length; }
    item(i) { if (typeof i === 'string' && !/^\d+$/.test(i)) return this.namedItem(i); return document.getElementsByTagName('*')[i] || null; }
    namedItem(name) { const all = Array.from(document.getElementsByTagName('*')).filter((e) => e.id === name || e.getAttribute('name') === name); return all.length === 0 ? null : all.length === 1 ? all[0] : all; }
  }
  tag(HTMLAllCollection, 'HTMLAllCollection');
  define('HTMLAllCollection', HTMLAllCollection);
  // [LegacyFalsy] / [[IsHTMLDDA]]: `document.all` is an object that reads
  // as falsy, has typeof "undefined" and == null, so IE-detection code
  // takes the standards path while `x === document.all` stays false for
  // undefined x (sanitizers such as YouTube's Trusted Types bridge rely
  // on it).
  const ALL = Symbol('all');
  acc(g.Document, 'all', function () {
    let a = this[ALL];
    if (!a) {
      a = Object.create(HTMLAllCollection.prototype);
      hidden(this, ALL, a);
      N.markHTMLDDA(a);
    }
    return a;
  });

  // Legacy document colors / misc.
  for (const [k, attr] of [['fgColor', 'text'], ['linkColor', 'link'], ['vlinkColor', 'vlink'], ['alinkColor', 'alink'], ['bgColor', 'bgcolor']]) {
    acc(g.Document, k, function () { return (this.body && this.body.getAttribute(attr)) || ''; }, function (v) { if (this.body) this.body.setAttribute(attr, v); });
  }
  acc(g.Document, 'applets', function () { return Object.setPrototypeOf([], g.HTMLCollection.prototype); });
  acc(g.Document, 'lastModified', function () { const d = new Date(); const p = (n) => String(n).padStart(2, '0'); return `${p(d.getMonth() + 1)}/${p(d.getDate())}/${d.getFullYear()} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`; });
  method(g.Document, 'clear', function () {});
  method(g.Document, 'captureEvents', function () {});
  method(g.Document, 'releaseEvents', function () {});
  method(g.Document, 'queryCommandIndeterm', function () { return false; });
  method(g.Document, 'queryCommandValue', function () { return ''; });
  method(g.Document, 'hasStorageAccess', function () { return Promise.resolve(true); });
  method(g.Document, 'requestStorageAccess', function () { return Promise.resolve(); });
  method(g.Document, 'startViewTransition', function (cb) {
    const update = Promise.resolve().then(() => (typeof cb === 'function' ? cb() : cb && cb.update ? cb.update() : undefined));
    return { updateCallbackDone: update, ready: update, finished: update, skipTransition() {}, types: new Set() };
  });

  // ------------------------------------------------------ CSSOM View extras
  const boxQuads = function () { const r = this.getBoundingClientRect ? this.getBoundingClientRect() : (() => { const rg = document.createRange(); rg.selectNodeContents(this); return rg.getBoundingClientRect(); })(); return [g.DOMQuad.fromRect(r)]; };
  for (const C of [g.Element, g.Text, g.Document]) {
    method(C, 'getBoxQuads', C === g.Document ? function () { return [g.DOMQuad.fromRect(new g.DOMRect(0, 0, g.innerWidth, g.innerHeight))]; } : boxQuads);
    method(C, 'convertQuadFromNode', function (q) { return g.DOMQuad.fromQuad(q); });
    method(C, 'convertRectFromNode', function (r) { return g.DOMQuad.fromRect(r); });
    method(C, 'convertPointFromNode', function (p) { return g.DOMPoint.fromPoint(p); });
  }
  acc(g.Element, 'currentCSSZoom', function () { return 1; });
  acc(g.HTMLElement, 'scrollParent', function () {
    for (let n = this.parentElement; n; n = n.parentElement) {
      const cs = g.getComputedStyle(n);
      if (/(auto|scroll|hidden)/.test(cs.overflowY + cs.overflowX)) return n;
    }
    return document.scrollingElement;
  });

  // ------------------------------------------------------------------ console
  const counts = new Map();
  if (!console.countReset) hidden(console, 'countReset', function countReset(label = 'default') { counts.delete(String(label)); });
  if (!console.dirxml) hidden(console, 'dirxml', function dirxml(...args) { console.log(...args); });

  // ---------------------------------------------------- Service workers
  class ServiceWorker extends g.EventTarget { constructor() { super(); ILLEGAL(); } get scriptURL() { return this[S].url; } get state() { return 'redundant'; } postMessage() {} }
  tag(ServiceWorker, 'ServiceWorker');
  N.installHandlers(ServiceWorker.prototype, ['statechange', 'error']);
  class ServiceWorkerRegistration extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get installing() { return null; } get waiting() { return null; } get active() { return null; }
    get navigationPreload() { return this[S].preload; }
    get scope() { return this[S].scope; }
    get updateViaCache() { return 'imports'; }
    get cookies() { return null; }
    update() { return Promise.resolve(this); }
    unregister() { return Promise.resolve(true); }
    showNotification() { return Promise.reject(new TypeError('No notification permission has been granted for this origin.')); }
    getNotifications() { return Promise.resolve([]); }
  }
  tag(ServiceWorkerRegistration, 'ServiceWorkerRegistration');
  N.installHandlers(ServiceWorkerRegistration.prototype, ['updatefound']);
  class NavigationPreloadManager { constructor() { ILLEGAL(); } enable() { return Promise.resolve(); } disable() { return Promise.resolve(); } setHeaderValue() { return Promise.resolve(); } getState() { return Promise.resolve({ enabled: false, headerValue: 'true' }); } }
  tag(NavigationPreloadManager, 'NavigationPreloadManager');
  // Service workers cannot run here: registration fails like in a context
  // where they are disabled; `ready` never settles (no active worker).
  class ServiceWorkerContainer extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get controller() { return null; }
    get ready() { return this[S].ready; }
    register(url) { return Promise.reject(domError(`Failed to register a ServiceWorker for scope ('${new URL('./', new URL(String(url), document.baseURI)).href}') with script ('${new URL(String(url), document.baseURI).href}'): The operation is not supported.`, 'NotSupportedError')); }
    getRegistration() { return Promise.resolve(undefined); }
    getRegistrations() { return Promise.resolve([]); }
    startMessages() {}
  }
  tag(ServiceWorkerContainer, 'ServiceWorkerContainer');
  N.installHandlers(ServiceWorkerContainer.prototype, ['controllerchange', 'message', 'messageerror']);
  for (const [n, C] of [['ServiceWorker', ServiceWorker], ['ServiceWorkerRegistration', ServiceWorkerRegistration], ['ServiceWorkerContainer', ServiceWorkerContainer], ['NavigationPreloadManager', NavigationPreloadManager]]) define(n, C);
  setNav('serviceWorker', make(ServiceWorkerContainer, { ready: new Promise(() => {}) }));

  // Cache Storage: functional, in memory for the page's lifetime.
  const reqKey = (r) => { const u = new URL(r instanceof g.Request ? r.url : String(r), document.baseURI); u.hash = ''; return u.href; };
  class Cache {
    constructor() { ILLEGAL(); }
    match(req, opts = {}) { return this.matchAll(req, opts).then((l) => l[0]); }
    matchAll(req, opts = {}) {
      const out = [];
      for (const [k, v] of this[S]) {
        if (req !== undefined) {
          let key = reqKey(req), stored = k;
          if (opts.ignoreSearch) { key = key.split('?')[0]; stored = stored.split('?')[0]; }
          if (key !== stored) continue;
          if (!opts.ignoreMethod && req instanceof g.Request && req.method !== 'GET' && req.method !== 'HEAD') continue;
        }
        out.push(v.response.clone());
      }
      return Promise.resolve(out);
    }
    add(req) { return this.addAll([req]); }
    addAll(reqs) {
      return Promise.all(Array.from(reqs).map((r) => g.fetch(r).then((resp) => { if (!resp.ok) throw new TypeError("Failed to execute 'addAll' on 'Cache': Request failed"); return [r, resp]; }))).then((pairs) => { for (const [r, resp] of pairs) this[S].set(reqKey(r), { response: resp }); });
    }
    put(req, resp) {
      if (!(resp instanceof g.Response)) return Promise.reject(new TypeError("Failed to execute 'put' on 'Cache': parameter 2 is not of type 'Response'."));
      if (resp.status === 206) return Promise.reject(new TypeError("Failed to execute 'put' on 'Cache': Partial response (status code 206) is unsupported"));
      this[S].set(reqKey(req), { response: resp.clone() });
      return Promise.resolve();
    }
    delete(req) { return Promise.resolve(this[S].delete(reqKey(req))); }
    keys(req) { return Promise.resolve(Array.from(this[S].keys()).filter((k) => req === undefined || k === reqKey(req)).map((k) => new g.Request(k))); }
  }
  tag(Cache, 'Cache');
  class CacheStorage {
    constructor() { ILLEGAL(); }
    match(req, opts = {}) { const names = opts.cacheName ? [opts.cacheName] : Array.from(this[S].keys()); return names.reduce((p, n) => p.then((hit) => hit || (this[S].has(n) ? this[S].get(n).match(req, opts) : undefined)), Promise.resolve(undefined)); }
    has(name) { return Promise.resolve(this[S].has(String(name))); }
    open(name) { name = String(name); if (!this[S].has(name)) this[S].set(name, make(Cache, new Map())); const c = this[S].get(name); hidden(c, S, c[S]); return Promise.resolve(c); }
    delete(name) { return Promise.resolve(this[S].delete(String(name))); }
    keys() { return Promise.resolve(Array.from(this[S].keys())); }
  }
  tag(CacheStorage, 'CacheStorage');
  define('Cache', Cache);
  define('CacheStorage', CacheStorage);
  const caches = make(CacheStorage, new Map());
  Object.defineProperty(g, 'caches', { get() { return caches; }, configurable: true, enumerable: true });

  // Scroll methods return a promise settled when the scroll completes
  // (CSSOM View): scrolling here is instant, so it is already settled.
  const promiseScroll = (o, name) => {
    const d = o && Object.getOwnPropertyDescriptor(o, name);
    if (!d || typeof d.value !== 'function') return;
    const f = d.value;
    const w = { [name](...args) { f.apply(this, args); return Promise.resolve(); } }[name];
    Object.defineProperty(w, 'length', { value: f.length, configurable: true });
    Object.defineProperty(o, name, Object.assign({}, d, { value: w }));
  };
  if (g.Element) for (const n of ['scroll', 'scrollTo', 'scrollBy', 'scrollIntoView']) promiseScroll(g.Element.prototype, n);

  // Element.checkVisibility(options) (CSSOM View): no box (disconnected,
  // display: none on it or a flat-tree ancestor, under content-visibility:
  // hidden) → false; optionally opacity 0 / visibility not visible too.
  if (g.Element && g.getComputedStyle) {
    const flatParent = (n) => {
      if (n.assignedSlot) return n.assignedSlot;
      const p = n.parentNode;
      if (p && p.nodeType === 11 && p.host) return p.host;
      return p && p.nodeType === 1 ? p : null;
    };
    Object.defineProperty(g.Element.prototype, 'checkVisibility', {
      value: function checkVisibility(options) {
        const o = options || {};
        if (!this.isConnected) return false;
        const checkOpacity = !!(o.checkOpacity || o.opacityProperty);
        const checkVis = !!(o.checkVisibilityCSS || o.visibilityProperty);
        for (let n = this, self = true; n; n = flatParent(n), self = false) {
          const s = g.getComputedStyle(n);
          if (s.display === 'none' || (self && s.display === 'contents')) return false;
          if (!self && s.getPropertyValue('content-visibility') === 'hidden') return false;
          if (checkOpacity && Number(s.opacity) === 0) return false;
        }
        if (checkVis && g.getComputedStyle(this).visibility !== 'visible') return false;
        return true;
      },
      writable: true, configurable: true, enumerable: true,
    });
  }
  for (const n of ['scroll', 'scrollTo', 'scrollBy']) {
    if (Object.prototype.hasOwnProperty.call(g, n)) promiseScroll(g, n);
    else if (g.Window) promiseScroll(g.Window.prototype, n);
  }
})();
