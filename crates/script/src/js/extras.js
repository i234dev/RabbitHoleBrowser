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
  const named = g.__rhNative.named;
  const acc = (C, name, get, set) => { if (C && !Object.prototype.hasOwnProperty.call(C.prototype, name)) Object.defineProperty(C.prototype, name, { get: get && globalThis.__rhNative.branded(C, get, 'get', name), set: set && globalThis.__rhNative.branded(C, set, 'set', name), configurable: true, enumerable: true }); };
  const method = (C, name, fn) => { if (C && !Object.prototype.hasOwnProperty.call(C.prototype, name)) hidden(C.prototype, name, named(fn, name)); };
  // Enumerated attribute limited to known values.
  const {getAttribute, setAttribute, removeAttribute} = Element.prototype;
  function enumAttr(C, prop, attr, values, missing = '', invalid = missing, empty = invalid) {
    if (!C) return;
    const accessors = {
      get [prop]() {
        if (!C.prototype.isPrototypeOf(this)) throw new TypeError("Illegal invocation");
        const raw = getAttribute.call(this, attr);
        if (raw === null) return missing;
        if (raw === '') return empty;
        const keyword = raw.replace(/[A-Z]/g, ch => ch.toLowerCase());
        return values.includes(keyword) ? keyword : invalid;
      },
      set [prop](v) {
        if (!C.prototype.isPrototypeOf(this)) throw new TypeError("Illegal invocation");
        if (missing === null && v == null) removeAttribute.call(this, attr);
        else setAttribute.call(this, attr, v);
      },
    };
    Object.defineProperty(C.prototype, prop, Object.getOwnPropertyDescriptor(accessors, prop));
  }
  const strAttr = (C, prop, attr) => acc(C, prop, function () { return this.getAttribute(attr) || ''; }, function (v) { this.setAttribute(attr, v); });

  const POLICIES = ['', 'no-referrer', 'no-referrer-when-downgrade', 'same-origin', 'origin', 'strict-origin', 'origin-when-cross-origin', 'strict-origin-when-cross-origin', 'unsafe-url'];
  enumAttr(g.HTMLElement, 'dir', 'dir', ['ltr', 'rtl', 'auto']);
  enumAttr(g.HTMLElement, 'enterKeyHint', 'enterkeyhint', ['enter','done','go','next','previous','search','send']);
  enumAttr(g.HTMLElement, 'inputMode', 'inputmode', ['none','text','decimal','numeric','tel','search','email','url']);
  for (const C of [g.HTMLAnchorElement,g.HTMLAreaElement,g.HTMLImageElement,g.HTMLIFrameElement,g.HTMLLinkElement,g.HTMLScriptElement])
    enumAttr(C, 'referrerPolicy', 'referrerpolicy', POLICIES);
  for (const C of [g.HTMLImageElement,g.HTMLLinkElement,g.HTMLMediaElement,g.HTMLScriptElement])
    enumAttr(C, 'crossOrigin', 'crossorigin', ['anonymous','use-credentials'], null, 'anonymous');
  for (const C of [g.HTMLImageElement,g.HTMLIFrameElement])
    enumAttr(C, 'loading', 'loading', ['eager','lazy'], 'eager');
  enumAttr(g.HTMLImageElement, 'decoding', 'decoding', ['async','sync','auto'], 'auto');
  enumAttr(g.HTMLLinkElement, 'as', 'as', ['audio','document','embed','fetch','font','image','json','manifest','object','report','script','sharedworker','style','track','video','worker','xslt']);
  enumAttr(g.HTMLMediaElement, 'preload', 'preload', ['none','metadata','auto'], 'metadata', 'metadata', 'auto');
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
  if (g.SVGSVGElement) {
    for (const n of ['afterprint', 'beforeprint', 'beforeunload', 'hashchange', 'languagechange', 'message', 'messageerror', 'offline', 'online', 'pagehide', 'pagereveal', 'pageshow', 'pageswap', 'popstate', 'rejectionhandled', 'storage', 'unhandledrejection', 'unload']) {
      if (!Object.prototype.hasOwnProperty.call(g.SVGSVGElement.prototype, 'on' + n)) Object.defineProperty(g.SVGSVGElement.prototype, 'on' + n, { get: g.__rhNative.branded(g.SVGSVGElement, function () { return g['on' + n]; }, 'get', 'on' + n), set: g.__rhNative.branded(g.SVGSVGElement, function (v) { g['on' + n] = v; }, 'set', 'on' + n), configurable: true, enumerable: true });
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
