// Canvas 2D (HTML §4.12.5): CanvasRenderingContext2D / OffscreenCanvas /
// OffscreenCanvasRenderingContext2D with a software rasterizer (paths with
// curves and arcs, nonzero/evenodd fill, strokes, rects, clipping,
// transforms, alpha, gradients, patterns, drawImage from images / canvases /
// bitmaps, ImageData get/put), Path2D (incl. SVG path data), TextMetrics,
// ImageData, ImageBitmap / createImageBitmap, ImageBitmapRenderingContext,
// toDataURL / toBlob (PNG). The bitmap of a connected <canvas> is handed to
// the engine for rendering. Text is measured but not rasterized.
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
  class CanvasGradient {
    constructor() { ILLEGAL(); }
    addColorStop(offset, color) {
      offset = +offset;
      if (!(offset >= 0 && offset <= 1)) throw domError(`Failed to execute 'addColorStop' on 'CanvasGradient': The provided value (${offset}) is outside the range (0.0, 1.0).`, 'IndexSizeError');
      const c = parseColor(color);
      if (!c) throw new SyntaxError(`Failed to execute 'addColorStop' on 'CanvasGradient': The value provided ('${color}') could not be parsed as a color.`);
      this[S].stops.push([offset, c]);
      this[S].stops.sort((a, b) => a[0] - b[0]);
    }
  }
  tag(CanvasGradient, 'CanvasGradient');
  class CanvasPattern {
    constructor() { ILLEGAL(); }
    setTransform(m = {}) { this[S].matrix = g.DOMMatrix.fromMatrix(m); }
  }
  tag(CanvasPattern, 'CanvasPattern');
  function gradientColor(gr, x, y) {
    const st = gr[S];
    let t;
    if (st.type === 'linear') {
      const dx = st.x1 - st.x0, dy = st.y1 - st.y0, len = dx * dx + dy * dy;
      t = len ? ((x - st.x0) * dx + (y - st.y0) * dy) / len : 0;
    } else if (st.type === 'radial') {
      const d = Math.hypot(x - st.x1, y - st.y1);
      t = st.r1 === st.r0 ? 0 : (d - st.r0) / (st.r1 - st.r0);
    } else {
      t = ((Math.atan2(y - st.y0, x - st.x0) - st.angle) / (2 * Math.PI)) % 1;
      if (t < 0) t += 1;
    }
    const stops = st.stops;
    if (!stops.length) return [0, 0, 0, 0];
    if (t <= stops[0][0]) return stops[0][1];
    for (let i = 1; i < stops.length; i++) {
      if (t <= stops[i][0]) {
        const [o0, c0] = stops[i - 1], [o1, c1] = stops[i];
        const f = o1 === o0 ? 1 : (t - o0) / (o1 - o0);
        return c0.map((v, k) => v + (c1[k] - v) * f);
      }
    }
    return stops[stops.length - 1][1];
  }

  // ----------------------------------------------------------------- paths
  // Subpaths are arrays of [x, y] points in user space (transformed when
  // added to a context path).
  class Path2D {
    constructor(arg) {
      const p = Object.create(new.target ? new.target.prototype : Path2D.prototype);
      hidden(p, S, { subpaths: [], cur: null, start: null });
      if (arg instanceof Path2D) for (const sp of arg[S].subpaths) p[S].subpaths.push({ pts: sp.pts.slice(), closed: sp.closed });
      else if (typeof arg === 'string') parseSvgPath(p, arg);
      return p;
    }
    addPath(path, m) {
      const mat = m ? g.DOMMatrix.fromMatrix(m) : null;
      for (const sp of path[S].subpaths) this[S].subpaths.push({ pts: sp.pts.map(([x, y]) => (mat ? [mat.a * x + mat.c * y + mat.e, mat.b * x + mat.d * y + mat.f] : [x, y])), closed: sp.closed });
    }
  }
  tag(Path2D, 'Path2D');
  const PATH_METHODS = {
    moveTo(x, y) { x = +x; y = +y; if (!isFinite(x) || !isFinite(y)) return; const p = this.tp(x, y); this[S].subpaths.push({ pts: [p], closed: false }); this[S].start = p; this[S].last = [x, y]; this[S].startUser = [x, y]; },
    lineTo(x, y) { x = +x; y = +y; if (!isFinite(x) || !isFinite(y)) return; this.ensure(x, y); this.cursp().pts.push(this.tp(x, y)); this[S].last = [x, y]; },
    closePath() { const sp = this.cursp(); if (!sp) return; sp.closed = true; const f = sp.pts[0]; this[S].subpaths.push({ pts: [f], closed: false }); this[S].last = this[S].startUser || this[S].last; },
    quadraticCurveTo(cx, cy, x, y) { this.ensure(cx, cy); const [x0, y0] = this[S].last; for (let i = 1; i <= 16; i++) { const t = i / 16, u = 1 - t; this.cursp().pts.push(this.tp(u * u * x0 + 2 * u * t * cx + t * t * x, u * u * y0 + 2 * u * t * cy + t * t * y)); } this[S].last = [x, y]; },
    bezierCurveTo(c1x, c1y, c2x, c2y, x, y) { this.ensure(c1x, c1y); const [x0, y0] = this[S].last; for (let i = 1; i <= 20; i++) { const t = i / 20, u = 1 - t; this.cursp().pts.push(this.tp(u * u * u * x0 + 3 * u * u * t * c1x + 3 * u * t * t * c2x + t * t * t * x, u * u * u * y0 + 3 * u * u * t * c1y + 3 * u * t * t * c2y + t * t * t * y)); } this[S].last = [x, y]; },
    arc(x, y, r, a0, a1, ccw = false) { this.ellipse(x, y, r, r, 0, a0, a1, ccw); },
    ellipse(x, y, rx, ry, rot, a0, a1, ccw = false) {
      if (rx < 0 || ry < 0) throw domError(`The ${rx < 0 ? 'major' : 'minor'}-axis radius provided (${rx < 0 ? rx : ry}) is negative.`, 'IndexSizeError');
      let sweep = a1 - a0;
      const TAU = Math.PI * 2;
      if (!ccw && sweep >= TAU) sweep = TAU; else if (ccw && -sweep >= TAU) sweep = -TAU;
      else { if (!ccw && sweep < 0) sweep = (sweep % TAU) + TAU; if (ccw && sweep > 0) sweep = (sweep % TAU) - TAU; }
      const steps = Math.max(4, Math.ceil(Math.abs(sweep) / (Math.PI / 32)));
      const cr = Math.cos(rot), sr = Math.sin(rot);
      for (let i = 0; i <= steps; i++) {
        const a = a0 + (sweep * i) / steps;
        const ex = rx * Math.cos(a), ey = ry * Math.sin(a);
        const px = x + ex * cr - ey * sr, py = y + ex * sr + ey * cr;
        if (i === 0) { if (this.cursp()) this.lineTo(px, py); else this.moveTo(px, py); } else this.lineTo(px, py);
      }
    },
    arcTo(x1, y1, x2, y2, r) {
      if (r < 0) throw domError(`The radius provided (${r}) is negative.`, 'IndexSizeError');
      this.ensure(x1, y1);
      const [x0, y0] = this[S].last;
      const v1 = [x0 - x1, y0 - y1], v2 = [x2 - x1, y2 - y1];
      const l1 = Math.hypot(...v1), l2 = Math.hypot(...v2);
      const cross = v1[0] * v2[1] - v1[1] * v2[0];
      if (!l1 || !l2 || !r || Math.abs(cross) < 1e-9) { this.lineTo(x1, y1); return; }
      const ang = Math.acos(Math.max(-1, Math.min(1, (v1[0] * v2[0] + v1[1] * v2[1]) / (l1 * l2))));
      const d = r / Math.tan(ang / 2);
      const t1 = [x1 + (v1[0] / l1) * d, y1 + (v1[1] / l1) * d], t2 = [x1 + (v2[0] / l2) * d, y1 + (v2[1] / l2) * d];
      const bis = [v1[0] / l1 + v2[0] / l2, v1[1] / l1 + v2[1] / l2], bl = Math.hypot(...bis);
      const dc = r / Math.sin(ang / 2);
      const c = [x1 + (bis[0] / bl) * dc, y1 + (bis[1] / bl) * dc];
      this.lineTo(t1[0], t1[1]);
      const a0 = Math.atan2(t1[1] - c[1], t1[0] - c[0]), a1 = Math.atan2(t2[1] - c[1], t2[0] - c[0]);
      this.arc(c[0], c[1], r, a0, a1, cross > 0);
    },
    rect(x, y, w, h) { this.moveTo(x, y); this.lineTo(x + w, y); this.lineTo(x + w, y + h); this.lineTo(x, y + h); this.closePath(); },
    roundRect(x, y, w, h, radii = 0) {
      const list = Array.isArray(radii) ? radii : [radii];
      if (list.length < 1 || list.length > 4) throw new RangeError(`Failed to execute 'roundRect': ${list.length} radii provided. Between one and four radii are necessary.`);
      const r = list.map((v) => (typeof v === 'object' ? Math.max(v.x || 0, v.y || 0) : +v));
      if (r.some((v) => v < 0)) throw new RangeError("Failed to execute 'roundRect': Radius value is negative.");
      const [tl, tr, br, bl] = r.length === 1 ? [r[0], r[0], r[0], r[0]] : r.length === 2 ? [r[0], r[1], r[0], r[1]] : r.length === 3 ? [r[0], r[1], r[2], r[1]] : r;
      this.moveTo(x + tl, y);
      this.lineTo(x + w - tr, y); if (tr) this.arc(x + w - tr, y + tr, tr, -Math.PI / 2, 0);
      this.lineTo(x + w, y + h - br); if (br) this.arc(x + w - br, y + h - br, br, 0, Math.PI / 2);
      this.lineTo(x + bl, y + h); if (bl) this.arc(x + bl, y + h - bl, bl, Math.PI / 2, Math.PI);
      this.lineTo(x, y + tl); if (tl) this.arc(x + tl, y + tl, tl, Math.PI, Math.PI * 1.5);
      this.closePath();
    },
  };
  const pathHelpers = {
    tp(x, y) { return [x, y]; },
    cursp() { const l = this[S].subpaths; return l[l.length - 1] || null; },
    ensure(x, y) { if (!this.cursp()) this.moveTo(x, y); },
  };
  for (const [k, f] of Object.entries(PATH_METHODS)) hidden(Path2D.prototype, k, f);
  for (const [k, f] of Object.entries(pathHelpers)) Object.defineProperty(Path2D.prototype, k, { value: f, writable: true, configurable: true, enumerable: false });
  function parseSvgPath(p, d) {
    const toks = String(d).match(/[MmLlHhVvCcSsQqTtAaZz]|-?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?/g) || [];
    let i = 0, cmd = '', cx = 0, cy = 0, sx = 0, sy = 0, lcx = 0, lcy = 0, lastCmd = '';
    const num = () => parseFloat(toks[i++]);
    while (i < toks.length) {
      if (/[A-Za-z]/.test(toks[i])) cmd = toks[i++];
      const rel = cmd === cmd.toLowerCase();
      const C = cmd.toUpperCase();
      const ox = rel ? cx : 0, oy = rel ? cy : 0;
      switch (C) {
        case 'M': cx = ox + num(); cy = oy + num(); p.moveTo(cx, cy); sx = cx; sy = cy; cmd = rel ? 'l' : 'L'; break;
        case 'L': cx = ox + num(); cy = oy + num(); p.lineTo(cx, cy); break;
        case 'H': cx = (rel ? cx : 0) + num(); p.lineTo(cx, cy); break;
        case 'V': cy = (rel ? cy : 0) + num(); p.lineTo(cx, cy); break;
        case 'C': { const a = ox + num(), b = oy + num(), c = ox + num(), e = oy + num(); cx = ox + num(); cy = oy + num(); p.bezierCurveTo(a, b, c, e, cx, cy); lcx = c; lcy = e; break; }
        case 'S': { const a = /[CS]/.test(lastCmd) ? 2 * cx - lcx : cx, b = /[CS]/.test(lastCmd) ? 2 * cy - lcy : cy; const c = ox + num(), e = oy + num(); cx = ox + num(); cy = oy + num(); p.bezierCurveTo(a, b, c, e, cx, cy); lcx = c; lcy = e; break; }
        case 'Q': { const a = ox + num(), b = oy + num(); cx = ox + num(); cy = oy + num(); p.quadraticCurveTo(a, b, cx, cy); lcx = a; lcy = b; break; }
        case 'T': { const a = /[QT]/.test(lastCmd) ? 2 * cx - lcx : cx, b = /[QT]/.test(lastCmd) ? 2 * cy - lcy : cy; cx = ox + num(); cy = oy + num(); p.quadraticCurveTo(a, b, cx, cy); lcx = a; lcy = b; break; }
        case 'A': { const rx = num(), ry = num(), rot = num(), large = num(), sweep = num(); const x = ox + num(), y = oy + num(); arcToSvg(p, cx, cy, rx, ry, rot, large, sweep, x, y); cx = x; cy = y; break; }
        case 'Z': p.closePath(); cx = sx; cy = sy; break;
        default: i++;
      }
      lastCmd = C;
      if (Number.isNaN(cx) || Number.isNaN(cy)) break;
    }
  }
  function arcToSvg(p, x1, y1, rx, ry, phiDeg, fa, fs, x2, y2) {
    if (!rx || !ry) { p.lineTo(x2, y2); return; }
    rx = Math.abs(rx); ry = Math.abs(ry);
    const phi = (phiDeg * Math.PI) / 180, cp = Math.cos(phi), sp = Math.sin(phi);
    const dx = (x1 - x2) / 2, dy = (y1 - y2) / 2;
    const x1p = cp * dx + sp * dy, y1p = -sp * dx + cp * dy;
    const lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if (lam > 1) { rx *= Math.sqrt(lam); ry *= Math.sqrt(lam); }
    const sign = fa === fs ? -1 : 1;
    const num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    const co = sign * Math.sqrt(Math.max(0, num / (rx * rx * y1p * y1p + ry * ry * x1p * x1p)));
    const cxp = (co * rx * y1p) / ry, cyp = (-co * ry * x1p) / rx;
    const cx = cp * cxp - sp * cyp + (x1 + x2) / 2, cy = sp * cxp + cp * cyp + (y1 + y2) / 2;
    const ang = (ux, uy, vx, vy) => Math.atan2(ux * vy - uy * vx, ux * vx + uy * vy);
    const t1 = ang(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let dt = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry);
    if (!fs && dt > 0) dt -= 2 * Math.PI; else if (fs && dt < 0) dt += 2 * Math.PI;
    p.ellipse(cx, cy, rx, ry, phi, t1, t1 + dt, !fs);
  }

  // ------------------------------------------------------------ rasterizer
  // Fill a set of device-space polygons into a coverage callback.
  function rasterize(polys, w, h, evenOdd, plot) {
    const edges = [];
    let minY = Infinity, maxY = -Infinity;
    for (const pts of polys) {
      for (let i = 0; i < pts.length; i++) {
        const a = pts[i], b = pts[(i + 1) % pts.length];
        if (a[1] === b[1]) continue;
        edges.push(a[1] < b[1] ? [a[0], a[1], b[0], b[1], 1] : [b[0], b[1], a[0], a[1], -1]);
        minY = Math.min(minY, a[1], b[1]); maxY = Math.max(maxY, a[1], b[1]);
      }
    }
    if (!edges.length) return;
    const y0 = Math.max(0, Math.floor(minY)), y1 = Math.min(h - 1, Math.ceil(maxY));
    const xs = [];
    for (let y = y0; y <= y1; y++) {
      const sy = y + 0.5;
      xs.length = 0;
      for (const e of edges) if (sy >= e[1] && sy < e[3]) xs.push([e[0] + ((sy - e[1]) * (e[2] - e[0])) / (e[3] - e[1]), e[4]]);
      if (!xs.length) continue;
      xs.sort((p, q) => p[0] - q[0]);
      let wind = 0;
      for (let i = 0; i < xs.length - 1; i++) {
        wind += evenOdd ? 1 : xs[i][1];
        const inside = evenOdd ? wind % 2 === 1 : wind !== 0;
        if (!inside) continue;
        const xa = Math.max(0, Math.ceil(xs[i][0] - 0.5)), xb = Math.min(w - 1, Math.ceil(xs[i + 1][0] - 0.5) - 1);
        for (let x = xa; x <= xb; x++) plot(x, y);
      }
    }
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
  // Stroke outline polygons for polylines.
  function strokePolys(subpaths, width, cap, join) {
    const hw = width / 2, out = [];
    const circle = (cx, cy) => { const pts = []; for (let i = 0; i < 12; i++) { const a = (i / 12) * 2 * Math.PI; pts.push([cx + hw * Math.cos(a), cy + hw * Math.sin(a)]); } return pts; };
    for (const sp of subpaths) {
      const pts = sp.closed && sp.pts.length > 2 ? sp.pts.concat([sp.pts[0]]) : sp.pts;
      if (pts.length < 2) continue;
      for (let i = 0; i < pts.length - 1; i++) {
        const [x0, y0] = pts[i], [x1, y1] = pts[i + 1];
        const len = Math.hypot(x1 - x0, y1 - y0);
        if (!len) continue;
        let nx = (-(y1 - y0) / len) * hw, ny = ((x1 - x0) / len) * hw;
        let ex0 = x0, ey0 = y0, ex1 = x1, ey1 = y1;
        if (cap === 'square' && !sp.closed) {
          const tx = ((x1 - x0) / len) * hw, ty = ((y1 - y0) / len) * hw;
          if (i === 0) { ex0 -= tx; ey0 -= ty; }
          if (i === pts.length - 2) { ex1 += tx; ey1 += ty; }
        }
        out.push([[ex0 + nx, ey0 + ny], [ex1 + nx, ey1 + ny], [ex1 - nx, ey1 - ny], [ex0 - nx, ey0 - ny]]);
        if (i > 0 && join !== 'bevel-none') out.push(circle(x0, y0));
      }
      if (cap === 'round' && !sp.closed) { out.push(circle(pts[0][0], pts[0][1])); out.push(circle(pts[pts.length - 1][0], pts[pts.length - 1][1])); }
    }
    return out;
  }

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
  // A surface: { width, height, data: Uint8ClampedArray, owner, dirty }.
  function newSurface(w, h, owner) { return { width: w, height: h, data: new Uint8ClampedArray(Math.max(0, w * h * 4)), owner, dirty: false, ctx: null }; }
  const surfaces = new WeakMap();
  function surfaceOf(canvas) {
    let s = surfaces.get(canvas);
    const w = canvas.width >>> 0, h = canvas.height >>> 0;
    if (!s) { s = newSurface(w, h, canvas); surfaces.set(canvas, s); }
    else if (s.width !== w || s.height !== h) {
      // Resizing a canvas clears it.
      s.width = w; s.height = h; s.data = new Uint8ClampedArray(w * h * 4);
      if (s.ctx) resetState(s.ctx);
      markDirty(s);
    }
    return s;
  }
  const pendingFlush = new Set();
  let flushQueued = false;
  function markDirty(s) {
    s.dirty = true;
    if (!s.owner || !(s.owner instanceof g.HTMLCanvasElement)) return;
    pendingFlush.add(s);
    if (!flushQueued) { flushQueued = true; queueMicrotask(flushCanvases); }
  }
  function flushCanvases() {
    flushQueued = false;
    for (const s of pendingFlush) {
      if (!s.owner.isConnected) continue;
      N.setCanvasImage(s.owner, s.width, s.height, s.data);
    }
    pendingFlush.clear();
  }

  // ------------------------------------------------ CanvasRenderingContext2D
  const DEFAULT_STATE = () => ({ fillStyle: '#000000', fill: [0, 0, 0, 1], strokeStyle: '#000000', stroke: [0, 0, 0, 1], globalAlpha: 1, globalCompositeOperation: 'source-over', lineWidth: 1, lineCap: 'butt', lineJoin: 'miter', miterLimit: 10, lineDash: [], lineDashOffset: 0, font: '10px sans-serif', textAlign: 'start', textBaseline: 'alphabetic', direction: 'inherit', letterSpacing: '0px', wordSpacing: '0px', fontKerning: 'auto', fontStretch: 'normal', fontVariantCaps: 'normal', textRendering: 'auto', shadowBlur: 0, shadowColor: 'rgba(0, 0, 0, 0)', shadowOffsetX: 0, shadowOffsetY: 0, filter: 'none', imageSmoothingEnabled: true, imageSmoothingQuality: 'low', m: [1, 0, 0, 1, 0, 0], clip: null });
  function resetState(ctx) { ctx[S].state = DEFAULT_STATE(); ctx[S].stack = []; ctx[S].path = newPath(ctx); }
  function newPath(ctx) {
    const p = Object.create(Path2D.prototype);
    hidden(p, S, { subpaths: [], last: null });
    hidden(p, 'tp', (x, y) => { const m = ctx[S].state.m; return [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]; });
    return p;
  }
  // The context's surface (a <canvas> resized since drawing is re-synced).
  const surf = (ctx) => (ctx[S].canvas instanceof g.HTMLCanvasElement ? surfaceOf(ctx[S].canvas) : ctx[S].surface);
  class CanvasRenderingContext2D {
    constructor() { ILLEGAL(); }
    get canvas() { return this[S].canvas; }
    getContextAttributes() { return Object.assign({}, this[S].attrs); }
    save() { const st = this[S].state; this[S].stack.push(Object.assign({}, st, { m: st.m.slice(), lineDash: st.lineDash.slice() })); }
    restore() { const s = this[S].stack.pop(); if (s) this[S].state = s; }
    reset() { const sf = surf(this); sf.data.fill(0); resetState(this); markDirty(sf); }
    isContextLost() { return false; }
    scale(x, y) { this.transform(x, 0, 0, y, 0, 0); }
    rotate(a) { const c = Math.cos(a), s = Math.sin(a); this.transform(c, s, -s, c, 0, 0); }
    translate(x, y) { this.transform(1, 0, 0, 1, x, y); }
    transform(a, b, c, d, e, f) {
      if (![a, b, c, d, e, f].every(Number.isFinite)) return;
      const m = this[S].state.m;
      this[S].state.m = [m[0] * a + m[2] * b, m[1] * a + m[3] * b, m[0] * c + m[2] * d, m[1] * c + m[3] * d, m[0] * e + m[2] * f + m[4], m[1] * e + m[3] * f + m[5]];
    }
    getTransform() { const m = this[S].state.m; return new g.DOMMatrix([m[0], m[1], m[2], m[3], m[4], m[5]]); }
    setTransform(a, b, c, d, e, f) {
      if (typeof a === 'object' || a === undefined) { const mm = g.DOMMatrix.fromMatrix(a || {}); this[S].state.m = [mm.a, mm.b, mm.c, mm.d, mm.e, mm.f]; return; }
      if (![a, b, c, d, e, f].every(Number.isFinite)) return;
      this[S].state.m = [a, b, c, d, e, f];
    }
    resetTransform() { this[S].state.m = [1, 0, 0, 1, 0, 0]; }
    beginPath() { this[S].path = newPath(this); }
    fill(a, b) { const [path, rule] = a instanceof Path2D ? [a, b] : [null, a]; this._fillPolys(polysOf(this, path), rule === 'evenodd', 'fill'); }
    stroke(path) { const st = this[S].state; const polys = strokePolys(subpathsOf(this, path instanceof Path2D ? path : null), st.lineWidth * Math.sqrt(Math.abs(st.m[0] * st.m[3] - st.m[1] * st.m[2])), st.lineCap, st.lineJoin); this._fillPolys(polys, false, 'stroke'); }
    clip(a, b) {
      const [path, rule] = a instanceof Path2D ? [a, b] : [null, a];
      const sf = surf(this), mask = new Uint8Array(sf.width * sf.height);
      rasterize(polysOf(this, path), sf.width, sf.height, rule === 'evenodd', (x, y) => { mask[y * sf.width + x] = 1; });
      const old = this[S].state.clip;
      if (old) for (let i = 0; i < mask.length; i++) mask[i] &= old[i];
      this[S].state.clip = mask;
    }
    isPointInPath(a, b, c, d) {
      const [path, x, y, rule] = a instanceof Path2D ? [a, b, c, d] : [null, a, b, c];
      return pointInPolys(polysOf(this, path), +x, +y, rule === 'evenodd');
    }
    isPointInStroke(a, b, c) {
      const [path, x, y] = a instanceof Path2D ? [a, b, c] : [null, a, b];
      const st = this[S].state;
      return pointInPolys(strokePolys(subpathsOf(this, path), st.lineWidth, st.lineCap, st.lineJoin), +x, +y, false);
    }
    fillRect(x, y, w, h) { const p = newPath(this); p.rect(+x, +y, +w, +h); this._fillPolys(polysOf(this, p, true), false, 'fill'); }
    strokeRect(x, y, w, h) { const p = newPath(this); p.rect(+x, +y, +w, +h); const st = this[S].state; this._fillPolys(strokePolys(p[S].subpaths, st.lineWidth, st.lineCap, st.lineJoin), false, 'stroke'); }
    clearRect(x, y, w, h) {
      const p = newPath(this); p.rect(+x, +y, +w, +h);
      const sf = surf(this), d = sf.data, clip = this[S].state.clip;
      rasterize(polysOf(this, p, true), sf.width, sf.height, false, (px, py) => { const i = py * sf.width + px; if (clip && !clip[i]) return; d[i * 4] = d[i * 4 + 1] = d[i * 4 + 2] = d[i * 4 + 3] = 0; });
      markDirty(sf);
    }
    _fillPolys(polys, evenOdd, which) {
      const st = this[S].state, sf = surf(this);
      const style = which === 'fill' ? st.fillStyle : st.strokeStyle;
      const solid = which === 'fill' ? st.fill : st.stroke;
      const inv = invert(st.m);
      const ga = st.globalAlpha, op = st.globalCompositeOperation, clip = st.clip;
      const colorAt = style instanceof CanvasGradient ? (x, y) => { const u = apply(inv, x + 0.5, y + 0.5); return gradientColor(style, u[0], u[1]); }
        : style instanceof CanvasPattern ? (x, y) => patternColor(style, apply(inv, x + 0.5, y + 0.5))
          : () => solid;
      if (op === 'copy') sf.data.fill(0);
      rasterize(polys, sf.width, sf.height, evenOdd, (x, y) => { if (clip && !clip[y * sf.width + x]) return; blend(sf.data, (y * sf.width + x) * 4, colorAt(x, y), ga, op); });
      markDirty(sf);
    }
    createImageData(a, b, settings) { return a instanceof ImageData ? new ImageData(a.width, a.height) : new ImageData(Math.abs(a >> 0), Math.abs(b >> 0), settings); }
    getImageData(sx, sy, sw, sh, settings) {
      sx = sx | 0; sy = sy | 0; sw = sw | 0; sh = sh | 0;
      if (!sw || !sh) throw domError(`Failed to execute 'getImageData' on 'CanvasRenderingContext2D': The source ${!sw ? 'width' : 'height'} is 0.`, 'IndexSizeError');
      if (sw < 0) { sx += sw; sw = -sw; }
      if (sh < 0) { sy += sh; sh = -sh; }
      const out = new ImageData(sw, sh, settings), sf = surf(this);
      for (let y = 0; y < sh; y++) {
        const yy = sy + y;
        if (yy < 0 || yy >= sf.height) continue;
        for (let x = 0; x < sw; x++) {
          const xx = sx + x;
          if (xx < 0 || xx >= sf.width) continue;
          const si = (yy * sf.width + xx) * 4, di = (y * sw + x) * 4;
          out.data[di] = sf.data[si]; out.data[di + 1] = sf.data[si + 1]; out.data[di + 2] = sf.data[si + 2]; out.data[di + 3] = sf.data[si + 3];
        }
      }
      return out;
    }
    putImageData(img, dx, dy, dirtyX = 0, dirtyY = 0, dirtyW = img.width, dirtyH = img.height) {
      const sf = surf(this);
      dx = dx | 0; dy = dy | 0;
      for (let y = Math.max(0, dirtyY); y < Math.min(img.height, dirtyY + dirtyH); y++) {
        const yy = dy + y;
        if (yy < 0 || yy >= sf.height) continue;
        for (let x = Math.max(0, dirtyX); x < Math.min(img.width, dirtyX + dirtyW); x++) {
          const xx = dx + x;
          if (xx < 0 || xx >= sf.width) continue;
          const si = (y * img.width + x) * 4, di = (yy * sf.width + xx) * 4;
          sf.data[di] = img.data[si]; sf.data[di + 1] = img.data[si + 1]; sf.data[di + 2] = img.data[si + 2]; sf.data[di + 3] = img.data[si + 3];
        }
      }
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
      if (!sw || !sh || !dw || !dh) return;
      const st = this[S].state, sf = surf(this);
      const p = newPath(this); p.rect(dx, dy, dw, dh);
      const inv = invert(st.m), clip = st.clip;
      rasterize(polysOf(this, p, true), sf.width, sf.height, false, (x, y) => {
        if (clip && !clip[y * sf.width + x]) return;
        const u = apply(inv, x + 0.5, y + 0.5);
        const ix = Math.floor(sx + ((u[0] - dx) / dw) * sw), iy = Math.floor(sy + ((u[1] - dy) / dh) * sh);
        if (ix < 0 || iy < 0 || ix >= src.width || iy >= src.height) return;
        const i = (iy * src.width + ix) * 4;
        blend(sf.data, (y * sf.width + x) * 4, [src.data[i], src.data[i + 1], src.data[i + 2], src.data[i + 3] / 255], st.globalAlpha, st.globalCompositeOperation);
      });
      markDirty(sf);
    }
    createLinearGradient(x0, y0, x1, y1) { const gr = Object.create(CanvasGradient.prototype); hidden(gr, S, { type: 'linear', x0: +x0, y0: +y0, x1: +x1, y1: +y1, stops: [] }); return gr; }
    createRadialGradient(x0, y0, r0, x1, y1, r1) { if (r0 < 0 || r1 < 0) throw domError(`The ${r0 < 0 ? 'r0' : 'r1'} provided is less than 0.`, 'IndexSizeError'); const gr = Object.create(CanvasGradient.prototype); hidden(gr, S, { type: 'radial', x0: +x0, y0: +y0, r0: +r0, x1: +x1, y1: +y1, r1: +r1, stops: [] }); return gr; }
    createConicGradient(angle, x, y) { const gr = Object.create(CanvasGradient.prototype); hidden(gr, S, { type: 'conic', angle: +angle, x0: +x, y0: +y, stops: [] }); return gr; }
    createPattern(image, repetition) {
      const src = sourcePixels(image);
      if (!src) return null;
      repetition = repetition === null || repetition === '' ? 'repeat' : String(repetition);
      if (!['repeat', 'repeat-x', 'repeat-y', 'no-repeat'].includes(repetition)) throw new SyntaxError(`Failed to execute 'createPattern': The provided type ('${repetition}') is not one of 'repeat', 'no-repeat', 'repeat-x', or 'repeat-y'.`);
      const p = Object.create(CanvasPattern.prototype);
      hidden(p, S, { src, repetition, matrix: null });
      return p;
    }
    fillText() { markDirty(surf(this)); }
    strokeText() { markDirty(surf(this)); }
    measureText(text) {
      text = String(text);
      const m = /(\d+(?:\.\d+)?)px/.exec(this[S].state.font);
      const size = m ? +m[1] : 10;
      let width = 0;
      for (const ch of text) width += ch === ' ' ? 0.28 : /[ilI.,'|!:;]/.test(ch) ? 0.28 : /[mwMW]/.test(ch) ? 0.85 : /[A-Z0-9]/.test(ch) ? 0.66 : 0.52;
      width *= size;
      const tm = Object.create(TextMetrics.prototype);
      hidden(tm, S, { width, actualBoundingBoxLeft: 0, actualBoundingBoxRight: width, fontBoundingBoxAscent: size * 0.91, fontBoundingBoxDescent: size * 0.21, actualBoundingBoxAscent: size * 0.72, actualBoundingBoxDescent: size * 0.16, emHeightAscent: size * 0.8, emHeightDescent: size * 0.2, hangingBaseline: size * 0.72, alphabeticBaseline: 0, ideographicBaseline: -size * 0.21 });
      return tm;
    }
    setLineDash(segments) { const s = Array.from(segments).map(Number); if (s.some((v) => !(v >= 0) || !isFinite(v))) return; this[S].state.lineDash = s.length % 2 ? s.concat(s) : s; }
    getLineDash() { return this[S].state.lineDash.slice(); }
    drawFocusIfNeeded() {}
    scrollPathIntoView() {}
  }
  tag(CanvasRenderingContext2D, 'CanvasRenderingContext2D');
  for (const k of Object.keys(PATH_METHODS)) hidden(CanvasRenderingContext2D.prototype, k, function (...args) { return PATH_METHODS[k].apply(this[S].path, args); });
  const STYLE_PROPS = ['globalAlpha', 'globalCompositeOperation', 'lineWidth', 'lineCap', 'lineJoin', 'miterLimit', 'lineDashOffset', 'font', 'textAlign', 'textBaseline', 'direction', 'letterSpacing', 'wordSpacing', 'fontKerning', 'fontStretch', 'fontVariantCaps', 'textRendering', 'shadowBlur', 'shadowColor', 'shadowOffsetX', 'shadowOffsetY', 'filter', 'imageSmoothingEnabled', 'imageSmoothingQuality', 'lang'];
  const ENUMS = { lineCap: ['butt', 'round', 'square'], lineJoin: ['round', 'bevel', 'miter'], textAlign: ['start', 'end', 'left', 'right', 'center'], textBaseline: ['top', 'hanging', 'middle', 'alphabetic', 'ideographic', 'bottom'], direction: ['ltr', 'rtl', 'inherit'], globalCompositeOperation: ['source-over', 'source-in', 'source-out', 'source-atop', 'destination-over', 'destination-in', 'destination-out', 'destination-atop', 'lighter', 'copy', 'xor', 'multiply', 'screen', 'overlay', 'darken', 'lighten', 'color-dodge', 'color-burn', 'hard-light', 'soft-light', 'difference', 'exclusion', 'hue', 'saturation', 'color', 'luminosity'], imageSmoothingQuality: ['low', 'medium', 'high'], fontKerning: ['auto', 'normal', 'none'] };
  for (const k of STYLE_PROPS) {
    Object.defineProperty(CanvasRenderingContext2D.prototype, k, {
      get() { return this[S].state[k] ?? (k === 'lang' ? 'inherit' : undefined); },
      set(v) {
        const st = this[S].state;
        if (ENUMS[k]) { if (ENUMS[k].includes(String(v))) st[k] = String(v); return; }
        if (['globalAlpha'].includes(k)) { v = +v; if (v >= 0 && v <= 1) st[k] = v; return; }
        if (['lineWidth', 'miterLimit'].includes(k)) { v = +v; if (v > 0 && isFinite(v)) st[k] = v; return; }
        if (['shadowBlur'].includes(k)) { v = +v; if (v >= 0 && isFinite(v)) st[k] = v; return; }
        if (['lineDashOffset', 'shadowOffsetX', 'shadowOffsetY'].includes(k)) { v = +v; if (isFinite(v)) st[k] = v; return; }
        if (k === 'imageSmoothingEnabled') { st[k] = !!v; return; }
        if (k === 'shadowColor') { const c = parseColor(v); if (c) st[k] = serializeColor(c); return; }
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
  function subpathsOf(ctx, path) {
    if (!path) return ctx[S].path[S].subpaths;
    const m = ctx[S].state.m;
    return path[S].subpaths.map((sp) => ({ pts: sp.pts.map(([x, y]) => [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]), closed: sp.closed }));
  }
  function polysOf(ctx, path, ownPath) { return (ownPath ? path[S].subpaths : subpathsOf(ctx, path)).filter((sp) => sp.pts.length > 2).map((sp) => sp.pts); }
  const invert = (m) => { const det = m[0] * m[3] - m[1] * m[2]; if (!det) return [0, 0, 0, 0, 0, 0]; return [m[3] / det, -m[1] / det, -m[2] / det, m[0] / det, (m[2] * m[5] - m[3] * m[4]) / det, (m[1] * m[4] - m[0] * m[5]) / det]; };
  const apply = (m, x, y) => [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
  function blend(d, i, c, ga, op) {
    const sa = (c[3] === undefined ? 1 : c[3]) * ga;
    if (sa <= 0 && op === 'source-over') return;
    const da = d[i + 3] / 255;
    switch (op) {
      case 'destination-out': { const na = da * (1 - sa); d[i + 3] = na * 255; return; }
      case 'destination-in': { d[i + 3] = da * sa * 255; return; }
      case 'copy': { d[i] = c[0]; d[i + 1] = c[1]; d[i + 2] = c[2]; d[i + 3] = sa * 255; return; }
      case 'destination-over': { const oa = da + sa * (1 - da); if (oa > 0) for (let k = 0; k < 3; k++) d[i + k] = (d[i + k] * da + c[k] * sa * (1 - da)) / oa; d[i + 3] = oa * 255; return; }
      default: {
        const oa = sa + da * (1 - sa);
        if (oa <= 0) { d[i] = d[i + 1] = d[i + 2] = d[i + 3] = 0; return; }
        for (let k = 0; k < 3; k++) d[i + k] = (c[k] * sa + d[i + k] * da * (1 - sa)) / oa;
        d[i + 3] = oa * 255;
      }
    }
  }
  function patternColor(p, [x, y]) {
    const st = p[S], src = st.src;
    let ix = Math.floor(x), iy = Math.floor(y);
    if (st.repetition === 'repeat' || st.repetition === 'repeat-x') ix = ((ix % src.width) + src.width) % src.width;
    if (st.repetition === 'repeat' || st.repetition === 'repeat-y') iy = ((iy % src.height) + src.height) % src.height;
    if (ix < 0 || iy < 0 || ix >= src.width || iy >= src.height) return [0, 0, 0, 0];
    const i = (iy * src.width + ix) * 4;
    return [src.data[i], src.data[i + 1], src.data[i + 2], src.data[i + 3] / 255];
  }
  // Pixels of a CanvasImageSource: { width, height, data }.
  function sourcePixels(image) {
    if (image instanceof g.HTMLCanvasElement) { const s = surfaceOf(image); return s.width && s.height ? s : null; }
    if (image instanceof OffscreenCanvas) return image[S].surface.width ? image[S].surface : null;
    if (image instanceof ImageBitmap) { if (image[S].closed) throw domError('The image source is detached.', 'InvalidStateError'); return image[S]; }
    if (image instanceof ImageData) return { width: image.width, height: image.height, data: image.data };
    if (image instanceof g.HTMLImageElement || (g.SVGImageElement && image instanceof g.SVGImageElement)) {
      const p = N.imagePixels(image);
      if (!p) { if (image.complete && image.naturalWidth === 0 && image.getAttribute('src')) throw domError("Failed to execute 'drawImage' on 'CanvasRenderingContext2D': The HTMLImageElement provided is in the 'broken' state.", 'InvalidStateError'); return null; }
      return { width: p.width, height: p.height, data: new Uint8ClampedArray(p.data) };
    }
    if (image instanceof g.HTMLVideoElement) return null;
    if (image && typeof image.width === 'number' && image.data) return image;
    throw new TypeError("The provided value is not of type '(CSSImageValue or HTMLCanvasElement or HTMLImageElement or HTMLVideoElement or ImageBitmap or OffscreenCanvas or SVGImageElement or VideoFrame)'.");
  }

  class TextMetrics { constructor() { ILLEGAL(); } }
  for (const k of ['width', 'actualBoundingBoxLeft', 'actualBoundingBoxRight', 'fontBoundingBoxAscent', 'fontBoundingBoxDescent', 'actualBoundingBoxAscent', 'actualBoundingBoxDescent', 'emHeightAscent', 'emHeightDescent', 'hangingBaseline', 'alphabeticBaseline', 'ideographicBaseline']) Object.defineProperty(TextMetrics.prototype, k, { get() { return this[S][k]; }, configurable: true, enumerable: true });
  hidden(TextMetrics.prototype, 'getActualBoundingBox', function (start = 0, end) { const r = this[S]; return new g.DOMRectReadOnly(0, -r.actualBoundingBoxAscent, r.width, r.actualBoundingBoxAscent + r.actualBoundingBoxDescent); void start; void end; });
  hidden(TextMetrics.prototype, 'getSelectionRects', function () { return [new g.DOMRectReadOnly(0, -this[S].fontBoundingBoxAscent, this[S].width, this[S].fontBoundingBoxAscent + this[S].fontBoundingBoxDescent)]; });
  hidden(TextMetrics.prototype, 'getIndexFromOffset', function (offset) { return Math.max(0, Math.round(offset / (this[S].width || 1))); });
  tag(TextMetrics, 'TextMetrics');

  // ----------------------------------------------------------- PNG encoding
  function encodePNG(w, h, data) {
    const raw = new Uint8Array(h * (w * 4 + 1));
    for (let y = 0; y < h; y++) { raw[y * (w * 4 + 1)] = 0; raw.set(data.subarray(y * w * 4, (y + 1) * w * 4), y * (w * 4 + 1) + 1); }
    // zlib stream with stored blocks.
    const blocks = Math.max(1, Math.ceil(raw.length / 65535));
    const z = new Uint8Array(2 + raw.length + blocks * 5 + 4);
    z[0] = 0x78; z[1] = 0x01;
    let o = 2, a = 1, b = 0;
    for (let k = 0; k < blocks; k++) {
      const chunk = raw.subarray(k * 65535, Math.min(raw.length, (k + 1) * 65535));
      z[o++] = k === blocks - 1 ? 1 : 0; z[o++] = chunk.length & 255; z[o++] = chunk.length >> 8; z[o++] = ~chunk.length & 255; z[o++] = (~chunk.length >> 8) & 255;
      z.set(chunk, o); o += chunk.length;
    }
    for (let i = 0; i < raw.length; i++) { a = (a + raw[i]) % 65521; b = (b + a) % 65521; }
    z[o++] = b >> 8; z[o++] = b & 255; z[o++] = a >> 8; z[o++] = a & 255;
    const crc32 = g.__rhStreams ? g.__rhStreams.crc32 : () => 0;
    const chunkOf = (type, body) => {
      const out = new Uint8Array(12 + body.length);
      const n = body.length;
      out[0] = n >>> 24; out[1] = (n >>> 16) & 255; out[2] = (n >>> 8) & 255; out[3] = n & 255;
      for (let i = 0; i < 4; i++) out[4 + i] = type.charCodeAt(i);
      out.set(body, 8);
      const c = crc32(out.subarray(4, 8 + n));
      out[8 + n] = c >>> 24; out[9 + n] = (c >>> 16) & 255; out[10 + n] = (c >>> 8) & 255; out[11 + n] = c & 255;
      return out;
    };
    const ihdr = new Uint8Array([w >>> 24, (w >>> 16) & 255, (w >>> 8) & 255, w & 255, h >>> 24, (h >>> 16) & 255, (h >>> 8) & 255, h & 255, 8, 6, 0, 0, 0]);
    const parts = [new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]), chunkOf('IHDR', ihdr), chunkOf('IDAT', z), chunkOf('IEND', new Uint8Array(0))];
    const total = parts.reduce((s, p) => s + p.length, 0);
    const png = new Uint8Array(total);
    let off = 0;
    for (const p of parts) { png.set(p, off); off += p.length; }
    return png;
  }
  const b64 = (bytes) => { let s = ''; for (let i = 0; i < bytes.length; i += 8192) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 8192)); return g.btoa(s); };

  // -------------------------------------------------------- HTMLCanvasElement
  const contexts = new WeakMap();
  function makeContext(owner, surface, attrs, C) {
    const ctx = Object.create(C.prototype);
    hidden(ctx, S, { canvas: owner, surface, attrs: { alpha: attrs.alpha !== false, colorSpace: attrs.colorSpace || 'srgb', desynchronized: !!attrs.desynchronized, willReadFrequently: !!attrs.willReadFrequently, colorType: 'unorm8' } });
    resetState(ctx);
    surface.ctx = ctx;
    return ctx;
  }
  const CE = g.HTMLCanvasElement;
  hidden(CE.prototype, 'getContext', function getContext(type, attrs = {}) {
    if (arguments.length < 1) throw new TypeError("Failed to execute 'getContext' on 'HTMLCanvasElement': 1 argument required, but only 0 present.");
    type = String(type);
    let rec = contexts.get(this);
    if (rec) return rec.type === type ? rec.ctx : null;
    if (type === '2d') {
      const ctx = makeContext(this, surfaceOf(this), attrs || {}, CanvasRenderingContext2D);
      contexts.set(this, { type, ctx });
      return ctx;
    }
    if (type === 'bitmaprenderer') {
      const ctx = Object.create(ImageBitmapRenderingContext.prototype);
      hidden(ctx, S, { canvas: this, surface: surfaceOf(this) });
      contexts.set(this, { type, ctx });
      return ctx;
    }
    // No WebGL / WebGPU.
    return null;
  });
  hidden(CE.prototype, 'toDataURL', function toDataURL(type = 'image/png') {
    const s = surfaceOf(this);
    if (!s.width || !s.height) return 'data:,';
    return 'data:image/png;base64,' + b64(encodePNG(s.width, s.height, s.data));
  });
  hidden(CE.prototype, 'toBlob', function toBlob(cb, type, quality) {
    if (typeof cb !== 'function') throw new TypeError("Failed to execute 'toBlob' on 'HTMLCanvasElement': The callback provided as parameter 1 is not a function.");
    const s = surfaceOf(this);
    const blob = s.width && s.height ? new g.Blob([encodePNG(s.width, s.height, s.data)], { type: 'image/png' }) : null;
    setTimeout(() => cb(blob), 0);
    void type; void quality;
  });
  hidden(CE.prototype, 'captureStream', function () { return null; });

  // ------------------------------------------------------------ OffscreenCanvas
  class OffscreenCanvas extends g.EventTarget {
    constructor(w, h) { super(); hidden(this, S, { surface: newSurface(w >>> 0, h >>> 0, this), ctx: null, type: null }); }
    get width() { return this[S].surface.width; }
    set width(v) { const s = this[S].surface; s.width = v >>> 0; s.data = new Uint8ClampedArray(s.width * s.height * 4); if (this[S].ctx && this[S].type === '2d') resetState(this[S].ctx); }
    get height() { return this[S].surface.height; }
    set height(v) { const s = this[S].surface; s.height = v >>> 0; s.data = new Uint8ClampedArray(s.width * s.height * 4); if (this[S].ctx && this[S].type === '2d') resetState(this[S].ctx); }
    getContext(type, attrs = {}) {
      type = String(type);
      if (this[S].ctx) return this[S].type === type ? this[S].ctx : null;
      if (type === '2d') { this[S].ctx = makeContext(this, this[S].surface, attrs || {}, OffscreenCanvasRenderingContext2D); this[S].type = type; return this[S].ctx; }
      if (type === 'bitmaprenderer') { const c = Object.create(ImageBitmapRenderingContext.prototype); hidden(c, S, { canvas: this, surface: this[S].surface }); this[S].ctx = c; this[S].type = type; return c; }
      return null;
    }
    transferToImageBitmap() { const s = this[S].surface; const bm = makeBitmap(s.width, s.height, s.data.slice()); s.data.fill(0); return bm; }
    convertToBlob(options = {}) { const s = this[S].surface; if (!s.width || !s.height) return Promise.reject(domError('The image is empty.', 'IndexSizeError')); void options; return Promise.resolve(new g.Blob([encodePNG(s.width, s.height, s.data)], { type: 'image/png' })); }
  }
  tag(OffscreenCanvas, 'OffscreenCanvas');
  N.installHandlers(OffscreenCanvas.prototype, ['contextlost', 'contextrestored']);
  class OffscreenCanvasRenderingContext2D {
    constructor() { ILLEGAL(); }
  }
  for (const k of Reflect.ownKeys(CanvasRenderingContext2D.prototype)) {
    if (k === 'constructor' || k === Symbol.toStringTag || k === 'drawFocusIfNeeded' || k === 'scrollPathIntoView' || k === 'getContextAttributes') continue;
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
        if (!sw || !sh) { reject(domError("The crop rect width or height is 0.", 'IndexSizeError')); return; }
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
          const img = new g.Image();
          const url = URL.createObjectURL(source);
          img.onload = () => { try { done(sourcePixels(img)); } catch (e) { reject(e); } URL.revokeObjectURL(url); };
          img.onerror = () => reject(domError('The source image could not be decoded.', 'InvalidStateError'));
          img.src = url;
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
      const s = this[S].surface;
      if (bm === null) { s.data.fill(0); markDirty(s); return; }
      if (bm[S].closed) throw domError("Failed to execute 'transferFromImageBitmap': The input ImageBitmap has been detached", 'InvalidStateError');
      if (this[S].canvas instanceof g.HTMLCanvasElement) { this[S].canvas.width = bm.width; this[S].canvas.height = bm.height; }
      const t = this[S].canvas instanceof g.HTMLCanvasElement ? surfaceOf(this[S].canvas) : s;
      t.data.set(bm[S].data.subarray(0, t.data.length));
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
