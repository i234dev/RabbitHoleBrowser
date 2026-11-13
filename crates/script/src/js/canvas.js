// Canvas 2D (HTML §4.12.5): CanvasRenderingContext2D / OffscreenCanvas /
// OffscreenCanvasRenderingContext2D drawing through the native rasterizer
// (crates/gfx: anti-aliased fill/stroke, dashes, clipping, gradients,
// patterns, compositing/blend modes, shadows, image drawing, text with the
// engine fonts), Path2D (incl. SVG path data), TextMetrics, ImageData,
// ImageBitmap / createImageBitmap, ImageBitmapRenderingContext, toDataURL /
// toBlob (PNG, JPEG). The bitmap of a connected <canvas> is handed to the
// engine for painting. Experimental software WebGL1 is in webgl.js;
// WebGL2 / WebGPU contexts remain unavailable.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('canvas');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);

  // -------------------------------------------------------------- colors
  const NAMED = { black: '000000', white: 'ffffff', red: 'ff0000', green: '008000', blue: '0000ff', yellow: 'ffff00', cyan: '00ffff', aqua: '00ffff', magenta: 'ff00ff', fuchsia: 'ff00ff', gray: '808080', grey: '808080', silver: 'c0c0c0', maroon: '800000', olive: '808000', lime: '00ff00', navy: '000080', purple: '800080', teal: '008080', orange: 'ffa500', pink: 'ffc0cb', brown: 'a52a2a', gold: 'ffd700', indigo: '4b0082', violet: 'ee82ee', coral: 'ff7f50', salmon: 'fa8072', tomato: 'ff6347', crimson: 'dc143c', darkgray: 'a9a9a9', lightgray: 'd3d3d3', darkblue: '00008b', darkred: '8b0000', darkgreen: '006400', lightblue: 'add8e6', lightgreen: '90ee90', beige: 'f5f5dc', khaki: 'f0e68c', turquoise: '40e0d0', chocolate: 'd2691e', tan: 'd2b48c', skyblue: '87ceeb', steelblue: '4682b4', slategray: '708090', whitesmoke: 'f5f5f5', ivory: 'fffff0', lavender: 'e6e6fa', plum: 'dda0dd', orchid: 'da70d6', royalblue: '4169e1', dodgerblue: '1e90ff', firebrick: 'b22222', seagreen: '2e8b57', forestgreen: '228b22', hotpink: 'ff69b4', deeppink: 'ff1493', midnightblue: '191970', darkorange: 'ff8c00', rebeccapurple: '663399' };
  function parseColor(str) {
    let s = String(str).trim().toLowerCase();
    if (s === 'transparent') return [0, 0, 0, 0];
    if (NAMED[s]) s = '#' + NAMED[s];
    let m;
    if ((m = /^#([0-9a-f]{3,8})$/.exec(s))) {
      let h = m[1];
      if (h.length === 3 || h.length === 4) h = h.split('').map((c) => c + c).join('');
      if (h.length !== 6 && h.length !== 8) return null;
      return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16), h.length === 8 ? parseInt(h.slice(6, 8), 16) / 255 : 1];
    }
    if ((m = /^rgba?\(([^)]*)\)$/.exec(s))) {
      const p = m[1].split(/[\s,/]+/).filter(Boolean);
      if (p.length < 3) return null;
      const ch = (v) => (v.endsWith('%') ? parseFloat(v) * 2.55 : parseFloat(v));
      const a = p[3] === undefined ? 1 : p[3].endsWith('%') ? parseFloat(p[3]) / 100 : parseFloat(p[3]);
      return [ch(p[0]), ch(p[1]), ch(p[2]), a].map((v, i) => (i < 3 ? Math.max(0, Math.min(255, Math.round(v))) : Math.max(0, Math.min(1, v))));
    }
    if ((m = /^hsla?\(([^)]*)\)$/.exec(s))) {
      const p = m[1].split(/[\s,/]+/).filter(Boolean);
      const h = (((parseFloat(p[0]) % 360) + 360) % 360) / 360, sat = parseFloat(p[1]) / 100, l = parseFloat(p[2]) / 100;
      const a = p[3] === undefined ? 1 : p[3].endsWith('%') ? parseFloat(p[3]) / 100 : parseFloat(p[3]);
      const q = l < 0.5 ? l * (1 + sat) : l + sat - l * sat, pp = 2 * l - q;
      const hue = (t) => { t = (t + 1) % 1; return t < 1 / 6 ? pp + (q - pp) * 6 * t : t < 1 / 2 ? q : t < 2 / 3 ? pp + (q - pp) * (2 / 3 - t) * 6 : pp; };
      return [hue(h + 1 / 3), hue(h), hue(h - 1 / 3)].map((v) => Math.round(v * 255)).concat([a]);
    }
    return null;
  }
  function serializeColor(c) {
    const hex = (v) => v.toString(16).padStart(2, '0');
    return c[3] === 1 ? `#${hex(c[0])}${hex(c[1])}${hex(c[2])}` : `rgba(${c[0]}, ${c[1]}, ${c[2]}, ${+c[3].toFixed(3)})`;
  }

  // ---------------------------------------------- gradients and patterns

  // ---------------------------------------------- gradients and patterns
  class CanvasGradient {
    constructor() { ILLEGAL(); }
    addColorStop(offset, color) {
      offset = +offset;
      if (!(offset >= 0 && offset <= 1)) throw domError(`Failed to execute 'addColorStop' on 'CanvasGradient': The provided value (${offset}) is outside the range (0.0, 1.0).`, 'IndexSizeError');
      const c = parseColor(color);
      if (!c) throw new SyntaxError(`Failed to execute 'addColorStop' on 'CanvasGradient': The value provided ('${color}') could not be parsed as a color.`);
      const stops = this[S].stops;
      let i = stops.length;
      while (i > 0 && stops[i - 1][0] > offset) i--;
      stops.splice(i, 0, [offset, c]);
    }
  }
  tag(CanvasGradient, 'CanvasGradient');
  class CanvasPattern {
    constructor() { ILLEGAL(); }
    setTransform(m = {}) { const x = g.DOMMatrix.fromMatrix(m); this[S].matrix = [x.a, x.b, x.c, x.d, x.e, x.f]; }
  }
  tag(CanvasPattern, 'CanvasPattern');
  const F64 = (a) => new Float64Array(a);
  function paintOf(style, solid) {
    if (style instanceof CanvasGradient) {
      const st = style[S];
      const stops = F64(st.stops.flatMap(([o, c]) => [o, c[0], c[1], c[2], c[3]]));
      return Object.assign({}, st, { kind: st.type, stops });
    }
    if (style instanceof CanvasPattern) {
      const st = style[S];
      return { kind: 'pattern', width: st.src.width, height: st.src.height, data: st.src.data, repetition: st.repetition, matrix: st.matrix ? F64(st.matrix) : null };
    }
    return { kind: 'solid', color: F64(solid) };
  }

  // ----------------------------------------------------------------- paths
  // A path records ops (0 move, 1 line, 2 quad, 3 cubic, 4 close) with
  // points mapped by `map` (the context transform for the current path,
  // identity for Path2D objects, which are in user space).
  const IDENT = [1, 0, 0, 1, 0, 0];
  function newPathState(map) { return { ops: [], map: map || null, last: null, start: null }; }
  function mapPt(st, x, y) { const m = st.map ? st.map() : IDENT; return [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]; }
  const ok = (...v) => v.every(Number.isFinite);
  const P = {
    moveTo(st, x, y) { if (!ok(x, y)) return; const p = mapPt(st, x, y); st.ops.push(0, p[0], p[1]); st.last = [x, y]; st.start = [x, y]; },
    lineTo(st, x, y) { if (!ok(x, y)) return; if (!st.last) { P.moveTo(st, x, y); return; } const p = mapPt(st, x, y); st.ops.push(1, p[0], p[1]); st.last = [x, y]; },
    quadraticCurveTo(st, cx, cy, x, y) { if (!ok(cx, cy, x, y)) return; if (!st.last) P.moveTo(st, cx, cy); const c = mapPt(st, cx, cy), p = mapPt(st, x, y); st.ops.push(2, c[0], c[1], p[0], p[1]); st.last = [x, y]; },
    bezierCurveTo(st, a, b, c, d, x, y) { if (!ok(a, b, c, d, x, y)) return; if (!st.last) P.moveTo(st, a, b); const c1 = mapPt(st, a, b), c2 = mapPt(st, c, d), p = mapPt(st, x, y); st.ops.push(3, c1[0], c1[1], c2[0], c2[1], p[0], p[1]); st.last = [x, y]; },
    closePath(st) { if (!st.last) return; st.ops.push(4); st.last = st.start; },
    ellipse(st, x, y, rx, ry, rot, a0, a1, ccw = false) {
      if (!ok(x, y, rx, ry, rot, a0, a1)) return;
      if (rx < 0 || ry < 0) throw domError(`The ${rx < 0 ? 'major' : 'minor'}-axis radius provided (${rx < 0 ? rx : ry}) is negative.`, 'IndexSizeError');
      const TAU = Math.PI * 2;
      let sweep = a1 - a0;
      if (!ccw && sweep >= TAU) sweep = TAU;
      else if (ccw && -sweep >= TAU) sweep = -TAU;
      else { if (!ccw && sweep < 0) sweep = (sweep % TAU) + TAU; if (ccw && sweep > 0) sweep = (sweep % TAU) - TAU; }
      const cr = Math.cos(rot), sr = Math.sin(rot);
      const pt = (a) => { const ex = rx * Math.cos(a), ey = ry * Math.sin(a); return [x + ex * cr - ey * sr, y + ex * sr + ey * cr]; };
      const d = (a) => { const ex = -rx * Math.sin(a), ey = ry * Math.cos(a); return [ex * cr - ey * sr, ex * sr + ey * cr]; };
      const p0 = pt(a0);
      if (st.last) P.lineTo(st, p0[0], p0[1]); else P.moveTo(st, p0[0], p0[1]);
      const n = Math.max(1, Math.ceil(Math.abs(sweep) / (Math.PI / 2)));
      const step = sweep / n, k = (4 / 3) * Math.tan(step / 4);
      for (let i = 0; i < n; i++) {
        const s = a0 + step * i, e = s + step;
        const ps = pt(s), pe = pt(e), ds = d(s), de = d(e);
        P.bezierCurveTo(st, ps[0] + k * ds[0], ps[1] + k * ds[1], pe[0] - k * de[0], pe[1] - k * de[1], pe[0], pe[1]);
      }
    },
    arc(st, x, y, r, a0, a1, ccw = false) { if (r < 0) throw domError(`The radius provided (${r}) is negative.`, 'IndexSizeError'); P.ellipse(st, x, y, r, r, 0, a0, a1, ccw); },
    arcTo(st, x1, y1, x2, y2, r) {
      if (!ok(x1, y1, x2, y2, r)) return;
      if (r < 0) throw domError(`The radius provided (${r}) is negative.`, 'IndexSizeError');
      if (!st.last) P.moveTo(st, x1, y1);
      const [x0, y0] = st.last;
      const v1 = [x0 - x1, y0 - y1], v2 = [x2 - x1, y2 - y1];
      const l1 = Math.hypot(...v1), l2 = Math.hypot(...v2);
      const cross = v1[0] * v2[1] - v1[1] * v2[0];
      if (!l1 || !l2 || !r || Math.abs(cross) < 1e-9) { P.lineTo(st, x1, y1); return; }
      const ang = Math.acos(Math.max(-1, Math.min(1, (v1[0] * v2[0] + v1[1] * v2[1]) / (l1 * l2))));
      const dd = r / Math.tan(ang / 2);
      const t1 = [x1 + (v1[0] / l1) * dd, y1 + (v1[1] / l1) * dd], t2 = [x1 + (v2[0] / l2) * dd, y1 + (v2[1] / l2) * dd];
      const bis = [v1[0] / l1 + v2[0] / l2, v1[1] / l1 + v2[1] / l2], bl = Math.hypot(...bis);
      const dc = r / Math.sin(ang / 2);
      const c = [x1 + (bis[0] / bl) * dc, y1 + (bis[1] / bl) * dc];
      P.lineTo(st, t1[0], t1[1]);
      P.ellipse(st, c[0], c[1], r, r, 0, Math.atan2(t1[1] - c[1], t1[0] - c[0]), Math.atan2(t2[1] - c[1], t2[0] - c[0]), cross > 0);
    },
    rect(st, x, y, w, h) { if (!ok(x, y, w, h)) return; P.moveTo(st, x, y); P.lineTo(st, x + w, y); P.lineTo(st, x + w, y + h); P.lineTo(st, x, y + h); P.closePath(st); },
    roundRect(st, x, y, w, h, radii = 0) {
      const list = Array.isArray(radii) ? radii : [radii];
      if (list.length < 1 || list.length > 4) throw new RangeError(`Failed to execute 'roundRect': ${list.length} radii provided. Between one and four radii are necessary.`);
      const r = list.map((v) => (typeof v === 'object' ? Math.max(v.x || 0, v.y || 0) : +v));
      if (r.some((v) => v < 0)) throw new RangeError("Failed to execute 'roundRect': Radius value is negative.");
      let [tl, tr, br, bl] = r.length === 1 ? [r[0], r[0], r[0], r[0]] : r.length === 2 ? [r[0], r[1], r[0], r[1]] : r.length === 3 ? [r[0], r[1], r[2], r[1]] : r;
      const f = Math.min(1, Math.abs(w) / (tl + tr || Infinity), Math.abs(w) / (bl + br || Infinity), Math.abs(h) / (tl + bl || Infinity), Math.abs(h) / (tr + br || Infinity));
      tl *= f; tr *= f; br *= f; bl *= f;
      P.moveTo(st, x + tl, y);
      P.lineTo(st, x + w - tr, y); if (tr) P.ellipse(st, x + w - tr, y + tr, tr, tr, 0, -Math.PI / 2, 0);
      P.lineTo(st, x + w, y + h - br); if (br) P.ellipse(st, x + w - br, y + h - br, br, br, 0, 0, Math.PI / 2);
      P.lineTo(st, x + bl, y + h); if (bl) P.ellipse(st, x + bl, y + h - bl, bl, bl, 0, Math.PI / 2, Math.PI);
      P.lineTo(st, x, y + tl); if (tl) P.ellipse(st, x + tl, y + tl, tl, tl, 0, Math.PI, Math.PI * 1.5);
      P.closePath(st);
      P.moveTo(st, x, y);
    },
  };
  class Path2D {
    constructor(arg) {
      const p = Object.create(new.target ? new.target.prototype : Path2D.prototype);
      hidden(p, S, newPathState(null));
      if (arg instanceof Path2D) { p[S].ops = arg[S].ops.slice(); p[S].last = arg[S].last; p[S].start = arg[S].start; }
      else if (typeof arg === 'string') parseSvgPath(p[S], arg);
      return p;
    }
    addPath(path, m) {
      const mat = m ? g.DOMMatrix.fromMatrix(m) : null;
      const t = mat ? [mat.a, mat.b, mat.c, mat.d, mat.e, mat.f] : IDENT;
      this[S].ops.push(...transformOps(path[S].ops, t));
    }
  }
  for (const k of Object.keys(P)) hidden(Path2D.prototype, k, function (...a) { return P[k](this[S], ...a); });
  tag(Path2D, 'Path2D');
  function transformOps(ops, m) {
    const out = [];
    for (let i = 0; i < ops.length;) {
      const op = ops[i];
      const n = op === 0 || op === 1 ? 1 : op === 2 ? 2 : op === 3 ? 3 : 0;
      out.push(op);
      for (let j = 0; j < n; j++) { const x = ops[i + 1 + 2 * j], y = ops[i + 2 + 2 * j]; out.push(m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]); }
      i += 1 + 2 * n;
    }
    return out;
  }
  function parseSvgPath(st, d) {
    const toks = String(d).match(/[MmLlHhVvCcSsQqTtAaZz]|-?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?/g) || [];
    let i = 0, cmd = '', cx = 0, cy = 0, sx = 0, sy = 0, lcx = 0, lcy = 0, lastCmd = '';
    const num = () => parseFloat(toks[i++]);
    while (i < toks.length) {
      if (/[A-Za-z]/.test(toks[i])) cmd = toks[i++];
      const rel = cmd === cmd.toLowerCase(), C = cmd.toUpperCase();
      const ox = rel ? cx : 0, oy = rel ? cy : 0;
      switch (C) {
        case 'M': cx = ox + num(); cy = oy + num(); P.moveTo(st, cx, cy); sx = cx; sy = cy; cmd = rel ? 'l' : 'L'; break;
        case 'L': cx = ox + num(); cy = oy + num(); P.lineTo(st, cx, cy); break;
        case 'H': cx = (rel ? cx : 0) + num(); P.lineTo(st, cx, cy); break;
        case 'V': cy = (rel ? cy : 0) + num(); P.lineTo(st, cx, cy); break;
        case 'C': { const a = ox + num(), b = oy + num(), c = ox + num(), e = oy + num(); cx = ox + num(); cy = oy + num(); P.bezierCurveTo(st, a, b, c, e, cx, cy); lcx = c; lcy = e; break; }
        case 'S': { const a = /[CS]/.test(lastCmd) ? 2 * cx - lcx : cx, b = /[CS]/.test(lastCmd) ? 2 * cy - lcy : cy; const c = ox + num(), e = oy + num(); cx = ox + num(); cy = oy + num(); P.bezierCurveTo(st, a, b, c, e, cx, cy); lcx = c; lcy = e; break; }
        case 'Q': { const a = ox + num(), b = oy + num(); cx = ox + num(); cy = oy + num(); P.quadraticCurveTo(st, a, b, cx, cy); lcx = a; lcy = b; break; }
        case 'T': { const a = /[QT]/.test(lastCmd) ? 2 * cx - lcx : cx, b = /[QT]/.test(lastCmd) ? 2 * cy - lcy : cy; cx = ox + num(); cy = oy + num(); P.quadraticCurveTo(st, a, b, cx, cy); lcx = a; lcy = b; break; }
        case 'A': { const rx = num(), ry = num(), rot = num(), large = num(), sweep = num(); const x = ox + num(), y = oy + num(); svgArc(st, cx, cy, rx, ry, rot, large, sweep, x, y); cx = x; cy = y; break; }
        case 'Z': P.closePath(st); cx = sx; cy = sy; break;
        default: i++;
      }
      lastCmd = C;
      if (Number.isNaN(cx) || Number.isNaN(cy)) break;
    }
  }
  function svgArc(st, x1, y1, rx, ry, phiDeg, fa, fs, x2, y2) {
    if (!rx || !ry) { P.lineTo(st, x2, y2); return; }
    rx = Math.abs(rx); ry = Math.abs(ry);
    const phi = (phiDeg * Math.PI) / 180, cp = Math.cos(phi), sp = Math.sin(phi);
    const dx = (x1 - x2) / 2, dy = (y1 - y2) / 2;
    const x1p = cp * dx + sp * dy, y1p = -sp * dx + cp * dy;
    const lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if (lam > 1) { rx *= Math.sqrt(lam); ry *= Math.sqrt(lam); }
    const sign = fa === fs ? -1 : 1;
    const nm = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    const co = sign * Math.sqrt(Math.max(0, nm / (rx * rx * y1p * y1p + ry * ry * x1p * x1p)));
    const cxp = (co * rx * y1p) / ry, cyp = (-co * ry * x1p) / rx;
    const cx = cp * cxp - sp * cyp + (x1 + x2) / 2, cy = sp * cxp + cp * cyp + (y1 + y2) / 2;
    const ang = (ux, uy, vx, vy) => Math.atan2(ux * vy - uy * vx, ux * vx + uy * vy);
    const t1 = ang(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let dt = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry);
    if (!fs && dt > 0) dt -= 2 * Math.PI; else if (fs && dt < 0) dt += 2 * Math.PI;
    P.ellipse(st, cx, cy, rx, ry, phi, t1, t1 + dt, !fs);
  }
  // Point-in-path (device space) by flattening curves.
  function flattenOps(ops) {
    const polys = [];
    let cur = null, last = [0, 0], start = [0, 0];
    for (let i = 0; i < ops.length;) {
      const op = ops[i];
      if (op === 0) { cur = [[ops[i + 1], ops[i + 2]]]; polys.push(cur); last = start = cur[0]; i += 3; }
      else if (op === 1) { if (!cur) { cur = [last]; polys.push(cur); } last = [ops[i + 1], ops[i + 2]]; cur.push(last); i += 3; }
      else if (op === 2 || op === 3) {
        const n = op === 2 ? 2 : 3;
        const pts = [last]; for (let j = 0; j < n; j++) pts.push([ops[i + 1 + 2 * j], ops[i + 2 + 2 * j]]);
        if (!cur) { cur = [last]; polys.push(cur); }
        for (let s = 1; s <= 12; s++) {
          const t = s / 12, u = 1 - t;
          cur.push(n === 2 ? [u * u * pts[0][0] + 2 * u * t * pts[1][0] + t * t * pts[2][0], u * u * pts[0][1] + 2 * u * t * pts[1][1] + t * t * pts[2][1]]
            : [u * u * u * pts[0][0] + 3 * u * u * t * pts[1][0] + 3 * u * t * t * pts[2][0] + t * t * t * pts[3][0], u * u * u * pts[0][1] + 3 * u * u * t * pts[1][1] + 3 * u * t * t * pts[2][1] + t * t * t * pts[3][1]]);
        }
        last = pts[n];
        i += 1 + 2 * n;
      } else { if (cur) { cur = null; last = start; } i += 1; }
    }
    return polys;
  }
  function pointInPolys(polys, x, y, evenOdd) {
    let wind = 0;
    for (const pts of polys) for (let i = 0; i < pts.length; i++) {
      const a = pts[i], b = pts[(i + 1) % pts.length];
      if ((a[1] <= y) !== (b[1] <= y)) {
        const xi = a[0] + ((y - a[1]) * (b[0] - a[0])) / (b[1] - a[1]);
        if (xi > x) wind += evenOdd ? 1 : a[1] < b[1] ? 1 : -1;
      }
    }
    return evenOdd ? wind % 2 === 1 : wind !== 0;
  }
  function nearPolyline(polys, x, y, hw) {
    for (const pts of polys) for (let i = 0; i < pts.length - 1; i++) {
      const [ax, ay] = pts[i], [bx, by] = pts[i + 1];
      const dx = bx - ax, dy = by - ay, len = dx * dx + dy * dy;
      const t = len ? Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / len)) : 0;
      if (Math.hypot(x - (ax + t * dx), y - (ay + t * dy)) <= hw) return true;
    }
    return false;
  }

  // SVG geometry (svg.js): path data → flattened subpaths with a closed flag.
  hidden(N, 'svgPathPolys', (d) => {
    const ops = new Path2D(String(d))[S].ops;
    const out = [];
    let cur = null, last = [0, 0];
    for (let i = 0; i < ops.length;) {
      const op = ops[i];
      if (op === 0) { cur = { pts: [[ops[i + 1], ops[i + 2]]], closed: false }; out.push(cur); last = cur.pts[0]; i += 3; }
      else if (op === 1) { if (!cur) { cur = { pts: [last], closed: false }; out.push(cur); } last = [ops[i + 1], ops[i + 2]]; cur.pts.push(last); i += 3; }
      else if (op === 2 || op === 3) {
        const n = op === 2 ? 2 : 3;
        const p = [last]; for (let j = 0; j < n; j++) p.push([ops[i + 1 + 2 * j], ops[i + 2 + 2 * j]]);
        if (!cur) { cur = { pts: [last], closed: false }; out.push(cur); }
        for (let s = 1; s <= 32; s++) {
          const t = s / 32, u = 1 - t;
          cur.pts.push(n === 2 ? [u * u * p[0][0] + 2 * u * t * p[1][0] + t * t * p[2][0], u * u * p[0][1] + 2 * u * t * p[1][1] + t * t * p[2][1]]
            : [u * u * u * p[0][0] + 3 * u * u * t * p[1][0] + 3 * u * t * t * p[2][0] + t * t * t * p[3][0], u * u * u * p[0][1] + 3 * u * u * t * p[1][1] + 3 * u * t * t * p[2][1] + t * t * t * p[3][1]]);
        }
        last = p[n];
        i += 1 + 2 * n;
      } else { if (cur) { cur.closed = true; last = cur.pts[0]; cur = null; } i += 1; }
    }
    return out;
  });
  hidden(N, 'measureText', (text, font) => N.canvasMeasureText(String(text), String(font)));

  // ------------------------------------------------------------- ImageData
  class ImageData {
    constructor(a, b, c, d) {
      let data, w, h, settings;
      if (a instanceof Uint8ClampedArray) {
        data = a; w = b >>> 0;
        if (!w || data.length % (4 * w)) throw domError("Failed to construct 'ImageData': The input data length is not a multiple of (4 * width).", 'IndexSizeError');
        h = c === undefined ? data.length / 4 / w : c >>> 0;
        if (data.length !== w * h * 4) throw domError("Failed to construct 'ImageData': The input data length is not equal to (4 * width * height).", 'IndexSizeError');
        settings = d;
      } else {
        w = a >>> 0; h = b >>> 0; settings = c;
        if (!w || !h) throw domError(`Failed to construct 'ImageData': The source ${!w ? 'width' : 'height'} is zero or not a number.`, 'IndexSizeError');
        data = new Uint8ClampedArray(w * h * 4);
      }
      const o = Object.create(new.target ? new.target.prototype : ImageData.prototype);
      hidden(o, S, { data, width: w, height: h, colorSpace: (settings && settings.colorSpace) || 'srgb', pixelFormat: (settings && settings.pixelFormat) || 'rgba-unorm8' });
      return o;
    }
    get width() { return this[S].width; }
    get height() { return this[S].height; }
    get data() { return this[S].data; }
    get colorSpace() { return this[S].colorSpace; }
    get pixelFormat() { return this[S].pixelFormat; }
  }
  tag(ImageData, 'ImageData');

  // ------------------------------------------------------------- surfaces
  // A surface: { id (native), width, height, owner, ctx }.
  function newSurface(w, h, owner) { return { id: N.canvasNew(w, h), width: w, height: h, owner, ctx: null }; }
  const surfaces = new WeakMap();
  function surfaceOf(canvas) {
    let s = surfaces.get(canvas);
    const w = canvas.width >>> 0, h = canvas.height >>> 0;
    if (!s) { s = newSurface(w, h, canvas); surfaces.set(canvas, s); }
    else if (s.width !== w || s.height !== h) {
      // Resizing a canvas clears it and resets the context.
      s.width = w; s.height = h;
      if (s.mode === 'webgl') N.resizeWebGLSurface(s, w, h);
      else {
        N.canvasResize(s.id, w, h);
        if (s.ctx) resetState(s.ctx);
      }
      markDirty(s);
    }
    return s;
  }
  const pixelsOf = (s) => (!s.lost && s.width && s.height ? new Uint8ClampedArray(N.canvasGetImageData(s.id, 0, 0, s.width, s.height)) : new Uint8ClampedArray(0));
  const pendingFlush = new Set();
  let flushQueued = false;
  function markDirty(s) {
    if (!s.owner || !(s.owner instanceof g.HTMLCanvasElement)) return;
    pendingFlush.add(s);
    if (!flushQueued) { flushQueued = true; queueMicrotask(flushCanvases); }
  }
  function flushCanvases() {
    flushQueued = false;
    for (const s of pendingFlush) if (s.owner.isConnected) {
      N.canvasPresent(s.id, s.owner);
      if (s.mode === 'webgl') N.webglSurfacePresented(s);
    }
    pendingFlush.clear();
  }

  // Internal shared surface hooks: WebGL presents actual pixels through
  // the same canvas pipeline, including drawImage and PNG/JPEG encoding.
  hidden(N, 'canvasMarkDirty', markDirty);
  hidden(N, 'canvasSurface', surfaceOf);

  // ------------------------------------------------ CanvasRenderingContext2D
  const DEFAULT_STATE = () => ({ fillStyle: '#000000', fill: [0, 0, 0, 1], strokeStyle: '#000000', stroke: [0, 0, 0, 1], globalAlpha: 1, globalCompositeOperation: 'source-over', lineWidth: 1, lineCap: 'butt', lineJoin: 'miter', miterLimit: 10, lineDash: [], lineDashOffset: 0, font: '10px sans-serif', textAlign: 'start', textBaseline: 'alphabetic', direction: 'inherit', letterSpacing: '0px', wordSpacing: '0px', fontKerning: 'auto', fontStretch: 'normal', fontVariantCaps: 'normal', textRendering: 'auto', shadowBlur: 0, shadowColor: 'rgba(0, 0, 0, 0)', shadow: [0, 0, 0, 0], shadowOffsetX: 0, shadowOffsetY: 0, filter: 'none', imageSmoothingEnabled: true, imageSmoothingQuality: 'low', lang: 'inherit', m: [1, 0, 0, 1, 0, 0], clip: null });
  function resetState(ctx) { ctx[S].state = DEFAULT_STATE(); ctx[S].stack = []; ctx[S].path = newPathState(() => ctx[S].state.m); }
  const surf = (ctx) => (ctx[S].canvas instanceof g.HTMLCanvasElement ? surfaceOf(ctx[S].canvas) : ctx[S].surface);
  function params(ctx) {
    const st = ctx[S].state;
    return { alpha: st.globalAlpha, op: st.globalCompositeOperation, clip: st.clip, shadowColor: st.shadow[3] > 0 ? F64(st.shadow) : null, shadowBlur: st.shadowBlur, shadowOffsetX: st.shadowOffsetX, shadowOffsetY: st.shadowOffsetY };
  }
  // Ops of the current path or of a Path2D (user space → device).
  const opsOf = (ctx, path) => F64(path ? transformOps(path[S].ops, ctx[S].state.m) : ctx[S].path.ops);
  function strokeSpec(st) {
    return { width: st.lineWidth, cap: st.lineCap, join: st.lineJoin, miter: st.miterLimit, dashes: st.lineDash.length ? F64(st.lineDash) : null, dashOffset: st.lineDashOffset };
  }
  class CanvasRenderingContext2D {
    constructor() { ILLEGAL(); }
    get canvas() { return this[S].canvas; }
    getContextAttributes() { return Object.assign({}, this[S].attrs); }
    save() { const st = this[S].state; this[S].stack.push(Object.assign({}, st, { m: st.m.slice(), lineDash: st.lineDash.slice() })); }
    restore() {
      const s = this[S].stack.pop();
      if (!s) return;
      const old = this[S].state.clip;
      this[S].state = s;
      releaseClip(this, old);
    }
    reset() { const sf = surf(this); N.canvasResize(sf.id, sf.width, sf.height); resetState(this); markDirty(sf); }
    isContextLost() { return false; }
    scale(x, y) { this.transform(x, 0, 0, y, 0, 0); }
    rotate(a) { const c = Math.cos(a), s = Math.sin(a); this.transform(c, s, -s, c, 0, 0); }
    translate(x, y) { this.transform(1, 0, 0, 1, x, y); }
    transform(a, b, c, d, e, f) {
      if (!ok(a, b, c, d, e, f)) return;
      const m = this[S].state.m;
      this[S].state.m = [m[0] * a + m[2] * b, m[1] * a + m[3] * b, m[0] * c + m[2] * d, m[1] * c + m[3] * d, m[0] * e + m[2] * f + m[4], m[1] * e + m[3] * f + m[5]];
    }
    getTransform() { const m = this[S].state.m; return new g.DOMMatrix([m[0], m[1], m[2], m[3], m[4], m[5]]); }
    setTransform(a, b, c, d, e, f) {
      if (typeof a === 'object' || a === undefined) { const mm = g.DOMMatrix.fromMatrix(a || {}); this[S].state.m = [mm.a, mm.b, mm.c, mm.d, mm.e, mm.f]; return; }
      if (!ok(a, b, c, d, e, f)) return;
      this[S].state.m = [a, b, c, d, e, f];
    }
    resetTransform() { this[S].state.m = [1, 0, 0, 1, 0, 0]; }
    beginPath() { this[S].path = newPathState(() => this[S].state.m); }
    fill(a, b) {
      const [path, rule] = a instanceof Path2D ? [a, b] : [null, a];
      const st = this[S].state, sf = surf(this);
      N.canvasFill(sf.id, opsOf(this, path), rule === 'evenodd' ? 'evenodd' : 'nonzero', paintOf(st.fillStyle, st.fill), F64(st.m), params(this));
      markDirty(sf);
    }
    stroke(path) {
      const st = this[S].state, sf = surf(this);
      N.canvasStroke(sf.id, opsOf(this, path instanceof Path2D ? path : null), strokeSpec(st), paintOf(st.strokeStyle, st.stroke), F64(st.m), params(this));
      markDirty(sf);
    }
    clip(a, b) {
      const [path, rule] = a instanceof Path2D ? [a, b] : [null, a];
      const sf = surf(this);
      const old = this[S].state.clip;
      this[S].state.clip = N.canvasClip(sf.id, opsOf(this, path), rule === 'evenodd' ? 'evenodd' : 'nonzero', old);
      releaseClip(this, old);
    }
    isPointInPath(a, b, c, d) {
      const [path, x, y, rule] = a instanceof Path2D ? [a, b, c, d] : [null, a, b, c];
      return pointInPolys(flattenOps(Array.from(opsOf(this, path))), +x, +y, rule === 'evenodd');
    }
    isPointInStroke(a, b, c) {
      const [path, x, y] = a instanceof Path2D ? [a, b, c] : [null, a, b];
      const st = this[S].state;
      const scale = Math.sqrt(Math.abs(st.m[0] * st.m[3] - st.m[1] * st.m[2])) || 1;
      return nearPolyline(flattenOps(Array.from(opsOf(this, path))), +x, +y, (st.lineWidth * scale) / 2);
    }
    fillRect(x, y, w, h) {
      if (!ok(x, y, w, h)) return;
      const p = newPathState(() => this[S].state.m); P.rect(p, +x, +y, +w, +h);
      const st = this[S].state, sf = surf(this);
      N.canvasFill(sf.id, F64(p.ops), 'nonzero', paintOf(st.fillStyle, st.fill), F64(st.m), params(this));
      markDirty(sf);
    }
    strokeRect(x, y, w, h) {
      if (!ok(x, y, w, h)) return;
      const p = newPathState(() => this[S].state.m); P.rect(p, +x, +y, +w, +h);
      const st = this[S].state, sf = surf(this);
      N.canvasStroke(sf.id, F64(p.ops), strokeSpec(st), paintOf(st.strokeStyle, st.stroke), F64(st.m), params(this));
      markDirty(sf);
    }
    clearRect(x, y, w, h) {
      if (!ok(x, y, w, h)) return;
      const p = newPathState(() => this[S].state.m); P.rect(p, +x, +y, +w, +h);
      const sf = surf(this);
      N.canvasClear(sf.id, F64(p.ops), { clip: this[S].state.clip });
      markDirty(sf);
    }
    createImageData(a, b, settings) { return a instanceof ImageData ? new ImageData(a.width, a.height) : new ImageData(Math.abs(a >> 0), Math.abs(b >> 0), settings); }
    getImageData(sx, sy, sw, sh, settings) {
      sx = sx | 0; sy = sy | 0; sw = sw | 0; sh = sh | 0;
      if (!sw || !sh) throw domError(`Failed to execute 'getImageData' on 'CanvasRenderingContext2D': The source ${!sw ? 'width' : 'height'} is 0.`, 'IndexSizeError');
      if (sw < 0) { sx += sw; sw = -sw; }
      if (sh < 0) { sy += sh; sh = -sh; }
      return new ImageData(new Uint8ClampedArray(N.canvasGetImageData(surf(this).id, sx, sy, sw, sh)), sw, sh, settings);
    }
    putImageData(img, dx, dy, dirtyX = 0, dirtyY = 0, dirtyW = img.width, dirtyH = img.height) {
      if (!(img instanceof ImageData)) throw new TypeError("Failed to execute 'putImageData' on 'CanvasRenderingContext2D': parameter 1 is not of type 'ImageData'.");
      if (dirtyW < 0) { dirtyX += dirtyW; dirtyW = -dirtyW; }
      if (dirtyH < 0) { dirtyY += dirtyH; dirtyH = -dirtyH; }
      const sf = surf(this);
      N.canvasPutImageData(sf.id, img.data, img.width, img.height, dx | 0, dy | 0, dirtyX | 0, dirtyY | 0, dirtyW | 0, dirtyH | 0);
      markDirty(sf);
    }
    drawImage(image, ...a) {
      const src = sourcePixels(image);
      if (!src) return;
      let sx = 0, sy = 0, sw = src.width, sh = src.height, dx, dy, dw, dh;
      if (a.length === 2) { [dx, dy] = a; dw = sw; dh = sh; }
      else if (a.length === 4) { [dx, dy, dw, dh] = a; }
      else if (a.length === 8) { [sx, sy, sw, sh, dx, dy, dw, dh] = a; }
      else throw new TypeError(`Failed to execute 'drawImage' on 'CanvasRenderingContext2D': Valid arities are: [3, 5, 9], but ${a.length + 1} arguments provided.`);
      if (sw < 0) { sx += sw; sw = -sw; }
      if (sh < 0) { sy += sh; sh = -sh; }
      if (dw < 0) { dx += dw; dw = -dw; }
      if (dh < 0) { dy += dh; dh = -dh; }
      if (!sw || !sh || !dw || !dh || !ok(sx, sy, sw, sh, dx, dy, dw, dh)) return;
      const st = this[S].state, sf = surf(this);
      N.canvasDrawImage(sf.id, src.data, src.width, src.height, F64([sx, sy, sw, sh]), F64([dx, dy, dw, dh]), F64(st.m), params(this), st.imageSmoothingEnabled);
      markDirty(sf);
    }
    createLinearGradient(x0, y0, x1, y1) { if (!ok(x0, y0, x1, y1)) throw new TypeError("Failed to execute 'createLinearGradient': The provided double value is non-finite."); const gr = Object.create(CanvasGradient.prototype); hidden(gr, S, { type: 'linear', x0: +x0, y0: +y0, x1: +x1, y1: +y1, stops: [] }); return gr; }
    createRadialGradient(x0, y0, r0, x1, y1, r1) { if (r0 < 0 || r1 < 0) throw domError(`The ${r0 < 0 ? 'r0' : 'r1'} provided is less than 0.`, 'IndexSizeError'); const gr = Object.create(CanvasGradient.prototype); hidden(gr, S, { type: 'radial', x0: +x0, y0: +y0, r0: +r0, x1: +x1, y1: +y1, r1: +r1, stops: [] }); return gr; }
    createConicGradient(angle, x, y) { const gr = Object.create(CanvasGradient.prototype); hidden(gr, S, { type: 'conic', angle: +angle, x0: +x, y0: +y, stops: [] }); return gr; }
    createPattern(image, repetition) {
      const src = sourcePixels(image);
      if (!src) return null;
      repetition = repetition === null || repetition === '' ? 'repeat' : String(repetition);
      if (!['repeat', 'repeat-x', 'repeat-y', 'no-repeat'].includes(repetition)) throw new SyntaxError(`Failed to execute 'createPattern': The provided type ('${repetition}') is not one of 'repeat', 'no-repeat', 'repeat-x', or 'repeat-y'.`);
      const p = Object.create(CanvasPattern.prototype);
      hidden(p, S, { src: { width: src.width, height: src.height, data: new Uint8ClampedArray(src.data) }, repetition, matrix: null });
      return p;
    }
    fillText(text, x, y, maxWidth) { drawText(this, text, x, y, maxWidth, false); }
    strokeText(text, x, y, maxWidth) { drawText(this, text, x, y, maxWidth, true); }
    measureText(text) {
      const r = N.canvasMeasureText(String(text), this[S].state.font);
      const tm = Object.create(TextMetrics.prototype);
      const align = alignOffset(this[S].state, r.width);
      const base = baselineOffset(this[S].state.textBaseline, r);
      hidden(tm, S, { width: r.width, actualBoundingBoxLeft: align, actualBoundingBoxRight: r.width - align, fontBoundingBoxAscent: r.fontAscent - base, fontBoundingBoxDescent: r.fontDescent + base, actualBoundingBoxAscent: r.ascent - base, actualBoundingBoxDescent: r.descent + base, emHeightAscent: r.fontAscent - base, emHeightDescent: r.fontDescent + base, hangingBaseline: r.fontAscent * 0.8 - base, alphabeticBaseline: -base, ideographicBaseline: -r.fontDescent - base });
      return tm;
    }
    setLineDash(segments) { const s = Array.from(segments).map(Number); if (s.some((v) => !(v >= 0) || !isFinite(v))) return; this[S].state.lineDash = s.length % 2 ? s.concat(s) : s; }
    getLineDash() { return this[S].state.lineDash.slice(); }
    drawFocusIfNeeded() {}
    scrollPathIntoView() {}
  }
  tag(CanvasRenderingContext2D, 'CanvasRenderingContext2D');
  for (const k of Object.keys(P)) hidden(CanvasRenderingContext2D.prototype, k, function (...a) { return P[k](this[S].path, ...a); });
  // Free a clip mask once neither the state nor the save stack uses it.
  function releaseClip(ctx, id) {
    if (id === null || ctx[S].state.clip === id || ctx[S].stack.some((s) => s.clip === id)) return;
    N.canvasFreeClip(surf(ctx).id, id);
  }
  // Horizontal offset of the text start from x for textAlign.
  function alignOffset(st, width) {
    const rtl = st.direction === 'rtl';
    switch (st.textAlign) {
      case 'center': return width / 2;
      case 'right': return width;
      case 'end': return rtl ? 0 : width;
      case 'start': return rtl ? width : 0;
      default: return 0;
    }
  }
  // Distance from the requested baseline to the alphabetic one (added to y).
  function baselineOffset(baseline, r) {
    switch (baseline) {
      case 'top': return r.fontAscent;
      case 'hanging': return r.fontAscent * 0.8;
      case 'middle': return (r.fontAscent - r.fontDescent) / 2;
      case 'ideographic': case 'bottom': return -r.fontDescent;
      default: return 0;
    }
  }
  function drawText(ctx, text, x, y, maxWidth, stroke) {
    text = String(text).replace(/[\t\n\f\r]/g, ' ');
    if (!ok(x, y)) return;
    const st = ctx[S].state, sf = surf(ctx);
    const r = N.canvasMeasureText(text, st.font);
    let m = st.m;
    if (maxWidth !== undefined && r.width > maxWidth && maxWidth > 0) {
      // Condense horizontally to fit.
      const f = maxWidth / r.width;
      m = [m[0] * f, m[1] * f, m[2], m[3], m[4] + (1 - f) * (m[0] * x), m[5] + (1 - f) * (m[1] * x)];
    }
    const tx = x - alignOffset(st, r.width), ty = y + baselineOffset(st.textBaseline, r);
    const paint = stroke ? paintOf(st.strokeStyle, st.stroke) : paintOf(st.fillStyle, st.fill);
    N.canvasFillText(sf.id, text, tx, ty, st.font, paint, F64(m), params(ctx), stroke ? st.lineWidth : 0);
    markDirty(sf);
  }
  const STYLE_PROPS = ['globalAlpha', 'globalCompositeOperation', 'lineWidth', 'lineCap', 'lineJoin', 'miterLimit', 'lineDashOffset', 'font', 'textAlign', 'textBaseline', 'direction', 'letterSpacing', 'wordSpacing', 'fontKerning', 'fontStretch', 'fontVariantCaps', 'textRendering', 'shadowBlur', 'shadowColor', 'shadowOffsetX', 'shadowOffsetY', 'filter', 'imageSmoothingEnabled', 'imageSmoothingQuality', 'lang'];
  const ENUMS = { lineCap: ['butt', 'round', 'square'], lineJoin: ['round', 'bevel', 'miter'], textAlign: ['start', 'end', 'left', 'right', 'center'], textBaseline: ['top', 'hanging', 'middle', 'alphabetic', 'ideographic', 'bottom'], direction: ['ltr', 'rtl', 'inherit'], globalCompositeOperation: ['source-over', 'source-in', 'source-out', 'source-atop', 'destination-over', 'destination-in', 'destination-out', 'destination-atop', 'lighter', 'copy', 'xor', 'multiply', 'screen', 'overlay', 'darken', 'lighten', 'color-dodge', 'color-burn', 'hard-light', 'soft-light', 'difference', 'exclusion', 'hue', 'saturation', 'color', 'luminosity'], imageSmoothingQuality: ['low', 'medium', 'high'], fontKerning: ['auto', 'normal', 'none'], fontStretch: ['ultra-condensed', 'extra-condensed', 'condensed', 'semi-condensed', 'normal', 'semi-expanded', 'expanded', 'extra-expanded', 'ultra-expanded'], fontVariantCaps: ['normal', 'small-caps', 'all-small-caps', 'petite-caps', 'all-petite-caps', 'unicase', 'titling-caps'], textRendering: ['auto', 'optimizeSpeed', 'optimizeLegibility', 'geometricPrecision'] };
  for (const k of STYLE_PROPS) {
    Object.defineProperty(CanvasRenderingContext2D.prototype, k, {
      get() { return this[S].state[k]; },
      set(v) {
        const st = this[S].state;
        if (ENUMS[k]) { if (ENUMS[k].includes(String(v))) st[k] = String(v); return; }
        if (k === 'globalAlpha') { v = +v; if (v >= 0 && v <= 1) st[k] = v; return; }
        if (k === 'lineWidth' || k === 'miterLimit') { v = +v; if (v > 0 && isFinite(v)) st[k] = v; return; }
        if (k === 'shadowBlur') { v = +v; if (v >= 0 && isFinite(v)) st[k] = v; return; }
        if (k === 'lineDashOffset' || k === 'shadowOffsetX' || k === 'shadowOffsetY') { v = +v; if (isFinite(v)) st[k] = v; return; }
        if (k === 'imageSmoothingEnabled') { st[k] = !!v; return; }
        if (k === 'shadowColor') { const c = parseColor(v); if (c) { st[k] = serializeColor(c); st.shadow = c; } return; }
        if (k === 'font') { if (/\d/.test(String(v))) st[k] = String(v).trim(); return; }
        st[k] = String(v);
      },
      configurable: true, enumerable: true,
    });
  }
  for (const [k, slot] of [['fillStyle', 'fill'], ['strokeStyle', 'stroke']]) {
    Object.defineProperty(CanvasRenderingContext2D.prototype, k, {
      get() { return this[S].state[k]; },
      set(v) {
        const st = this[S].state;
        if (v instanceof CanvasGradient || v instanceof CanvasPattern) { st[k] = v; return; }
        const c = parseColor(v);
        if (c) { st[k] = serializeColor(c); st[slot] = c; }
      },
      configurable: true, enumerable: true,
    });
  }
  // Pixels of a CanvasImageSource: { width, height, data }.
  function sourcePixels(image) {
    if (image instanceof g.HTMLCanvasElement) { const s = surfaceOf(image); if (s.lost) throw domError('The WebGL drawing buffer is lost.', 'InvalidStateError'); return s.width && s.height ? { width: s.width, height: s.height, data: pixelsOf(s) } : null; }
    if (image instanceof OffscreenCanvas) { const s = image[S].surface; if (s.lost) throw domError('The WebGL drawing buffer is lost.', 'InvalidStateError'); return s.width && s.height ? { width: s.width, height: s.height, data: pixelsOf(s) } : null; }
    if (image instanceof ImageBitmap) { if (image[S].closed) throw domError('The image source is detached.', 'InvalidStateError'); return image[S]; }
    if (image instanceof ImageData) return { width: image.width, height: image.height, data: image.data };
    if (image instanceof g.HTMLImageElement || (g.SVGImageElement && image instanceof g.SVGImageElement)) {
      const p = N.imagePixels(image);
      if (!p) { if (image.complete && image.naturalWidth === 0 && image.getAttribute('src')) throw domError("Failed to execute 'drawImage' on 'CanvasRenderingContext2D': The HTMLImageElement provided is in the 'broken' state.", 'InvalidStateError'); return null; }
      return { width: p.width, height: p.height, data: new Uint8ClampedArray(p.data) };
    }
    if (image instanceof g.HTMLVideoElement) return null;
    throw new TypeError("The provided value is not of type '(CSSImageValue or HTMLCanvasElement or HTMLImageElement or HTMLVideoElement or ImageBitmap or OffscreenCanvas or SVGImageElement or VideoFrame)'.");
  }

  hidden(N, 'canvasSourcePixels', sourcePixels);

  class TextMetrics { constructor() { ILLEGAL(); } }
  for (const k of ['width', 'actualBoundingBoxLeft', 'actualBoundingBoxRight', 'fontBoundingBoxAscent', 'fontBoundingBoxDescent', 'actualBoundingBoxAscent', 'actualBoundingBoxDescent', 'emHeightAscent', 'emHeightDescent', 'hangingBaseline', 'alphabeticBaseline', 'ideographicBaseline']) Object.defineProperty(TextMetrics.prototype, k, { get() { return this[S][k]; }, configurable: true, enumerable: true });
  hidden(TextMetrics.prototype, 'getActualBoundingBox', function () { const r = this[S]; return new g.DOMRectReadOnly(-r.actualBoundingBoxLeft, -r.actualBoundingBoxAscent, r.actualBoundingBoxLeft + r.actualBoundingBoxRight, r.actualBoundingBoxAscent + r.actualBoundingBoxDescent); });
  hidden(TextMetrics.prototype, 'getSelectionRects', function () { const r = this[S]; return [new g.DOMRectReadOnly(-r.actualBoundingBoxLeft, -r.fontBoundingBoxAscent, r.width, r.fontBoundingBoxAscent + r.fontBoundingBoxDescent)]; });
  hidden(TextMetrics.prototype, 'getIndexFromOffset', function (offset) { return Math.max(0, Math.round(offset / (this[S].width || 1))); });
  tag(TextMetrics, 'TextMetrics');

  const b64 = (bytes) => { let s = ''; for (let i = 0; i < bytes.length; i += 8192) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 8192)); return g.btoa(s); };
  const mimeOf = (type) => (String(type || '').toLowerCase() === 'image/jpeg' ? 'image/jpeg' : 'image/png');
  function encode(s, type, quality) { const b = s.width && s.height && !s.lost ? N.canvasEncode(s.id, mimeOf(type), quality === undefined ? 0.92 : +quality) : null; return b ? new Uint8Array(b) : null; }

  // -------------------------------------------------------- HTMLCanvasElement
  const contexts = new WeakMap();
  function makeContext(owner, surface, attrs, C) {
    const ctx = Object.create(C.prototype);
    hidden(ctx, S, { canvas: owner, surface, attrs: { alpha: attrs.alpha !== false, colorSpace: attrs.colorSpace || 'srgb', desynchronized: !!attrs.desynchronized, willReadFrequently: !!attrs.willReadFrequently, colorType: 'unorm8' } });
    resetState(ctx);
    surface.ctx = ctx;
    surface.mode = '2d';
    return ctx;
  }
  const CE = g.HTMLCanvasElement;
  hidden(CE.prototype, 'getContext', function getContext(type, attrs = {}) {
    if (arguments.length < 1) throw new TypeError("Failed to execute 'getContext' on 'HTMLCanvasElement': 1 argument required, but only 0 present.");
    type = String(type);
    if (type === 'experimental-webgl') type = 'webgl';
    const rec = contexts.get(this);
    if (rec) return rec.type === type ? rec.ctx : null;
    if (type === '2d') {
      const ctx = makeContext(this, surfaceOf(this), attrs || {}, CanvasRenderingContext2D);
      contexts.set(this, { type, ctx });
      return ctx;
    }
    if (type === 'bitmaprenderer') {
      const ctx = Object.create(ImageBitmapRenderingContext.prototype);
      const surface = surfaceOf(this); hidden(ctx, S, { canvas: this, surface });
      surface.ctx = ctx; surface.mode = type;
      contexts.set(this, { type, ctx });
      return ctx;
    }
    if (type === 'webgl' || type === 'webgl2') {
      const limits = N.webglLimits(), w = this.width >>> 0, h = this.height >>> 0;
      if (Math.max(w, 1) > limits.maxDimension || Math.max(h, 1) > limits.maxDimension || Math.max(w, 1) * Math.max(h, 1) > limits.maxPixels) {
        this.dispatchEvent(new g.WebGLContextEvent('webglcontextcreationerror', { cancelable: true, statusMessage: 'Drawing buffer exceeds the software renderer limits' }));
        return null;
      }
      const ctx = N.makeWebGLContext(this, surfaceOf(this), attrs === null ? {} : attrs, type === 'webgl2' ? 2 : 1);
      if (ctx) contexts.set(this, { type, ctx });
      return ctx;
    }
    return null;
  });
  // Setting either reflected dimension clears a WebGL drawing buffer even
  // when its numeric value is unchanged, while retaining GL object state.
  for (const dimension of ['width', 'height']) {
    const descriptor = Object.getOwnPropertyDescriptor(CE.prototype, dimension);
    if (descriptor && descriptor.set) Object.defineProperty(CE.prototype, dimension, {
      ...descriptor,
      set(value) {
        descriptor.set.call(this, value);
        const surface = surfaces.get(this);
        if (surface && surface.mode === 'webgl') {
          surface.width = this.width >>> 0; surface.height = this.height >>> 0;
          N.resizeWebGLSurface(surface, surface.width, surface.height);
          markDirty(surface);
        }
      },
    });
  }
  hidden(CE.prototype, 'toDataURL', function toDataURL(type, quality) {
    const s = surfaceOf(this);
    const bytes = encode(s, type, quality);
    return bytes ? `data:${mimeOf(type)};base64,${b64(bytes)}` : 'data:,';
  });
  hidden(CE.prototype, 'toBlob', function toBlob(cb, type, quality) {
    if (typeof cb !== 'function') throw new TypeError("Failed to execute 'toBlob' on 'HTMLCanvasElement': The callback provided as parameter 1 is not a function.");
    const bytes = encode(surfaceOf(this), type, quality);
    const blob = bytes ? new g.Blob([bytes], { type: mimeOf(type) }) : null;
    setTimeout(() => cb(blob), 0);
  });
  hidden(CE.prototype, 'captureStream', function () { return null; });

  // ------------------------------------------------------------ OffscreenCanvas
  class OffscreenCanvas extends g.EventTarget {
    constructor(w, h) { super(); hidden(this, S, { surface: newSurface(w >>> 0, h >>> 0, this), ctx: null, type: null }); }
    get width() { return this[S].surface.width; }
    set width(v) { const s = this[S].surface; s.width = v >>> 0; if (!['webgl', 'webgl2'].includes(this[S].type)) N.canvasResize(s.id, s.width, s.height); if (['webgl', 'webgl2'].includes(this[S].type)) N.resizeWebGLSurface(s, s.width, s.height); else if (this[S].ctx && this[S].type === '2d') resetState(this[S].ctx); }
    get height() { return this[S].surface.height; }
    set height(v) { const s = this[S].surface; s.height = v >>> 0; if (!['webgl', 'webgl2'].includes(this[S].type)) N.canvasResize(s.id, s.width, s.height); if (['webgl', 'webgl2'].includes(this[S].type)) N.resizeWebGLSurface(s, s.width, s.height); else if (this[S].ctx && this[S].type === '2d') resetState(this[S].ctx); }
    getContext(type, attrs = {}) {
      type = String(type);
      if (type === 'experimental-webgl') type = 'webgl';
      if (this[S].ctx) return this[S].type === type ? this[S].ctx : null;
      if (type === '2d') { this[S].ctx = makeContext(this, this[S].surface, attrs || {}, OffscreenCanvasRenderingContext2D); this[S].type = type; return this[S].ctx; }
      if (type === 'bitmaprenderer') { const c = Object.create(ImageBitmapRenderingContext.prototype); hidden(c, S, { canvas: this, surface: this[S].surface }); this[S].surface.ctx = c; this[S].surface.mode = type; this[S].ctx = c; this[S].type = type; return c; }
      if (type === 'webgl' || type === 'webgl2') { const c = N.makeWebGLContext(this, this[S].surface, attrs === null ? {} : attrs, type === 'webgl2' ? 2 : 1); if (c) { this[S].ctx = c; this[S].type = type; } return c; }
      return null;
    }
    transferToImageBitmap() { const s = this[S].surface; const bm = makeBitmap(s.width, s.height, pixelsOf(s)); N.canvasResize(s.id, s.width, s.height); return bm; }
    convertToBlob(options = {}) {
      const bytes = encode(this[S].surface, options.type, options.quality);
      return bytes ? Promise.resolve(new g.Blob([bytes], { type: mimeOf(options.type) })) : Promise.reject(domError('The image is empty.', 'IndexSizeError'));
    }
  }
  tag(OffscreenCanvas, 'OffscreenCanvas');
  N.installHandlers(OffscreenCanvas.prototype, ['contextlost', 'contextrestored']);
  class OffscreenCanvasRenderingContext2D { constructor() { ILLEGAL(); } }
  for (const k of Reflect.ownKeys(CanvasRenderingContext2D.prototype)) {
    if (k === 'constructor' || k === Symbol.toStringTag || k === 'drawFocusIfNeeded' || k === 'scrollPathIntoView') continue;
    Object.defineProperty(OffscreenCanvasRenderingContext2D.prototype, k, Object.getOwnPropertyDescriptor(CanvasRenderingContext2D.prototype, k));
  }
  tag(OffscreenCanvasRenderingContext2D, 'OffscreenCanvasRenderingContext2D');

  // ------------------------------------------------------------ ImageBitmap
  class ImageBitmap {
    constructor() { ILLEGAL(); }
    get width() { return this[S].closed ? 0 : this[S].width; }
    get height() { return this[S].closed ? 0 : this[S].height; }
    close() { this[S].closed = true; this[S].data = new Uint8ClampedArray(0); }
  }
  tag(ImageBitmap, 'ImageBitmap');
  function makeBitmap(w, h, data) { const b = Object.create(ImageBitmap.prototype); hidden(b, S, { width: w, height: h, data, closed: false }); return b; }
  hidden(g, 'createImageBitmap', function createImageBitmap(source, ...rest) {
    return new Promise((resolve, reject) => {
      const done = (px) => {
        let [sx, sy, sw, sh] = rest.length >= 4 ? rest : [0, 0, px.width, px.height];
        sx |= 0; sy |= 0; sw |= 0; sh |= 0;
        if (!sw || !sh) { reject(domError('The crop rect width or height is 0.', 'IndexSizeError')); return; }
        const out = new Uint8ClampedArray(Math.abs(sw * sh) * 4);
        for (let y = 0; y < sh; y++) for (let x = 0; x < sw; x++) {
          const ix = sx + x, iy = sy + y;
          if (ix < 0 || iy < 0 || ix >= px.width || iy >= px.height) continue;
          out.set(px.data.subarray((iy * px.width + ix) * 4, (iy * px.width + ix) * 4 + 4), (y * sw + x) * 4);
        }
        resolve(makeBitmap(sw, sh, out));
      };
      try {
        if (source instanceof g.Blob) {
          source.arrayBuffer().then((buf) => {
            const d = N.decodeImage(new Uint8Array(buf));
            if (!d) { reject(domError('The source image could not be decoded.', 'InvalidStateError')); return; }
            done({ width: d.width, height: d.height, data: new Uint8ClampedArray(d.data) });
          }, reject);
          return;
        }
        if (source instanceof g.HTMLImageElement && !source.complete) {
          source.addEventListener('load', () => { try { done(sourcePixels(source)); } catch (e) { reject(e); } }, { once: true });
          source.addEventListener('error', () => reject(domError('The image element contains no image data.', 'InvalidStateError')), { once: true });
          return;
        }
        const px = sourcePixels(source);
        if (!px) { reject(domError('The source image could not be decoded.', 'InvalidStateError')); return; }
        done(px);
      } catch (e) { reject(e); }
    });
  });
  class ImageBitmapRenderingContext {
    constructor() { ILLEGAL(); }
    get canvas() { return this[S].canvas; }
    transferFromImageBitmap(bm) {
      const isEl = this[S].canvas instanceof g.HTMLCanvasElement;
      if (bm === null) { const s = isEl ? surfaceOf(this[S].canvas) : this[S].surface; N.canvasResize(s.id, s.width, s.height); markDirty(s); return; }
      if (bm[S].closed) throw domError("Failed to execute 'transferFromImageBitmap': The input ImageBitmap has been detached", 'InvalidStateError');
      if (isEl) { this[S].canvas.width = bm.width; this[S].canvas.height = bm.height; }
      const t = isEl ? surfaceOf(this[S].canvas) : this[S].surface;
      N.canvasPutImageData(t.id, bm[S].data, bm.width, bm.height, 0, 0, 0, 0, bm.width, bm.height);
      bm.close();
      markDirty(t);
    }
  }
  tag(ImageBitmapRenderingContext, 'ImageBitmapRenderingContext');

  define('CanvasRenderingContext2D', CanvasRenderingContext2D);
  define('OffscreenCanvasRenderingContext2D', OffscreenCanvasRenderingContext2D);
  define('OffscreenCanvas', OffscreenCanvas);
  define('CanvasGradient', CanvasGradient);
  define('CanvasPattern', CanvasPattern);
  define('Path2D', Path2D);
  define('ImageData', ImageData);
  define('ImageBitmap', ImageBitmap);
  define('ImageBitmapRenderingContext', ImageBitmapRenderingContext);
  define('TextMetrics', TextMetrics);
})();
