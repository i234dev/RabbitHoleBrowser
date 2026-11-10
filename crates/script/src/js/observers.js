// IntersectionObserver (https://w3c.github.io/IntersectionObserver/) and
// ResizeObserver (https://drafts.csswg.org/resize-observer/), driven by the
// real layout: the host runs the "update the rendering" hook below after
// DOM changes, scrolling and viewport changes.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('observer');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const ios = new Set();
  const ros = new Set();
  const now = () => g.performance.now();
  const rect = (x, y, w, h) => new g.DOMRectReadOnly(x, y, w, h);
  const isElement = (v) => v instanceof g.Element;

  function updateActive() { N.setRenderObservers(ios.size + ros.size > 0); }

  // --------------------------------------------------- IntersectionObserver

  function parseMargin(text, what) {
    const parts = String(text).trim().split(/\s+/).filter(Boolean);
    if (parts.length > 4) throw new SyntaxError(`Failed to construct 'IntersectionObserver': Extra text found at the end of ${what}.`);
    const vals = parts.map((p) => {
      const m = /^(-?\d*\.?\d+(?:e[+-]?\d+)?)(px|%)?$/i.exec(p);
      if (!m || (!m[2] && parseFloat(m[1]) !== 0)) throw new SyntaxError(`Failed to construct 'IntersectionObserver': ${what} must be specified in pixels or percent.`);
      return { v: parseFloat(m[1]), pct: m[2] === '%' };
    });
    if (vals.length === 0) vals.push({ v: 0, pct: false });
    while (vals.length < 4) vals.push(vals.length === 1 ? vals[0] : vals.length === 2 ? vals[0] : vals[1]);
    if (vals.length === 4 && parts.length === 3) vals[3] = vals[1];
    return vals;
  }
  const marginText = (m) => m.map((x) => x.v + (x.pct ? '%' : 'px')).join(' ');

  class IntersectionObserverEntry {
    constructor(init) {
      init = init || {};
      hidden(this, S, {
        time: +init.time || 0,
        rootBounds: init.rootBounds ? g.DOMRectReadOnly.fromRect(init.rootBounds) : null,
        boundingClientRect: g.DOMRectReadOnly.fromRect(init.boundingClientRect || {}),
        intersectionRect: g.DOMRectReadOnly.fromRect(init.intersectionRect || {}),
        isIntersecting: !!init.isIntersecting,
        isVisible: !!init.isVisible,
        intersectionRatio: +init.intersectionRatio || 0,
        target: init.target || null,
      });
    }
    get time() { return this[S].time; }
    get rootBounds() { return this[S].rootBounds; }
    get boundingClientRect() { return this[S].boundingClientRect; }
    get intersectionRect() { return this[S].intersectionRect; }
    get isIntersecting() { return this[S].isIntersecting; }
    get isVisible() { return this[S].isVisible; }
    get intersectionRatio() { return this[S].intersectionRatio; }
    get target() { return this[S].target; }
    toJSON() { const o = {}; for (const k of ['time', 'rootBounds', 'boundingClientRect', 'intersectionRect', 'isIntersecting', 'isVisible', 'intersectionRatio', 'target']) o[k] = this[k]; return o; }
  }
  tag(IntersectionObserverEntry, 'IntersectionObserverEntry');

  class IntersectionObserver {
    constructor(callback, options = {}) {
      if (typeof callback !== 'function') throw new TypeError("Failed to construct 'IntersectionObserver': parameter 1 is not of type 'Function'.");
      options = options || {};
      const root = options.root === undefined ? null : options.root;
      if (root !== null && !isElement(root) && !(root instanceof g.Document)) throw new TypeError("Failed to construct 'IntersectionObserver': Failed to read the 'root' property from 'IntersectionObserverInit': The provided value is not of type '(Document or Element)'.");
      let thr = options.threshold === undefined ? [0] : options.threshold;
      thr = (Array.isArray(thr) ? thr : [thr]).map(Number);
      for (const t of thr) if (!(t >= 0 && t <= 1)) throw new RangeError("Failed to construct 'IntersectionObserver': Threshold values must be numbers between 0 and 1");
      if (thr.length === 0) thr = [0];
      thr.sort((a, b) => a - b);
      const margin = parseMargin(options.rootMargin === undefined ? '0px' : options.rootMargin, 'rootMargin');
      const scrollMargin = parseMargin(options.scrollMargin === undefined ? '0px' : options.scrollMargin, 'scrollMargin');
      hidden(this, S, { callback, root, thresholds: Object.freeze(thr), margin, scrollMargin, delay: +options.delay || 0, trackVisibility: !!options.trackVisibility, targets: new Map(), records: [] });
    }
    get root() { return this[S].root; }
    get rootMargin() { return marginText(this[S].margin); }
    get scrollMargin() { return marginText(this[S].scrollMargin); }
    get thresholds() { return this[S].thresholds; }
    get delay() { return this[S].delay; }
    get trackVisibility() { return this[S].trackVisibility; }
    observe(target) {
      if (!isElement(target)) throw new TypeError("Failed to execute 'observe' on 'IntersectionObserver': parameter 1 is not of type 'Element'.");
      if (this[S].targets.has(target)) return;
      this[S].targets.set(target, { index: -1, intersecting: false });
      ios.add(this);
      updateActive();
      N.requestRenderUpdate();
    }
    unobserve(target) {
      this[S].targets.delete(target);
      if (!this[S].targets.size) { ios.delete(this); updateActive(); }
    }
    disconnect() { this[S].targets.clear(); ios.delete(this); updateActive(); }
    takeRecords() { const r = this[S].records; this[S].records = []; return r; }
  }
  tag(IntersectionObserver, 'IntersectionObserver');

  function rootBoundsOf(io) {
    const st = io[S];
    let b;
    if (st.root === null || st.root instanceof g.Document) {
      b = { x: 0, y: 0, w: g.innerWidth, h: g.innerHeight };
    } else {
      if (!st.root.isConnected) return null;
      const r = st.root.getBoundingClientRect();
      b = { x: r.left + (st.root.clientLeft || 0), y: r.top + (st.root.clientTop || 0), w: st.root.clientWidth || r.width, h: st.root.clientHeight || r.height };
    }
    const m = st.margin.map((x, i) => (x.pct ? (x.v / 100) * (i % 2 === 0 ? b.h : b.w) : x.v));
    return { x: b.x - m[3], y: b.y - m[0], w: b.w + m[1] + m[3], h: b.h + m[0] + m[2] };
  }

  function checkIntersections(time) {
    const fire = [];
    for (const io of Array.from(ios)) {
      const st = io[S];
      const rb = rootBoundsOf(io);
      for (const [target, prev] of st.targets) {
        let tr = { x: 0, y: 0, w: 0, h: 0 };
        let ir = { x: 0, y: 0, w: 0, h: 0 };
        let intersecting = false;
        const inRoot = target.isConnected && (st.root === null || st.root instanceof g.Document ? true : st.root !== target && st.root.contains(target));
        let rendered = false;
        if (inRoot) {
          const r = target.getBoundingClientRect();
          tr = { x: r.left, y: r.top, w: r.width, h: r.height };
          rendered = r.width > 0 || r.height > 0 || target.getClientRects().length > 0;
        }
        if (inRoot && rendered && rb) {
          const x1 = Math.max(tr.x, rb.x), y1 = Math.max(tr.y, rb.y);
          const x2 = Math.min(tr.x + tr.w, rb.x + rb.w), y2 = Math.min(tr.y + tr.h, rb.y + rb.h);
          if (x2 >= x1 && y2 >= y1) {
            intersecting = true;
            ir = { x: x1, y: y1, w: x2 - x1, h: y2 - y1 };
          }
        }
        const area = tr.w * tr.h;
        const ratio = intersecting ? (area > 0 ? (ir.w * ir.h) / area : 1) : 0;
        let index = st.thresholds.findIndex((t) => t > ratio);
        if (index < 0) index = st.thresholds.length;
        // A zero threshold means "any intersection", even edge-adjacent.
        if (intersecting && ratio === 0 && st.thresholds[0] === 0) index = Math.max(index, 1);
        if (index !== prev.index || intersecting !== prev.intersecting) {
          prev.index = index;
          prev.intersecting = intersecting;
          st.records.push(new IntersectionObserverEntry({
            time,
            rootBounds: rb ? rect(rb.x, rb.y, rb.w, rb.h) : null,
            boundingClientRect: rect(tr.x, tr.y, tr.w, tr.h),
            intersectionRect: rect(ir.x, ir.y, ir.w, ir.h),
            isIntersecting: intersecting,
            isVisible: false,
            intersectionRatio: ratio,
            target,
          }));
        }
      }
      if (st.records.length) fire.push(io);
    }
    for (const io of fire) {
      const records = io.takeRecords();
      if (!records.length) continue;
      try { io[S].callback.call(io, records, io); } catch (e) { reportError(e); }
    }
  }

  // ---------------------------------------------------------- ResizeObserver

  class ResizeObserverSize {
    constructor() { ILLEGAL(); }
    get inlineSize() { return this[S].inline; }
    get blockSize() { return this[S].block; }
  }
  tag(ResizeObserverSize, 'ResizeObserverSize');
  const roSize = (inline, block) => { const s = Object.create(ResizeObserverSize.prototype); hidden(s, S, { inline, block }); return s; };

  class ResizeObserverEntry {
    constructor() { ILLEGAL(); }
    get target() { return this[S].target; }
    get contentRect() { return this[S].contentRect; }
    get borderBoxSize() { return this[S].border; }
    get contentBoxSize() { return this[S].content; }
    get devicePixelContentBoxSize() { return this[S].device; }
  }
  tag(ResizeObserverEntry, 'ResizeObserverEntry');

  class ResizeObserver {
    constructor(callback) {
      if (typeof callback !== 'function') throw new TypeError("Failed to construct 'ResizeObserver': parameter 1 is not of type 'Function'.");
      hidden(this, S, { callback, targets: new Map() });
    }
    observe(target, options) {
      if (!isElement(target)) throw new TypeError("Failed to execute 'observe' on 'ResizeObserver': parameter 1 is not of type 'Element'.");
      const box = (options && options.box) || 'content-box';
      if (!['content-box', 'border-box', 'device-pixel-content-box'].includes(box)) throw new TypeError(`Failed to execute 'observe' on 'ResizeObserver': The provided value '${box}' is not a valid enum value of type ResizeObserverBoxOptions.`);
      this[S].targets.set(target, { box, last: null });
      ros.add(this);
      updateActive();
      N.requestRenderUpdate();
    }
    unobserve(target) {
      this[S].targets.delete(target);
      if (!this[S].targets.size) { ros.delete(this); updateActive(); }
    }
    disconnect() { this[S].targets.clear(); ros.delete(this); updateActive(); }
  }
  tag(ResizeObserver, 'ResizeObserver');

  function boxesOf(el) {
    const r = el.getBoundingClientRect();
    if (!el.isConnected || (r.width === 0 && r.height === 0 && el.getClientRects().length === 0)) return { bw: 0, bh: 0, cw: 0, ch: 0, pl: 0, pt: 0 };
    let pl = 0, pt = 0, pr = 0, pb = 0, bl = 0, bt = 0, br = 0, bb = 0;
    try {
      const cs = g.getComputedStyle(el);
      const px = (v) => parseFloat(v) || 0;
      pl = px(cs.paddingLeft); pt = px(cs.paddingTop); pr = px(cs.paddingRight); pb = px(cs.paddingBottom);
      bl = px(cs.borderLeftWidth); bt = px(cs.borderTopWidth); br = px(cs.borderRightWidth); bb = px(cs.borderBottomWidth);
    } catch (e) {}
    const cw = Math.max(0, r.width - pl - pr - bl - br), ch = Math.max(0, r.height - pt - pb - bt - bb);
    return { bw: r.width, bh: r.height, cw, ch, pl, pt };
  }

  function checkResizes() {
    for (const ro of Array.from(ros)) {
      const st = ro[S];
      const entries = [];
      for (const [target, rec] of st.targets) {
        const b = boxesOf(target);
        const size = rec.box === 'border-box' ? [b.bw, b.bh] : [b.cw, b.ch];
        if (rec.last && rec.last[0] === size[0] && rec.last[1] === size[1]) continue;
        // The first observation of a 0x0 target is reported too (its
        // initial "last size" is -1 x -1 per spec... except 0x0 matches 0x0).
        if (!rec.last && size[0] === 0 && size[1] === 0 && b.bw === 0) { rec.last = size; continue; }
        rec.last = size;
        const e = Object.create(ResizeObserverEntry.prototype);
        const dpr = g.devicePixelRatio || 1;
        hidden(e, S, {
          target,
          contentRect: rect(b.pl, b.pt, b.cw, b.ch),
          border: Object.freeze([roSize(b.bw, b.bh)]),
          content: Object.freeze([roSize(b.cw, b.ch)]),
          device: Object.freeze([roSize(Math.round(b.cw * dpr), Math.round(b.ch * dpr))]),
        });
        entries.push(e);
      }
      if (entries.length) {
        try { st.callback.call(ro, entries, ro); } catch (e) { reportError(e); }
      }
    }
  }

  function reportError(e) {
    try { console.error('Uncaught ' + (e && e.stack ? e.stack : e)); } catch (_) {}
  }

  // The host calls this during "update the rendering".
  N.setRenderHook(function () {
    const t = now();
    if (ros.size) checkResizes();
    if (ios.size) checkIntersections(t);
  });

  define('IntersectionObserver', IntersectionObserver);
  define('IntersectionObserverEntry', IntersectionObserverEntry);
  define('ResizeObserver', ResizeObserver);
  define('ResizeObserverEntry', ResizeObserverEntry);
  define('ResizeObserverSize', ResizeObserverSize);
})();
