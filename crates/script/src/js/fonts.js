// CSS Font Loading: FontFace (URL or binary sources, load(), status,
// descriptors), FontFaceSet (document.fonts: setlike, load(), check(),
// ready, loading events), FontFaceSetLoadEvent, FontFaceFeatures /
// FontFaceVariations / FontFaceVariationAxis / FontFacePalettes /
// FontFacePalette. Faces added to document.fonts become @font-face rules
// for the style engine.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('fonts');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const make = (C, st) => { const o = Object.create(C.prototype); hidden(o, S, st); return o; };

  const DESCRIPTORS = { style: 'normal', weight: 'normal', stretch: 'normal', width: 'normal', variant: 'normal', variations: 'normal', unicodeRange: 'U+0-10FFFF', featureSettings: 'normal', variationSettings: 'normal', display: 'auto', ascentOverride: 'normal', descentOverride: 'normal', lineGapOverride: 'normal', sizeAdjust: '100%' };
  const b64 = (bytes) => { let s = ''; for (let i = 0; i < bytes.length; i += 8192) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 8192)); return g.btoa(s); };
  class FontFaceFeatures { constructor() { ILLEGAL(); } }
  class FontFaceVariationAxis { constructor() { ILLEGAL(); } get name() { return this[S].name; } get axisTag() { return this[S].tag; } get minimumValue() { return this[S].min; } get maximumValue() { return this[S].max; } get defaultValue() { return this[S].def; } }
  class FontFaceVariations { constructor() { ILLEGAL(); } get size() { return 0; } has() { return false; } forEach() {} entries() { return [][Symbol.iterator](); } keys() { return [][Symbol.iterator](); } values() { return [][Symbol.iterator](); } [Symbol.iterator]() { return [][Symbol.iterator](); } }
  // Iterable (value iterator) collections: no palettes from the font backend.
  const ITER = { entries() { return [][Symbol.iterator](); }, keys() { return [][Symbol.iterator](); }, values() { return [][Symbol.iterator](); }, forEach() {} };
  class FontFacePalette { constructor() { ILLEGAL(); } get length() { return 0; } get usableWithLightBackground() { return true; } get usableWithDarkBackground() { return true; } [Symbol.iterator]() { return [][Symbol.iterator](); } }
  class FontFacePalettes { constructor() { ILLEGAL(); } get length() { return 0; } [Symbol.iterator]() { return [][Symbol.iterator](); } }
  for (const C of [FontFacePalette, FontFacePalettes]) for (const [k, f] of Object.entries(ITER)) Object.defineProperty(C.prototype, k, { value: f, writable: true, configurable: true });
  for (const [C, n] of [[FontFaceFeatures, 'FontFaceFeatures'], [FontFaceVariationAxis, 'FontFaceVariationAxis'], [FontFaceVariations, 'FontFaceVariations'], [FontFacePalette, 'FontFacePalette'], [FontFacePalettes, 'FontFacePalettes']]) { tag(C, n); define(n, C); }

  class FontFace {
    constructor(family, source, descriptors = {}) {
      const f = Object.create(new.target ? new.target.prototype : FontFace.prototype);
      const st = Object.assign({ family: String(family), status: 'unloaded', source: null, binary: null, cssUrl: null }, DESCRIPTORS);
      for (const k of Object.keys(DESCRIPTORS)) if (descriptors[k] !== undefined) st[k] = String(descriptors[k]);
      hidden(f, S, st);
      st.loaded = new Promise((res, rej) => { st.resolve = res; st.reject = rej; });
      st.loaded.catch(() => {});
      if (typeof source === 'string') {
        st.source = source;
        const m = /url\(\s*(['"]?)([^'")]+)\1\s*\)/.exec(source);
        if (!m) { st.status = 'error'; st.reject(domError(`Failed to construct 'FontFace': The source provided ('${source}') could not be parsed as a value list.`, 'SyntaxError')); }
        else st.cssUrl = new URL(m[2], document.baseURI).href;
      } else if (source instanceof ArrayBuffer || ArrayBuffer.isView(source)) {
        const bytes = source instanceof ArrayBuffer ? new Uint8Array(source) : new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
        st.binary = bytes.slice();
        st.cssUrl = 'data:font/ttf;base64,' + b64(st.binary);
        st.status = 'loading';
        Promise.resolve().then(() => { st.status = 'loaded'; st.resolve(f); });
      } else throw new TypeError("Failed to construct 'FontFace': The provided value is not of type '(ArrayBuffer or ArrayBufferView or CSSOMString)'.");
      return f;
    }
    get family() { return this[S].family; }
    set family(v) { this[S].family = String(v); syncFaces(); }
    get status() { return this[S].status; }
    get loaded() { return this[S].loaded; }
    get features() { return make(FontFaceFeatures, {}); }
    get variationAxes() { return Object.freeze([]); }
    get palettes() { return make(FontFacePalettes, {}); }
    load() {
      const st = this[S];
      if (st.status !== 'unloaded') return st.loaded;
      st.status = 'loading';
      fontSet && fontSet[S].faces.has(this) && setLoading();
      g.fetch(st.cssUrl).then((r) => { if (!r.ok) throw new Error(String(r.status)); return r.arrayBuffer(); }).then(() => {
        st.status = 'loaded';
        st.resolve(this);
        loadedOne(this, true);
      }, () => {
        st.status = 'error';
        st.reject(domError('A network error occurred.', 'NetworkError'));
        loadedOne(this, false);
      });
      return st.loaded;
    }
  }
  for (const k of Object.keys(DESCRIPTORS)) Object.defineProperty(FontFace.prototype, k, { get() { return this[S][k]; }, set(v) { this[S][k] = String(v); syncFaces(); }, configurable: true, enumerable: true });
  tag(FontFace, 'FontFace');

  class FontFaceSetLoadEvent extends g.Event {
    constructor(type, init = {}) { super(type, init); hidden(this, S, { fontfaces: Object.freeze(Array.from(init.fontfaces || [])) }); }
    get fontfaces() { return this[S].fontfaces; }
  }
  tag(FontFaceSetLoadEvent, 'FontFaceSetLoadEvent');

  // ----------------------------------------------------------- FontFaceSet
  class FontFaceSet extends g.EventTarget {
    constructor(initialFaces = []) { super(); hidden(this, S, { faces: new Set(initialFaces), status: 'loaded', loading: new Set(), loadedFaces: [], failedFaces: [], ready: Promise.resolve(this) }); }
    get size() { return this[S].faces.size; }
    get status() { return this[S].status; }
    get ready() { return this[S].ready; }
    add(f) { if (!(f instanceof FontFace)) throw new TypeError("Failed to execute 'add' on 'FontFaceSet': parameter 1 is not of type 'FontFace'."); this[S].faces.add(f); if (f[S].status === 'loading') setLoading(); syncFaces(); return this; }
    // CSS-connected faces belong to their @font-face rules (CSS Font
    // Loading §2.2): delete() and clear() leave them in place.
    delete(f) { if (f instanceof FontFace && f[S].css) return false; const r = this[S].faces.delete(f); syncFaces(); return r; }
    clear() { for (const f of Array.from(this[S].faces)) if (!f[S].css) this[S].faces.delete(f); syncFaces(); }
    has(f) { return this[S].faces.has(f); }
    forEach(cb, t) { for (const f of this[S].faces) cb.call(t, f, f, this); }
    entries() { return Array.from(this[S].faces, (f) => [f, f])[Symbol.iterator](); }
    keys() { return this[S].faces.values(); }
    values() { return this[S].faces.values(); }
    [Symbol.iterator]() { return this[S].faces.values(); }
    load(font, text = ' ') {
      const families = parseFontFamilies(font);
      if (!families) return Promise.reject(new SyntaxError(`Failed to execute 'load' on 'FontFaceSet': Could not resolve '${font}' as a font.`));
      const matches = matching(this, families, text);
      return Promise.all(matches.map((f) => f.load())).then(() => matches);
    }
    check(font, text = ' ') {
      const families = parseFontFamilies(font);
      if (!families) throw new SyntaxError(`Failed to execute 'check' on 'FontFaceSet': Could not resolve '${font}' as a font.`);
      return matching(this, families, text).every((f) => f.status === 'loaded');
    }
  }
  tag(FontFaceSet, 'FontFaceSet');
  N.installHandlers(FontFaceSet.prototype, ['loading', 'loadingdone', 'loadingerror']);
  // Faces of the listed families whose unicode-range includes a character
  // of `text` (CSS Font Loading §2.5.1 "find the matching font faces").
  function matching(set, families, text) {
    const cps = Array.from(String(text), (c) => c.codePointAt(0));
    return Array.from(set[S].faces).filter((f) => {
      if (!families.includes(f.family.replace(/^['"]|['"]$/g, '').toLowerCase())) return false;
      const ranges = parseRanges(f.unicodeRange);
      return !ranges || cps.some((c) => ranges.some(([lo, hi]) => c >= lo && c <= hi));
    });
  }
  function parseRanges(text) {
    const out = [];
    for (const part of String(text).split(',')) {
      const m = /^\s*u\+([0-9a-f?]{1,6})(?:-([0-9a-f]{1,6}))?\s*$/i.exec(part);
      if (!m) return null;
      const lo = parseInt(m[1].replace(/\?/g, '0'), 16);
      const hi = m[2] ? parseInt(m[2], 16) : parseInt(m[1].replace(/\?/g, 'f'), 16);
      out.push([lo, hi]);
    }
    return out;
  }
  // The document's @font-face rules as CSS-connected faces, listed first.
  let cssAdded = false;
  function addCssFaces() {
    if (cssAdded || !fontSet || !N.cssFontFaces) return;
    cssAdded = true;
    const st = fontSet[S];
    const own = Array.from(st.faces);
    st.faces.clear();
    for (const d of N.cssFontFaces()) {
      const f = Object.create(FontFace.prototype);
      const fs = Object.assign({}, DESCRIPTORS, { family: d.family, style: d.style, weight: d.weight, stretch: d.stretch, width: d.stretch, unicodeRange: d.unicodeRange, display: d.display, status: d.status, source: null, binary: null, cssUrl: d.url || null, css: true });
      hidden(f, S, fs);
      fs.loaded = new Promise((res, rej) => { fs.resolve = res; fs.reject = rej; });
      fs.loaded.catch(() => {});
      if (d.status === 'loaded') fs.resolve(f);
      st.faces.add(f);
    }
    for (const f of own) st.faces.add(f);
  }
  function parseFontFamilies(font) {
    const m = /(?:^|\s)(\d+(?:\.\d+)?(?:px|pt|em|rem|%|vw|vh)|xx-small|x-small|small|medium|large|x-large|xx-large|smaller|larger)(?:\/[^\s]+)?\s+(.+)$/i.exec(String(font).trim());
    if (!m) return null;
    return m[2].split(',').map((s) => s.trim().replace(/^['"]|['"]$/g, '').toLowerCase());
  }
  let fontSet = null;
  function setLoading() {
    const st = fontSet[S];
    if (st.status === 'loading') return;
    st.status = 'loading';
    st.ready = new Promise((res) => { st.readyResolve = res; });
    setTimeout(() => fontSet.dispatchEvent(new FontFaceSetLoadEvent('loading', { fontfaces: Array.from(st.faces).filter((f) => f.status === 'loading') })), 0);
  }
  function loadedOne(face, ok) {
    if (!fontSet) return;
    const st = fontSet[S];
    if (!st.faces.has(face)) return;
    (ok ? st.loadedFaces : st.failedFaces).push(face);
    if (Array.from(st.faces).some((f) => f.status === 'loading')) return;
    st.status = 'loaded';
    const loaded = st.loadedFaces.splice(0), failed = st.failedFaces.splice(0);
    setTimeout(() => {
      fontSet.dispatchEvent(new FontFaceSetLoadEvent('loadingdone', { fontfaces: loaded }));
      if (failed.length) fontSet.dispatchEvent(new FontFaceSetLoadEvent('loadingerror', { fontfaces: failed }));
      if (st.readyResolve) { st.readyResolve(fontSet); st.readyResolve = null; }
    }, 0);
  }
  // Script-added faces as @font-face rules for the style engine.
  function syncFaces() {
    if (!fontSet) return;
    const rules = [];
    for (const f of fontSet[S].faces) {
      const st = f[S];
      if (!st.cssUrl || st.css) continue;
      rules.push(`@font-face { font-family: ${JSON.stringify(st.family)}; src: url(${JSON.stringify(st.cssUrl)}); font-style: ${st.style}; font-weight: ${st.weight}; font-display: ${st.display}; }`);
    }
    N.setScriptFontFaces(rules.join('\n'));
  }
  fontSet = Object.create(FontFaceSet.prototype);
  hidden(fontSet, S, { faces: new Set(), status: 'loaded', loading: new Set(), loadedFaces: [], failedFaces: [], ready: Promise.resolve(null) });
  fontSet[S].ready = Promise.resolve(fontSet);
  Object.defineProperty(g.Document.prototype, 'fonts', { get() { addCssFaces(); return fontSet; }, configurable: true, enumerable: true });
  define('FontFace', FontFace);
  define('FontFaceSet', FontFaceSet);
  define('FontFaceSetLoadEvent', FontFaceSetLoadEvent);
})();
