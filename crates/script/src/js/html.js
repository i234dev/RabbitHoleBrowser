// HTML Standard surface implemented in JavaScript on top of the native DOM:
// content-attribute reflection (generated table), event handler
// attributes, HTML insertion methods, forms (constraint validation,
// collections), tables, media elements, templates, DOMStringMap, misc.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('html');
  const SD = Symbol('defaultValue'), SRV = Symbol('returnValue'), SPOP = Symbol('popoverOpen');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  // Define accessors / methods only where the native bindings lack them.
  const acc = (C, name, get, set) => {
    if (!C || Object.prototype.hasOwnProperty.call(C.prototype, name)) return;
    Object.defineProperty(C.prototype, name, { get, set, configurable: true, enumerable: true });
  };
  const method = (C, name, fn, force) => {
    if (!C || (!force && Object.prototype.hasOwnProperty.call(C.prototype, name))) return;
    Object.defineProperty(C.prototype, name, { value: fn, writable: true, configurable: true });
  };
  const resolveURL = (v) => { try { return new URL(v, document.baseURI).href; } catch (e) { return String(v); } };

  // ---------------------------------------------------------- reflection

  const table = g.__rhReflectTable || {};
  delete g.__rhReflectTable;
  const MIXINS = {
    HTMLOrSVGOrMathMLElement: ['HTMLElement', 'SVGElement'],
    ARIAMixin: ['Element'],
    HTMLHyperlinkElementUtils: ['HTMLAnchorElement', 'HTMLAreaElement'],
    PopoverInvokerElement: ['HTMLButtonElement', 'HTMLInputElement'],
    SVGURIReference: [],
  };
  const tokenLists = new WeakMap();
  function reflector(attr, kind, dflt) {
    switch (kind) {
      case 'b': return [function () { return this.hasAttribute(attr); }, function (v) { this.toggleAttribute(attr, !!v); }];
      case 'u': return [function () { const v = this.getAttribute(attr); return v === null ? '' : resolveURL(v); }, function (v) { this.setAttribute(attr, v); }];
      case 'l': case 'nn': return [function () {
        const v = this.getAttribute(attr);
        const n = v === null ? NaN : parseInt(v, 10);
        if (Number.isNaN(n) || n < -2147483648 || n > 2147483647 || (kind === 'nn' && n < 0)) return dflt !== undefined ? +dflt : (kind === 'nn' ? -1 : 0);
        return n;
      }, function (v) {
        v = Number(v) | 0;
        if (kind === 'nn' && v < 0) throw domError(`The value provided (${v}) is negative.`, 'IndexSizeError');
        this.setAttribute(attr, String(v));
      }];
      case 'ul': case 'pos': return [function () {
        const v = this.getAttribute(attr);
        const n = v === null ? NaN : parseInt(v, 10);
        const min = kind === 'pos' ? 1 : 0;
        if (Number.isNaN(n) || n < min || n > 2147483647) return dflt !== undefined ? +dflt : min;
        return n;
      }, function (v) { v = Number(v) >>> 0; this.setAttribute(attr, String(v > 2147483647 ? (dflt !== undefined ? +dflt : 0) : v)); }];
      case 'd': return [function () { const n = parseFloat(this.getAttribute(attr)); return Number.isFinite(n) ? n : (dflt !== undefined ? +dflt : 0); }, function (v) { this.setAttribute(attr, String(+v)); }];
      case 't': return [function () {
        let m = tokenLists.get(this);
        if (!m) { m = {}; tokenLists.set(this, m); }
        return m[attr] || (m[attr] = makeTokenList(this, attr));
      }, function (v) { this.setAttribute(attr, v); }];
      default: return [function () { const v = this.getAttribute(attr); return v === null ? (dflt !== undefined ? String(dflt).replace(/^"|"$/g, '') : '') : v; }, function (v) { this.setAttribute(attr, v); }];
    }
  }
  for (const [iface, entries] of Object.entries(table)) {
    const targets = MIXINS[iface] || [iface];
    for (const t of targets) {
      const C = g[t];
      if (!C) continue;
      for (const [prop, attr, kind, dflt] of entries) {
        if (prop in C.prototype) continue;
        const [get, set] = reflector(attr, kind, dflt);
        acc(C, prop, get, set);
      }
    }
  }

  // A DOMTokenList over any attribute (classList-like).
  function makeTokenList(el, attr) {
    const tl = Object.create(g.DOMTokenList.prototype);
    const read = () => (el.getAttribute(attr) || '').split(/[ \t\n\f\r]+/).filter(Boolean);
    const write = (list) => el.setAttribute(attr, Array.from(new Set(list)).join(' '));
    const check = (t) => {
      t = String(t);
      if (t === '') throw domError('The token provided must not be empty.', 'SyntaxError');
      if (/[ \t\n\f\r]/.test(t)) throw domError(`The token provided ('${t}') contains HTML space characters, which are not valid in tokens.`, 'InvalidCharacterError');
      return t;
    };
    const m = {
      add(...t) { const l = read(); for (const x of t) { check(x); if (!l.includes(x)) l.push(x); } write(l); },
      remove(...t) { t.forEach(check); write(read().filter((x) => !t.includes(x))); },
      toggle(t, force) { check(t); const l = read(); const has = l.includes(t); if (has && force !== true) { write(l.filter((x) => x !== t)); return false; } if (!has && force !== false) { l.push(t); write(l); return true; } return has; },
      contains(t) { return read().includes(String(t)); },
      replace(a, b) { check(a); check(b); const l = read(); const i = l.indexOf(a); if (i < 0) return false; l[i] = b; write(l); return true; },
      item(i) { return read()[i] ?? null; },
      supports() { return true; },
      forEach(cb, thisArg) { read().forEach((v, i) => cb.call(thisArg, v, i, tl)); },
      entries() { return read().entries(); }, keys() { return read().keys(); }, values() { return read().values(); },
      toString() { return el.getAttribute(attr) || ''; },
      [Symbol.iterator]() { return read()[Symbol.iterator](); },
    };
    for (const k of Reflect.ownKeys(m)) hidden(tl, k, m[k]);
    Object.defineProperty(tl, 'length', { get: () => read().length, configurable: true });
    Object.defineProperty(tl, 'value', { get: () => el.getAttribute(attr) || '', set: (v) => el.setAttribute(attr, v), configurable: true });
    return new Proxy(tl, { get(t, k) { return typeof k === 'string' && /^\d+$/.test(k) ? read()[+k] : Reflect.get(t, k, t); } });
  }

  // --------------------------------------------- event handler attributes

  const GLOBAL_HANDLERS = ['abort', 'auxclick', 'beforeinput', 'beforematch', 'beforetoggle', 'blur', 'cancel', 'canplay', 'canplaythrough', 'change', 'click', 'close', 'command', 'contextlost', 'contextmenu', 'contextrestored', 'copy', 'cuechange', 'cut', 'dblclick', 'drag', 'dragend', 'dragenter', 'dragleave', 'dragover', 'dragstart', 'drop', 'durationchange', 'emptied', 'ended', 'error', 'focus', 'formdata', 'input', 'invalid', 'keydown', 'keypress', 'keyup', 'load', 'loadeddata', 'loadedmetadata', 'loadstart', 'mousedown', 'mouseenter', 'mouseleave', 'mousemove', 'mouseout', 'mouseover', 'mouseup', 'paste', 'pause', 'play', 'playing', 'progress', 'ratechange', 'reset', 'resize', 'scroll', 'scrollend', 'securitypolicyviolation', 'seeked', 'seeking', 'select', 'slotchange', 'stalled', 'submit', 'suspend', 'timeupdate', 'toggle', 'volumechange', 'waiting', 'webkitanimationend', 'webkitanimationiteration', 'webkitanimationstart', 'webkittransitionend', 'wheel',
    'animationstart', 'animationend', 'animationiteration', 'animationcancel', 'transitionstart', 'transitionend', 'transitionrun', 'transitioncancel', 'pointerdown', 'pointerup', 'pointermove', 'pointerover', 'pointerout', 'pointerenter', 'pointerleave', 'pointercancel', 'pointerrawupdate', 'gotpointercapture', 'lostpointercapture', 'touchstart', 'touchend', 'touchmove', 'touchcancel', 'selectstart', 'selectionchange', 'focusin', 'focusout', 'compositionstart', 'compositionupdate', 'compositionend', 'search', 'fullscreenchange', 'fullscreenerror'];
  const WINDOW_HANDLERS = ['afterprint', 'beforeprint', 'beforeunload', 'hashchange', 'languagechange', 'message', 'messageerror', 'offline', 'online', 'pagehide', 'pagereveal', 'pageshow', 'pageswap', 'popstate', 'rejectionhandled', 'storage', 'unhandledrejection', 'unload', 'devicemotion', 'deviceorientation', 'deviceorientationabsolute', 'orientationchange', 'gamepadconnected', 'gamepaddisconnected'];
  const missing = (C, names) => names.filter((n) => !Object.prototype.hasOwnProperty.call(C.prototype, 'on' + n));
  for (const C of [g.HTMLElement, g.SVGElement, g.Document, g.Window, g.MathMLElement].filter(Boolean)) {
    const target = C === g.Window ? g : C.prototype;
    const names = (C === g.Window ? GLOBAL_HANDLERS.concat(WINDOW_HANDLERS) : GLOBAL_HANDLERS).filter((n) => !Object.prototype.hasOwnProperty.call(target, 'on' + n) && !(C !== g.Window && Object.prototype.hasOwnProperty.call(g.Element.prototype, 'on' + n)));
    if (names.length) N.installHandlers(target, names);
  }
  if (g.HTMLBodyElement) {
    // <body>/<frameset> forward window handlers.
    for (const C of [g.HTMLBodyElement, g.HTMLFrameSetElement].filter(Boolean)) {
      for (const n of WINDOW_HANDLERS.concat(['blur', 'error', 'focus', 'load', 'resize', 'scroll'])) {
        if (Object.prototype.hasOwnProperty.call(C.prototype, 'on' + n)) continue;
        Object.defineProperty(C.prototype, 'on' + n, { get() { return g['on' + n]; }, set(v) { g['on' + n] = v; }, configurable: true, enumerable: true });
      }
    }
  }

  // ----------------------------------------------- HTML insertion methods

  function fragmentFor(context, html) {
    const t = document.createElement('template');
    t.innerHTML = String(html);
    return document.importNode(t.content, true);
  }
  const E = g.Element, CD = g.CharacterData, SR = g.ShadowRoot, DF = g.DocumentFragment;
  for (const [C, insert] of [[E, true], [SR, true]]) {
    method(C, 'setHTMLUnsafe', function (html) {
      if (this instanceof g.HTMLTemplateElement) { this.content.replaceChildren(fragmentFor(this, html)); return; }
      this.replaceChildren(fragmentFor(this, html));
    });
    method(C, 'setHTML', function (html) { this.setHTMLUnsafe(sanitize(String(html))); });
    method(C, 'getHTML', function (opts) {
      if (!opts || !opts.serializableShadowRoots) return this.innerHTML;
      return this.innerHTML;
    });
    method(C, 'appendHTMLUnsafe', function (html) { this.append(fragmentFor(this, html)); });
    method(C, 'prependHTMLUnsafe', function (html) { this.prepend(fragmentFor(this, html)); });
    method(C, 'appendHTML', function (html) { this.append(fragmentFor(this, sanitize(String(html)))); });
    method(C, 'prependHTML', function (html) { this.prepend(fragmentFor(this, sanitize(String(html)))); });
    void insert;
  }
  for (const C of [E, CD]) {
    method(C, 'beforeHTMLUnsafe', function (html) { this.before(fragmentFor(this.parentNode, html)); });
    method(C, 'afterHTMLUnsafe', function (html) { this.after(fragmentFor(this.parentNode, html)); });
    method(C, 'replaceWithHTMLUnsafe', function (html) { this.replaceWith(fragmentFor(this.parentNode, html)); });
    method(C, 'beforeHTML', function (html) { this.before(fragmentFor(this.parentNode, sanitize(String(html)))); });
    method(C, 'afterHTML', function (html) { this.after(fragmentFor(this.parentNode, sanitize(String(html)))); });
    method(C, 'replaceWithHTML', function (html) { this.replaceWith(fragmentFor(this.parentNode, sanitize(String(html)))); });
  }
  void DF;

  // ------------------------------------------------------------ Sanitizer

  const DEFAULT_REMOVE = ['script', 'iframe', 'object', 'embed', 'frame', 'frameset', 'noscript', 'noembed', 'noframes', 'template', 'style', 'link', 'meta', 'base'];
  function sanitize(html, sanitizer) {
    const t = document.createElement('template');
    t.innerHTML = html;
    const cfg = sanitizer ? sanitizer.get() : { removeElements: DEFAULT_REMOVE };
    const walk = (root) => {
      for (const el of Array.from(root.querySelectorAll('*'))) {
        const name = el.localName;
        if ((cfg.removeElements || []).includes(name) || (cfg.elements && !cfg.elements.includes(name))) { el.remove(); continue; }
        if ((cfg.replaceWithChildrenElements || []).includes(name)) { el.replaceWith(...el.childNodes); continue; }
        for (const a of Array.from(el.getAttributeNames())) {
          const v = el.getAttribute(a) || '';
          if (/^on/i.test(a) || ((a === 'href' || a === 'src' || a === 'action' || a === 'formaction') && /^\s*javascript:/i.test(v)) || (cfg.removeAttributes || []).includes(a) || (cfg.attributes && !cfg.attributes.includes(a))) el.removeAttribute(a);
        }
      }
    };
    walk(t.content);
    return t.innerHTML;
  }
  class Sanitizer {
    constructor(config = {}) {
      const c = typeof config === 'string' ? {} : (config || {});
      hidden(this, S, {
        elements: c.elements ? c.elements.map((e) => (typeof e === 'string' ? e : e.name)) : null,
        removeElements: (c.removeElements || DEFAULT_REMOVE).map((e) => (typeof e === 'string' ? e : e.name)),
        replaceWithChildrenElements: (c.replaceWithChildrenElements || []).map((e) => (typeof e === 'string' ? e : e.name)),
        attributes: c.attributes ? c.attributes.map((e) => (typeof e === 'string' ? e : e.name)) : null,
        removeAttributes: (c.removeAttributes || []).map((e) => (typeof e === 'string' ? e : e.name)),
        comments: !!c.comments, dataAttributes: c.dataAttributes !== false,
      });
    }
    get() { const s = this[S]; return { elements: s.elements, removeElements: s.removeElements.slice(), replaceWithChildrenElements: s.replaceWithChildrenElements.slice(), attributes: s.attributes, removeAttributes: s.removeAttributes.slice(), comments: s.comments, dataAttributes: s.dataAttributes }; }
    allowElement(e) { const n = typeof e === 'string' ? e : e.name; const s = this[S]; s.removeElements = s.removeElements.filter((x) => x !== n); if (s.elements && !s.elements.includes(n)) s.elements.push(n); return true; }
    removeElement(e) { const n = typeof e === 'string' ? e : e.name; const s = this[S]; if (!s.removeElements.includes(n)) s.removeElements.push(n); return true; }
    replaceElementWithChildren(e) { this[S].replaceWithChildrenElements.push(typeof e === 'string' ? e : e.name); return true; }
    allowAttribute(a) { const n = typeof a === 'string' ? a : a.name; const s = this[S]; s.removeAttributes = s.removeAttributes.filter((x) => x !== n); if (s.attributes && !s.attributes.includes(n)) s.attributes.push(n); return true; }
    removeAttribute(a) { this[S].removeAttributes.push(typeof a === 'string' ? a : a.name); return true; }
    setComments(v) { this[S].comments = !!v; return true; }
    setDataAttributes(v) { this[S].dataAttributes = !!v; return true; }
    removeUnsafe() { const s = this[S]; for (const n of DEFAULT_REMOVE) if (!s.removeElements.includes(n)) s.removeElements.push(n); return true; }
  }
  tag(Sanitizer, 'Sanitizer');
  define('Sanitizer', Sanitizer);
  g.Document.parseHTMLUnsafe = function (html) { return new g.DOMParser().parseFromString(String(html), 'text/html'); };
  g.Document.parseHTML = function (html, opts) { return new g.DOMParser().parseFromString(sanitize(String(html), opts && opts.sanitizer instanceof Sanitizer ? opts.sanitizer : null), 'text/html'); };

  // -------------------------------------------------------- HTMLElement

  const HE = g.HTMLElement;
  acc(HE, 'accessKeyLabel', function () { return ''; });
  acc(HE, 'autofocus', function () { return this.hasAttribute('autofocus'); }, function (v) { this.toggleAttribute('autofocus', !!v); });
  acc(HE, 'autocorrect', function () { const v = (this.getAttribute('autocorrect') || '').toLowerCase(); return v !== 'off'; }, function (v) { this.setAttribute('autocorrect', v ? 'on' : 'off'); });
  acc(HE, 'writingSuggestions', function () { const v = (this.getAttribute('writingsuggestions') || '').toLowerCase(); return v === 'false' ? 'false' : 'true'; }, function (v) { this.setAttribute('writingsuggestions', v); });
  acc(HE, 'headingOffset', function () { const n = parseInt(this.getAttribute('headingoffset'), 10); return Number.isFinite(n) ? Math.max(0, Math.min(8, n)) : 0; }, function (v) { this.setAttribute('headingoffset', String(v | 0)); });
  acc(HE, 'headingReset', function () { return this.hasAttribute('headingreset'); }, function (v) { this.toggleAttribute('headingreset', !!v); });
  for (const C of [HE, g.SVGElement]) {
    acc(C, 'autofocus', function () { return this.hasAttribute('autofocus'); }, function (v) { this.toggleAttribute('autofocus', !!v); });
  }

  // ElementInternals / CustomStateSet (form-associated custom elements).
  class CustomStateSet {
    constructor() { ILLEGAL(); }
    get size() { return this[S].size; }
    has(v) { return this[S].has(String(v)); }
    add(v) { this[S].add(String(v)); return this; }
    delete(v) { return this[S].delete(String(v)); }
    clear() { this[S].clear(); }
    forEach(cb, t) { this[S].forEach((v) => cb.call(t, v, v, this)); }
    entries() { return this[S].entries(); } keys() { return this[S].keys(); } values() { return this[S].values(); }
    [Symbol.iterator]() { return this[S].values(); }
  }
  tag(CustomStateSet, 'CustomStateSet');
  const internalsOf = new WeakMap();
  class ElementInternals {
    constructor() { ILLEGAL(); }
    get shadowRoot() { const r = this[S].el.shadowRoot; return r; }
    setFormValue(value, state) { this[S].value = value; this[S].state = state === undefined ? value : state; }
    get form() { return formAssociated(this[S].el) ? this[S].el.closest('form') : null; }
    setValidity(flags = {}, message, anchor) {
      const v = {};
      for (const k of VALIDITY_FLAGS) v[k] = !!flags[k];
      const any = VALIDITY_FLAGS.some((k) => v[k]);
      if (any && (message === undefined || message === '')) throw new TypeError("Failed to execute 'setValidity' on 'ElementInternals': The second argument should not be empty if one or more flags in the first argument are true.");
      this[S].flags = v;
      this[S].message = any ? String(message) : '';
      this[S].anchor = anchor || null;
    }
    get willValidate() { return formAssociated(this[S].el) && !this[S].el.hasAttribute('disabled') && !this[S].el.hasAttribute('readonly'); }
    get validity() { return makeValidity(() => Object.assign({ customError: false }, this[S].flags)); }
    get validationMessage() { return this[S].message; }
    checkValidity() { return checkValidityOf(this[S].el, this, false); }
    reportValidity() { return checkValidityOf(this[S].el, this, true); }
    get labels() { const id = this[S].el.id; return document.querySelectorAll(id ? `label[for="${CSS.escape(id)}"]` : 'label:not(*)'); }
    get states() { return this[S].states; }
  }
  tag(ElementInternals, 'ElementInternals');
  // ARIA reflection on ElementInternals (role, aria*): stored, not rendered.
  for (const k of ['role', 'ariaActiveDescendantElement', 'ariaAtomic', 'ariaAutoComplete', 'ariaBrailleLabel', 'ariaBrailleRoleDescription', 'ariaBusy', 'ariaChecked', 'ariaColCount', 'ariaColIndex', 'ariaColIndexText', 'ariaColSpan', 'ariaControlsElements', 'ariaCurrent', 'ariaDescribedByElements', 'ariaDescription', 'ariaDetailsElements', 'ariaDisabled', 'ariaErrorMessageElements', 'ariaExpanded', 'ariaFlowToElements', 'ariaHasPopup', 'ariaHidden', 'ariaInvalid', 'ariaKeyShortcuts', 'ariaLabel', 'ariaLabelledByElements', 'ariaLevel', 'ariaLive', 'ariaModal', 'ariaMultiLine', 'ariaMultiSelectable', 'ariaOrientation', 'ariaOwnsElements', 'ariaPlaceholder', 'ariaPosInSet', 'ariaPressed', 'ariaReadOnly', 'ariaRelevant', 'ariaRequired', 'ariaRoleDescription', 'ariaRowCount', 'ariaRowIndex', 'ariaRowIndexText', 'ariaRowSpan', 'ariaSelected', 'ariaSetSize', 'ariaSort', 'ariaValueMax', 'ariaValueMin', 'ariaValueNow', 'ariaValueText']) {
    Object.defineProperty(ElementInternals.prototype, k, { get() { return this[S].aria[k] ?? null; }, set(v) { this[S].aria[k] = v; }, configurable: true, enumerable: true });
  }
  const formAssociated = (el) => { try { const C = customElements.get(el.localName); return !!(C && C.formAssociated); } catch (e) { return false; } };
  method(HE, 'attachInternals', function () {
    if (internalsOf.has(this)) throw domError("Failed to execute 'attachInternals' on 'HTMLElement': ElementInternals for the specified element was already attached.", 'NotSupportedError');
    if (!this.localName.includes('-')) throw domError("Failed to execute 'attachInternals' on 'HTMLElement': Unable to attach ElementInternals to non-custom elements.", 'NotSupportedError');
    const states = Object.create(CustomStateSet.prototype);
    hidden(states, S, new Set());
    const i = Object.create(ElementInternals.prototype);
    hidden(i, S, { el: this, value: null, state: null, flags: {}, message: '', anchor: null, aria: {}, states });
    internalsOf.set(this, i);
    return i;
  });

  // ------------------------------------------------- constraint validation

  const VALIDITY_FLAGS = ['valueMissing', 'typeMismatch', 'patternMismatch', 'tooLong', 'tooShort', 'rangeUnderflow', 'rangeOverflow', 'stepMismatch', 'badInput', 'customError'];
  class ValidityState {
    constructor() { ILLEGAL(); }
    get valid() { const f = this[S](); return !VALIDITY_FLAGS.some((k) => f[k]); }
  }
  for (const k of VALIDITY_FLAGS) Object.defineProperty(ValidityState.prototype, k, { get() { return !!this[S]()[k]; }, configurable: true, enumerable: true });
  tag(ValidityState, 'ValidityState');
  define('ValidityState', ValidityState);
  function makeValidity(compute) { const v = Object.create(ValidityState.prototype); hidden(v, S, compute); return v; }
  const customMessages = new WeakMap();
  const EMAIL = /^[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$/;
  const NUMERIC = ['number', 'range', 'date', 'month', 'week', 'time', 'datetime-local'];
  function numValue(type, v) {
    if (v === '') return NaN;
    switch (type) {
      case 'date': { const t = Date.parse(v + 'T00:00:00Z'); return /^\d{4,}-\d\d-\d\d$/.test(v) ? t : NaN; }
      case 'month': { const m = /^(\d{4,})-(\d\d)$/.exec(v); return m ? (+m[1] - 1970) * 12 + (+m[2] - 1) : NaN; }
      case 'week': { const m = /^(\d{4,})-W(\d\d)$/.exec(v); return m ? Date.UTC(+m[1], 0, 1) + (+m[2] - 1) * 604800000 : NaN; }
      case 'time': { const m = /^(\d\d):(\d\d)(?::(\d\d(?:\.\d+)?))?$/.exec(v); return m ? (+m[1] * 3600 + +m[2] * 60 + +(m[3] || 0)) * 1000 : NaN; }
      case 'datetime-local': { const t = Date.parse(v.replace(' ', 'T') + 'Z'); return Number.isFinite(t) ? t : NaN; }
      default: { const t = /^-?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/.test(v.trim()) ? parseFloat(v) : NaN; return t; }
    }
  }
  function validityFlags(el) {
    const f = {};
    const custom = customMessages.get(el);
    f.customError = !!custom;
    const name = el.localName;
    if (!['input', 'textarea', 'select'].includes(name)) return f;
    const type = name === 'input' ? (el.type || 'text') : name;
    const value = el.value == null ? '' : String(el.value);
    if (el.hasAttribute('required')) {
      if (type === 'checkbox') f.valueMissing = !el.checked;
      else if (type === 'radio') {
        const nm = el.name;
        const group = nm && el.form ? Array.from(el.form.querySelectorAll('input[type=radio]')).filter((r) => r.name === nm) : nm ? Array.from(document.querySelectorAll(`input[type=radio][name="${CSS.escape(nm)}"]`)) : [el];
        f.valueMissing = !group.some((r) => r.checked);
      } else if (type === 'file') f.valueMissing = !(el.files && el.files.length);
      else if (name === 'select') f.valueMissing = el.selectedIndex < 0 || (el.options[el.selectedIndex] && el.options[el.selectedIndex].value === '' && !el.multiple && el.options[el.selectedIndex].index === 0);
      else f.valueMissing = value === '';
    }
    if (value !== '') {
      if (type === 'email') f.typeMismatch = !(el.multiple ? value.split(',').every((v) => EMAIL.test(v.trim())) : EMAIL.test(value));
      if (type === 'url') f.typeMismatch = !/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(value) || (() => { try { new URL(value); return false; } catch (e) { return true; } })();
      const pattern = el.getAttribute && el.getAttribute('pattern');
      if (pattern !== null && pattern !== undefined && ['text', 'search', 'tel', 'url', 'email', 'password'].includes(type)) {
        try { f.patternMismatch = !new RegExp('^(?:' + pattern + ')$', 'v').test(value); } catch (e) { try { f.patternMismatch = !new RegExp('^(?:' + pattern + ')$', 'u').test(value); } catch (e2) {} }
      }
      const len = value.length;
      const maxL = parseInt(el.getAttribute('maxlength'), 10), minL = parseInt(el.getAttribute('minlength'), 10);
      if (el.__rhDirty && Number.isFinite(maxL) && len > maxL) f.tooLong = true;
      if (el.__rhDirty && Number.isFinite(minL) && len < minL) f.tooShort = true;
      if (NUMERIC.includes(type)) {
        const n = numValue(type, value);
        if (Number.isNaN(n)) f.badInput = type === 'number' ? false : false;
        else {
          const min = numValue(type, el.getAttribute('min') || ''), max = numValue(type, el.getAttribute('max') || '');
          if (!Number.isNaN(min) && n < min) f.rangeUnderflow = true;
          if (!Number.isNaN(max) && n > max) f.rangeOverflow = true;
          const stepAttr = (el.getAttribute('step') || '').toLowerCase();
          if (stepAttr !== 'any') {
            const unit = type === 'time' || type === 'datetime-local' ? 1000 : type === 'date' ? 86400000 : type === 'week' ? 604800000 : 1;
            const dfl = type === 'time' || type === 'datetime-local' ? 60 : 1;
            let step = parseFloat(stepAttr);
            if (!(step > 0)) step = dfl;
            step *= unit;
            const base = !Number.isNaN(min) ? min : 0;
            const q = (n - base) / step;
            if (Math.abs(q - Math.round(q)) > 1e-9) f.stepMismatch = true;
          }
        }
      }
    }
    return f;
  }
  const barred = (el) => {
    if (el.disabled || el.closest && el.closest('fieldset[disabled]') && !el.closest('legend')) return true;
    if (el.localName === 'input' && ['hidden', 'reset', 'button'].includes(el.type)) return true;
    if (el.localName === 'button' && el.type !== 'submit') return true;
    if (['output', 'fieldset', 'object'].includes(el.localName)) return true;
    if (el.hasAttribute && el.hasAttribute('readonly') && ['input', 'textarea'].includes(el.localName)) return true;
    return !!(el.closest && el.closest('datalist'));
  };
  function message(el, f) {
    const custom = customMessages.get(el);
    if (custom) return custom;
    if (f.valueMissing) return el.type === 'checkbox' ? 'Please check this box if you want to proceed.' : el.localName === 'select' ? 'Please select an item in the list.' : 'Please fill out this field.';
    if (f.typeMismatch) return el.type === 'email' ? 'Please enter an email address.' : 'Please enter a URL.';
    if (f.patternMismatch) return 'Please match the requested format.';
    if (f.tooLong) return `Please shorten this text to ${el.getAttribute('maxlength')} characters or less.`;
    if (f.tooShort) return `Please lengthen this text to ${el.getAttribute('minlength')} characters or more.`;
    if (f.rangeUnderflow) return `Value must be greater than or equal to ${el.getAttribute('min')}.`;
    if (f.rangeOverflow) return `Value must be less than or equal to ${el.getAttribute('max')}.`;
    if (f.stepMismatch) return 'Please enter a valid value.';
    if (f.badInput) return 'Please enter a number.';
    return '';
  }
  function checkValidityOf(el, internals, report) {
    let valid;
    if (internals) valid = !internals.willValidate || internals.validity.valid;
    else valid = !el.willValidate || el.validity.valid;
    if (!valid) el.dispatchEvent(new Event('invalid', { cancelable: true }));
    if (!valid && report) { try { el.focus(); } catch (e) {} }
    return valid;
  }
  const LISTED = ['button', 'fieldset', 'input', 'object', 'output', 'select', 'textarea'];
  for (const name of ['HTMLInputElement', 'HTMLTextAreaElement', 'HTMLSelectElement', 'HTMLButtonElement', 'HTMLFieldSetElement', 'HTMLOutputElement', 'HTMLObjectElement']) {
    const C = g[name];
    if (!C) continue;
    Object.defineProperty(C.prototype, 'validity', { get() { const el = this; return makeValidity(() => validityFlags(el)); }, configurable: true, enumerable: true });
    Object.defineProperty(C.prototype, 'willValidate', { get() { return !barred(this); }, configurable: true, enumerable: true });
    Object.defineProperty(C.prototype, 'validationMessage', { get() { return this.willValidate ? message(this, validityFlags(this)) : ''; }, configurable: true, enumerable: true });
    method(C, 'setCustomValidity', function (m) { m = String(m); if (m) customMessages.set(this, m); else customMessages.delete(this); }, true);
    method(C, 'checkValidity', function () { return checkValidityOf(this, null, false); }, true);
    method(C, 'reportValidity', function () { return checkValidityOf(this, null, true); }, true);
  }
  // Dirty value flag for tooLong / tooShort (only user edits count).
  for (const C of [g.HTMLInputElement, g.HTMLTextAreaElement]) {
    const d = Object.getOwnPropertyDescriptor(C.prototype, 'value');
    if (d && d.set) Object.defineProperty(C.prototype, 'value', { get: d.get, set(v) { d.set.call(this, v); hidden(this, '__rhDirty', false); }, configurable: true, enumerable: true });
  }
  const FormP = g.HTMLFormElement && g.HTMLFormElement.prototype;
  if (FormP) {
    const controls = (form) => Array.from(form.elements).filter((e) => LISTED.includes(e.localName));
    method(g.HTMLFormElement, 'checkValidity', function () { let ok = true; for (const c of controls(this)) if (c.willValidate && !c.validity.valid) { ok = false; c.dispatchEvent(new Event('invalid', { cancelable: true })); } return ok; }, true);
    method(g.HTMLFormElement, 'reportValidity', function () { const ok = this.checkValidity(); if (!ok) { const first = controls(this).find((c) => c.willValidate && !c.validity.valid); if (first) try { first.focus(); } catch (e) {} } return ok; }, true);
    acc(g.HTMLFormElement, 'encoding', function () { return this.enctype; }, function (v) { this.enctype = v; });
    acc(g.HTMLFormElement, 'rel', function () { return this.getAttribute('rel') || ''; }, function (v) { this.setAttribute('rel', v); });
    acc(g.HTMLFormElement, 'relList', function () { let m = tokenLists.get(this); if (!m) { m = {}; tokenLists.set(this, m); } return m.rel || (m.rel = makeTokenList(this, 'rel')); });
  }

  // HTMLFormControlsCollection / RadioNodeList / HTMLOptionsCollection.
  class RadioNodeList extends g.NodeList {
    constructor() { ILLEGAL(); }
    get value() { const c = Array.from(this).find((r) => r.localName === 'input' && r.type === 'radio' && r.checked); return c ? c.value : ''; }
    set value(v) { for (const r of this) if (r.localName === 'input' && r.type === 'radio' && r.value === String(v)) { r.checked = true; break; } }
  }
  tag(RadioNodeList, 'RadioNodeList');
  define('RadioNodeList', RadioNodeList);
  class HTMLFormControlsCollection extends g.HTMLCollection {
    constructor() { ILLEGAL(); }
  }
  tag(HTMLFormControlsCollection, 'HTMLFormControlsCollection');
  define('HTMLFormControlsCollection', HTMLFormControlsCollection);
  class HTMLOptionsCollection extends g.HTMLCollection {
    constructor() { ILLEGAL(); }
  }
  tag(HTMLOptionsCollection, 'HTMLOptionsCollection');
  define('HTMLOptionsCollection', HTMLOptionsCollection);
  function namedItemOf(list, name) {
    const hits = Array.from(list).filter((e) => e.id === name || e.getAttribute('name') === name);
    if (hits.length === 0) return null;
    if (hits.length === 1) return hits[0];
    const r = Object.setPrototypeOf(hits, RadioNodeList.prototype);
    return r;
  }
  const elementsDesc = FormP && Object.getOwnPropertyDescriptor(FormP, 'elements');
  if (elementsDesc && elementsDesc.get) {
    Object.defineProperty(FormP, 'elements', {
      get() {
        const list = elementsDesc.get.call(this);
        Object.setPrototypeOf(list, HTMLFormControlsCollection.prototype);
        hidden(list, 'namedItem', (name) => namedItemOf(list, String(name)));
        return list;
      }, configurable: true, enumerable: true,
    });
  }
  const optionsDesc = g.HTMLSelectElement && Object.getOwnPropertyDescriptor(g.HTMLSelectElement.prototype, 'options');
  if (optionsDesc && optionsDesc.get) {
    Object.defineProperty(g.HTMLSelectElement.prototype, 'options', {
      get() {
        const sel = this;
        const list = optionsDesc.get.call(this);
        Object.setPrototypeOf(list, HTMLOptionsCollection.prototype);
        hidden(list, 'add', (el, before) => sel.add(el, before));
        hidden(list, 'remove', (i) => sel.remove(i));
        Object.defineProperty(list, 'selectedIndex', { get: () => sel.selectedIndex, set: (v) => { sel.selectedIndex = v; }, configurable: true });
        hidden(list, 'namedItem', (name) => Array.from(list).find((o) => o.id === name || o.getAttribute('name') === name) || null);
        return list;
      }, configurable: true, enumerable: true,
    });
  }
  method(g.HTMLSelectElement, 'showPicker', function () {});
  method(g.HTMLInputElement, 'showPicker', function () {});
  acc(g.HTMLInputElement, 'valueAsDate', function () {
    if (!['date', 'month', 'week', 'time', 'datetime-local'].includes(this.type) || this.type === 'datetime-local') return null;
    const n = numValue(this.type, this.value);
    if (Number.isNaN(n)) return null;
    if (this.type === 'month') return new Date(Date.UTC(1970 + Math.floor(n / 12), n % 12, 1));
    return new Date(n);
  }, function (d) {
    if (d === null) { this.value = ''; return; }
    if (!(d instanceof Date)) throw new TypeError("Failed to set the 'valueAsDate' property on 'HTMLInputElement': The provided value is not a Date.");
    const iso = d.toISOString();
    if (this.type === 'date') this.value = iso.slice(0, 10);
    else if (this.type === 'month') this.value = iso.slice(0, 7);
    else if (this.type === 'time') this.value = iso.slice(11, 16);
    else throw domError("This input element does not support Date values.", 'InvalidStateError');
  });
  for (const C of [g.HTMLTextAreaElement]) {
    acc(C, 'selectionDirection', function () { return 'none'; }, function () {});
    method(C, 'setRangeText', function (text, start, end) {
      const v = this.value;
      if (start === undefined) { start = this.selectionStart || 0; end = this.selectionEnd || 0; }
      this.value = v.slice(0, start) + String(text) + v.slice(end);
    });
  }
  for (const C of [g.HTMLButtonElement, g.HTMLInputElement]) {
    acc(C, 'popoverTargetElement', function () { const id = this.getAttribute('popovertarget'); return id ? document.getElementById(id) : null; }, function (el) { if (el && el.id) this.setAttribute('popovertarget', el.id); else this.removeAttribute('popovertarget'); });
    acc(C, 'popoverTargetAction', function () { const v = (this.getAttribute('popovertargetaction') || '').toLowerCase(); return ['show', 'hide'].includes(v) ? v : 'toggle'; }, function (v) { this.setAttribute('popovertargetaction', v); });
  }
  acc(g.HTMLButtonElement, 'commandForElement', function () { const id = this.getAttribute('commandfor'); return id ? document.getElementById(id) : null; }, function (el) { if (el && el.id) this.setAttribute('commandfor', el.id); else this.removeAttribute('commandfor'); });
  acc(g.HTMLButtonElement, 'command', function () { return this.getAttribute('command') || ''; }, function (v) { this.setAttribute('command', v); });

  // HTMLOutputElement / HTMLMeterElement / HTMLFieldSetElement / object.
  if (g.HTMLOutputElement) {
    const O = g.HTMLOutputElement;
    acc(O, 'type', function () { return 'output'; });
    acc(O, 'defaultValue', function () { return this[SD] !== undefined ? this[SD] : this.textContent; }, function (v) { this[SD] = String(v); });
    acc(O, 'value', function () { return this.textContent; }, function (v) { this.textContent = v; });
    acc(O, 'form', function () { return this.closest('form'); });
    acc(O, 'labels', function () { return labelsFor(this); });
    acc(O, 'htmlFor', function () { let m = tokenLists.get(this); if (!m) { m = {}; tokenLists.set(this, m); } return m.for || (m.for = makeTokenList(this, 'for')); });
  }
  if (g.HTMLMeterElement) {
    const num = (el, a, d) => { const n = parseFloat(el.getAttribute(a)); return Number.isFinite(n) ? n : d; };
    const M = g.HTMLMeterElement;
    acc(M, 'min', function () { return num(this, 'min', 0); }, function (v) { this.setAttribute('min', v); });
    acc(M, 'max', function () { return Math.max(num(this, 'max', 1), this.min); }, function (v) { this.setAttribute('max', v); });
    acc(M, 'value', function () { return Math.min(Math.max(num(this, 'value', 0), this.min), this.max); }, function (v) { this.setAttribute('value', v); });
    acc(M, 'low', function () { return Math.min(Math.max(num(this, 'low', this.min), this.min), this.max); }, function (v) { this.setAttribute('low', v); });
    acc(M, 'high', function () { return Math.min(Math.max(num(this, 'high', this.max), this.low), this.max); }, function (v) { this.setAttribute('high', v); });
    acc(M, 'optimum', function () { return Math.min(Math.max(num(this, 'optimum', (this.min + this.max) / 2), this.min), this.max); }, function (v) { this.setAttribute('optimum', v); });
    acc(M, 'labels', function () { return labelsFor(this); });
  }
  if (g.HTMLProgressElement) {
    acc(g.HTMLProgressElement, 'position', function () { return this.hasAttribute('value') ? this.value / this.max : -1; });
    acc(g.HTMLProgressElement, 'labels', function () { return labelsFor(this); });
  }
  if (g.HTMLFieldSetElement) {
    const F = g.HTMLFieldSetElement;
    acc(F, 'type', function () { return 'fieldset'; });
    acc(F, 'form', function () { return this.closest('form'); });
    acc(F, 'elements', function () { const l = this.querySelectorAll('button, fieldset, input, object, output, select, textarea'); Object.setPrototypeOf(l, HTMLFormControlsCollection.prototype); return l; });
  }
  for (const C of [g.HTMLObjectElement, g.HTMLLegendElement, g.HTMLOptionElement].filter(Boolean)) acc(C, 'form', function () { return this.closest('form'); });
  if (g.HTMLObjectElement) {
    acc(g.HTMLObjectElement, 'contentDocument', function () { return null; });
    acc(g.HTMLObjectElement, 'contentWindow', function () { return null; });
    method(g.HTMLObjectElement, 'getSVGDocument', function () { return null; });
  }
  for (const C of [g.HTMLEmbedElement, g.HTMLIFrameElement, g.HTMLFrameElement].filter(Boolean)) method(C, 'getSVGDocument', function () { return null; });
  if (g.HTMLFrameElement) {
    acc(g.HTMLFrameElement, 'contentDocument', function () { return null; });
    acc(g.HTMLFrameElement, 'contentWindow', function () { return null; });
  }
  function labelsFor(el) {
    const out = [];
    for (const l of document.querySelectorAll('label')) if (l.control === el) out.push(l);
    return Object.setPrototypeOf(out, g.NodeList.prototype);
  }

  // ------------------------------------------------------------- tables

  const T = g.HTMLTableElement;
  if (T) {
    const childOf = (t, name) => Array.from(t.children).find((c) => c.localName === name) || null;
    acc(T, 'caption', function () { return childOf(this, 'caption'); }, function (c) { const old = childOf(this, 'caption'); if (old) old.remove(); if (c) this.prepend(c); });
    acc(T, 'tHead', function () { return childOf(this, 'thead'); }, function (h) { const old = childOf(this, 'thead'); if (old) old.remove(); if (h) this.insertBefore(h, Array.from(this.children).find((c) => !['caption', 'colgroup'].includes(c.localName)) || null); });
    acc(T, 'tFoot', function () { return childOf(this, 'tfoot'); }, function (f) { const old = childOf(this, 'tfoot'); if (old) old.remove(); if (f) this.appendChild(f); });
    method(T, 'createCaption', function () { return this.caption || this.insertBefore(document.createElement('caption'), this.firstChild); });
    method(T, 'deleteCaption', function () { const c = this.caption; if (c) c.remove(); });
    method(T, 'createTHead', function () { if (this.tHead) return this.tHead; const h = document.createElement('thead'); this.tHead = h; return h; });
    method(T, 'deleteTHead', function () { const h = this.tHead; if (h) h.remove(); });
    method(T, 'createTFoot', function () { if (this.tFoot) return this.tFoot; const f = document.createElement('tfoot'); this.appendChild(f); return f; });
    method(T, 'deleteTFoot', function () { const f = this.tFoot; if (f) f.remove(); });
    method(T, 'createTBody', function () { const b = document.createElement('tbody'); const bodies = Array.from(this.children).filter((c) => c.localName === 'tbody'); const last = bodies[bodies.length - 1]; this.insertBefore(b, last ? last.nextSibling : null); return b; });
    method(T, 'deleteRow', function (i) { const rows = this.rows; if (i === -1) i = rows.length - 1; if (i < 0 || i >= rows.length) { if (i === -1) return; throw domError(`The index provided (${i}) is outside the range of rows.`, 'IndexSizeError'); } rows[i].remove(); });
  }
  const TS = g.HTMLTableSectionElement;
  if (TS) {
    acc(TS, 'rows', function () { const l = Array.from(this.children).filter((c) => c.localName === 'tr'); return Object.setPrototypeOf(l, g.HTMLCollection.prototype); });
    method(TS, 'deleteRow', function (i) { const rows = this.rows; if (i === -1) i = rows.length - 1; if (i < 0 || i >= rows.length) { if (i === -1) return; throw domError('Index out of range.', 'IndexSizeError'); } rows[i].remove(); });
  }
  const TR = g.HTMLTableRowElement;
  if (TR) {
    acc(TR, 'sectionRowIndex', function () { const p = this.parentNode; return p ? Array.from(p.children).filter((c) => c.localName === 'tr').indexOf(this) : -1; });
    method(TR, 'deleteCell', function (i) { const cells = Array.from(this.children).filter((c) => c.localName === 'td' || c.localName === 'th'); if (i === -1) i = cells.length - 1; if (i < 0 || i >= cells.length) { if (i === -1) return; throw domError('Index out of range.', 'IndexSizeError'); } cells[i].remove(); });
  }
  const TC = g.HTMLTableCellElement;
  if (TC) {
    acc(TC, 'cellIndex', function () { const p = this.parentNode; return p && p.localName === 'tr' ? Array.from(p.children).filter((c) => c.localName === 'td' || c.localName === 'th').indexOf(this) : -1; });
    acc(TC, 'scope', function () { const v = (this.getAttribute('scope') || '').toLowerCase(); return ['row', 'col', 'rowgroup', 'colgroup'].includes(v) ? v : ''; }, function (v) { this.setAttribute('scope', v); });
    acc(TC, 'abbr', function () { return this.getAttribute('abbr') || ''; }, function (v) { this.setAttribute('abbr', v); });
  }

  // ------------------------------------------------------- media elements

  class TimeRanges {
    constructor() { ILLEGAL(); }
    get length() { return this[S].length; }
    start(i) { if (i >= this[S].length) throw domError(`The index provided (${i}) is greater than or equal to the maximum bound (${this[S].length}).`, 'IndexSizeError'); return this[S][i][0]; }
    end(i) { if (i >= this[S].length) throw domError(`The index provided (${i}) is greater than or equal to the maximum bound (${this[S].length}).`, 'IndexSizeError'); return this[S][i][1]; }
  }
  tag(TimeRanges, 'TimeRanges');
  const timeRanges = (list) => { const t = Object.create(TimeRanges.prototype); hidden(t, S, list || []); return t; };
  class MediaError {
    constructor() { ILLEGAL(); }
    get code() { return this[S].code; }
    get message() { return this[S].message; }
  }
  const MEDIA_ERR = { MEDIA_ERR_ABORTED: 1, MEDIA_ERR_NETWORK: 2, MEDIA_ERR_DECODE: 3, MEDIA_ERR_SRC_NOT_SUPPORTED: 4 };
  for (const [k, v] of Object.entries(MEDIA_ERR)) { Object.defineProperty(MediaError, k, { value: v, enumerable: true }); Object.defineProperty(MediaError.prototype, k, { value: v, enumerable: true }); }
  tag(MediaError, 'MediaError');

  // Track lists (empty: no media pipeline).
  function trackList(C) { const l = Object.create(C.prototype); hidden(l, S, []); return l; }
  class TrackListBase extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get length() { return this[S].length; }
    getTrackById(id) { return this[S].find((t) => t.id === String(id)) || null; }
    [Symbol.iterator]() { return this[S][Symbol.iterator](); }
  }
  class AudioTrackList extends TrackListBase {}
  class VideoTrackList extends TrackListBase { get selectedIndex() { return this[S].findIndex((t) => t.selected); } }
  class TextTrackList extends TrackListBase {}
  for (const [C, n, ev] of [[AudioTrackList, 'AudioTrackList', ['change', 'addtrack', 'removetrack']], [VideoTrackList, 'VideoTrackList', ['change', 'addtrack', 'removetrack']], [TextTrackList, 'TextTrackList', ['change', 'addtrack', 'removetrack']]]) {
    tag(C, n);
    N.installHandlers(C.prototype, ev);
  }
  // Remove the intermediate base from the prototype chain the inventory sees.
  Object.setPrototypeOf(AudioTrackList.prototype, g.EventTarget.prototype);
  Object.setPrototypeOf(VideoTrackList.prototype, g.EventTarget.prototype);
  Object.setPrototypeOf(TextTrackList.prototype, g.EventTarget.prototype);
  for (const C of [AudioTrackList, VideoTrackList, TextTrackList]) {
    for (const k of ['length', 'getTrackById', Symbol.iterator]) Object.defineProperty(C.prototype, k, Object.getOwnPropertyDescriptor(TrackListBase.prototype, k));
  }
  class AudioTrack { constructor() { ILLEGAL(); } get id() { return this[S].id; } get kind() { return this[S].kind; } get label() { return this[S].label; } get language() { return this[S].language; } get enabled() { return this[S].enabled; } set enabled(v) { this[S].enabled = !!v; } get sourceBuffer() { return null; } }
  class VideoTrack { constructor() { ILLEGAL(); } get id() { return this[S].id; } get kind() { return this[S].kind; } get label() { return this[S].label; } get language() { return this[S].language; } get selected() { return this[S].selected; } set selected(v) { this[S].selected = !!v; } get sourceBuffer() { return null; } }
  tag(AudioTrack, 'AudioTrack'); tag(VideoTrack, 'VideoTrack');

  class TextTrackCueList {
    constructor() { ILLEGAL(); }
    get length() { return this[S].length; }
    getCueById(id) { return this[S].find((c) => c.id === String(id)) || null; }
    [Symbol.iterator]() { return this[S][Symbol.iterator](); }
  }
  tag(TextTrackCueList, 'TextTrackCueList');
  const cueList = (arr) => { const l = Object.create(TextTrackCueList.prototype); hidden(l, S, arr); return new Proxy(l, { get(t, k) { return typeof k === 'string' && /^\d+$/.test(k) ? arr[+k] : Reflect.get(t, k, t); } }); };
  class TextTrack extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get kind() { return this[S].kind; } get label() { return this[S].label; } get language() { return this[S].language; }
    get id() { return this[S].id; } get inBandMetadataTrackDispatchType() { return ''; }
    get mode() { return this[S].mode; } set mode(v) { if (['disabled', 'hidden', 'showing'].includes(v)) this[S].mode = v; }
    get cues() { return this[S].mode === 'disabled' ? null : cueList(this[S].cues); }
    get activeCues() { return this[S].mode === 'disabled' ? null : cueList([]); }
    addCue(c) { if (!this[S].cues.includes(c)) { this[S].cues.push(c); this[S].cues.sort((a, b) => a.startTime - b.startTime); c[S].track = this; } }
    removeCue(c) { const i = this[S].cues.indexOf(c); if (i < 0) throw domError('The specified cue is not listed in the TextTrack\'s list of cues.', 'NotFoundError'); this[S].cues.splice(i, 1); c[S].track = null; }
  }
  tag(TextTrack, 'TextTrack');
  N.installHandlers(TextTrack.prototype, ['cuechange']);
  function newTextTrack(kind, label, language) {
    const t = Object.create(TextTrack.prototype);
    hidden(t, S, { kind: kind || 'subtitles', label: label || '', language: language || '', id: '', mode: 'hidden', cues: [] });
    return t;
  }
  class TextTrackCue extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get track() { return this[S].track || null; }
    get id() { return this[S].id; } set id(v) { this[S].id = String(v); }
    get startTime() { return this[S].start; } set startTime(v) { this[S].start = +v; }
    get endTime() { return this[S].end; } set endTime(v) { this[S].end = +v; }
    get pauseOnExit() { return this[S].pause; } set pauseOnExit(v) { this[S].pause = !!v; }
  }
  tag(TextTrackCue, 'TextTrackCue');
  N.installHandlers(TextTrackCue.prototype, ['enter', 'exit']);
  class VTTCue extends TextTrackCue {
    constructor(start, end, text) {
      const c = Object.create(new.target.prototype);
      hidden(c, S, { id: '', start: +start, end: +end, pause: false, text: String(text), track: null, vertical: '', snapToLines: true, line: 'auto', position: 'auto', size: 100, align: 'center', region: null });
      return c;
    }
    get text() { return this[S].text; } set text(v) { this[S].text = String(v); }
    get vertical() { return this[S].vertical; } set vertical(v) { this[S].vertical = v; }
    get snapToLines() { return this[S].snapToLines; } set snapToLines(v) { this[S].snapToLines = !!v; }
    get line() { return this[S].line; } set line(v) { this[S].line = v; }
    get position() { return this[S].position; } set position(v) { this[S].position = v; }
    get size() { return this[S].size; } set size(v) { this[S].size = +v; }
    get align() { return this[S].align; } set align(v) { this[S].align = v; }
    get region() { return this[S].region; } set region(v) { this[S].region = v; }
    getCueAsHTML() { const f = document.createDocumentFragment(); f.textContent = this[S].text; return f; }
  }
  tag(VTTCue, 'VTTCue');

  const ME = g.HTMLMediaElement;
  if (ME) {
    const NET = { NETWORK_EMPTY: 0, NETWORK_IDLE: 1, NETWORK_LOADING: 2, NETWORK_NO_SOURCE: 3, HAVE_NOTHING: 0, HAVE_METADATA: 1, HAVE_CURRENT_DATA: 2, HAVE_FUTURE_DATA: 3, HAVE_ENOUGH_DATA: 4 };
    for (const [k, v] of Object.entries(NET)) { Object.defineProperty(ME, k, { value: v, enumerable: true }); Object.defineProperty(ME.prototype, k, { value: v, enumerable: true, configurable: true }); }
    const media = new WeakMap();
    const ms = (el) => { let m = media.get(el); if (!m) { m = { audio: trackList(AudioTrackList), video: trackList(VideoTrackList), text: trackList(TextTrackList), rate: 1, defaultRate: 1, srcObject: null, preservesPitch: true, time: 0, paused: true, volume: 1, muted: null }; media.set(el, m); } return m; };
    // Replace the inert data properties with spec-shaped accessors.
    for (const k of ['paused', 'ended', 'duration', 'currentTime', 'volume', 'playbackRate', 'readyState', 'networkState', 'seeking', 'videoWidth', 'videoHeight', 'error']) delete ME.prototype[k];
    acc(ME, 'paused', function () { return ms(this).paused; });
    acc(ME, 'ended', function () { return false; });
    acc(ME, 'duration', function () { return NaN; });
    acc(ME, 'currentTime', function () { return ms(this).time; }, function (v) { ms(this).time = +v || 0; });
    acc(ME, 'volume', function () { return ms(this).volume; }, function (v) { v = +v; if (!(v >= 0 && v <= 1)) throw domError(`The volume provided (${v}) is outside the range [0, 1].`, 'IndexSizeError'); ms(this).volume = v; this.dispatchEvent(new Event('volumechange')); });
    acc(ME, 'muted', function () { const m = ms(this); return m.muted === null ? this.hasAttribute('muted') : m.muted; }, function (v) { ms(this).muted = !!v; this.dispatchEvent(new Event('volumechange')); });
    acc(ME, 'playbackRate', function () { return ms(this).rate; }, function (v) { ms(this).rate = +v; this.dispatchEvent(new Event('ratechange')); });
    acc(ME, 'defaultPlaybackRate', function () { return ms(this).defaultRate; }, function (v) { ms(this).defaultRate = +v; });
    acc(ME, 'readyState', function () { return 0; });
    acc(ME, 'networkState', function () { return this.currentSrc ? 3 : 0; });
    acc(ME, 'seeking', function () { return false; });
    acc(ME, 'error', function () { return null; });
    acc(ME, 'preservesPitch', function () { return ms(this).preservesPitch; }, function (v) { ms(this).preservesPitch = !!v; });
    acc(ME, 'defaultMuted', function () { return this.hasAttribute('muted'); }, function (v) { this.toggleAttribute('muted', !!v); });
    acc(ME, 'srcObject', function () { return ms(this).srcObject; }, function (v) { ms(this).srcObject = v; });
    acc(ME, 'buffered', function () { return timeRanges([]); });
    acc(ME, 'played', function () { return timeRanges([]); });
    acc(ME, 'seekable', function () { return timeRanges([]); });
    acc(ME, 'audioTracks', function () { return ms(this).audio; });
    acc(ME, 'videoTracks', function () { return ms(this).video; });
    acc(ME, 'textTracks', function () { return ms(this).text; });
    acc(ME, 'loading', function () { const v = (this.getAttribute('loading') || '').toLowerCase(); return v === 'lazy' ? 'lazy' : 'eager'; }, function (v) { this.setAttribute('loading', v); });
    acc(ME, 'sinkId', function () { return ''; });
    acc(ME, 'mediaKeys', function () { return null; });
    acc(ME, 'remote', function () { return null; });
    acc(ME, 'disableRemotePlayback', function () { return this.hasAttribute('disableremoteplayback'); }, function (v) { this.toggleAttribute('disableremoteplayback', !!v); });
    method(ME, 'fastSeek', function (t) { this.currentTime = t; });
    method(ME, 'getStartDate', function () { return new Date(NaN); });
    method(ME, 'setSinkId', function () { return Promise.resolve(); });
    method(ME, 'setMediaKeys', function () { return Promise.resolve(); });
    method(ME, 'captureStream', function () { return null; }, true);
    // No audio/video output: playback is refused like an unsupported source.
    method(ME, 'play', function () {
      const el = this;
      return new Promise((resolve, reject) => {
        setTimeout(() => { el.dispatchEvent(new Event('error')); reject(domError('The element has no supported sources.', 'NotSupportedError')); }, 0);
      });
    }, true);
    method(ME, 'pause', function () { const m = ms(this); if (!m.paused) { m.paused = true; this.dispatchEvent(new Event('pause')); } }, true);
    method(ME, 'load', function () { ms(this).time = 0; this.dispatchEvent(new Event('emptied')); }, true);
    method(ME, 'canPlayType', function () { return ''; }, true);
    method(ME, 'addTextTrack', function (kind, label, language) { const t = newTextTrack(kind, label, language); ms(this).text[S].push(t); return t; }, true);
  }
  if (g.HTMLVideoElement) {
    const V = g.HTMLVideoElement;
    acc(V, 'videoWidth', function () { return 0; });
    acc(V, 'videoHeight', function () { return 0; });
    method(V, 'getVideoPlaybackQuality', function () { return { creationTime: performance.now(), droppedVideoFrames: 0, totalVideoFrames: 0, corruptedVideoFrames: 0 }; });
    method(V, 'requestVideoFrameCallback', function (cb) { return requestAnimationFrame(() => {}); });
    method(V, 'cancelVideoFrameCallback', function (id) { cancelAnimationFrame(id); });
    method(V, 'requestPictureInPicture', function () { return Promise.reject(domError('Picture-in-Picture is not available.', 'NotSupportedError')); });
    acc(V, 'disablePictureInPicture', function () { return this.hasAttribute('disablepictureinpicture'); }, function (v) { this.toggleAttribute('disablepictureinpicture', !!v); });
  }
  if (g.HTMLAudioElement) {
    const AudioCtor = function Audio(src) {
      const a = document.createElement('audio');
      a.setAttribute('preload', 'auto');
      if (src !== undefined) a.src = src;
      return a;
    };
    AudioCtor.prototype = g.HTMLAudioElement.prototype;
    define('Audio', AudioCtor);
  }
  if (g.HTMLTrackElement) {
    const TE = g.HTMLTrackElement;
    for (const [k, v] of Object.entries({ NONE: 0, LOADING: 1, LOADED: 2, ERROR: 3 })) { Object.defineProperty(TE, k, { value: v, enumerable: true }); Object.defineProperty(TE.prototype, k, { value: v, enumerable: true, configurable: true }); }
    const tracks = new WeakMap();
    acc(TE, 'readyState', function () { return 0; });
    acc(TE, 'track', function () { let t = tracks.get(this); if (!t) { t = newTextTrack(this.getAttribute('kind') || 'subtitles', this.getAttribute('label'), this.getAttribute('srclang')); tracks.set(this, t); } return t; });
  }

  // ------------------------------------------------------------ templates

  const TPL = g.HTMLTemplateElement;
  if (TPL) {
    acc(TPL, 'shadowRootMode', function () { const v = (this.getAttribute('shadowrootmode') || '').toLowerCase(); return v === 'open' || v === 'closed' ? v : ''; }, function (v) { this.setAttribute('shadowrootmode', v); });
    acc(TPL, 'shadowRootDelegatesFocus', function () { return this.hasAttribute('shadowrootdelegatesfocus'); }, function (v) { this.toggleAttribute('shadowrootdelegatesfocus', !!v); });
    acc(TPL, 'shadowRootClonable', function () { return this.hasAttribute('shadowrootclonable'); }, function (v) { this.toggleAttribute('shadowrootclonable', !!v); });
    acc(TPL, 'shadowRootSerializable', function () { return this.hasAttribute('shadowrootserializable'); }, function (v) { this.toggleAttribute('shadowrootserializable', !!v); });
    acc(TPL, 'shadowRootSlotAssignment', function () { return (this.getAttribute('shadowrootslotassignment') || '').toLowerCase() === 'manual' ? 'manual' : 'named'; }, function (v) { this.setAttribute('shadowrootslotassignment', v); });
    acc(TPL, 'shadowRootCustomElementRegistry', function () { return this.getAttribute('shadowrootcustomelementregistry'); }, function (v) { if (v === null) this.removeAttribute('shadowrootcustomelementregistry'); else this.setAttribute('shadowrootcustomelementregistry', v); });
    // Declarative shadow DOM: <template shadowrootmode> in the parsed
    // document becomes the parent's shadow root.
    const attachDeclarative = (root) => {
      for (const t of Array.from(root.querySelectorAll('template[shadowrootmode]'))) {
        const host = t.parentNode;
        const mode = t.shadowRootMode;
        if (!host || !mode || host.nodeType !== 1 || host.shadowRoot) continue;
        let sr;
        try { sr = host.attachShadow({ mode, delegatesFocus: t.shadowRootDelegatesFocus, clonable: t.shadowRootClonable, serializable: t.shadowRootSerializable, slotAssignment: t.shadowRootSlotAssignment }); } catch (e) { continue; }
        sr.appendChild(t.content);
        t.remove();
        attachDeclarative(sr);
      }
    };
    if (document.querySelector('template[shadowrootmode]')) attachDeclarative(document);
    document.addEventListener('DOMContentLoaded', () => attachDeclarative(document), { once: true });
  }
  if (g.HTMLScriptElement) {
    g.HTMLScriptElement.supports = function (type) { return ['classic', 'module', 'importmap', 'speculationrules'].includes(String(type)); };
    acc(g.HTMLScriptElement, 'htmlFor', function () { return this.getAttribute('for') || ''; }, function (v) { this.setAttribute('for', v); });
    acc(g.HTMLScriptElement, 'event', function () { return this.getAttribute('event') || ''; }, function (v) { this.setAttribute('event', v); });
  }
  for (const C of [g.HTMLScriptElement, g.HTMLLinkElement, g.HTMLStyleElement].filter(Boolean)) {
    acc(C, 'blocking', function () { let m = tokenLists.get(this); if (!m) { m = {}; tokenLists.set(this, m); } return m.blocking || (m.blocking = makeTokenList(this, 'blocking')); }, function (v) { this.setAttribute('blocking', v); });
  }
  if (g.HTMLCanvasElement) method(g.HTMLCanvasElement, 'transferControlToOffscreen', function () { return new g.OffscreenCanvas(this.width, this.height); });
  if (g.HTMLImageElement) {
    // decode(): settles with the element's load / error.
    method(g.HTMLImageElement, 'decode', function decode() {
      const img = this;
      if (img.complete) return img.naturalWidth || !img.getAttribute('src') ? Promise.resolve() : Promise.reject(domError('The source image cannot be decoded.', 'EncodingError'));
      return new Promise((resolve, reject) => {
        img.addEventListener('load', () => resolve(), { once: true });
        img.addEventListener('error', () => reject(domError('The source image cannot be decoded.', 'EncodingError')), { once: true });
      });
    }, true);
    acc(g.HTMLImageElement, 'x', function () { return Math.round(this.getBoundingClientRect().left + scrollX); });
    acc(g.HTMLImageElement, 'y', function () { return Math.round(this.getBoundingClientRect().top + scrollY); });
  }
  // HTMLHyperlinkElementUtils: the components of the element's URL (null
  // without an href or when it does not parse), and setters that run the
  // URL setter and write the result back to the href attribute.
  const hyperlinkURL = (el) => {
    const h = el.getAttribute('href');
    if (h === null) return null;
    try { return new URL(h, document.baseURI); } catch (e) { return null; }
  };
  for (const C of [g.HTMLAnchorElement, g.HTMLAreaElement]) {
    if (!C) continue;
    for (const k of ['protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin', 'username', 'password']) {
      acc(C, k, function () {
        const u = hyperlinkURL(this);
        if (!u) return k === 'protocol' ? ':' : '';
        return u[k];
      }, k === 'origin' ? undefined : function (v) {
        const u = hyperlinkURL(this);
        if (!u) return;
        u[k] = v;
        this.setAttribute('href', u.href);
      });
    }
  }
  if (g.HTMLAreaElement) {
    acc(g.HTMLAreaElement, 'relList', function () { let m = tokenLists.get(this); if (!m) { m = {}; tokenLists.set(this, m); } return m.rel || (m.rel = makeTokenList(this, 'rel')); });
    method(g.HTMLAreaElement, 'toString', function () { return this.href; });
  }
  if (g.HTMLMapElement) acc(g.HTMLMapElement, 'areas', function () { return Object.setPrototypeOf(Array.from(this.querySelectorAll('area')), g.HTMLCollection.prototype); });
  if (g.HTMLDataListElement) acc(g.HTMLDataListElement, 'options', function () { return Object.setPrototypeOf(Array.from(this.querySelectorAll('option')), g.HTMLCollection.prototype); });
  if (g.HTMLOptGroupElement) acc(g.HTMLOptGroupElement, 'label', function () { return this.getAttribute('label') || ''; }, function (v) { this.setAttribute('label', v); });
  if (g.HTMLLabelElement) acc(g.HTMLLabelElement, 'form', function () { const c = this.control; return c ? c.form || null : null; });
  if (g.HTMLLegendElement) acc(g.HTMLLegendElement, 'form', function () { const f = this.parentNode; return f && f.localName === 'fieldset' ? f.form : null; });
  if (g.HTMLSelectedContentElement) void 0;
  if (g.HTMLDialogElement) {
    acc(g.HTMLDialogElement, 'returnValue', function () { return this[SRV] || ''; }, function (v) { this[SRV] = String(v); });
    acc(g.HTMLDialogElement, 'closedBy', function () { const v = (this.getAttribute('closedby') || '').toLowerCase(); return ['any', 'closerequest', 'none'].includes(v) ? v : 'auto'; }, function (v) { this.setAttribute('closedby', v); });
    method(g.HTMLDialogElement, 'requestClose', function (rv) { if (!this.open) return; if (this.dispatchEvent(new Event('cancel', { cancelable: true }))) this.close(rv); });
    const close = g.HTMLDialogElement.prototype.close;
    method(g.HTMLDialogElement, 'close', function (rv) { if (rv !== undefined) this.returnValue = rv; if (this.open) close.call(this); }, true);
  }
  if (g.HTMLDetailsElement) acc(g.HTMLDetailsElement, 'name', function () { return this.getAttribute('name') || ''; }, function (v) { this.setAttribute('name', v); });

  // ---------------------------------------------------- HTMLElement misc

  // Popover API (state only; top-layer rendering not available).
  for (const [name, fn] of [
    ['showPopover', function () { if (!this.hasAttribute('popover')) throw domError("Failed to execute 'showPopover' on 'HTMLElement': Not supported on elements that do not have a valid value for the 'popover' attribute.", 'NotSupportedError'); if (!this[SPOP]) { this[SPOP] = true; this.dispatchEvent(new ToggleEventCtor('toggle', { oldState: 'closed', newState: 'open' })); } }],
    ['hidePopover', function () { if (this[SPOP]) { this[SPOP] = false; this.dispatchEvent(new ToggleEventCtor('toggle', { oldState: 'open', newState: 'closed' })); } }],
    ['togglePopover', function (force) { const open = typeof force === 'boolean' ? force : typeof force === 'object' && force && typeof force.force === 'boolean' ? force.force : !this[SPOP]; if (open) this.showPopover(); else this.hidePopover(); return open; }],
  ]) method(HE, name, fn, true);
  const ToggleEventCtor = g.ToggleEvent || g.Event;

  // -------------------------------------------------------- DOMStringMap

  class DOMStringMap { constructor() { ILLEGAL(); } }
  tag(DOMStringMap, 'DOMStringMap');
  define('DOMStringMap', DOMStringMap);
  const datasetDesc = Object.getOwnPropertyDescriptor(HE.prototype, 'dataset');
  if (datasetDesc && datasetDesc.get) {
    for (const C of [HE, g.SVGElement, g.MathMLElement].filter(Boolean)) {
      Object.defineProperty(C.prototype, 'dataset', {
        get() { const d = datasetDesc.get.call(this); try { if (Object.getPrototypeOf(d) !== DOMStringMap.prototype) Object.setPrototypeOf(d, DOMStringMap.prototype); } catch (e) {} return d; },
        configurable: true, enumerable: true,
      });
    }
  }

  // ------------------------------------------------------ DOMStringList

  class DOMStringList {
    constructor() { ILLEGAL(); }
    get length() { return this[S].length; }
    item(i) { return this[S][i] ?? null; }
    contains(s) { return this[S].includes(String(s)); }
    [Symbol.iterator]() { return this[S][Symbol.iterator](); }
  }
  tag(DOMStringList, 'DOMStringList');
  define('DOMStringList', DOMStringList);
  hidden(g, '__rhDOMStringList', (arr) => { const l = Object.create(DOMStringList.prototype); hidden(l, S, arr.slice()); arr.forEach((v, i) => Object.defineProperty(l, i, { value: v, enumerable: true })); return l; });

  // ------------------------------------------------------------ exports

  define('ElementInternals', ElementInternals);
  define('CustomStateSet', CustomStateSet);
  define('TimeRanges', TimeRanges);
  define('MediaError', MediaError);
  define('AudioTrackList', AudioTrackList);
  define('VideoTrackList', VideoTrackList);
  define('TextTrackList', TextTrackList);
  define('AudioTrack', AudioTrack);
  define('VideoTrack', VideoTrack);
  define('TextTrack', TextTrack);
  define('TextTrackCue', TextTrackCue);
  define('TextTrackCueList', TextTrackCueList);
  define('VTTCue', VTTCue);
  hidden(g, '__rhHtml', { makeTokenList, sanitize, timeRanges });
})();
