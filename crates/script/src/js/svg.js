// SVG 2 DOM: the basic data types (SVGLength, SVGAngle, SVGNumber,
// SVGPreserveAspectRatio, SVGTransform and their lists), the animated
// attribute wrappers (live views of the element's attributes), the element
// IDL attributes, geometry (getBBox, getTotalLength, getPointAtLength,
// isPointInFill / isPointInStroke), coordinate systems (getCTM,
// getScreenCTM, viewBox / preserveAspectRatio), text content metrics and
// SVGSVGElement helpers. animVal always equals baseVal (no SMIL).
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const acc = (C, k, get, set) => Object.defineProperty(C.prototype, k, { get, set, configurable: true, enumerable: true });
  const consts = (C, map) => { for (const [k, v] of Object.entries(map)) { Object.defineProperty(C, k, { value: v, enumerable: true }); Object.defineProperty(C.prototype, k, { value: v, enumerable: true }); } };
  const S = Symbol('svg');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const make = (C, st) => { const o = Object.create(C.prototype); hidden(o, S, st); return o; };
  const roError = () => domError('The object is read-only.', 'NoModificationAllowedError');
  const NUM = '[+-]?(?:\\d+\\.?\\d*|\\.\\d+)(?:[eE][+-]?\\d+)?';
  const numbers = (s) => (String(s || '').match(new RegExp(NUM, 'g')) || []).map(Number);
  const fmt = (n) => String(Math.round(n * 1e6) / 1e6);
  const isSvg = (n) => n instanceof g.SVGElement;
  const isOuterSvg = (n) => n instanceof g.SVGSVGElement && !isSvg(n.parentNode);

  // Per-element caches so `rect.x === rect.x`.
  const caches = new WeakMap();
  function cached(el, key, create) {
    let m = caches.get(el);
    if (!m) { m = new Map(); caches.set(el, m); }
    let v = m.get(key);
    if (!v) { v = create(); m.set(key, v); }
    return v;
  }

  // ---------------------------------------------------------------- SVGLength
  const LEN_UNITS = ['', '', '%', 'em', 'ex', 'px', 'cm', 'mm', 'in', 'pt', 'pc'];
  const LEN_RE = new RegExp(`^\\s*(${NUM})(px|cm|mm|in|pt|pc|em|ex|%)?\\s*$`, 'i');
  function parseLength(s) {
    if (s === null || s === undefined) return null;
    const m = LEN_RE.exec(String(s));
    if (!m) return null;
    const u = (m[2] || '').toLowerCase();
    return { unit: u ? LEN_UNITS.indexOf(u) : 1, num: +m[1] };
  }
  const serLength = (unit, num) => fmt(num) + (LEN_UNITS[unit] || '');
  function fontSize(el) { try { return parseFloat(g.getComputedStyle(el).fontSize) || 16; } catch (e) { return 16; } }
  // Size of the viewport an element's percentages resolve against.
  function viewportSize(el) {
    let vp = el && el.parentNode;
    while (vp && !(vp instanceof g.SVGSVGElement)) vp = vp.parentNode;
    if (!vp) return [0, 0];
    const vb = parseViewBox(vp.getAttribute('viewBox'));
    if (vb) return [vb[2], vb[3]];
    return svgViewportSize(vp);
  }
  function unitFactor(unit, el, dir) {
    switch (unit) {
      case 1: case 5: return 1;
      case 6: return 96 / 2.54;
      case 7: return 96 / 25.4;
      case 8: return 96;
      case 9: return 4 / 3;
      case 10: return 16;
      case 3: return el ? fontSize(el) : 16;
      case 4: return (el ? fontSize(el) : 16) / 2;
      case 2: {
        if (!el) return 0;
        const [w, h] = viewportSize(el);
        return (dir === 'x' ? w : dir === 'y' ? h : Math.sqrt((w * w + h * h) / 2)) / 100;
      }
      default: return 0;
    }
  }
  // State: attached { el, attr, dir, ro, def } | in a list / detached { unit, num, owner, ro }.
  function lenRead(l) {
    const st = l[S];
    if (st.attr) return parseLength(st.el.getAttribute(st.attr)) || st.def || { unit: 1, num: 0 };
    return st;
  }
  function lenWrite(l, unit, num) {
    const st = l[S];
    if (st.ro) throw roError();
    if (st.attr) st.el.setAttribute(st.attr, serLength(unit, num));
    else { st.unit = unit; st.num = num; if (st.owner) st.owner.commit(); }
  }
  const ctxEl = (l) => l[S].el || (l[S].owner && l[S].owner.el) || null;
  class SVGLength {
    constructor() { ILLEGAL(); }
    get unitType() { return lenRead(this).unit; }
    get value() { const r = lenRead(this); return r.num * unitFactor(r.unit, ctxEl(this), this[S].dir); }
    set value(v) {
      v = +v;
      if (!Number.isFinite(v)) throw new TypeError("Failed to set the 'value' property on 'SVGLength': The provided float value is non-finite.");
      const r = lenRead(this);
      const f = unitFactor(r.unit, ctxEl(this), this[S].dir);
      if (f) lenWrite(this, r.unit, v / f); else lenWrite(this, 1, v);
    }
    get valueInSpecifiedUnits() { return lenRead(this).num; }
    set valueInSpecifiedUnits(v) { v = +v; if (!Number.isFinite(v)) throw new TypeError("Failed to set the 'valueInSpecifiedUnits' property on 'SVGLength': The provided float value is non-finite."); lenWrite(this, lenRead(this).unit, v); }
    get valueAsString() { const r = lenRead(this); return serLength(r.unit, r.num); }
    set valueAsString(s) {
      const r = parseLength(s);
      if (!r) throw domError(`Failed to set the 'valueAsString' property on 'SVGLength': The value provided ('${s}') is invalid.`, 'SyntaxError');
      lenWrite(this, r.unit, r.num);
    }
    newValueSpecifiedUnits(unit, v) {
      unit = unit >>> 0;
      if (unit < 1 || unit > 10) throw domError("Failed to execute 'newValueSpecifiedUnits' on 'SVGLength': Cannot set value with unknown or invalid units (" + unit + ').', 'NotSupportedError');
      lenWrite(this, unit, +v);
    }
    convertToSpecifiedUnits(unit) {
      unit = unit >>> 0;
      if (unit < 1 || unit > 10) throw domError("Failed to execute 'convertToSpecifiedUnits' on 'SVGLength': Cannot convert to unknown or invalid units (" + unit + ').', 'NotSupportedError');
      const f = unitFactor(unit, ctxEl(this), this[S].dir);
      if (!f) throw domError("Failed to execute 'convertToSpecifiedUnits' on 'SVGLength': Could not resolve relative length.", 'NotSupportedError');
      lenWrite(this, unit, this.value / f);
    }
  }
  consts(SVGLength, { SVG_LENGTHTYPE_UNKNOWN: 0, SVG_LENGTHTYPE_NUMBER: 1, SVG_LENGTHTYPE_PERCENTAGE: 2, SVG_LENGTHTYPE_EMS: 3, SVG_LENGTHTYPE_EXS: 4, SVG_LENGTHTYPE_PX: 5, SVG_LENGTHTYPE_CM: 6, SVG_LENGTHTYPE_MM: 7, SVG_LENGTHTYPE_IN: 8, SVG_LENGTHTYPE_PT: 9, SVG_LENGTHTYPE_PC: 10 });
  tag(SVGLength, 'SVGLength');

  // ----------------------------------------------------------------- SVGAngle
  const ANG_UNITS = ['', '', 'deg', 'rad', 'grad'];
  const ANG_RE = new RegExp(`^\\s*(${NUM})(deg|rad|grad|turn)?\\s*$`, 'i');
  function parseAngle(s) {
    if (s === null || s === undefined) return null;
    const m = ANG_RE.exec(String(s));
    if (!m) return null;
    const u = (m[2] || '').toLowerCase();
    if (u === 'turn') return { unit: 2, num: +m[1] * 360 };
    return { unit: u ? ANG_UNITS.indexOf(u) : 1, num: +m[1] };
  }
  const angFactor = (unit) => (unit === 3 ? 180 / Math.PI : unit === 4 ? 0.9 : 1);
  function angRead(a) { const st = a[S]; if (st.attr) return parseAngle(st.el.getAttribute(st.attr)) || { unit: 1, num: 0 }; return st; }
  function angWrite(a, unit, num) {
    const st = a[S];
    if (st.ro) throw roError();
    if (st.attr) st.el.setAttribute(st.attr, fmt(num) + ANG_UNITS[unit]);
    else { st.unit = unit; st.num = num; }
  }
  class SVGAngle {
    constructor() { ILLEGAL(); }
    get unitType() { return angRead(this).unit; }
    get value() { const r = angRead(this); return r.num * angFactor(r.unit); }
    set value(v) { const r = angRead(this); angWrite(this, r.unit, +v / angFactor(r.unit)); }
    get valueInSpecifiedUnits() { return angRead(this).num; }
    set valueInSpecifiedUnits(v) { angWrite(this, angRead(this).unit, +v); }
    get valueAsString() { const r = angRead(this); return fmt(r.num) + ANG_UNITS[r.unit]; }
    set valueAsString(s) { const r = parseAngle(s); if (!r) throw domError(`Failed to set the 'valueAsString' property on 'SVGAngle': The value provided ('${s}') is invalid.`, 'SyntaxError'); angWrite(this, r.unit, r.num); }
    newValueSpecifiedUnits(unit, v) { unit = unit >>> 0; if (unit < 1 || unit > 4) throw domError("Failed to execute 'newValueSpecifiedUnits' on 'SVGAngle': Cannot set value with unknown or invalid units (" + unit + ').', 'NotSupportedError'); angWrite(this, unit, +v); }
    convertToSpecifiedUnits(unit) { unit = unit >>> 0; if (unit < 1 || unit > 4) throw domError("Failed to execute 'convertToSpecifiedUnits' on 'SVGAngle': Cannot convert to unknown or invalid units (" + unit + ').', 'NotSupportedError'); angWrite(this, unit, this.value / angFactor(unit)); }
  }
  consts(SVGAngle, { SVG_ANGLETYPE_UNKNOWN: 0, SVG_ANGLETYPE_UNSPECIFIED: 1, SVG_ANGLETYPE_DEG: 2, SVG_ANGLETYPE_RAD: 3, SVG_ANGLETYPE_GRAD: 4 });
  tag(SVGAngle, 'SVGAngle');

  class SVGNumber {
    constructor() { ILLEGAL(); }
    get value() { return this[S].num; }
    set value(v) { const st = this[S]; if (st.ro) throw roError(); st.num = +v; if (st.owner) st.owner.commit(); }
  }
  tag(SVGNumber, 'SVGNumber');

  // ---------------------------------------------------- SVGPreserveAspectRatio
  const ALIGNS = ['', 'none', 'xMinYMin', 'xMidYMin', 'xMaxYMin', 'xMinYMid', 'xMidYMid', 'xMaxYMid', 'xMinYMax', 'xMidYMax', 'xMaxYMax'];
  function parsePAR(s) {
    const parts = String(s || '').trim().split(/\s+/);
    const align = ALIGNS.indexOf(parts[0]);
    if (align < 1) return { align: 6, meet: 1 };
    return { align, meet: parts[1] === 'slice' ? 2 : 1 };
  }
  function parRead(p) { const st = p[S]; return st.attr ? parsePAR(st.el.getAttribute(st.attr)) : st; }
  function parWrite(p, align, meet) {
    const st = p[S];
    if (st.ro) throw roError();
    if (st.attr) st.el.setAttribute(st.attr, ALIGNS[align] + (align !== 1 && meet === 2 ? ' slice' : ''));
    else { st.align = align; st.meet = meet; }
  }
  class SVGPreserveAspectRatio {
    constructor() { ILLEGAL(); }
    get align() { return parRead(this).align; }
    set align(v) { v = v & 0xffff; if (v < 1 || v > 10) throw new TypeError('The alignment provided is invalid.'); parWrite(this, v, parRead(this).meet); }
    get meetOrSlice() { return parRead(this).meet; }
    set meetOrSlice(v) { v = v & 0xffff; if (v < 1 || v > 2) throw new TypeError('The meetOrSlice provided is invalid.'); parWrite(this, parRead(this).align, v); }
  }
  consts(SVGPreserveAspectRatio, { SVG_PRESERVEASPECTRATIO_UNKNOWN: 0, SVG_PRESERVEASPECTRATIO_NONE: 1, SVG_PRESERVEASPECTRATIO_XMINYMIN: 2, SVG_PRESERVEASPECTRATIO_XMIDYMIN: 3, SVG_PRESERVEASPECTRATIO_XMAXYMIN: 4, SVG_PRESERVEASPECTRATIO_XMINYMID: 5, SVG_PRESERVEASPECTRATIO_XMIDYMID: 6, SVG_PRESERVEASPECTRATIO_XMAXYMID: 7, SVG_PRESERVEASPECTRATIO_XMINYMAX: 8, SVG_PRESERVEASPECTRATIO_XMIDYMAX: 9, SVG_PRESERVEASPECTRATIO_XMAXYMAX: 10, SVG_MEETORSLICE_UNKNOWN: 0, SVG_MEETORSLICE_MEET: 1, SVG_MEETORSLICE_SLICE: 2 });
  tag(SVGPreserveAspectRatio, 'SVGPreserveAspectRatio');

  // ------------------------------------------------------------- SVGTransform
  const T = { UNKNOWN: 0, MATRIX: 1, TRANSLATE: 2, SCALE: 3, ROTATE: 4, SKEWX: 5, SKEWY: 6 };
  const rad = (d) => (d * Math.PI) / 180;
  function transformOf(type, a) {
    // a: the function's arguments; returns { type, m: DOMMatrix, angle, args }.
    let m;
    switch (type) {
      case T.MATRIX: m = new g.DOMMatrix([a[0], a[1], a[2], a[3], a[4], a[5]]); break;
      case T.TRANSLATE: m = new g.DOMMatrix([1, 0, 0, 1, a[0] || 0, a[1] || 0]); break;
      case T.SCALE: m = new g.DOMMatrix([a[0], 0, 0, a[1] === undefined ? a[0] : a[1], 0, 0]); break;
      case T.ROTATE: {
        const c = Math.cos(rad(a[0])), s = Math.sin(rad(a[0])), cx = a[1] || 0, cy = a[2] || 0;
        m = new g.DOMMatrix([c, s, -s, c, cx - c * cx + s * cy, cy - s * cx - c * cy]);
        break;
      }
      case T.SKEWX: m = new g.DOMMatrix([1, 0, Math.tan(rad(a[0])), 1, 0, 0]); break;
      case T.SKEWY: m = new g.DOMMatrix([1, Math.tan(rad(a[0])), 0, 1, 0, 0]); break;
      default: m = new g.DOMMatrix();
    }
    return { type, m, angle: type === T.ROTATE || type === T.SKEWX || type === T.SKEWY ? a[0] : 0, args: a };
  }
  const T_NAMES = { matrix: T.MATRIX, translate: T.TRANSLATE, scale: T.SCALE, rotate: T.ROTATE, skewX: T.SKEWX, skewY: T.SKEWY };
  function parseTransforms(s) {
    const out = [];
    const re = /(matrix|translate|scale|rotate|skewX|skewY)\s*\(([^)]*)\)/g;
    let m;
    while ((m = re.exec(String(s || '')))) {
      const a = numbers(m[2]);
      const need = { matrix: [6], translate: [1, 2], scale: [1, 2], rotate: [1, 3], skewX: [1], skewY: [1] }[m[1]];
      if (!need.includes(a.length)) return [];
      out.push(transformOf(T_NAMES[m[1]], a));
    }
    return out;
  }
  function serTransform(t) {
    const m = t.m;
    switch (t.type) {
      case T.MATRIX: return `matrix(${[m.a, m.b, m.c, m.d, m.e, m.f].map(fmt).join(' ')})`;
      case T.TRANSLATE: return `translate(${fmt(m.e)} ${fmt(m.f)})`;
      case T.SCALE: return `scale(${fmt(m.a)} ${fmt(m.d)})`;
      case T.ROTATE: return t.args[1] || t.args[2] ? `rotate(${fmt(t.angle)} ${fmt(t.args[1] || 0)} ${fmt(t.args[2] || 0)})` : `rotate(${fmt(t.angle)})`;
      case T.SKEWX: return `skewX(${fmt(t.angle)})`;
      case T.SKEWY: return `skewY(${fmt(t.angle)})`;
      default: return '';
    }
  }
  function trSet(t, type, args) {
    const st = t[S];
    if (st.ro) throw roError();
    st.t = transformOf(type, args);
    if (st.owner) st.owner.commit();
  }
  class SVGTransform {
    constructor() { ILLEGAL(); }
    get type() { return this[S].t.type; }
    get matrix() { return this[S].t.m; }
    get angle() { return this[S].t.angle; }
    setMatrix(m = {}) { const x = g.DOMMatrix.fromMatrix(m); trSet(this, T.MATRIX, [x.a, x.b, x.c, x.d, x.e, x.f]); }
    setTranslate(tx, ty) { trSet(this, T.TRANSLATE, [+tx, +ty]); }
    setScale(sx, sy) { trSet(this, T.SCALE, [+sx, +sy]); }
    setRotate(angle, cx, cy) { trSet(this, T.ROTATE, [+angle, +cx, +cy]); }
    setSkewX(angle) { trSet(this, T.SKEWX, [+angle]); }
    setSkewY(angle) { trSet(this, T.SKEWY, [+angle]); }
  }
  consts(SVGTransform, { SVG_TRANSFORM_UNKNOWN: 0, SVG_TRANSFORM_MATRIX: 1, SVG_TRANSFORM_TRANSLATE: 2, SVG_TRANSFORM_SCALE: 3, SVG_TRANSFORM_ROTATE: 4, SVG_TRANSFORM_SKEWX: 5, SVG_TRANSFORM_SKEWY: 6 });
  tag(SVGTransform, 'SVGTransform');
  const newTransform = (t, owner) => make(SVGTransform, { t, owner: owner || null, ro: false });
  // Product of a transform attribute (identity when absent / invalid).
  function attrMatrix(el, attr) {
    const m = new g.DOMMatrix();
    for (const t of parseTransforms(el.getAttribute(attr))) m.multiplySelf(t.m);
    return m;
  }

  // -------------------------------------------------------------------- lists
  // A list attached to an attribute re-parses it when it changed and
  // writes it back after each mutation. kind: { parse(str, list) → items,
  // ser(items) → str, adopt(item, list) → item, check(item) }.
  function listState(el, attr, kind, ro) { return { el, attr, kind, ro, items: [], src: undefined }; }
  function sync(list) {
    const st = list[S];
    if (st.attr) {
      const src = st.el.getAttribute(st.attr);
      if (src !== st.src) { st.src = src; st.items = st.kind.parse(src || '', st); }
    }
    const n = st.items.length;
    for (let i = 0; i < n; i++) if (!Object.prototype.hasOwnProperty.call(list, i)) Object.defineProperty(list, i, { get() { return sync(this)[i]; }, set(v) { this.replaceItem(v, i); }, configurable: true, enumerable: true });
    for (let i = n; Object.prototype.hasOwnProperty.call(list, i); i++) delete list[i];
    return st.items;
  }
  function commitState(st) {
    if (!st.attr) return;
    st.src = st.kind.ser(st.items);
    st.el.setAttribute(st.attr, st.src);
  }
  class SVGListBase {
    get length() { return sync(this).length; }
    get numberOfItems() { return sync(this).length; }
    clear() { if (this[S].ro) throw roError(); sync(this); this[S].items = []; commitState(this[S]); sync(this); }
    initialize(item) { if (this[S].ro) throw roError(); const it = this[S].kind.adopt(item, this[S]); this[S].items = [it]; commitState(this[S]); sync(this); return it; }
    getItem(i) { const items = sync(this); i = i >>> 0; if (i >= items.length) throw domError(`Failed to execute 'getItem': The index provided (${i}) is greater than or equal to the maximum bound (${items.length}).`, 'IndexSizeError'); return items[i]; }
    insertItemBefore(item, i) { if (this[S].ro) throw roError(); const items = sync(this); const it = this[S].kind.adopt(item, this[S]); items.splice(Math.min(i >>> 0, items.length), 0, it); commitState(this[S]); sync(this); return it; }
    replaceItem(item, i) { if (this[S].ro) throw roError(); const items = sync(this); i = i >>> 0; if (i >= items.length) throw domError(`Failed to execute 'replaceItem': The index provided (${i}) is greater than or equal to the maximum bound (${items.length}).`, 'IndexSizeError'); const it = this[S].kind.adopt(item, this[S]); items[i] = it; commitState(this[S]); sync(this); return it; }
    removeItem(i) { if (this[S].ro) throw roError(); const items = sync(this); i = i >>> 0; if (i >= items.length) throw domError(`Failed to execute 'removeItem': The index provided (${i}) is greater than or equal to the maximum bound (${items.length}).`, 'IndexSizeError'); const [it] = items.splice(i, 1); commitState(this[S]); sync(this); if (it && it[S]) it[S].owner = null; return it; }
    appendItem(item) { if (this[S].ro) throw roError(); const items = sync(this); const it = this[S].kind.adopt(item, this[S]); items.push(it); commitState(this[S]); sync(this); return it; }
    [Symbol.iterator]() { return sync(this).slice()[Symbol.iterator](); }
  }
  function listClass(name, kind, extra) {
    const C = class extends SVGListBase { constructor() { super(); ILLEGAL(); } };
    Object.defineProperty(C, 'name', { value: name });
    for (const k of Object.getOwnPropertyNames(SVGListBase.prototype)) if (k !== 'constructor') Object.defineProperty(C.prototype, k, Object.getOwnPropertyDescriptor(SVGListBase.prototype, k));
    Object.defineProperty(C.prototype, Symbol.iterator, Object.getOwnPropertyDescriptor(SVGListBase.prototype, Symbol.iterator));
    Object.setPrototypeOf(C.prototype, Object.prototype);
    Object.setPrototypeOf(C, Function.prototype);
    if (extra) for (const [k, f] of Object.entries(extra)) hidden(C.prototype, k, f);
    tag(C, name);
    C.kind = kind;
    return C;
  }
  const ownerOf = (st) => ({ el: st.el, commit: () => commitState(st) });
  const LengthKind = {
    parse: (s, st) => String(s).trim().split(/[\s,]+/).filter(Boolean).map((t) => parseLength(t)).filter(Boolean).map((r) => make(SVGLength, { unit: r.unit, num: r.num, owner: ownerOf(st), dir: st.dir, ro: st.ro })),
    ser: (items) => items.map((l) => serLength(l[S].unit, l[S].num)).join(' '),
    adopt(item, st) { if (!(item instanceof SVGLength)) throw new TypeError("parameter 1 is not of type 'SVGLength'."); const r = lenRead(item); if (!item[S].attr && !item[S].owner) { Object.assign(item[S], { unit: r.unit, num: r.num, owner: ownerOf(st), dir: st.dir }); return item; } return make(SVGLength, { unit: r.unit, num: r.num, owner: ownerOf(st), dir: st.dir, ro: false }); },
  };
  const NumberKind = {
    parse: (s, st) => numbers(s).map((n) => make(SVGNumber, { num: n, owner: ownerOf(st), ro: st.ro })),
    ser: (items) => items.map((n) => fmt(n[S].num)).join(' '),
    adopt(item, st) { if (!(item instanceof SVGNumber)) throw new TypeError("parameter 1 is not of type 'SVGNumber'."); if (!item[S].owner) { item[S].owner = ownerOf(st); return item; } return make(SVGNumber, { num: item[S].num, owner: ownerOf(st), ro: false }); },
  };
  function livePoint(x, y, st) {
    const p = new g.DOMPoint(x, y);
    const v = { x, y };
    for (const k of ['x', 'y']) Object.defineProperty(p, k, { get() { return v[k]; }, set(n) { if (st.ro) throw roError(); v[k] = +n; commitState(st); }, configurable: true, enumerable: true });
    return p;
  }
  const PointKind = {
    parse: (s, st) => { const n = numbers(s); const out = []; for (let i = 0; i + 1 < n.length; i += 2) out.push(livePoint(n[i], n[i + 1], st)); return out; },
    ser: (items) => items.map((p) => fmt(p.x) + ',' + fmt(p.y)).join(' '),
    adopt(item, st) { if (!item || typeof item !== 'object') throw new TypeError("parameter 1 is not of type 'DOMPoint'."); return livePoint(+item.x || 0, +item.y || 0, st); },
  };
  const StringKind = {
    parse: (s) => String(s).trim().split(/\s+/).filter(Boolean),
    ser: (items) => items.join(' '),
    adopt: (item) => String(item),
  };
  const TransformKind = {
    parse: (s, st) => parseTransforms(s).map((t) => make(SVGTransform, { t, owner: ownerOf(st), ro: st.ro })),
    ser: (items) => items.map((t) => serTransform(t[S].t)).join(' '),
    adopt(item, st) { if (!(item instanceof SVGTransform)) throw new TypeError("parameter 1 is not of type 'SVGTransform'."); if (!item[S].owner) { item[S].owner = ownerOf(st); return item; } return make(SVGTransform, { t: transformOf(item[S].t.type, item[S].t.args.slice()), owner: ownerOf(st), ro: false }); },
  };
  const SVGLengthList = listClass('SVGLengthList', LengthKind);
  const SVGNumberList = listClass('SVGNumberList', NumberKind);
  const SVGPointList = listClass('SVGPointList', PointKind);
  const SVGStringList = listClass('SVGStringList', StringKind);
  const SVGTransformList = listClass('SVGTransformList', TransformKind, {
    createSVGTransformFromMatrix(m = {}) { const x = g.DOMMatrix.fromMatrix(m); return newTransform(transformOf(T.MATRIX, [x.a, x.b, x.c, x.d, x.e, x.f])); },
    consolidate() {
      if (this[S].ro) throw roError();
      const items = sync(this);
      if (!items.length) return null;
      const m = new g.DOMMatrix();
      for (const t of items) m.multiplySelf(t[S].t.m);
      const t = make(SVGTransform, { t: transformOf(T.MATRIX, [m.a, m.b, m.c, m.d, m.e, m.f]), owner: ownerOf(this[S]), ro: false });
      this[S].items = [t];
      commitState(this[S]);
      sync(this);
      return t;
    },
  });
  const newList = (C, el, attr, ro, dir) => { const st = listState(el, attr, C.kind, ro); st.dir = dir; return make(C, st); };

  // ----------------------------------------------------------- SVGAnimated*
  function animatedClass(name, base) {
    const C = class { constructor() { ILLEGAL(); } };
    Object.defineProperty(C, 'name', { value: name });
    acc(C, 'baseVal', base.get, base.set);
    acc(C, 'animVal', base.anim || base.get);
    tag(C, name);
    return C;
  }
  const SVGAnimatedLength = animatedClass('SVGAnimatedLength', { get() { return this[S].base; }, anim() { return this[S].anim; } });
  const SVGAnimatedAngle = animatedClass('SVGAnimatedAngle', { get() { return this[S].base; }, anim() { return this[S].anim; } });
  const SVGAnimatedLengthList = animatedClass('SVGAnimatedLengthList', { get() { return this[S].base; }, anim() { return this[S].anim; } });
  const SVGAnimatedNumberList = animatedClass('SVGAnimatedNumberList', { get() { return this[S].base; }, anim() { return this[S].anim; } });
  const SVGAnimatedTransformList = animatedClass('SVGAnimatedTransformList', { get() { return this[S].base; }, anim() { return this[S].anim; } });
  const SVGAnimatedPreserveAspectRatio = animatedClass('SVGAnimatedPreserveAspectRatio', { get() { return this[S].base; }, anim() { return this[S].anim; } });
  const SVGAnimatedString = animatedClass('SVGAnimatedString', {
    get() { const st = this[S]; return st.get ? st.get() : st.el.getAttribute(st.attr) ?? ''; },
    set(v) { const st = this[S]; st.el.setAttribute(st.attr, String(v)); },
  });
  const SVGAnimatedNumber = animatedClass('SVGAnimatedNumber', {
    get() { const st = this[S]; const n = numbers(st.el.getAttribute(st.attr)); return n.length ? n[st.index || 0] ?? st.def : st.def; },
    set(v) { const st = this[S]; st.el.setAttribute(st.attr, fmt(+v)); },
  });
  const SVGAnimatedInteger = animatedClass('SVGAnimatedInteger', {
    get() { const st = this[S]; const n = numbers(st.el.getAttribute(st.attr)); return n.length ? Math.trunc(n[st.index || 0] ?? st.def) : st.def; },
    set(v) { const st = this[S]; st.el.setAttribute(st.attr, String(v | 0)); },
  });
  const SVGAnimatedBoolean = animatedClass('SVGAnimatedBoolean', {
    get() { const st = this[S]; return st.el.getAttribute(st.attr) === 'true'; },
    set(v) { const st = this[S]; st.el.setAttribute(st.attr, v ? 'true' : 'false'); },
  });
  const SVGAnimatedEnumeration = animatedClass('SVGAnimatedEnumeration', {
    get() { const st = this[S]; const v = st.el.getAttribute(st.attr); if (v === null) return st.def; const i = st.values.indexOf(v.trim()); return i > 0 ? i : st.unknown ? st.unknown(v) : 0; },
    set(v) { const st = this[S]; v = v & 0xffff; if (!v || !st.values[v]) throw new TypeError(`Failed to set the 'baseVal' property on 'SVGAnimatedEnumeration': The enumeration value provided (${v}) is larger than the largest allowed value.`); st.el.setAttribute(st.attr, st.values[v]); },
  });
  const SVGAnimatedRect = animatedClass('SVGAnimatedRect', { get() { return rectFor(this[S].el, this[S].attr, false); }, anim() { return rectFor(this[S].el, this[S].attr, true); } });
  function parseViewBox(s) {
    if (s === null || s === undefined) return null;
    const n = String(s).trim().split(/[\s,]+/).map(Number);
    return n.length === 4 && n.every(Number.isFinite) && n[2] >= 0 && n[3] >= 0 ? n : null;
  }
  // viewBox as a live DOMRect (null without a valid attribute).
  function rectFor(el, attr, ro) {
    const vb = parseViewBox(el.getAttribute(attr));
    if (!vb) return null;
    const r = ro ? new g.DOMRectReadOnly(vb[0], vb[1], vb[2], vb[3]) : new g.DOMRect(vb[0], vb[1], vb[2], vb[3]);
    if (!ro) {
      ['x', 'y', 'width', 'height'].forEach((k, i) => Object.defineProperty(r, k, {
        get() { const cur = parseViewBox(el.getAttribute(attr)) || vb; return cur[i]; },
        set(v) { const cur = parseViewBox(el.getAttribute(attr)) || vb.slice(); cur[i] = +v; el.setAttribute(attr, cur.map(fmt).join(' ')); },
        configurable: true, enumerable: true,
      }));
    }
    return r;
  }

  // Factories for element properties (cached per element and attribute).
  const length = (attr, dir, def) => function () { return cached(this, 'L' + attr, () => make(SVGAnimatedLength, { base: make(SVGLength, { el: this, attr, dir, def, ro: false }), anim: make(SVGLength, { el: this, attr, dir, def, ro: true }) })); };
  const lengthList = (attr, dir) => function () { return cached(this, 'LL' + attr, () => make(SVGAnimatedLengthList, { base: newList(SVGLengthList, this, attr, false, dir), anim: newList(SVGLengthList, this, attr, true, dir) })); };
  const numberList = (attr) => function () { return cached(this, 'NL' + attr, () => make(SVGAnimatedNumberList, { base: newList(SVGNumberList, this, attr, false), anim: newList(SVGNumberList, this, attr, true) })); };
  const transformList = (attr) => function () { return cached(this, 'TL' + attr, () => make(SVGAnimatedTransformList, { base: newList(SVGTransformList, this, attr, false), anim: newList(SVGTransformList, this, attr, true) })); };
  const stringList = (attr) => function () { return cached(this, 'SL' + attr, () => newList(SVGStringList, this, attr, false)); };
  const number = (attr, def = 0) => function () { return cached(this, 'N' + attr, () => make(SVGAnimatedNumber, { el: this, attr, def })); };
  const string = (attr) => function () { return cached(this, 'S' + attr, () => make(SVGAnimatedString, { el: this, attr })); };
  const href = function () { return cached(this, 'href', () => make(SVGAnimatedString, { el: this, attr: 'href', get: () => this.getAttribute('href') ?? this.getAttributeNS('http://www.w3.org/1999/xlink', 'href') ?? this.getAttribute('xlink:href') ?? '' })); };
  const enumeration = (attr, values, def, unknown) => function () { return cached(this, 'E' + attr, () => make(SVGAnimatedEnumeration, { el: this, attr, values, def, unknown })); };
  const par = function () { return cached(this, 'PAR', () => make(SVGAnimatedPreserveAspectRatio, { base: make(SVGPreserveAspectRatio, { el: this, attr: 'preserveAspectRatio', ro: false }), anim: make(SVGPreserveAspectRatio, { el: this, attr: 'preserveAspectRatio', ro: true }) })); };
  const viewBox = function () { return cached(this, 'VB', () => make(SVGAnimatedRect, { el: this, attr: 'viewBox' })); };
  const UNITS = ['', 'userSpaceOnUse', 'objectBoundingBox'];
  const reflectStr = (attr) => [function () { return this.getAttribute(attr) || ''; }, function (v) { this.setAttribute(attr, v); }];
  const crossOrigin = [function () { const v = this.getAttribute('crossorigin'); if (v === null) return null; return v.toLowerCase() === 'use-credentials' ? 'use-credentials' : 'anonymous'; }, function (v) { if (v === null) this.removeAttribute('crossorigin'); else this.setAttribute('crossorigin', v); }];
  const referrerPolicy = [function () { const v = (this.getAttribute('referrerpolicy') || '').toLowerCase(); return ['no-referrer', 'no-referrer-when-downgrade', 'same-origin', 'origin', 'strict-origin', 'origin-when-cross-origin', 'strict-origin-when-cross-origin', 'unsafe-url'].includes(v) ? v : ''; }, function (v) { this.setAttribute('referrerpolicy', v); }];

  const props = (C, list) => { if (C) for (const [k, get, set] of list) acc(C, k, get, set); };
  const R = (n) => g[n];
  props(R('SVGGraphicsElement'), [['transform', transformList('transform')], ['requiredExtensions', stringList('requiredExtensions')], ['systemLanguage', stringList('systemLanguage')]]);
  props(R('SVGGeometryElement'), [['pathLength', number('pathLength', 0)]]);
  props(R('SVGRectElement'), [['x', length('x', 'x')], ['y', length('y', 'y')], ['width', length('width', 'x')], ['height', length('height', 'y')], ['rx', length('rx', 'x')], ['ry', length('ry', 'y')]]);
  props(R('SVGCircleElement'), [['cx', length('cx', 'x')], ['cy', length('cy', 'y')], ['r', length('r', 'o')]]);
  props(R('SVGEllipseElement'), [['cx', length('cx', 'x')], ['cy', length('cy', 'y')], ['rx', length('rx', 'x')], ['ry', length('ry', 'y')]]);
  props(R('SVGLineElement'), [['x1', length('x1', 'x')], ['y1', length('y1', 'y')], ['x2', length('x2', 'x')], ['y2', length('y2', 'y')]]);
  const points = function () { return cached(this, 'points', () => newList(SVGPointList, this, 'points', false)); };
  const animatedPoints = function () { return cached(this, 'apoints', () => newList(SVGPointList, this, 'points', true)); };
  props(R('SVGPolylineElement'), [['points', points], ['animatedPoints', animatedPoints]]);
  props(R('SVGPolygonElement'), [['points', points], ['animatedPoints', animatedPoints]]);
  const box = [['x', length('x', 'x')], ['y', length('y', 'y')], ['width', length('width', 'x')], ['height', length('height', 'y')]];
  props(R('SVGForeignObjectElement'), box);
  props(R('SVGImageElement'), box.concat([['preserveAspectRatio', par], ['href', href], ['crossOrigin', ...crossOrigin], ['referrerPolicy', ...referrerPolicy]]));
  props(R('SVGUseElement'), box.concat([['href', href]]));
  const PCT100 = { unit: 2, num: 100 };
  props(R('SVGSVGElement'), [['x', length('x', 'x')], ['y', length('y', 'y')], ['width', length('width', 'x', PCT100)], ['height', length('height', 'y', PCT100)], ['viewBox', viewBox], ['preserveAspectRatio', par]]);
  props(R('SVGSymbolElement'), [['viewBox', viewBox], ['preserveAspectRatio', par]]);
  props(R('SVGViewElement'), [['viewBox', viewBox], ['preserveAspectRatio', par]]);
  props(R('SVGGradientElement'), [['gradientUnits', enumeration('gradientUnits', UNITS, 2)], ['gradientTransform', transformList('gradientTransform')], ['spreadMethod', enumeration('spreadMethod', ['', 'pad', 'reflect', 'repeat'], 1)], ['href', href]]);
  consts(R('SVGGradientElement'), { SVG_SPREADMETHOD_UNKNOWN: 0, SVG_SPREADMETHOD_PAD: 1, SVG_SPREADMETHOD_REFLECT: 2, SVG_SPREADMETHOD_REPEAT: 3 });
  const PCT = (n) => ({ unit: 2, num: n });
  props(R('SVGLinearGradientElement'), [['x1', length('x1', 'x', PCT(0))], ['y1', length('y1', 'y', PCT(0))], ['x2', length('x2', 'x', PCT(100))], ['y2', length('y2', 'y', PCT(0))]]);
  props(R('SVGRadialGradientElement'), [['cx', length('cx', 'x', PCT(50))], ['cy', length('cy', 'y', PCT(50))], ['r', length('r', 'o', PCT(50))], ['fx', function () { return cached(this, 'Lfx', () => { const d = () => parseLength(this.getAttribute('cx')) || PCT(50); const mk = (ro) => make(SVGLength, { el: this, attr: 'fx', dir: 'x', get def() { return d(); }, ro }); return make(SVGAnimatedLength, { base: mk(false), anim: mk(true) }); }); }], ['fy', function () { return cached(this, 'Lfy', () => { const d = () => parseLength(this.getAttribute('cy')) || PCT(50); const mk = (ro) => make(SVGLength, { el: this, attr: 'fy', dir: 'y', get def() { return d(); }, ro }); return make(SVGAnimatedLength, { base: mk(false), anim: mk(true) }); }); }], ['fr', length('fr', 'o', PCT(0))]]);
  props(R('SVGPatternElement'), box.concat([['patternUnits', enumeration('patternUnits', UNITS, 2)], ['patternContentUnits', enumeration('patternContentUnits', UNITS, 1)], ['patternTransform', transformList('patternTransform')], ['viewBox', viewBox], ['preserveAspectRatio', par], ['href', href]]));
  props(R('SVGStopElement'), [['offset', function () { return cached(this, 'offset', () => make(SVGAnimatedNumber, { el: this, attr: 'offset', def: 0 })); }]]);
  // Marker orientation: 'auto' | 'auto-start-reverse' | <angle>.
  const MK = R('SVGMarkerElement');
  if (MK) {
    props(MK, [
      ['refX', length('refX', 'x')], ['refY', length('refY', 'y')], ['markerWidth', length('markerWidth', 'x', { unit: 1, num: 3 })], ['markerHeight', length('markerHeight', 'y', { unit: 1, num: 3 })],
      ['markerUnits', enumeration('markerUnits', ['', 'userSpaceOnUse', 'strokeWidth'], 2)],
      ['orientType', enumeration('orient', ['', 'auto', '', 'auto-start-reverse'], 2, (v) => (parseAngle(v) ? 2 : 0))],
      ['orientAngle', function () { return cached(this, 'orientAngle', () => make(SVGAnimatedAngle, { base: make(SVGAngle, { el: this, attr: 'orient', ro: false }), anim: make(SVGAngle, { el: this, attr: 'orient', ro: true }) })); }],
      ['viewBox', viewBox], ['preserveAspectRatio', par],
      ['orient', ...reflectStr('orient')],
    ]);
    hidden(MK.prototype, 'setOrientToAuto', function () { this.setAttribute('orient', 'auto'); });
    hidden(MK.prototype, 'setOrientToAngle', function (a) { if (!(a instanceof SVGAngle)) throw new TypeError("Failed to execute 'setOrientToAngle' on 'SVGMarkerElement': parameter 1 is not of type 'SVGAngle'."); this.setAttribute('orient', a.valueAsString); });
    consts(MK, { SVG_MARKERUNITS_UNKNOWN: 0, SVG_MARKERUNITS_USERSPACEONUSE: 1, SVG_MARKERUNITS_STROKEWIDTH: 2, SVG_MARKER_ORIENT_UNKNOWN: 0, SVG_MARKER_ORIENT_AUTO: 1, SVG_MARKER_ORIENT_ANGLE: 2, SVG_MARKER_ORIENT_AUTO_START_REVERSE: 3 });
  }
  props(R('SVGAElement'), [['href', href], ['target', string('target')], ['referrerPolicy', ...referrerPolicy]]);
  props(R('SVGScriptElement'), [['href', href], ['crossOrigin', ...crossOrigin], ['referrerPolicy', ...referrerPolicy]]);
  props(R('SVGStyleElement'), [['disabled', function () { return this.sheet ? this.sheet.disabled : false; }, function (v) { if (this.sheet) this.sheet.disabled = !!v; }]]);
  props(R('SVGTextContentElement'), [['textLength', length('textLength', 'x')], ['lengthAdjust', enumeration('lengthAdjust', ['', 'spacing', 'spacingAndGlyphs'], 1)]]);
  consts(R('SVGTextContentElement'), { LENGTHADJUST_UNKNOWN: 0, LENGTHADJUST_SPACING: 1, LENGTHADJUST_SPACINGANDGLYPHS: 2 });
  props(R('SVGTextPositioningElement'), [['x', lengthList('x', 'x')], ['y', lengthList('y', 'y')], ['dx', lengthList('dx', 'x')], ['dy', lengthList('dy', 'y')], ['rotate', numberList('rotate')]]);
  props(R('SVGTextPathElement'), [['startOffset', length('startOffset', 'x')], ['method', enumeration('method', ['', 'align', 'stretch'], 1)], ['spacing', enumeration('spacing', ['', 'auto', 'exact'], 2)], ['side', enumeration('side', ['', 'left', 'right'], 1)], ['href', href]]);
  consts(R('SVGTextPathElement'), { TEXTPATH_METHODTYPE_UNKNOWN: 0, TEXTPATH_METHODTYPE_ALIGN: 1, TEXTPATH_METHODTYPE_STRETCH: 2, TEXTPATH_SPACINGTYPE_UNKNOWN: 0, TEXTPATH_SPACINGTYPE_AUTO: 1, TEXTPATH_SPACINGTYPE_EXACT: 2, TEXTPATH_SIDETYPE_UNKNOWN: 0, TEXTPATH_SIDETYPE_LEFT: 1, TEXTPATH_SIDETYPE_RIGHT: 2 });
  // <symbol> is a graphics element in SVG 2.
  if (R('SVGSymbolElement') && R('SVGGraphicsElement')) {
    Object.setPrototypeOf(g.SVGSymbolElement.prototype, g.SVGGraphicsElement.prototype);
    Object.setPrototypeOf(g.SVGSymbolElement, g.SVGGraphicsElement);
  }
  class SVGUnitTypes { constructor() { ILLEGAL(); } }
  consts(SVGUnitTypes, { SVG_UNIT_TYPE_UNKNOWN: 0, SVG_UNIT_TYPE_USERSPACEONUSE: 1, SVG_UNIT_TYPE_OBJECTBOUNDINGBOX: 2 });
  tag(SVGUnitTypes, 'SVGUnitTypes');

  // -------------------------------------------------------------- geometry
  const len = (el, attr, dir) => { const r = parseLength(el.getAttribute(attr)); return r ? r.num * unitFactor(r.unit, el, dir) : 0; };
  function geometryPath(el) {
    switch (el.localName) {
      case 'path': return el.getAttribute('d') || '';
      case 'rect': {
        const x = len(el, 'x', 'x'), y = len(el, 'y', 'y'), w = len(el, 'width', 'x'), h = len(el, 'height', 'y');
        if (w <= 0 || h <= 0) return '';
        let rx = el.hasAttribute('rx') ? len(el, 'rx', 'x') : null, ry = el.hasAttribute('ry') ? len(el, 'ry', 'y') : null;
        if (rx === null) rx = ry || 0;
        if (ry === null) ry = rx;
        rx = Math.min(Math.max(rx, 0), w / 2); ry = Math.min(Math.max(ry, 0), h / 2);
        if (!rx || !ry) return `M${x} ${y}H${x + w}V${y + h}H${x}Z`;
        return `M${x + rx} ${y}H${x + w - rx}A${rx} ${ry} 0 0 1 ${x + w} ${y + ry}V${y + h - ry}A${rx} ${ry} 0 0 1 ${x + w - rx} ${y + h}H${x + rx}A${rx} ${ry} 0 0 1 ${x} ${y + h - ry}V${y + ry}A${rx} ${ry} 0 0 1 ${x + rx} ${y}Z`;
      }
      case 'circle': case 'ellipse': {
        const cx = len(el, 'cx', 'x'), cy = len(el, 'cy', 'y');
        const rx = el.localName === 'circle' ? len(el, 'r', 'o') : len(el, 'rx', 'x'), ry = el.localName === 'circle' ? rx : len(el, 'ry', 'y');
        if (rx <= 0 || ry <= 0) return '';
        return `M${cx + rx} ${cy}A${rx} ${ry} 0 0 1 ${cx} ${cy + ry}A${rx} ${ry} 0 0 1 ${cx - rx} ${cy}A${rx} ${ry} 0 0 1 ${cx} ${cy - ry}A${rx} ${ry} 0 0 1 ${cx + rx} ${cy}Z`;
      }
      case 'line': return `M${len(el, 'x1', 'x')} ${len(el, 'y1', 'y')}L${len(el, 'x2', 'x')} ${len(el, 'y2', 'y')}`;
      case 'polyline': case 'polygon': {
        const n = numbers(el.getAttribute('points'));
        if (n.length < 2) return '';
        let d = `M${n[0]} ${n[1]}`;
        for (let i = 2; i + 1 < n.length; i += 2) d += `L${n[i]} ${n[i + 1]}`;
        return el.localName === 'polygon' ? d + 'Z' : d;
      }
      default: return '';
    }
  }
  const polysOf = (el) => N.svgPathPolys(geometryPath(el));
  // Segments in drawing order (closing segments included).
  function segmentsOf(el) {
    const segs = [];
    for (const p of polysOf(el)) {
      const pts = p.closed ? p.pts.concat([p.pts[0]]) : p.pts;
      for (let i = 0; i + 1 < pts.length; i++) segs.push([pts[i], pts[i + 1]]);
      if (pts.length === 1) segs.push([pts[0], pts[0]]);
    }
    return segs;
  }
  const segLen = (s) => Math.hypot(s[1][0] - s[0][0], s[1][1] - s[0][1]);
  const GE = R('SVGGeometryElement');
  if (GE) {
    hidden(GE.prototype, 'getTotalLength', function getTotalLength() { return segmentsOf(this).reduce((a, s) => a + segLen(s), 0); });
    hidden(GE.prototype, 'getPointAtLength', function getPointAtLength(distance) {
      const segs = segmentsOf(this);
      if (!segs.length) throw domError("Failed to execute 'getPointAtLength' on 'SVGGeometryElement': The element's path is empty.", 'InvalidStateError');
      let d = Math.max(0, +distance || 0);
      for (const s of segs) {
        const l = segLen(s);
        if (d <= l) { const t = l ? d / l : 0; return new g.DOMPoint(s[0][0] + (s[1][0] - s[0][0]) * t, s[0][1] + (s[1][1] - s[0][1]) * t); }
        d -= l;
      }
      const last = segs[segs.length - 1][1];
      return new g.DOMPoint(last[0], last[1]);
    });
    hidden(GE.prototype, 'isPointInFill', function isPointInFill(p = {}) {
      const x = +p.x || 0, y = +p.y || 0;
      let rule = 'nonzero';
      try { rule = g.getComputedStyle(this).getPropertyValue('fill-rule') || this.getAttribute('fill-rule') || 'nonzero'; } catch (e) { /* default */ }
      let wind = 0;
      for (const poly of polysOf(this)) {
        const pts = poly.pts;
        for (let i = 0; i < pts.length; i++) {
          const a = pts[i], b = pts[(i + 1) % pts.length];
          if ((a[1] <= y) !== (b[1] <= y)) {
            const xi = a[0] + ((y - a[1]) * (b[0] - a[0])) / (b[1] - a[1]);
            if (xi > x) wind += a[1] < b[1] ? 1 : -1;
          }
        }
      }
      return rule.trim() === 'evenodd' ? Math.abs(wind) % 2 === 1 : wind !== 0;
    });
    hidden(GE.prototype, 'isPointInStroke', function isPointInStroke(p = {}) {
      const x = +p.x || 0, y = +p.y || 0;
      let w = 1;
      try { w = parseFloat(g.getComputedStyle(this).getPropertyValue('stroke-width')) || parseFloat(this.getAttribute('stroke-width')) || 1; } catch (e) { /* default */ }
      for (const s of segmentsOf(this)) {
        const [ax, ay] = s[0], [bx, by] = s[1];
        const dx = bx - ax, dy = by - ay, l2 = dx * dx + dy * dy;
        const t = l2 ? Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / l2)) : 0;
        if (Math.hypot(x - (ax + t * dx), y - (ay + t * dy)) <= w / 2) return true;
      }
      return false;
    });
  }

  // ------------------------------------------------- coordinate systems
  function svgViewportSize(svg) {
    if (isOuterSvg(svg)) {
      const r = svg.getBoundingClientRect();
      if (r.width || r.height) return [r.width, r.height];
      const w = parseLength(svg.getAttribute('width')), h = parseLength(svg.getAttribute('height'));
      return [w && w.unit !== 2 ? w.num * unitFactor(w.unit, null) : 300, h && h.unit !== 2 ? h.num * unitFactor(h.unit, null) : 150];
    }
    return [svg.width.baseVal.value, svg.height.baseVal.value];
  }
  // viewBox → viewport transform of an <svg> (or identity).
  function viewBoxMatrix(svg, size) {
    const vb = parseViewBox(svg.getAttribute('viewBox'));
    if (!vb || !vb[2] || !vb[3]) return new g.DOMMatrix();
    const [w, h] = size || svgViewportSize(svg);
    const p = parsePAR(svg.getAttribute('preserveAspectRatio'));
    let sx = w / vb[2], sy = h / vb[3];
    if (p.align !== 1) {
      const s = p.meet === 2 ? Math.max(sx, sy) : Math.min(sx, sy);
      sx = sy = s;
    }
    let tx = -vb[0] * sx, ty = -vb[1] * sy;
    if (p.align !== 1) {
      const ax = (p.align - 2) % 3, ay = Math.floor((p.align - 2) / 3);
      tx += ax === 1 ? (w - vb[2] * sx) / 2 : ax === 2 ? w - vb[2] * sx : 0;
      ty += ay === 1 ? (h - vb[3] * sy) / 2 : ay === 2 ? h - vb[3] * sy : 0;
    }
    return new g.DOMMatrix([sx, 0, 0, sy, tx, ty]);
  }
  // Transform from el's user space to its nearest viewport (screen: to the
  // page viewport, through every enclosing <svg>).
  function ctm(el, screen) {
    let m = new g.DOMMatrix();
    let n = el;
    while (n && isSvg(n)) {
      if (n instanceof g.SVGSVGElement) {
        m = viewBoxMatrix(n).multiply(m);
        if (isOuterSvg(n)) {
          if (screen) {
            const r = n.getBoundingClientRect();
            m = new g.DOMMatrix([1, 0, 0, 1, r.left, r.top]).multiply(m);
          }
          break;
        }
        m = new g.DOMMatrix([1, 0, 0, 1, n.x.baseVal.value, n.y.baseVal.value]).multiply(m);
        if (!screen && n !== el) break;
      } else {
        if (n.hasAttribute('transform')) m = attrMatrix(n, 'transform').multiply(m);
        if (n.localName === 'use' && n !== el) m = new g.DOMMatrix([1, 0, 0, 1, len(n, 'x', 'x'), len(n, 'y', 'y')]).multiply(m);
      }
      n = n.parentNode;
    }
    return m;
  }
  const SG = R('SVGGraphicsElement');
  const SE = R('SVGElement');
  const nativeBBox = SE && SE.prototype.getBBox;
  const nativeRect = g.Element.prototype.getBoundingClientRect;
  function textMetrics(el) {
    let font = '16px sans-serif';
    try { const cs = g.getComputedStyle(el); font = `${cs.fontStyle || 'normal'} ${cs.fontWeight || 'normal'} ${cs.fontSize || '16px'} ${cs.fontFamily || 'sans-serif'}`; } catch (e) { /* default */ }
    return (text) => N.measureText(text, font);
  }
  function unionRect(a, b) {
    if (!a) return b;
    if (!b) return a;
    const x = Math.min(a.x, b.x), y = Math.min(a.y, b.y);
    return { x, y, width: Math.max(a.x + a.width, b.x + b.width) - x, height: Math.max(a.y + a.height, b.y + b.height) - y };
  }
  function mapRect(r, m) {
    if (!r) return null;
    const pts = [[r.x, r.y], [r.x + r.width, r.y], [r.x, r.y + r.height], [r.x + r.width, r.y + r.height]].map(([x, y]) => [m.a * x + m.c * y + m.e, m.b * x + m.d * y + m.f]);
    const xs = pts.map((p) => p[0]), ys = pts.map((p) => p[1]);
    return { x: Math.min(...xs), y: Math.min(...ys), width: Math.max(...xs) - Math.min(...xs), height: Math.max(...ys) - Math.min(...ys) };
  }
  // Bounding box in the element's user space (null = empty).
  function bbox(el, depth = 0) {
    if (depth > 64) return null;
    const name = el.localName;
    if (GE && el instanceof GE) {
      let r = null;
      for (const p of polysOf(el)) for (const [x, y] of p.pts) r = unionRect(r, { x, y, width: 0, height: 0 });
      return r;
    }
    if (name === 'image' || name === 'foreignObject' || name === 'foreignobject') return { x: len(el, 'x', 'x'), y: len(el, 'y', 'y'), width: len(el, 'width', 'x'), height: len(el, 'height', 'y') };
    if (name === 'use') {
      const id = (el.getAttribute('href') || el.getAttribute('xlink:href') || '').replace(/^#/, '');
      const ref = id && el.ownerDocument.getElementById(id);
      if (!ref || !isSvg(ref)) return null;
      const r = ref.hasAttribute('transform') ? mapRect(bbox(ref, depth + 1), attrMatrix(ref, 'transform')) : bbox(ref, depth + 1);
      return r && { x: r.x + len(el, 'x', 'x'), y: r.y + len(el, 'y', 'y'), width: r.width, height: r.height };
    }
    if (name === 'text' || name === 'tspan' || name === 'textPath') {
      const text = el.textContent || '';
      const m = textMetrics(el)(text);
      let x = len(el, 'x', 'x') + len(el, 'dx', 'x');
      const y = len(el, 'y', 'y') + len(el, 'dy', 'y');
      let anchor = 'start';
      try { anchor = g.getComputedStyle(el).getPropertyValue('text-anchor') || el.getAttribute('text-anchor') || 'start'; } catch (e) { /* default */ }
      if (anchor === 'middle') x -= m.width / 2; else if (anchor === 'end') x -= m.width;
      return { x, y: y - m.fontAscent, width: m.width, height: m.fontAscent + m.fontDescent };
    }
    let r = null;
    for (const c of el.children) {
      if (!(SG && c instanceof SG) || ['defs', 'symbol', 'clipPath', 'mask', 'pattern', 'marker', 'linearGradient', 'radialGradient'].includes(c.localName)) continue;
      let cr = bbox(c, depth + 1);
      if (!cr) continue;
      if (c instanceof g.SVGSVGElement) cr = mapRect(cr, new g.DOMMatrix([1, 0, 0, 1, c.x.baseVal.value, c.y.baseVal.value]).multiply(viewBoxMatrix(c)));
      else if (c.hasAttribute('transform')) cr = mapRect(cr, attrMatrix(c, 'transform'));
      r = unionRect(r, cr);
    }
    return r;
  }
  if (SG) {
    const getBBox = function getBBox() {
      const r = bbox(this);
      if (r) return new g.DOMRect(r.x, r.y, r.width, r.height);
      return nativeBBox ? nativeBBox.call(this) : new g.DOMRect();
    };
    hidden(SG.prototype, 'getBBox', getBBox);
    if (g.SVGSVGElement) hidden(g.SVGSVGElement.prototype, 'getBBox', getBBox);
    hidden(SG.prototype, 'getCTM', function getCTM() { return this.isConnected ? ctm(this, false) : null; });
    hidden(SG.prototype, 'getScreenCTM', function getScreenCTM() { return this.isConnected ? ctm(this, true) : null; });
    // Elements inside an <svg> have no layout boxes of their own: their
    // client rect is their bounding box in screen space.
    hidden(SG.prototype, 'getBoundingClientRect', function getBoundingClientRect() {
      const r = nativeRect.call(this);
      if (r.width || r.height || isOuterSvg(this) || !this.isConnected || !this.ownerSVGElement) return r;
      const b = mapRect(bbox(this), ctm(this, true));
      return b ? new g.DOMRect(b.x, b.y, b.width, b.height) : r;
    });
  }
  if (SE) {
    acc(SE, 'viewportElement', function () {
      for (let n = this.parentNode; n && isSvg(n); n = n.parentNode) if (n instanceof g.SVGSVGElement || n.localName === 'symbol') return n;
      return null;
    });
  }
  acc(g.Document, 'rootElement', function () { const d = this.documentElement; return d && d instanceof g.SVGSVGElement ? d : null; });

  // ---------------------------------------------------------- text content
  const TC = R('SVGTextContentElement');
  if (TC) {
    const chars = (el) => Array.from(el.textContent || '');
    const startX = (el) => len(el, 'x', 'x') + len(el, 'dx', 'x');
    const startY = (el) => len(el, 'y', 'y') + len(el, 'dy', 'y');
    const bad = (el, i, what) => { if (i < 0 || i >= chars(el).length) throw domError(`Failed to execute '${what}' on 'SVGTextContentElement': The charnum provided (${i}) is greater than or equal to the maximum bound (${chars(el).length}).`, 'IndexSizeError'); };
    const methods = {
      getNumberOfChars() { return chars(this).length; },
      getComputedTextLength() { return textMetrics(this)(this.textContent || '').width; },
      getSubStringLength(i, n) { i = i >>> 0; bad(this, i, 'getSubStringLength'); return textMetrics(this)(chars(this).slice(i, i + (n >>> 0)).join('')).width; },
      getStartPositionOfChar(i) { i = i >>> 0; bad(this, i, 'getStartPositionOfChar'); return new g.DOMPoint(startX(this) + textMetrics(this)(chars(this).slice(0, i).join('')).width, startY(this)); },
      getEndPositionOfChar(i) { i = i >>> 0; bad(this, i, 'getEndPositionOfChar'); return new g.DOMPoint(startX(this) + textMetrics(this)(chars(this).slice(0, i + 1).join('')).width, startY(this)); },
      getExtentOfChar(i) {
        i = i >>> 0; bad(this, i, 'getExtentOfChar');
        const mt = textMetrics(this), cs = chars(this);
        const x0 = startX(this) + mt(cs.slice(0, i).join('')).width, m = mt(cs[i]);
        return new g.DOMRect(x0, startY(this) - m.fontAscent, m.width, m.fontAscent + m.fontDescent);
      },
      getRotationOfChar(i) { i = i >>> 0; bad(this, i, 'getRotationOfChar'); const r = numbers(this.getAttribute('rotate')); return r.length ? r[Math.min(i, r.length - 1)] : 0; },
      getCharNumAtPosition(p = {}) {
        const mt = textMetrics(this), cs = chars(this), x = +p.x || 0, y = +p.y || 0;
        let cx = startX(this);
        const fm = mt('x');
        if (y < startY(this) - fm.fontAscent || y > startY(this) + fm.fontDescent) return -1;
        for (let i = 0; i < cs.length; i++) { const w = mt(cs[i]).width; if (x >= cx && x < cx + w) return i; cx += w; }
        return -1;
      },
      selectSubString(i, n) {
        i = i >>> 0; bad(this, i, 'selectSubString');
        const sel = g.getSelection && g.getSelection();
        const t = this.firstChild;
        if (!sel || !t || t.nodeType !== 3) return;
        const r = document.createRange();
        r.setStart(t, Math.min(i, t.length)); r.setEnd(t, Math.min(i + (n >>> 0), t.length));
        sel.removeAllRanges(); sel.addRange(r);
      },
    };
    for (const [k, f] of Object.entries(methods)) hidden(TC.prototype, k, f);
  }

  // ------------------------------------------------------------ SVGSVGElement
  const SS = R('SVGSVGElement');
  if (SS) {
    const views = new WeakMap();
    const view = (el) => { let v = views.get(el); if (!v) { v = { scale: 1, translate: new g.DOMPoint(0, 0) }; views.set(el, v); } return v; };
    acc(SS, 'currentScale', function () { return view(this).scale; }, function (v) { v = +v; if (Number.isFinite(v)) view(this).scale = v; });
    acc(SS, 'currentTranslate', function () { return view(this).translate; });
    const inRect = (el, r, enclose) => {
      const b = mapRect(bbox(el), ctm(el, false));
      if (!b) return false;
      if (enclose) return b.x >= r.x && b.y >= r.y && b.x + b.width <= r.x + r.width && b.y + b.height <= r.y + r.height;
      return b.x < r.x + r.width && r.x < b.x + b.width && b.y < r.y + r.height && r.y < b.y + b.height;
    };
    const list = (svg, rect, ref, enclose) => {
      const out = [];
      for (const el of svg.querySelectorAll('*')) if (GE && (el instanceof GE || el.localName === 'text' || el.localName === 'image' || el.localName === 'use') && (!ref || ref.contains(el)) && inRect(el, rect, enclose)) out.push(el);
      return Object.freeze(Object.assign(out, { item(i) { return this[i] ?? null; } }));
    };
    const methods = {
      createSVGNumber() { return make(SVGNumber, { num: 0, owner: null, ro: false }); },
      createSVGLength() { return make(SVGLength, { unit: 1, num: 0, owner: null, ro: false }); },
      createSVGAngle() { return make(SVGAngle, { unit: 1, num: 0, ro: false }); },
      createSVGPoint() { return new g.DOMPoint(); },
      createSVGMatrix() { return new g.DOMMatrix(); },
      createSVGRect() { return new g.DOMRect(); },
      createSVGTransform() { return newTransform(transformOf(T.MATRIX, [1, 0, 0, 1, 0, 0])); },
      createSVGTransformFromMatrix(m = {}) { const x = g.DOMMatrix.fromMatrix(m); return newTransform(transformOf(T.MATRIX, [x.a, x.b, x.c, x.d, x.e, x.f])); },
      getElementById(id) { id = String(id); for (const el of this.querySelectorAll('[id]')) if (el.id === id) return el; return null; },
      suspendRedraw() { return 1; },
      unsuspendRedraw() {},
      unsuspendRedrawAll() {},
      forceRedraw() {},
      deselectAll() { const s = g.getSelection && g.getSelection(); if (s) s.removeAllRanges(); },
      getIntersectionList(rect, ref) { return list(this, rect, ref, false); },
      getEnclosureList(rect, ref) { return list(this, rect, ref, true); },
      checkIntersection(el, rect) { return inRect(el, rect, false); },
      checkEnclosure(el, rect) { return inRect(el, rect, true); },
    };
    for (const [k, f] of Object.entries(methods)) hidden(SS.prototype, k, f);
  }

  define('SVGLength', SVGLength);
  define('SVGAngle', SVGAngle);
  define('SVGNumber', SVGNumber);
  define('SVGPreserveAspectRatio', SVGPreserveAspectRatio);
  define('SVGTransform', SVGTransform);
  define('SVGLengthList', SVGLengthList);
  define('SVGNumberList', SVGNumberList);
  define('SVGPointList', SVGPointList);
  define('SVGStringList', SVGStringList);
  define('SVGTransformList', SVGTransformList);
  define('SVGUnitTypes', SVGUnitTypes);
  for (const C of [SVGAnimatedLength, SVGAnimatedAngle, SVGAnimatedLengthList, SVGAnimatedNumberList, SVGAnimatedTransformList, SVGAnimatedPreserveAspectRatio, SVGAnimatedString, SVGAnimatedNumber, SVGAnimatedInteger, SVGAnimatedBoolean, SVGAnimatedEnumeration, SVGAnimatedRect]) define(C.name, C);
})();
