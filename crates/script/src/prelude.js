// Web APIs implemented in JavaScript on top of the native bindings.
// Evaluated once per ScriptHost, before page scripts.
(function () {
  'use strict';
  const g = globalThis;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });

  // ---------------------------------------------------------- DOMMatrix
  const IDENT = () => [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
  // Column-major 4x4 like the spec's m11..m44 (m[i*4+j] = m(i+1)(j+1)).
  function mul(a, b) {
    const r = new Array(16).fill(0);
    for (let i = 0; i < 4; i++)
      for (let j = 0; j < 4; j++) {
        let s = 0;
        for (let k = 0; k < 4; k++) s += b[i * 4 + k] * a[k * 4 + j];
        r[i * 4 + j] = s;
      }
    return r;
  }
  function angle(v) {
    const m = /^(-?[\d.e+-]+)(deg|rad|grad|turn)?$/i.exec(String(v).trim());
    if (!m) return 0;
    const n = parseFloat(m[1]);
    switch ((m[2] || 'deg').toLowerCase()) {
      case 'rad': return n;
      case 'grad': return n * Math.PI / 200;
      case 'turn': return n * 2 * Math.PI;
      default: return n * Math.PI / 180;
    }
  }
  function len(v) { return parseFloat(v) || 0; }
  function parseTransform(str) {
    let m = IDENT();
    let is2D = true;
    str = String(str).trim();
    if (str === '' || str === 'none') return { m, is2D };
    const re = /([a-zA-Z0-9]+)\(([^)]*)\)/g;
    let match, any = false;
    while ((match = re.exec(str))) {
      any = true;
      const fn = match[1].toLowerCase();
      const a = match[2].split(/\s*,\s*|\s+/).filter(Boolean);
      let t = IDENT();
      switch (fn) {
        case 'matrix': t = [+a[0], +a[1], 0, 0, +a[2], +a[3], 0, 0, 0, 0, 1, 0, +a[4], +a[5], 0, 1]; break;
        case 'matrix3d': t = a.map(Number); is2D = false; break;
        case 'translate': t[12] = len(a[0]); t[13] = len(a[1] || 0); break;
        case 'translatex': t[12] = len(a[0]); break;
        case 'translatey': t[13] = len(a[0]); break;
        case 'translatez': t[14] = len(a[0]); is2D = false; break;
        case 'translate3d': t[12] = len(a[0]); t[13] = len(a[1]); t[14] = len(a[2]); is2D = false; break;
        case 'scale': t[0] = +a[0]; t[5] = a[1] !== undefined ? +a[1] : +a[0]; break;
        case 'scalex': t[0] = +a[0]; break;
        case 'scaley': t[5] = +a[0]; break;
        case 'scale3d': t[0] = +a[0]; t[5] = +a[1]; t[10] = +a[2]; is2D = false; break;
        case 'rotate': case 'rotatez': { const r = angle(a[0]); t[0] = Math.cos(r); t[1] = Math.sin(r); t[4] = -Math.sin(r); t[5] = Math.cos(r); break; }
        case 'skew': t[4] = Math.tan(angle(a[0])); t[1] = Math.tan(angle(a[1] || 0)); break;
        case 'skewx': t[4] = Math.tan(angle(a[0])); break;
        case 'skewy': t[1] = Math.tan(angle(a[0])); break;
        default: throw new SyntaxError("Failed to parse '" + str + "'");
      }
      m = mul(m, t);
    }
    if (!any) throw new SyntaxError("Failed to parse '" + str + "'");
    return { m, is2D };
  }
  const NAMES = ['m11', 'm12', 'm13', 'm14', 'm21', 'm22', 'm23', 'm24', 'm31', 'm32', 'm33', 'm34', 'm41', 'm42', 'm43', 'm44'];
  class DOMMatrixReadOnly {
    constructor(init) {
      let m = IDENT(), is2D = true;
      if (typeof init === 'string') ({ m, is2D } = parseTransform(init));
      else if (init && typeof init.length === 'number') {
        const a = Array.from(init, Number);
        if (a.length === 6) m = [a[0], a[1], 0, 0, a[2], a[3], 0, 0, 0, 0, 1, 0, a[4], a[5], 0, 1];
        else if (a.length === 16) { m = a; is2D = false; }
        else throw new TypeError('DOMMatrix init array must have 6 or 16 elements');
      }
      Object.defineProperty(this, '_m', { value: m, writable: true });
      Object.defineProperty(this, '_2d', { value: is2D, writable: true });
    }
    static fromMatrix(o) {
      if (o instanceof DOMMatrixReadOnly) return new this(o._2d ? [o.a, o.b, o.c, o.d, o.e, o.f] : o._m.slice());
      o = o || {};
      return new this([o.a ?? o.m11 ?? 1, o.b ?? o.m12 ?? 0, o.c ?? o.m21 ?? 0, o.d ?? o.m22 ?? 1, o.e ?? o.m41 ?? 0, o.f ?? o.m42 ?? 0]);
    }
    static fromFloat32Array(a) { return new this(a); }
    static fromFloat64Array(a) { return new this(a); }
    get a() { return this._m[0]; } get b() { return this._m[1]; } get c() { return this._m[4]; }
    get d() { return this._m[5]; } get e() { return this._m[12]; } get f() { return this._m[13]; }
    get is2D() { return this._2d; }
    get isIdentity() { const i = IDENT(); return this._m.every((v, k) => v === i[k]); }
    multiply(o) { return new DOMMatrix(this._m.slice()).multiplySelf(o); }
    translate(x = 0, y = 0, z = 0) { return new DOMMatrix(this._m.slice()).translateSelf(x, y, z); }
    scale(sx = 1, sy = sx, sz = 1, ox = 0, oy = 0, oz = 0) { return new DOMMatrix(this._m.slice()).scaleSelf(sx, sy, sz, ox, oy, oz); }
    scale3d(s = 1, ox = 0, oy = 0, oz = 0) { return this.scale(s, s, s, ox, oy, oz); }
    rotate(rx = 0, ry, rz) { return new DOMMatrix(this._m.slice()).rotateSelf(rx, ry, rz); }
    skewX(a = 0) { return this.multiply(new DOMMatrix([1, 0, Math.tan(a * Math.PI / 180), 1, 0, 0])); }
    skewY(a = 0) { return this.multiply(new DOMMatrix([1, Math.tan(a * Math.PI / 180), 0, 1, 0, 0])); }
    flipX() { return this.multiply(new DOMMatrix([-1, 0, 0, 1, 0, 0])); }
    flipY() { return this.multiply(new DOMMatrix([1, 0, 0, -1, 0, 0])); }
    inverse() { return new DOMMatrix(this._m.slice()).invertSelf(); }
    transformPoint(p = {}) {
      const x = p.x || 0, y = p.y || 0, z = p.z || 0, w = p.w ?? 1, m = this._m;
      return new DOMPoint(m[0] * x + m[4] * y + m[8] * z + m[12] * w, m[1] * x + m[5] * y + m[9] * z + m[13] * w, m[2] * x + m[6] * y + m[10] * z + m[14] * w, m[3] * x + m[7] * y + m[11] * z + m[15] * w);
    }
    toFloat32Array() { return new Float32Array(this._m); }
    toFloat64Array() { return new Float64Array(this._m); }
    toString() {
      return this._2d ? `matrix(${[this.a, this.b, this.c, this.d, this.e, this.f].join(', ')})` : `matrix3d(${this._m.join(', ')})`;
    }
    toJSON() {
      const o = { a: this.a, b: this.b, c: this.c, d: this.d, e: this.e, f: this.f, is2D: this.is2D, isIdentity: this.isIdentity };
      NAMES.forEach((n, i) => (o[n] = this._m[i]));
      return o;
    }
  }
  NAMES.forEach((n, i) => Object.defineProperty(DOMMatrixReadOnly.prototype, n, { get() { return this._m[i]; }, configurable: true }));
  class DOMMatrix extends DOMMatrixReadOnly {
    multiplySelf(o) { const other = o instanceof DOMMatrixReadOnly ? o : DOMMatrix.fromMatrix(o); this._m = mul(this._m, other._m); this._2d = this._2d && other._2d; return this; }
    preMultiplySelf(o) { const other = o instanceof DOMMatrixReadOnly ? o : DOMMatrix.fromMatrix(o); this._m = mul(other._m, this._m); this._2d = this._2d && other._2d; return this; }
    translateSelf(x = 0, y = 0, z = 0) { const t = IDENT(); t[12] = x; t[13] = y; t[14] = z; if (z) this._2d = false; this._m = mul(this._m, t); return this; }
    scaleSelf(sx = 1, sy = sx, sz = 1, ox = 0, oy = 0, oz = 0) {
      this.translateSelf(ox, oy, oz);
      const t = IDENT(); t[0] = sx; t[5] = sy; t[10] = sz; if (sz !== 1) this._2d = false;
      this._m = mul(this._m, t);
      return this.translateSelf(-ox, -oy, -oz);
    }
    rotateSelf(rx = 0, ry, rz) {
      if (ry === undefined && rz === undefined) { rz = rx; rx = 0; ry = 0; }
      const r = (rz || 0) * Math.PI / 180, t = IDENT();
      t[0] = Math.cos(r); t[1] = Math.sin(r); t[4] = -Math.sin(r); t[5] = Math.cos(r);
      if (rx || ry) this._2d = false;
      this._m = mul(this._m, t);
      return this;
    }
    invertSelf() {
      const m = this._m, inv = new Array(16);
      inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
      inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
      inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
      inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
      inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
      inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
      inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
      inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
      inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
      inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
      inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
      inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
      inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
      inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
      inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
      inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
      const det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
      this._m = det === 0 ? new Array(16).fill(NaN) : inv.map(v => v / det);
      if (det === 0) this._2d = false;
      return this;
    }
    setMatrixValue(s) { const { m, is2D } = parseTransform(s); this._m = m; this._2d = is2D; return this; }
  }
  ['a', 'b', 'c', 'd', 'e', 'f'].forEach((n, i) => {
    const idx = [0, 1, 4, 5, 12, 13][i];
    Object.defineProperty(DOMMatrix.prototype, n, { get() { return this._m[idx]; }, set(v) { this._m[idx] = +v; }, configurable: true });
  });
  NAMES.forEach((n, i) => Object.defineProperty(DOMMatrix.prototype, n, { get() { return this._m[i]; }, set(v) { this._m[i] = +v; if (![0, 1, 4, 5, 12, 13, 10, 15].includes(i) && v !== 0) this._2d = false; }, configurable: true }));
  // Extra matrix operations (Geometry Interfaces §6).
  const MR = DOMMatrixReadOnly.prototype, MW = DOMMatrix.prototype;
  MW.scale3dSelf = function (s = 1, ox = 0, oy = 0, oz = 0) { return this.scaleSelf(s, s, s, ox, oy, oz); };
  MW.skewXSelf = function (a = 0) { this._m = mul(this._m, [1, 0, 0, 0, Math.tan(a * Math.PI / 180), 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]); return this; };
  MW.skewYSelf = function (a = 0) { this._m = mul(this._m, [1, Math.tan(a * Math.PI / 180), 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]); return this; };
  MW.rotateFromVectorSelf = function (x = 0, y = 0) { return this.rotateSelf(x === 0 && y === 0 ? 0 : Math.atan2(y, x) * 180 / Math.PI); };
  MW.rotateAxisAngleSelf = function (x = 0, y = 0, z = 0, angle = 0) {
    const len = Math.hypot(x, y, z);
    if (!len) return this;
    x /= len; y /= len; z /= len;
    const r = angle * Math.PI / 180, sc = Math.sin(r / 2) * Math.cos(r / 2), sq = Math.sin(r / 2) ** 2;
    const t = [
      1 - 2 * (y * y + z * z) * sq, 2 * (x * y * sq + z * sc), 2 * (x * z * sq - y * sc), 0,
      2 * (x * y * sq - z * sc), 1 - 2 * (x * x + z * z) * sq, 2 * (y * z * sq + x * sc), 0,
      2 * (x * z * sq + y * sc), 2 * (y * z * sq - x * sc), 1 - 2 * (x * x + y * y) * sq, 0,
      0, 0, 0, 1];
    if (x !== 0 || y !== 0) this._2d = false;
    this._m = mul(this._m, t);
    return this;
  };
  MR.scaleNonUniform = function (sx = 1, sy = 1) { return this.scale(sx, sy, 1, 0, 0, 0); };
  MR.rotateFromVector = function (x, y) { return new DOMMatrix(this._m.slice()).rotateFromVectorSelf(x, y); };
  MR.rotateAxisAngle = function (x, y, z, a) { return new DOMMatrix(this._m.slice()).rotateAxisAngleSelf(x, y, z, a); };
  MR.skewX = function (a) { return new DOMMatrix(this._m.slice()).skewXSelf(a); };
  MR.skewY = function (a) { return new DOMMatrix(this._m.slice()).skewYSelf(a); };
  Object.defineProperty(DOMMatrixReadOnly.prototype, Symbol.toStringTag, { value: 'DOMMatrixReadOnly', configurable: true });
  Object.defineProperty(DOMMatrix.prototype, Symbol.toStringTag, { value: 'DOMMatrix', configurable: true });

  // ------------------------------------------------------------ DOMPoint
  const P = Symbol('point');
  class DOMPointReadOnly {
    constructor(x = 0, y = 0, z = 0, w = 1) { Object.defineProperty(this, P, { value: [+x, +y, +z, +w] }); }
    static fromPoint(p = {}) { return new this(p.x ?? 0, p.y ?? 0, p.z ?? 0, p.w ?? 1); }
    get x() { return this[P][0]; } get y() { return this[P][1]; } get z() { return this[P][2]; } get w() { return this[P][3]; }
    matrixTransform(m) { return (m instanceof DOMMatrixReadOnly ? m : DOMMatrix.fromMatrix(m)).transformPoint(this); }
    toJSON() { return { x: this.x, y: this.y, z: this.z, w: this.w }; }
  }
  class DOMPoint extends DOMPointReadOnly {
    get x() { return this[P][0]; } set x(v) { this[P][0] = +v; }
    get y() { return this[P][1]; } set y(v) { this[P][1] = +v; }
    get z() { return this[P][2]; } set z(v) { this[P][2] = +v; }
    get w() { return this[P][3]; } set w(v) { this[P][3] = +v; }
  }
  Object.defineProperty(DOMPointReadOnly.prototype, Symbol.toStringTag, { value: 'DOMPointReadOnly', configurable: true });
  Object.defineProperty(DOMPoint.prototype, Symbol.toStringTag, { value: 'DOMPoint', configurable: true });

  // ------------------------------------------------------------- DOMRect
  const R = Symbol('rect');
  class DOMRectReadOnly {
    constructor(x = 0, y = 0, width = 0, height = 0) { Object.defineProperty(this, R, { value: [+x, +y, +width, +height] }); }
    static fromRect(o = {}) { return new this(o.x ?? 0, o.y ?? 0, o.width ?? 0, o.height ?? 0); }
    get x() { return this[R][0]; } get y() { return this[R][1]; }
    get width() { return this[R][2]; } get height() { return this[R][3]; }
    get top() { const r = this[R]; return Math.min(r[1], r[1] + r[3]); }
    get right() { const r = this[R]; return Math.max(r[0], r[0] + r[2]); }
    get bottom() { const r = this[R]; return Math.max(r[1], r[1] + r[3]); }
    get left() { const r = this[R]; return Math.min(r[0], r[0] + r[2]); }
    toJSON() { return { x: this.x, y: this.y, width: this.width, height: this.height, top: this.top, right: this.right, bottom: this.bottom, left: this.left }; }
  }
  class DOMRect extends DOMRectReadOnly {
    get x() { return this[R][0]; } set x(v) { this[R][0] = +v; }
    get y() { return this[R][1]; } set y(v) { this[R][1] = +v; }
    get width() { return this[R][2]; } set width(v) { this[R][2] = +v; }
    get height() { return this[R][3]; } set height(v) { this[R][3] = +v; }
  }
  Object.defineProperty(DOMRectReadOnly.prototype, Symbol.toStringTag, { value: 'DOMRectReadOnly', configurable: true });
  Object.defineProperty(DOMRect.prototype, Symbol.toStringTag, { value: 'DOMRect', configurable: true });
  class DOMRectList {
    constructor() { throw new TypeError('Illegal constructor'); }
    get length() { return this[R].length; }
    item(i) { return this[R][i] ?? null; }
    [Symbol.iterator]() { return this[R][Symbol.iterator](); }
  }
  Object.defineProperty(DOMRectList.prototype, Symbol.toStringTag, { value: 'DOMRectList', configurable: true });
  function makeRectList(rects) {
    const l = Object.create(DOMRectList.prototype);
    Object.defineProperty(l, R, { value: rects });
    rects.forEach((r, i) => Object.defineProperty(l, i, { value: r, enumerable: true }));
    return l;
  }

  // ------------------------------------------------------------- DOMQuad
  const Q = Symbol('quad');
  class DOMQuad {
    constructor(p1 = {}, p2 = {}, p3 = {}, p4 = {}) { Object.defineProperty(this, Q, { value: [p1, p2, p3, p4].map((p) => DOMPoint.fromPoint(p)) }); }
    static fromRect(r = {}) { const x = r.x ?? 0, y = r.y ?? 0, w = r.width ?? 0, h = r.height ?? 0; return new DOMQuad({ x, y }, { x: x + w, y }, { x: x + w, y: y + h }, { x, y: y + h }); }
    static fromQuad(q = {}) { return new DOMQuad(q.p1, q.p2, q.p3, q.p4); }
    get p1() { return this[Q][0]; } get p2() { return this[Q][1]; } get p3() { return this[Q][2]; } get p4() { return this[Q][3]; }
    getBounds() {
      const xs = this[Q].map((p) => p.x), ys = this[Q].map((p) => p.y);
      const x = Math.min(...xs), y = Math.min(...ys);
      return new DOMRect(x, y, Math.max(...xs) - x, Math.max(...ys) - y);
    }
    toJSON() { return { p1: this.p1, p2: this.p2, p3: this.p3, p4: this.p4 }; }
  }
  Object.defineProperty(DOMQuad.prototype, Symbol.toStringTag, { value: 'DOMQuad', configurable: true });

  define('DOMMatrixReadOnly', DOMMatrixReadOnly);
  define('DOMMatrix', DOMMatrix);
  define('WebKitCSSMatrix', DOMMatrix);
  define('SVGMatrix', DOMMatrix);
  define('DOMPointReadOnly', DOMPointReadOnly);
  define('DOMPoint', DOMPoint);
  define('SVGPoint', DOMPoint);
  define('DOMRectReadOnly', DOMRectReadOnly);
  define('DOMRect', DOMRect);
  define('SVGRect', DOMRect);
  define('DOMRectList', DOMRectList);
  define('DOMQuad', DOMQuad);
  // Natives build rects / rect lists through these.
  g.__rhNative.setFactory('rect', (x, y, w, h) => new DOMRect(x, y, w, h));
  g.__rhNative.setFactory('rectList', (rects) => makeRectList(Array.from(rects)));
})();

// ------------------------------------------------- HTML table DOM API
// (HTML §4.9: caption/tHead/tFoot, create*/delete*, rows in header /
// body / footer order, insertRow/deleteRow, sectionRowIndex...)
(function () {
  'use strict';
  const g = globalThis;
  const T = g.HTMLTableElement && g.HTMLTableElement.prototype;
  const S = g.HTMLTableSectionElement && g.HTMLTableSectionElement.prototype;
  const R = g.HTMLTableRowElement && g.HTMLTableRowElement.prototype;
  if (!T || !S || !R) return;
  const tagOf = (n) => (n && n.nodeType === 1 ? String(n.localName || n.tagName).toLowerCase() : '');
  const kids = (n, tag) => Array.prototype.filter.call(n.childNodes, (c) => tagOf(c) === tag);
  const first = (n, tag) => kids(n, tag)[0] || null;
  const coll = (list) => {
    list.item = (i) => list[i] || null;
    list.namedItem = (name) => list.find((e) => e.id === name || e.getAttribute('name') === name) || null;
    return list;
  };
  const indexError = () => {
    try { return new DOMException('Index out of range', 'IndexSizeError'); } catch (e) { const err = new Error('IndexSizeError'); err.code = 1; return err; }
  };
  const hierarchyError = () => {
    try { return new DOMException('Wrong element type', 'HierarchyRequestError'); } catch (e) { const err = new Error('HierarchyRequestError'); err.code = 3; return err; }
  };
  const acc = (proto, name, get, set) => { try { Object.defineProperty(proto, name, { get, set, configurable: true, enumerable: true }); } catch (e) {} };
  const tableRows = (t) => {
    const out = [];
    const head = first(t, 'thead');
    const foot = first(t, 'tfoot');
    if (head) out.push(...kids(head, 'tr'));
    for (const c of t.childNodes) {
      const tg = tagOf(c);
      if (tg === 'tr') out.push(c);
      else if (tg === 'tbody') out.push(...kids(c, 'tr'));
    }
    if (foot) out.push(...kids(foot, 'tr'));
    return out;
  };
  acc(T, 'caption', function () { return first(this, 'caption'); }, function (v) {
    const old = first(this, 'caption');
    if (old === v) return;
    if (old) this.removeChild(old);
    if (v) {
      if (tagOf(v) !== 'caption') throw hierarchyError();
      this.insertBefore(v, this.firstChild);
    }
  });
  const section = (tag) => [function () { return first(this, tag); }, function (v) {
    const old = first(this, tag);
    if (old === v) return;
    if (v && tagOf(v) !== tag) throw hierarchyError();
    if (old) this.removeChild(old);
    if (!v) return;
    if (tag === 'thead') {
      const ref = Array.prototype.find.call(this.childNodes, (c) => c.nodeType === 1 && tagOf(c) !== 'caption' && tagOf(c) !== 'colgroup');
      this.insertBefore(v, ref || null);
    } else {
      this.appendChild(v);
    }
  }];
  acc(T, 'tHead', ...section('thead'));
  acc(T, 'tFoot', ...section('tfoot'));
  acc(T, 'tBodies', function () { return coll(kids(this, 'tbody')); });
  acc(T, 'rows', function () { return coll(tableRows(this)); });
  T.createCaption = function () {
    let c = first(this, 'caption');
    if (!c) {
      c = this.ownerDocument.createElement('caption');
      this.insertBefore(c, this.firstChild);
    }
    return c;
  };
  T.deleteCaption = function () { const c = first(this, 'caption'); if (c) this.removeChild(c); };
  T.createTHead = function () {
    let h = first(this, 'thead');
    if (!h) {
      h = this.ownerDocument.createElement('thead');
      this.tHead = h;
    }
    return h;
  };
  T.deleteTHead = function () { const h = first(this, 'thead'); if (h) this.removeChild(h); };
  T.createTFoot = function () {
    let f = first(this, 'tfoot');
    if (!f) {
      f = this.ownerDocument.createElement('tfoot');
      this.appendChild(f);
    }
    return f;
  };
  T.deleteTFoot = function () { const f = first(this, 'tfoot'); if (f) this.removeChild(f); };
  T.createTBody = function () {
    const b = this.ownerDocument.createElement('tbody');
    const bodies = kids(this, 'tbody');
    const last = bodies[bodies.length - 1];
    this.insertBefore(b, last ? last.nextSibling : null);
    return b;
  };
  T.insertRow = function (index) {
    index = index === undefined ? -1 : Math.trunc(Number(index)) || 0;
    const rows = tableRows(this);
    if (index < -1 || index > rows.length) throw indexError();
    const tr = this.ownerDocument.createElement('tr');
    if (rows.length === 0) {
      const bodies = kids(this, 'tbody');
      if (bodies.length) bodies[bodies.length - 1].appendChild(tr);
      else this.appendChild(this.ownerDocument.createElement('tbody')).appendChild(tr);
    } else if (index === -1 || index === rows.length) {
      const last = rows[rows.length - 1];
      last.parentNode.insertBefore(tr, last.nextSibling);
    } else {
      rows[index].parentNode.insertBefore(tr, rows[index]);
    }
    return tr;
  };
  T.deleteRow = function (index) {
    const rows = tableRows(this);
    index = Math.trunc(Number(index)) || 0;
    if (index === -1) index = rows.length - 1;
    if (index < 0 || index >= rows.length) { if (index === -1 && rows.length === 0) return; throw indexError(); }
    rows[index].parentNode.removeChild(rows[index]);
  };
  acc(S, 'rows', function () { return coll(kids(this, 'tr')); });
  S.insertRow = function (index) {
    index = index === undefined ? -1 : Math.trunc(Number(index)) || 0;
    const rows = kids(this, 'tr');
    if (index < -1 || index > rows.length) throw indexError();
    const tr = this.ownerDocument.createElement('tr');
    this.insertBefore(tr, index === -1 || index === rows.length ? null : rows[index]);
    return tr;
  };
  S.deleteRow = function (index) {
    const rows = kids(this, 'tr');
    index = Math.trunc(Number(index)) || 0;
    if (index === -1) index = rows.length - 1;
    if (index < 0 || index >= rows.length) { if (rows.length === 0) return; throw indexError(); }
    this.removeChild(rows[index]);
  };
  acc(R, 'cells', function () { return coll(Array.prototype.filter.call(this.childNodes, (c) => tagOf(c) === 'td' || tagOf(c) === 'th')); });
  acc(R, 'rowIndex', function () {
    let p = this.parentNode;
    if (p && tagOf(p) !== 'table') p = p.parentNode;
    if (!p || tagOf(p) !== 'table') return -1;
    return tableRows(p).indexOf(this);
  });
  acc(R, 'sectionRowIndex', function () {
    const p = this.parentNode;
    if (!p) return -1;
    return kids(p, 'tr').indexOf(this);
  });
  R.insertCell = function (index) {
    index = index === undefined ? -1 : Math.trunc(Number(index)) || 0;
    const cells = this.cells;
    if (index < -1 || index > cells.length) throw indexError();
    const td = this.ownerDocument.createElement('td');
    this.insertBefore(td, index === -1 || index === cells.length ? null : cells[index]);
    return td;
  };
  R.deleteCell = function (index) {
    const cells = this.cells;
    index = Math.trunc(Number(index)) || 0;
    if (index === -1) index = cells.length - 1;
    if (index < 0 || index >= cells.length) { if (cells.length === 0) return; throw indexError(); }
    this.removeChild(cells[index]);
  };
})();

// --------------------------------------------- legacy event initialisers
(function () {
  'use strict';
  const g = globalThis;
  const U = g.UIEvent && g.UIEvent.prototype;
  if (U && !U.initUIEvent) {
    U.initUIEvent = function (type, bubbles, cancelable, view, detail) {
      this.initEvent(type, bubbles, cancelable);
      Object.defineProperty(this, 'view', { value: view === undefined ? null : view, configurable: true });
      Object.defineProperty(this, 'detail', { value: Number(detail) || 0, configurable: true });
    };
  }
})();
