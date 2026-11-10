// Session history for same-document entries (history.back/forward/go over
// pushState entries with popstate / hashchange) and the Navigation API
// (window.navigation, NavigationHistoryEntry, NavigateEvent with
// intercept(), NavigationTransition, NavigationActivation, ...).
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('navigation');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const make = (C, st) => { const o = Object.create(C.prototype); hidden(o, S, st); return o; };
  const H = g.History.prototype;
  const hist = g.history;
  const nativePush = hist.pushState.bind(hist), nativeReplace = hist.replaceState.bind(hist), nativeGo = hist.go.bind(hist);
  let keyN = 0;
  const newKey = () => (++keyN).toString(36) + '-' + Math.random().toString(36).slice(2, 8);

  // --------------------------------------------------------- entries
  class NavigationHistoryEntry extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get url() { return this[S].url; }
    get key() { return this[S].key; }
    get id() { return this[S].id; }
    get index() { return entries.indexOf(this); }
    get sameDocument() { return true; }
    getState() { return this[S].navState === undefined ? undefined : g.structuredClone(this[S].navState); }
  }
  tag(NavigationHistoryEntry, 'NavigationHistoryEntry');
  N.installHandlers(NavigationHistoryEntry.prototype, ['dispose']);
  const entry = (url, histState, navState, key) => make(NavigationHistoryEntry, { url, key: key || newKey(), id: newKey(), histState, navState });
  const entries = [entry(location.href, null, undefined)];
  let index = 0;

  for (const k of Object.getOwnPropertyNames(hist)) { if (['pushState', 'replaceState', 'back', 'forward', 'go'].includes(k)) { try { delete hist[k]; } catch (e) {} } }
  const sameDoc = (a, b) => { try { const x = new URL(a), y = new URL(b); x.hash = ''; y.hash = ''; return x.href === y.href; } catch (e) { return false; } };
  hidden(H, 'pushState', function pushState(state, unused, url) {
    nativePush(state, unused, url);
    for (const e of entries.splice(index + 1)) e.dispatchEvent(new Event('dispose'));
    const from = entries[index];
    entries.push(entry(location.href, state, undefined));
    index = entries.length - 1;
    fireCurrentChange(from, 'push');
  });
  hidden(H, 'replaceState', function replaceState(state, unused, url) {
    nativeReplace(state, unused, url);
    const from = entries[index];
    entries[index] = entry(location.href, state, from.getState(), from[S].key);
    fireCurrentChange(from, 'replace');
  });
  function traverse(delta) {
    const target = index + delta;
    if (delta === 0) { location.reload(); return; }
    if (target < 0 || target >= entries.length) {
      // Cross-document history is the embedder's (back to a previous page).
      if (target < 0) nativeGo(delta);
      return;
    }
    setTimeout(() => {
      const from = entries[index];
      const oldURL = location.href;
      index = target;
      const to = entries[index];
      nativeReplace(to[S].histState, '', to.url);
      fireCurrentChange(from, 'traverse');
      g.dispatchEvent(new g.PopStateEvent('popstate', { state: to[S].histState }));
      if (oldURL !== to.url && sameDoc(oldURL, to.url) && new URL(oldURL).hash !== new URL(to.url).hash) g.dispatchEvent(new g.HashChangeEvent('hashchange', { oldURL, newURL: to.url }));
    }, 0);
  }
  hidden(H, 'back', function back() { traverse(-1); });
  hidden(H, 'forward', function forward() { traverse(1); });
  hidden(H, 'go', function go(delta = 0) { traverse(delta | 0); });
  // Hash navigations (location.hash = ...) add entries too.
  g.addEventListener('hashchange', (e) => {
    if (entries[index].url === location.href) return;
    const from = entries[index];
    for (const x of entries.splice(index + 1)) x.dispatchEvent(new Event('dispose'));
    entries.push(entry(location.href, null, undefined));
    index = entries.length - 1;
    fireCurrentChange(from, 'push');
    void e;
  });

  // ------------------------------------------------------- Navigation API
  class NavigationDestination {
    constructor() { ILLEGAL(); }
    get url() { return this[S].url; }
    get key() { return this[S].entry ? this[S].entry.key : ''; }
    get id() { return this[S].entry ? this[S].entry.id : ''; }
    get index() { return this[S].entry ? this[S].entry.index : -1; }
    get sameDocument() { return this[S].sameDocument; }
    getState() { return this[S].state === undefined ? undefined : g.structuredClone(this[S].state); }
  }
  tag(NavigationDestination, 'NavigationDestination');
  class NavigationTransition {
    constructor() { ILLEGAL(); }
    get navigationType() { return this[S].type; }
    get from() { return this[S].from; }
    get to() { return this[S].to; }
    get finished() { return this[S].finished; }
    get committed() { return this[S].committed; }
  }
  tag(NavigationTransition, 'NavigationTransition');
  class NavigationActivation {
    constructor() { ILLEGAL(); }
    get from() { return null; }
    get entry() { return entries[index]; }
    get navigationType() { return 'push'; }
  }
  tag(NavigationActivation, 'NavigationActivation');
  class NavigationPrecommitController {
    constructor() { ILLEGAL(); }
    redirect(url, options = {}) { this[S].redirect = { url: new URL(url, document.baseURI).href, options }; }
    addHandler(h) { this[S].handlers.push(h); }
  }
  tag(NavigationPrecommitController, 'NavigationPrecommitController');
  class NavigateEvent extends g.Event {
    constructor(type, init = {}) {
      super(type, init);
      if (!init.destination) throw new TypeError("Failed to construct 'NavigateEvent': required member destination is undefined.");
      hidden(this, S, { navigationType: init.navigationType || 'push', destination: init.destination, canIntercept: !!init.canIntercept, userInitiated: !!init.userInitiated, hashChange: !!init.hashChange, signal: init.signal, formData: init.formData || null, downloadRequest: init.downloadRequest ?? null, info: init.info, hasUAVisualTransition: !!init.hasUAVisualTransition, sourceElement: init.sourceElement || null, handlers: [], precommit: [], intercepted: false, focusReset: 'after-transition', scroll: 'after-transition' });
    }
    get navigationType() { return this[S].navigationType; }
    get destination() { return this[S].destination; }
    get canIntercept() { return this[S].canIntercept; }
    get userInitiated() { return this[S].userInitiated; }
    get hashChange() { return this[S].hashChange; }
    get signal() { return this[S].signal; }
    get formData() { return this[S].formData; }
    get downloadRequest() { return this[S].downloadRequest; }
    get info() { return this[S].info; }
    get hasUAVisualTransition() { return this[S].hasUAVisualTransition; }
    get sourceElement() { return this[S].sourceElement; }
    intercept(options = {}) {
      if (!this[S].canIntercept) throw domError("Failed to execute 'intercept' on 'NavigateEvent': A navigation with URL '" + this[S].destination.url + "' cannot be intercepted.", 'SecurityError');
      if (!this.isTrustedNavigate) void 0;
      this[S].intercepted = true;
      if (typeof options.handler === 'function') this[S].handlers.push(options.handler);
      if (typeof options.precommitHandler === 'function') this[S].precommit.push(options.precommitHandler);
      if (options.focusReset) this[S].focusReset = options.focusReset;
      if (options.scroll) this[S].scroll = options.scroll;
    }
    scroll() { if (this[S].destination.url.includes('#')) { const el = document.getElementById(decodeURIComponent(this[S].destination.url.split('#')[1])); if (el) el.scrollIntoView(); } else g.scrollTo(0, 0); }
  }
  tag(NavigateEvent, 'NavigateEvent');
  class NavigationCurrentEntryChangeEvent extends g.Event {
    constructor(type, init = {}) {
      super(type, init);
      if (!init.from) throw new TypeError("Failed to construct 'NavigationCurrentEntryChangeEvent': required member from is undefined.");
      hidden(this, S, { from: init.from, navigationType: init.navigationType ?? null });
    }
    get from() { return this[S].from; }
    get navigationType() { return this[S].navigationType; }
  }
  tag(NavigationCurrentEntryChangeEvent, 'NavigationCurrentEntryChangeEvent');

  let transition = null;
  class Navigation extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    entries() { return entries.slice(); }
    get currentEntry() { return entries[index]; }
    updateCurrentEntry(options = {}) { if (!('state' in options)) throw new TypeError("Failed to execute 'updateCurrentEntry' on 'Navigation': required member state is undefined."); const from = entries[index]; from[S].navState = g.structuredClone(options.state); this.dispatchEvent(new NavigationCurrentEntryChangeEvent('currententrychange', { from, navigationType: null })); }
    get transition() { return transition; }
    get activation() { return activation; }
    get canGoBack() { return index > 0; }
    get canGoForward() { return index < entries.length - 1; }
    navigate(url, options = {}) {
      let u;
      try { u = new URL(url, document.baseURI).href; } catch (e) { return rejectedResult(domError(`Failed to execute 'navigate' on 'Navigation': Invalid URL '${url}'.`, 'SyntaxError')); }
      const type = options.history === 'replace' ? 'replace' : 'push';
      return runNavigate(u, type, options.state, options.info, () => {
        if (type === 'replace') nativeReplace(null, '', u); else nativePush(null, '', u);
      });
    }
    reload(options = {}) { return runNavigate(location.href, 'reload', options.state, options.info, null); }
    traverseTo(key, options = {}) {
      const i = entries.findIndex((e) => e.key === key);
      if (i < 0) return rejectedResult(domError("Failed to execute 'traverseTo' on 'Navigation': Invalid key", 'InvalidStateError'));
      return traverseResult(i - index, options.info);
    }
    back(options = {}) { if (index === 0) return rejectedResult(domError("Failed to execute 'back' on 'Navigation': Cannot go back.", 'InvalidStateError')); return traverseResult(-1, options.info); }
    forward(options = {}) { if (index >= entries.length - 1) return rejectedResult(domError("Failed to execute 'forward' on 'Navigation': Cannot go forward.", 'InvalidStateError')); return traverseResult(1, options.info); }
  }
  tag(Navigation, 'Navigation');
  N.installHandlers(Navigation.prototype, ['navigate', 'navigatesuccess', 'navigateerror', 'currententrychange']);
  const navigation = make(Navigation, {});
  const activation = make(NavigationActivation, {});
  function fireCurrentChange(from, type) {
    navigation.dispatchEvent(new NavigationCurrentEntryChangeEvent('currententrychange', { from, navigationType: type }));
  }
  function rejectedResult(e) {
    const p1 = Promise.reject(e), p2 = Promise.reject(e);
    p1.catch(() => {}); p2.catch(() => {});
    return { committed: p1, finished: p2 };
  }
  function traverseResult(delta, info) {
    const target = entries[index + delta];
    const committed = new Promise((res) => navigation.addEventListener('currententrychange', () => res(entries[index]), { once: true }));
    hist.go(delta);
    return { committed, finished: committed.then((e) => { navigation.dispatchEvent(new Event('navigatesuccess')); return e; }) };
    void target; void info;
  }
  // Dispatch `navigate`; run intercept handlers (same-document) or perform
  // the default (cross-document) navigation.
  function runNavigate(url, type, state, info, commitSameDoc) {
    const controller = new AbortController();
    const same = sameDoc(url, location.href);
    const dest = make(NavigationDestination, { url, entry: null, sameDocument: same, state });
    const ev = new NavigateEvent('navigate', { cancelable: true, navigationType: type, destination: dest, canIntercept: new URL(url).origin === location.origin, hashChange: same && url.includes('#') && url.split('#')[0] === location.href.split('#')[0] && url !== location.href, signal: controller.signal, info });
    let resolveCommit, rejectCommit, resolveFinish, rejectFinish;
    const committed = new Promise((a, b) => { resolveCommit = a; rejectCommit = b; });
    const finished = new Promise((a, b) => { resolveFinish = a; rejectFinish = b; });
    committed.catch(() => {}); finished.catch(() => {});
    const from = entries[index];
    if (!navigation.dispatchEvent(ev)) {
      const e = domError('Navigation was aborted', 'AbortError');
      controller.abort(e);
      rejectCommit(e); rejectFinish(e);
      navigation.dispatchEvent(new g.ErrorEvent('navigateerror', { error: e, message: e.message }));
      return { committed, finished };
    }
    if (ev[S].intercepted || (same && ev.hashChange)) {
      transition = make(NavigationTransition, { type, from, to: dest, finished, committed });
      if (commitSameDoc) {
        // Same-document commit through history.
        if (type === 'replace') H.replaceState.call(hist, null, '', url); else if (type === 'push') H.pushState.call(hist, null, '', url);
        if (state !== undefined) entries[index][S].navState = g.structuredClone(state);
      }
      resolveCommit(entries[index]);
      Promise.all(ev[S].handlers.map((h) => { try { return h(); } catch (e) { return Promise.reject(e); } })).then(() => {
        transition = null;
        navigation.dispatchEvent(new Event('navigatesuccess'));
        resolveFinish(entries[index]);
      }, (e) => {
        transition = null;
        navigation.dispatchEvent(new g.ErrorEvent('navigateerror', { error: e, message: String(e && e.message || e) }));
        rejectFinish(e);
      });
      return { committed, finished };
    }
    // Cross-document.
    if (type === 'reload') location.reload(); else if (type === 'replace') location.replace(url); else location.assign(url);
    return { committed, finished };
  }
  Object.defineProperty(g, 'navigation', { get() { return navigation; }, configurable: true, enumerable: true });

  // NotRestoredReasons (bfcache diagnostics on the navigation entry).
  class NotRestoredReasonDetails { constructor() { ILLEGAL(); } get reason() { return this[S].reason; } toJSON() { return { reason: this[S].reason }; } }
  tag(NotRestoredReasonDetails, 'NotRestoredReasonDetails');
  class NotRestoredReasons {
    constructor() { ILLEGAL(); }
    get src() { return null; } get id() { return null; } get name() { return null; } get url() { return location.href; }
    get reasons() { return []; } get children() { return []; }
    toJSON() { return { src: null, id: null, name: null, url: location.href, reasons: [], children: [] }; }
  }
  tag(NotRestoredReasons, 'NotRestoredReasons');

  for (const [n, C] of [['Navigation', Navigation], ['NavigationHistoryEntry', NavigationHistoryEntry], ['NavigationDestination', NavigationDestination], ['NavigationTransition', NavigationTransition], ['NavigationActivation', NavigationActivation], ['NavigationPrecommitController', NavigationPrecommitController], ['NavigateEvent', NavigateEvent], ['NavigationCurrentEntryChangeEvent', NavigationCurrentEntryChangeEvent], ['NotRestoredReasonDetails', NotRestoredReasonDetails], ['NotRestoredReasons', NotRestoredReasons]]) define(n, C);
})();
