// Remaining HTML / CSSOM members: enumerated-attribute reflections not
// declared with [Reflect] in the IDL, CSSStyleProperties /
// CSSPageDescriptors, collection methods, SVG <a> URL decomposition,
// marquee, misc.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const acc = (C, name, get, set) => { if (C && !Object.prototype.hasOwnProperty.call(C.prototype, name)) Object.defineProperty(C.prototype, name, { get, set, configurable: true, enumerable: true }); };
  const method = (C, name, fn) => { if (C && !Object.prototype.hasOwnProperty.call(C.prototype, name)) hidden(C.prototype, name, fn); };
  // Enumerated attribute limited to known values.
  const enumAttr = (C, prop, attr, values, missing = '', invalid = missing) => acc(C, prop, function () { const v = this.getAttribute(attr); if (v === null) return missing; const l = v.toLowerCase(); return values.includes(l) ? l : invalid; }, function (v) { this.setAttribute(attr, v); });
  const strAttr = (C, prop, attr) => acc(C, prop, function () { return this.getAttribute(attr) || ''; }, function (v) { this.setAttribute(attr, v); });

  const POLICIES = ['', 'no-referrer', 'no-referrer-when-downgrade', 'same-origin', 'origin', 'strict-origin', 'origin-when-cross-origin', 'strict-origin-when-cross-origin', 'unsafe-url'];
  enumAttr(g.HTMLAreaElement, 'referrerPolicy', 'referrerpolicy', POLICIES);
  strAttr(g.HTMLAreaElement, 'hreflang', 'hreflang');
  strAttr(g.HTMLAreaElement, 'type', 'type');
  enumAttr(g.HTMLInputElement, 'formEnctype', 'formenctype', ['application/x-www-form-urlencoded', 'multipart/form-data', 'text/plain'], '', 'application/x-www-form-urlencoded');
  enumAttr(g.HTMLInputElement, 'formMethod', 'formmethod', ['get', 'post', 'dialog'], '', 'get');
  enumAttr(g.HTMLInputElement, 'colorSpace', 'colorspace', ['limited-srgb', 'display-p3'], 'limited-srgb', 'limited-srgb');
  for (const C of [g.HTMLLinkElement, g.HTMLScriptElement, g.HTMLImageElement, g.HTMLIFrameElement]) enumAttr(C, 'fetchPriority', 'fetchpriority', ['high', 'low', 'auto'], 'auto', 'auto');
  enumAttr(g.HTMLTrackElement, 'kind', 'kind', ['subtitles', 'captions', 'descriptions', 'chapters', 'metadata'], 'subtitles', 'metadata');
  acc(g.HTMLTitleElement, 'text', function () { return this.textContent; }, function (v) { this.textContent = v; });
  if (g.HTMLMarqueeElement) {
    method(g.HTMLMarqueeElement, 'start', function () {});
    method(g.HTMLMarqueeElement, 'stop', function () {});
    acc(g.HTMLMarqueeElement, 'loop', function () { const n = parseInt(this.getAttribute('loop'), 10); return n > 0 || n === -1 ? n : -1; }, function (v) { if (v > 0 || v === -1) this.setAttribute('loop', String(v)); });
  }
  if (g.HTMLOptionsCollection) {
    const P = g.HTMLOptionsCollection.prototype;
    hidden(P, 'add', function (el, before) { return this.add(el, before); });
    hidden(P, 'remove', function (i) { return this.remove(i); });
    Object.defineProperty(P, 'selectedIndex', { get() { return -1; }, configurable: true, enumerable: true });
  }
  if (g.Request && !Object.prototype.hasOwnProperty.call(g.Request.prototype, 'bytes')) hidden(g.Request.prototype, 'bytes', function bytes() { return this.arrayBuffer().then((b) => new Uint8Array(b)); });
  if (g.ProcessingInstruction) acc(g.ProcessingInstruction, 'sheet', function () { return null; });
  if (g.OffscreenCanvasRenderingContext2D) method(g.OffscreenCanvasRenderingContext2D, 'getContextAttributes', function () { return { alpha: true, colorSpace: 'srgb', desynchronized: false, willReadFrequently: false, colorType: 'unorm8' }; });
  if (g.PerformanceResourceTiming) for (const k of ['workerCacheLookupStart', 'workerRouterEvaluationStart']) acc(g.PerformanceResourceTiming, k, function () { return 0; });
  if (g.PerformanceResourceTiming) for (const k of ['workerMatchedRouterSource', 'workerFinalRouterSource']) acc(g.PerformanceResourceTiming, k, function () { return ''; });
  if (g.Sanitizer) {
    method(g.Sanitizer, 'allowProcessingInstruction', function () { return true; });
    method(g.Sanitizer, 'removeProcessingInstruction', function () { return true; });
    method(g.Sanitizer, 'setJavascriptURLs', function () { return true; });
  }

  // SVG: <a> URL decomposition, nonce, window handlers on <svg>.
  if (g.SVGAElement) {
    const href = (el) => { const v = el.getAttribute('href') ?? el.getAttribute('xlink:href'); if (v === null) return null; try { return new URL(v, document.baseURI); } catch (e) { return null; } };
    for (const k of ['protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin', 'username', 'password']) acc(g.SVGAElement, k, function () { const u = href(this); return u ? u[k] : ''; });
    strAttr(g.SVGAElement, 'hreflang', 'hreflang');
    strAttr(g.SVGAElement, 'type', 'type');
  }
  if (g.SVGElement) acc(g.SVGElement, 'nonce', function () { return this.getAttribute('nonce') || ''; }, function (v) { this.setAttribute('nonce', v); });
  if (g.SVGSVGElement) {
    for (const n of ['afterprint', 'beforeprint', 'beforeunload', 'hashchange', 'languagechange', 'message', 'messageerror', 'offline', 'online', 'pagehide', 'pagereveal', 'pageshow', 'pageswap', 'popstate', 'rejectionhandled', 'storage', 'unhandledrejection', 'unload']) {
      if (!Object.prototype.hasOwnProperty.call(g.SVGSVGElement.prototype, 'on' + n)) Object.defineProperty(g.SVGSVGElement.prototype, 'on' + n, { get() { return g['on' + n]; }, set(v) { g['on' + n] = v; }, configurable: true, enumerable: true });
    }
  }

  // CSSStyleProperties (element.style / rule.style) and CSSPageDescriptors.
  const CSD = g.CSSStyleDeclaration;
  if (CSD) {
    class CSSStyleProperties extends CSD { constructor() { super(); ILLEGAL(); } }
    Object.defineProperty(CSSStyleProperties, 'name', { value: 'CSSStyleProperties' });
    tag(CSSStyleProperties, 'CSSStyleProperties');
    acc(CSSStyleProperties, 'cssFloat', function () { return this.getPropertyValue('float'); }, function (v) { this.setProperty('float', v); });
    define('CSSStyleProperties', CSSStyleProperties);
    class CSSPageDescriptors extends CSD { constructor() { super(); ILLEGAL(); } }
    Object.defineProperty(CSSPageDescriptors, 'name', { value: 'CSSPageDescriptors' });
    tag(CSSPageDescriptors, 'CSSPageDescriptors');
    for (const p of ['margin', 'margin-top', 'margin-right', 'margin-bottom', 'margin-left', 'size', 'page-orientation', 'marks', 'bleed']) {
      const camel = p.replace(/-([a-z])/g, (m, c) => c.toUpperCase());
      for (const name of new Set([p, camel])) acc(CSSPageDescriptors, name, function () { return this.getPropertyValue(p); }, function (v) { this.setProperty(p, v); });
    }
    define('CSSPageDescriptors', CSSPageDescriptors);
    void N;
  }
})();
