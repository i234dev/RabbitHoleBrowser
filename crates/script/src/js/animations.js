// Web Animations: AnimationTimeline / DocumentTimeline, AnimationEffect /
// KeyframeEffect (keyframe processing, timing model), Animation (the
// play / pause / finish / cancel / reverse / playback-rate procedures,
// ready and finished promises, finish / cancel / remove events, replaced
// animations, commitStyles), AnimationPlaybackEvent, Element.animate and
// getAnimations, and CSSAnimation / CSSTransition objects for the
// engine's CSS animations and transitions.
//
// Effects render: every effect in effect is handed to the engine
// (__rhNative.animFrames / animProgress) with its eased iteration progress;
// the engine interpolates the keyframes over the element's style. The
// document timeline advances at animation frames, where the host calls
// the hook below to update animations (phase 0) and send their events
// (phase 1).
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('anim');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const abortError = () => domError('The user aborted a request.', 'AbortError');
  const handled = (p) => { p.catch(() => {}); return p; };

  // ------------------------------------------------------------ easing
  // Parsed timing functions: { css: canonical text, fn: (x, before) => y }.
  function cssUnescape(s) {
    s = s.replace(/\/\*[\s\S]*?\*\//g, ' ');
    return s.replace(/\\([0-9a-fA-F]{1,6})\s?|\\([^\n0-9a-fA-F])/g, (m, hex, ch) => (hex ? String.fromCodePoint(parseInt(hex, 16)) : ch));
  }
  function bezier(x1, y1, x2, y2) {
    const cx = 3 * x1, bx = 3 * (x2 - x1) - cx, ax = 1 - cx - bx;
    const cy = 3 * y1, by = 3 * (y2 - y1) - cy, ay = 1 - cy - by;
    const sx = (t) => ((ax * t + bx) * t + cx) * t;
    const sy = (t) => ((ay * t + by) * t + cy) * t;
    const dx = (t) => (3 * ax * t + 2 * bx) * t + cx;
    const solve = (x) => {
      let t = x;
      for (let i = 0; i < 8; i++) {
        const e = sx(t) - x;
        if (Math.abs(e) < 1e-7) return t;
        const d = dx(t);
        if (Math.abs(d) < 1e-6) break;
        t -= e / d;
      }
      let lo = 0, hi = 1;
      t = x;
      for (let i = 0; i < 60; i++) {
        const v = sx(t);
        if (Math.abs(v - x) < 1e-7) return t;
        if (x > v) lo = t; else hi = t;
        t = (hi - lo) / 2 + lo;
      }
      return t;
    };
    return (x) => {
      if (x < 0) {
        // Tangent at the start.
        const grad = x1 > 0 ? y1 / x1 : (y1 === 0 && x2 > 0 ? y2 / x2 : 0);
        return grad * x;
      }
      if (x > 1) {
        const grad = x2 < 1 ? (y2 - 1) / (x2 - 1) : (y2 === 1 && x1 < 1 ? (y1 - 1) / (x1 - 1) : 0);
        return 1 + grad * (x - 1);
      }
      if (x === 0 || x === 1) return x;
      return sy(solve(x));
    };
  }
  function steps(n, pos) {
    const jumps = pos === 'jump-none' ? n - 1 : pos === 'jump-both' ? n + 1 : n;
    const start = pos === 'start' || pos === 'jump-start' || pos === 'jump-both';
    return (x, before) => {
      let step = Math.floor(x * n);
      if (start) step += 1;
      if (before && (x * n) % 1 === 0) step -= 1;
      if (x >= 0 && step < 0) step = 0;
      if (x <= 1 && step > jumps) step = jumps;
      return step / jumps;
    };
  }
  const fmt = (n) => String(Math.round(n * 1e6) / 1e6);
  const KEYWORDS = {
    ease: [0.25, 0.1, 0.25, 1], 'ease-in': [0.42, 0, 1, 1], 'ease-out': [0, 0, 0.58, 1], 'ease-in-out': [0.42, 0, 0.58, 1],
  };
  const NUM = /^[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?$/;
  const easingCache = new Map();
  function parseEasing(text) {
    const key = String(text);
    let e = easingCache.get(key);
    if (e) return e;
    const s = cssUnescape(key).trim().toLowerCase().replace(/\s+/g, ' ');
    e = null;
    if (s === 'linear') e = { css: 'linear', fn: (x) => x };
    else if (KEYWORDS[s]) e = { css: s, fn: bezier(...KEYWORDS[s]) };
    else if (s === 'step-start') e = { css: 'steps(1, start)', fn: steps(1, 'start') };
    else if (s === 'step-end') e = { css: 'steps(1)', fn: steps(1, 'end') };
    else {
      const m = /^([a-z-]+)\((.*)\)$/.exec(s);
      if (m) {
        const args = m[2].split(',').map((a) => a.trim());
        if (m[1] === 'cubic-bezier' && args.length === 4 && args.every((a) => NUM.test(a))) {
          const v = args.map(Number);
          if (v[0] >= 0 && v[0] <= 1 && v[2] >= 0 && v[2] <= 1) e = { css: `cubic-bezier(${v.map(fmt).join(', ')})`, fn: bezier(...v) };
        } else if (m[1] === 'steps' && (args.length === 1 || args.length === 2) && /^[+]?\d+$/.test(args[0])) {
          const n = Number(args[0]);
          const pos = args[1] === undefined ? 'end' : args[1];
          const ok = ['start', 'end', 'jump-start', 'jump-end', 'jump-none', 'jump-both'].includes(pos);
          if (ok && n >= (pos === 'jump-none' ? 2 : 1)) {
            const css = pos === 'end' || pos === 'jump-end' ? `steps(${n})` : `steps(${n}, ${pos})`;
            e = { css, fn: steps(n, pos) };
          }
        } else if (m[1] === 'linear' && args.length >= 1 && args[0] !== '') {
          e = parseLinear(args);
        }
      }
    }
    if (!e) throw new TypeError(`'${key}' is not a valid value for easing`);
    if (easingCache.size > 256) easingCache.clear();
    easingCache.set(key, e);
    return e;
  }
  // linear(<number> [<percentage>{1,2}]?, ...)
  function parseLinear(args) {
    const pts = [];
    for (const a of args) {
      const parts = a.split(' ').filter(Boolean);
      if (parts.length < 1 || parts.length > 3 || !NUM.test(parts[0])) return null;
      const out = Number(parts[0]);
      const pcts = parts.slice(1).map((p) => (/^[-+]?(\d+\.?\d*|\.\d+)%$/.test(p) ? Number(p.slice(0, -1)) / 100 : NaN));
      if (pcts.some(Number.isNaN)) return null;
      if (pcts.length === 0) pts.push({ out, in: null });
      for (const p of pcts) pts.push({ out, in: p });
    }
    if (pts.length < 2) return null;
    if (pts[0].in === null) pts[0].in = 0;
    if (pts[pts.length - 1].in === null) pts[pts.length - 1].in = 1;
    let max = -Infinity;
    for (const p of pts) if (p.in !== null) { p.in = Math.max(p.in, max); max = p.in; }
    for (let i = 0; i < pts.length; i++) {
      if (pts[i].in !== null) continue;
      let j = i; while (pts[j].in === null) j++;
      const a = pts[i - 1].in, b = pts[j].in;
      for (let k = i; k < j; k++) pts[k].in = a + ((b - a) * (k - i + 1)) / (j - i + 1);
      i = j;
    }
    const css = `linear(${pts.map((p) => `${fmt(p.out)} ${fmt(p.in * 100)}%`).join(', ')})`;
    const fn = (x) => {
      let i = 0;
      if (x >= pts[pts.length - 1].in) i = pts.length - 2;
      else while (i < pts.length - 2 && x >= pts[i + 1].in) i++;
      while (i > 0 && pts[i].in === pts[i + 1].in) i--;
      const p = pts[i], q = pts[i + 1];
      if (q.in === p.in) return q.out;
      return p.out + ((q.out - p.out) * (x - p.in)) / (q.in - p.in);
    };
    return { css, fn };
  }

  // ------------------------------------------------------- properties
  // Animatable CSS properties (CSS names).
  const ANIMATABLE = new Set((
    'accent-color align-content align-items align-self all aspect-ratio backdrop-filter background background-attachment background-blend-mode ' +
    'background-clip background-color background-image background-origin background-position background-position-x background-position-y ' +
    'background-repeat background-size block-size border border-block border-block-color border-block-end border-block-end-color ' +
    'border-block-end-style border-block-end-width border-block-start border-block-start-color border-block-start-style border-block-start-width ' +
    'border-block-style border-block-width border-bottom border-bottom-color border-bottom-left-radius border-bottom-right-radius ' +
    'border-bottom-style border-bottom-width border-collapse border-color border-end-end-radius border-end-start-radius border-image ' +
    'border-image-outset border-image-repeat border-image-slice border-image-source border-image-width border-inline border-inline-color ' +
    'border-inline-end border-inline-end-color border-inline-end-style border-inline-end-width border-inline-start border-inline-start-color ' +
    'border-inline-start-style border-inline-start-width border-inline-style border-inline-width border-left border-left-color ' +
    'border-left-style border-left-width border-radius border-right border-right-color border-right-style border-right-width border-spacing ' +
    'border-start-end-radius border-start-start-radius border-style border-top border-top-color border-top-left-radius border-top-right-radius ' +
    'border-top-style border-top-width border-width bottom box-decoration-break box-shadow box-sizing break-after break-before break-inside ' +
    'caption-side caret-color clear clip clip-path clip-rule color color-interpolation color-interpolation-filters color-scheme column-count ' +
    'column-fill column-gap column-rule column-rule-color column-rule-style column-rule-width column-span column-width columns content ' +
    'counter-increment counter-reset counter-set cursor cx cy d display dominant-baseline empty-cells fill fill-opacity fill-rule filter flex ' +
    'flex-basis flex-direction flex-flow flex-grow flex-shrink flex-wrap float flood-color flood-opacity font font-family font-feature-settings ' +
    'font-kerning font-language-override font-optical-sizing font-palette font-size font-size-adjust font-stretch font-style font-synthesis ' +
    'font-variant font-variant-caps font-variant-east-asian font-variant-ligatures font-variant-numeric font-variant-position ' +
    'font-variation-settings font-weight gap grid grid-area grid-auto-columns grid-auto-flow grid-auto-rows grid-column grid-column-end ' +
    'grid-column-gap grid-column-start grid-gap grid-row grid-row-end grid-row-gap grid-row-start grid-template grid-template-areas ' +
    'grid-template-columns grid-template-rows height hyphens image-orientation image-rendering inline-size inset inset-block inset-block-end ' +
    'inset-block-start inset-inline inset-inline-end inset-inline-start isolation justify-content justify-items justify-self left ' +
    'letter-spacing lighting-color line-break line-height list-style list-style-image list-style-position list-style-type margin margin-block ' +
    'margin-block-end margin-block-start margin-bottom margin-inline margin-inline-end margin-inline-start margin-left margin-right margin-top ' +
    'marker marker-end marker-mid marker-start mask mask-border mask-clip mask-composite mask-image mask-mode mask-origin mask-position ' +
    'mask-repeat mask-size mask-type max-block-size max-height max-inline-size max-width min-block-size min-height min-inline-size min-width ' +
    'mix-blend-mode object-fit object-position offset offset-anchor offset-distance offset-path offset-position offset-rotate opacity order ' +
    'outline outline-color outline-offset outline-style outline-width overflow overflow-anchor overflow-clip-margin overflow-wrap overflow-x ' +
    'overflow-y overscroll-behavior overscroll-behavior-x overscroll-behavior-y padding padding-block padding-block-end padding-block-start ' +
    'padding-bottom padding-inline padding-inline-end padding-inline-start padding-left padding-right padding-top page-break-after ' +
    'page-break-before page-break-inside paint-order perspective perspective-origin place-content place-items place-self pointer-events ' +
    'position quotes r resize right rotate row-gap rx ry scale scroll-behavior scroll-margin scroll-padding scroll-snap-align scroll-snap-stop ' +
    'scroll-snap-type scrollbar-color scrollbar-gutter scrollbar-width shape-image-threshold shape-margin shape-outside shape-rendering ' +
    'stop-color stop-opacity stroke stroke-dasharray stroke-dashoffset stroke-linecap stroke-linejoin stroke-miterlimit stroke-opacity ' +
    'stroke-width tab-size table-layout text-align text-align-last text-anchor text-decoration text-decoration-color text-decoration-line ' +
    'text-decoration-skip-ink text-decoration-style text-decoration-thickness text-emphasis text-emphasis-color text-emphasis-position ' +
    'text-emphasis-style text-indent text-justify text-overflow text-rendering text-shadow text-transform text-underline-offset ' +
    'text-underline-position text-wrap top touch-action transform transform-box transform-origin transform-style translate user-select ' +
    'vector-effect vertical-align visibility white-space widows orphans width word-break word-spacing word-wrap x y z-index zoom ' +
    'alignment-baseline anchor-name anchor-scope appearance backface-visibility block-step-align block-step-insert block-step-round ' +
    'block-step-size buffered-rendering color-adjust column-height column-wrap field-sizing font-variant-alternates font-variant-emoji ' +
    'font-width hanging-punctuation ime-mode initial-letter mask-position-x mask-position-y math-depth math-shift math-style path-length ' +
    'position-anchor position-try-fallbacks position-try-order position-visibility ruby-align ruby-position stroke-color text-autospace ' +
    'text-box text-box-edge text-box-trim text-decoration-skip text-group-align text-spacing-trim view-transition-class view-transition-name ' +
    'white-space-collapse position-area position-try inset-area overlay content-visibility scroll-timeline view-timeline timeline-scope'
  ).split(' '));
  // IDL attribute name -> CSS property name, or null when not animatable.
  function cssName(idl) {
    if (typeof idl !== 'string') return null;
    if (idl.startsWith('--')) return idl;
    if (idl === 'cssFloat') return 'float';
    if (idl === 'cssOffset') return 'offset';
    if (idl === 'float' || idl === 'offset' || idl.includes('-') || /[A-Z]/.test(idl[0])) return null;
    const css = idl.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());
    return ANIMATABLE.has(css) ? css : null;
  }
  function idlName(css) {
    if (css.startsWith('--')) return css;
    if (css === 'float') return 'cssFloat';
    if (css === 'offset') return 'cssOffset';
    return css.replace(/-([a-z])/g, (m, c) => c.toUpperCase());
  }

  // ----------------------------------------------------------- timing
  const FILLS = ['none', 'forwards', 'backwards', 'both', 'auto'];
  const DIRECTIONS = ['normal', 'reverse', 'alternate', 'alternate-reverse'];
  const COMPOSITES = ['replace', 'add', 'accumulate'];
  const TIMING_DEFAULTS = { delay: 0, direction: 'normal', duration: 'auto', easing: 'linear', endDelay: 0, fill: 'auto', iterationStart: 0, iterations: 1 };
  const finiteDouble = (v, what) => {
    const n = Number(v);
    if (!Number.isFinite(n)) throw new TypeError(`Failed to read the '${what}' property: The provided double value is non-finite.`);
    return n;
  };
  // Validate and apply the members of an (Optional)EffectTiming dictionary.
  function applyTiming(t, input) {
    const out = Object.assign({}, t);
    if (input === undefined || input === null) return out;
    if (typeof input !== 'object' && typeof input !== 'function') throw new TypeError('EffectTiming must be an object');
    // Dictionary members in lexicographic order.
    let v = input.delay;
    if (v !== undefined) out.delay = finiteDouble(v, 'delay');
    v = input.direction;
    if (v !== undefined) { v = String(v); if (!DIRECTIONS.includes(v)) throw new TypeError(`The provided value '${v}' is not a valid enum value of type PlaybackDirection.`); out.direction = v; }
    v = input.duration;
    if (v !== undefined) {
      if (typeof v === 'string') {
        if (v !== 'auto') throw new TypeError(`Failed to read the 'duration' property: '${v}' is not a valid duration.`);
      } else {
        v = Number(v);
        if (Number.isNaN(v) || v < 0) throw new TypeError("Failed to read the 'duration' property: duration must be non-negative or auto.");
      }
      out.duration = v;
    }
    v = input.easing;
    if (v !== undefined) out.easing = parseEasing(String(v)).css;
    v = input.endDelay;
    if (v !== undefined) out.endDelay = finiteDouble(v, 'endDelay');
    v = input.fill;
    if (v !== undefined) { v = String(v); if (!FILLS.includes(v)) throw new TypeError(`The provided value '${v}' is not a valid enum value of type FillMode.`); out.fill = v; }
    v = input.iterationStart;
    if (v !== undefined) { v = finiteDouble(v, 'iterationStart'); if (v < 0) throw new TypeError("Failed to read the 'iterationStart' property: iterationStart must be non-negative."); out.iterationStart = v; }
    v = input.iterations;
    if (v !== undefined) { v = Number(v); if (Number.isNaN(v) || v < 0) throw new TypeError("Failed to read the 'iterations' property: iterationCount must be non-negative."); out.iterations = v; }
    return out;
  }
  function timingFromOptions(options) {
    if (options === undefined || options === null) return Object.assign({}, TIMING_DEFAULTS);
    if (typeof options !== 'object' && typeof options !== 'function') {
      const d = Number(options);
      if (Number.isNaN(d) || d < 0) throw new TypeError("Failed to read the 'duration' property: duration must be non-negative or auto.");
      return Object.assign({}, TIMING_DEFAULTS, { duration: d });
    }
    return applyTiming(TIMING_DEFAULTS, options);
  }
  const iterDuration = (t) => (t.duration === 'auto' ? 0 : t.duration);
  const activeDuration = (t) => { const d = iterDuration(t); return d === 0 || t.iterations === 0 ? 0 : d * t.iterations; };
  const endTimeOf = (t) => Math.max(t.delay + activeDuration(t) + t.endDelay, 0);

  // Effect timing at `local` (ms or null) for an animation playing
  // forwards (`backwards` false) or backwards.
  function computeTiming(t, local, backwards) {
    const dur = iterDuration(t);
    const active = activeDuration(t);
    const end = endTimeOf(t);
    const fill = t.fill === 'auto' ? 'none' : t.fill;
    const r = { phase: 'idle', activeTime: null, progress: null, iteration: null, active, end, dur };
    if (local === null || local === undefined) return r;
    const beforeEnd = Math.max(Math.min(t.delay, end), 0);
    const activeEnd = Math.max(Math.min(t.delay + active, end), 0);
    if (local < beforeEnd || (backwards && local === beforeEnd)) r.phase = 'before';
    else if (local > activeEnd || (!backwards && local === activeEnd)) r.phase = 'after';
    else r.phase = 'active';
    if (r.phase === 'before') { if (fill === 'backwards' || fill === 'both') r.activeTime = Math.max(local - t.delay, 0); }
    else if (r.phase === 'active') r.activeTime = local - t.delay;
    else if (fill === 'forwards' || fill === 'both') r.activeTime = Math.max(Math.min(local - t.delay, active), 0);
    if (r.activeTime === null) return r;
    let overall = dur === 0 ? (r.phase === 'before' ? 0 : t.iterations) : r.activeTime / dur;
    overall += t.iterationStart;
    let simple = overall === Infinity ? t.iterationStart % 1 : overall % 1;
    if (simple === 0 && (r.phase === 'active' || r.phase === 'after') && r.activeTime === active && t.iterations !== 0 && overall !== 0) simple = 1;
    let iteration;
    if (r.phase === 'after' && t.iterations === Infinity) iteration = Infinity;
    else if (simple === 1) iteration = Math.floor(overall) - 1;
    else iteration = Math.floor(overall);
    let forwards = true;
    if (t.direction === 'reverse') forwards = false;
    else if (t.direction !== 'normal') {
      let d = iteration;
      if (t.direction === 'alternate-reverse') d += 1;
      forwards = d === Infinity ? true : d % 2 === 0;
    }
    const directed = forwards ? simple : 1 - simple;
    const before = (r.phase === 'before' && forwards) || (r.phase === 'after' && !forwards);
    r.progress = parseEasing(t.easing).fn(directed, before);
    r.iteration = iteration;
    return r;
  }

  // -------------------------------------------------------- keyframes
  const OFFSET_ERR = () => new TypeError('Offsets must be null or in the range [0,1].');
  const SORT_ERR = () => new TypeError('Offsets must be monotonically non-decreasing.');
  function readOffset(v) {
    if (v === undefined || v === null) return null;
    if (typeof v === 'string') {
      let t = v.trim();
      for (let m; (m = /^calc\((.*)\)$/i.exec(t)); ) t = m[1].trim();
      v = t;
    }
    const n = Number(v);
    if (!Number.isFinite(n)) throw new TypeError('Non-finite offset');
    return n;
  }
  function readComposite(v) {
    if (v === undefined) return 'auto';
    v = String(v);
    if (v !== 'auto' && !COMPOSITES.includes(v)) throw new TypeError(`The provided value '${v}' is not a valid enum value of type CompositeOperationOrAuto.`);
    return v;
  }
  const isIterable = (v) => v !== null && typeof v === 'object' && typeof v[Symbol.iterator] === 'function';
  // Properties of a keyframe-like object (sorted by IDL name), read once.
  function readProps(obj, allowLists) {
    const names = [];
    for (const k of Object.keys(obj)) { const css = cssName(k); if (css) names.push([k, css]); }
    names.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
    const out = [];
    for (const [k, css] of names) {
      const raw = obj[k];
      let values;
      if (allowLists && isIterable(raw)) values = Array.from(raw, (x) => String(x));
      else values = String(raw);
      out.push([css, values]);
    }
    return out;
  }
  // -> [{ offset, easing, composite, props: Map(css -> value) }]
  function processKeyframes(input) {
    if (input === undefined || input === null) return [];
    if (typeof input !== 'object' && typeof input !== 'function') throw new TypeError("Failed to execute: The provided value is not of type '(sequence<object> or object)'.");
    const method = input[Symbol.iterator];
    if (method !== undefined && method !== null) {
      if (typeof method !== 'function') throw new TypeError('Keyframes argument is not iterable');
      const frames = [];
      for (const kf of input) {
        if (kf !== undefined && kf !== null && typeof kf !== 'object' && typeof kf !== 'function') throw new TypeError('Keyframes must be objects');
        const o = kf || {};
        const composite = readComposite(o.composite);
        const easing = o.easing === undefined ? 'linear' : String(o.easing);
        const offset = readOffset(o.offset);
        const props = new Map(readProps(o, false));
        frames.push({ offset, easing, composite, props });
      }
      let last = -Infinity;
      for (const f of frames) {
        if (f.offset === null) continue;
        if (f.offset < 0 || f.offset > 1) throw OFFSET_ERR();
        if (f.offset < last) throw SORT_ERR();
        last = f.offset;
      }
      for (const f of frames) f.easing = parseEasing(f.easing).css;
      return frames;
    }
    // Property-indexed keyframes.
    const compositeIn = input.composite;
    const easingIn = input.easing;
    const offsetIn = input.offset;
    const props = readProps(input, true);
    const byOffset = [];
    for (const [css, values] of props) {
      const list = Array.isArray(values) ? values : [values];
      list.forEach((v, i) => {
        const off = list.length === 1 ? 1 : i / (list.length - 1);
        let f = byOffset.find((x) => x.computed === off);
        if (!f) { f = { computed: off, offset: null, easing: 'linear', composite: 'auto', props: new Map() }; byOffset.push(f); }
        f.props.set(css, v);
      });
    }
    byOffset.sort((a, b) => a.computed - b.computed);
    const offsets = isIterable(offsetIn) ? Array.from(offsetIn, readOffset) : offsetIn === undefined ? [] : [readOffset(offsetIn)];
    offsets.forEach((o, i) => { if (i < byOffset.length) byOffset[i].offset = o; });
    let last = -Infinity;
    for (const o of offsets.slice(0, byOffset.length)) {
      if (o === null) continue;
      if (o < 0 || o > 1) throw OFFSET_ERR();
      if (o < last) throw SORT_ERR();
      last = o;
    }
    let easings = isIterable(easingIn) ? Array.from(easingIn, String) : easingIn === undefined ? [] : [String(easingIn)];
    easings = easings.map((e) => parseEasing(e).css);
    if (easings.length) byOffset.forEach((f, i) => { f.easing = easings[i % easings.length]; });
    const composites = isIterable(compositeIn) ? Array.from(compositeIn, readComposite) : compositeIn === undefined ? [] : [readComposite(compositeIn)];
    if (composites.length) byOffset.forEach((f, i) => { f.composite = composites[i % composites.length]; });
    return byOffset.map((f) => ({ offset: f.offset, easing: f.easing, composite: f.composite, props: f.props }));
  }
  function computedOffsets(frames) {
    const n = frames.length;
    const out = frames.map((f) => f.offset);
    if (n === 0) return out;
    if (out[n - 1] === null) out[n - 1] = 1;
    if (n > 1 && out[0] === null) out[0] = 0;
    for (let i = 1; i < n; i++) {
      if (out[i] !== null) continue;
      let j = i; while (out[j] === null) j++;
      const a = out[i - 1], b = out[j];
      for (let k = i; k < j; k++) out[k] = a + ((b - a) * (k - i + 1)) / (j - i + 1);
      i = j;
    }
    return out;
  }

  // ---------------------------------------------------------- timeline
  class AnimationTimeline {
    constructor() { ILLEGAL(); }
    get currentTime() { return N.animTimeline() - this[S].origin; }
    get duration() { return null; }
  }
  tag(AnimationTimeline, 'AnimationTimeline');
  class DocumentTimeline extends AnimationTimeline {
    constructor(options) {
      const t = Object.create(new.target ? new.target.prototype : DocumentTimeline.prototype);
      let origin = 0;
      if (options !== undefined && options !== null) origin = finiteDouble(options.originTime === undefined ? 0 : options.originTime, 'originTime');
      hidden(t, S, { origin });
      return t;
    }
  }
  tag(DocumentTimeline, 'DocumentTimeline');
  const docTimeline = Object.create(DocumentTimeline.prototype);
  hidden(docTimeline, S, { origin: 0 });
  const timelineTime = (tl) => (tl ? tl.currentTime : null);

  // ----------------------------------------------------------- effects
  let effectSeq = 0;
  class AnimationEffect {
    constructor() { ILLEGAL(); }
    getTiming() {
      const t = this[S].timing;
      return { delay: t.delay, direction: t.direction, duration: t.duration, easing: t.easing, endDelay: t.endDelay, fill: t.fill, iterationStart: t.iterationStart, iterations: t.iterations };
    }
    getComputedTiming() {
      const st = this[S];
      const a = st.animation;
      if (a && a[S].external) cssRefresh(a);
      const local = a ? currentTimeOf(a) : null;
      const backwards = a ? a[S].rate < 0 : false;
      const t = st.timing;
      const r = computeTiming(t, local, backwards);
      const out = this.getTiming();
      out.duration = r.dur;
      out.fill = t.fill === 'auto' ? 'none' : t.fill;
      return Object.assign(out, { activeDuration: r.active, currentIteration: r.iteration, endTime: r.end, localTime: local, progress: r.progress });
    }
    updateTiming(timing) {
      const st = this[S];
      st.timing = applyTiming(st.timing, timing);
      const a = st.animation;
      if (a) { updateFinishedState(a, false, false); touch(a); }
    }
  }
  tag(AnimationEffect, 'AnimationEffect');

  const PSEUDOS = ['::before', '::after', '::marker', '::first-line', '::first-letter', '::placeholder', '::selection', '::backdrop', '::file-selector-button', '::view-transition'];
  function checkPseudo(v) {
    if (v === null || v === undefined) return null;
    v = String(v).toLowerCase();
    if (v === ':before' || v === ':after' || v === ':first-line' || v === ':first-letter') v = ':' + v;
    if (!v.startsWith('::') || !(PSEUDOS.includes(v) || /^::view-transition-(group|image-pair|old|new)\(.+\)$/.test(v))) {
      throw domError(`Failed to set the 'pseudoElement' property: '${v}' is not a valid pseudo-element.`, 'SyntaxError');
    }
    return v;
  }
  function checkTarget(t) {
    if (t === undefined || t === null) return null;
    if (!(t instanceof g.Element)) throw new TypeError("Failed to construct 'KeyframeEffect': parameter 1 is not of type 'Element'.");
    return t;
  }
  function checkComposite(v, type) {
    v = String(v);
    if (!COMPOSITES.includes(v)) throw new TypeError(`The provided value '${v}' is not a valid enum value of type ${type}.`);
    return v;
  }
  class KeyframeEffect extends AnimationEffect {
    constructor(target, keyframes, options) {
      const e = Object.create(new.target ? new.target.prototype : KeyframeEffect.prototype);
      if (arguments.length === 1 && target instanceof KeyframeEffect) {
        const s = target[S];
        hidden(e, S, { id: ++effectSeq, target: s.target, pseudo: s.pseudo, frames: s.frames.slice(), timing: Object.assign({}, s.timing), composite: s.composite, iterationComposite: s.iterationComposite, animation: null, version: 1, pushed: false });
        return e;
      }
      const tgt = checkTarget(target);
      const timing = timingFromOptions(options);
      let composite = 'replace', iterationComposite = 'replace', pseudo = null;
      if (options !== null && typeof options === 'object') {
        if (options.composite !== undefined) composite = checkComposite(options.composite, 'CompositeOperation');
        if (options.iterationComposite !== undefined) iterationComposite = checkComposite(options.iterationComposite, 'IterationCompositeOperation');
        pseudo = checkPseudo(options.pseudoElement);
      }
      const frames = processKeyframes(keyframes);
      hidden(e, S, { id: ++effectSeq, target: tgt, pseudo, frames, timing, composite, iterationComposite, animation: null, version: 1, pushed: false });
      return e;
    }
    get target() { return this[S].target; }
    set target(v) {
      const st = this[S];
      v = checkTarget(v);
      if (v === st.target) return;
      st.target = v; st.version++;
      if (st.animation) touch(st.animation);
    }
    get pseudoElement() { return this[S].pseudo; }
    set pseudoElement(v) { this[S].pseudo = checkPseudo(v); this[S].version++; if (this[S].animation) touch(this[S].animation); }
    get composite() { return this[S].composite; }
    set composite(v) { this[S].composite = checkComposite(v, 'CompositeOperation'); if (this[S].animation) touch(this[S].animation); }
    get iterationComposite() { return this[S].iterationComposite; }
    set iterationComposite(v) { this[S].iterationComposite = checkComposite(v, 'IterationCompositeOperation'); }
    getKeyframes() {
      const a = this[S].animation;
      if (a && a[S].external) cssRefresh(a);
      const f = this[S].frames;
      const offs = computedOffsets(f);
      return f.map((k, i) => {
        const o = { offset: k.offset, computedOffset: offs[i], easing: k.easing, composite: k.composite };
        for (const [css, v] of k.props) o[idlName(css)] = v;
        return o;
      });
    }
    setKeyframes(kf) {
      const st = this[S];
      st.frames = processKeyframes(kf);
      st.version++;
      if (st.animation) touch(st.animation);
    }
  }
  tag(KeyframeEffect, 'KeyframeEffect');
  // CSS names this effect animates.
  const effectProps = (e) => { const out = new Set(); for (const f of e[S].frames) for (const k of f.props.keys()) out.add(k); return out; };

  // --------------------------------------------------------- animations
  let animSeq = 0;
  const known = new Set();     // animations that may be relevant
  const ticking = new Set();   // animations whose effect changes over time
  const pendingSet = new Set();
  let eventQueue = [];
  let activeFlag = false;
  const cssAnims = new Map();  // engine id -> CSSAnimation / CSSTransition

  function refreshActive() {
    const on = ticking.size > 0 || pendingSet.size > 0 || eventQueue.length > 0 || cssAnims.size > 0;
    if (on !== activeFlag) { activeFlag = on; N.animSetActive(on); }
  }

  function currentTimeOf(a, ignoreHold) {
    const st = a[S];
    if (st.holdTime !== null && !ignoreHold) return st.holdTime;
    const tt = timelineTime(st.timeline);
    if (tt === null || st.startTime === null) return null;
    return (tt - st.startTime) * st.rate + 0;
  }
  const effectEnd = (a) => { const e = a[S].effect; return e ? endTimeOf(e[S].timing) : 0; };
  function playStateOf(a) {
    const st = a[S];
    const ct = currentTimeOf(a);
    if (ct === null && !st.pendingTask) return 'idle';
    if (st.pendingTask === 'pause' || (st.startTime === null && st.pendingTask !== 'play')) return 'paused';
    const rate = st.pendingRate !== null ? st.pendingRate : st.rate;
    if (ct !== null && ((rate > 0 && ct >= effectEnd(a)) || (rate < 0 && ct <= 0))) return 'finished';
    return 'running';
  }
  function applyPendingRate(a) {
    const st = a[S];
    if (st.pendingRate === null) return;
    st.rate = st.pendingRate;
    st.pendingRate = null;
  }
  function newReady(a) {
    const st = a[S];
    st.ready = new Promise((res, rej) => { st.readyResolve = res; st.readyReject = rej; });
    st.readyPending = true;
  }
  function resolveReady(a) {
    const st = a[S];
    if (!st.readyPending) return;
    st.readyPending = false;
    st.readyResolve(a);
  }
  function newFinished(a) {
    const st = a[S];
    st.finished = new Promise((res, rej) => { st.finishedResolve = res; st.finishedReject = rej; });
    st.finishedState = 'pending';
  }
  function setPending(a, task) {
    const st = a[S];
    st.pendingTask = task;
    if (task) pendingSet.add(a); else pendingSet.delete(a);
  }
  function queueEvent(a, type, currentTime, timelineTimeValue) {
    eventQueue.push({ target: a, type, currentTime, timelineTime: timelineTimeValue });
    refreshActive();
  }

  function silentlySetCurrentTime(a, seek) {
    const st = a[S];
    if (seek === null) {
      if (currentTimeOf(a) !== null) throw new TypeError("Failed to set the 'currentTime' property on 'Animation': currentTime may not be changed from resolved to unresolved");
      return;
    }
    const tt = timelineTime(st.timeline);
    if (st.holdTime !== null || st.startTime === null || tt === null || st.rate === 0) st.holdTime = seek;
    else st.startTime = tt - seek / st.rate;
    if (tt === null) st.startTime = null;
    st.previousCurrentTime = null;
  }
  function setCurrentTime(a, seek) {
    const st = a[S];
    silentlySetCurrentTime(a, seek);
    if (st.pendingTask === 'pause') {
      st.holdTime = seek;
      applyPendingRate(a);
      st.startTime = null;
      setPending(a, null);
      resolveReady(a);
    }
    updateFinishedState(a, true, false);
  }
  function setStartTime(a, newStart) {
    const st = a[S];
    const tt = timelineTime(st.timeline);
    if (tt === null && newStart !== null) st.holdTime = null;
    const previous = currentTimeOf(a);
    applyPendingRate(a);
    st.startTime = newStart;
    if (newStart !== null) st.holdTime = st.rate !== 0 ? null : previous;
    else st.holdTime = previous;
    if (st.pendingTask) { setPending(a, null); resolveReady(a); }
    updateFinishedState(a, true, false);
  }
  function play(a, autoRewind) {
    const st = a[S];
    const abortedPause = st.pendingTask === 'pause';
    let hasPendingReady = false;
    let seek = null;
    const rate = st.pendingRate !== null ? st.pendingRate : st.rate;
    const ct = currentTimeOf(a);
    const end = effectEnd(a);
    if (rate > 0 && autoRewind && (ct === null || ct < 0 || ct >= end)) seek = 0;
    else if (rate < 0 && autoRewind && (ct === null || ct <= 0 || ct > end)) {
      if (end === Infinity) throw domError("Failed to execute 'play' on 'Animation': Cannot play reversed Animation with infinite target effect end.", 'InvalidStateError');
      seek = end;
    } else if (rate === 0 && ct === null) seek = 0;
    if (seek === null && st.startTime === null && ct === null) seek = 0;
    if (seek !== null) st.holdTime = seek;
    if (st.holdTime !== null) st.startTime = null;
    if (st.pendingTask) { setPending(a, null); hasPendingReady = true; }
    if (st.holdTime === null && seek === null && !abortedPause && st.pendingRate === null) return;
    if (!hasPendingReady) newReady(a);
    setPending(a, 'play');
    updateFinishedState(a, false, false);
  }
  function pause(a) {
    const st = a[S];
    if (st.pendingTask === 'pause') return;
    if (playStateOf(a) === 'paused') return;
    let seek = null;
    if (currentTimeOf(a) === null) {
      if (st.rate >= 0) seek = 0;
      else {
        if (effectEnd(a) === Infinity) throw domError("Failed to execute 'pause' on 'Animation': Cannot pause, Animation has infinite target effect end.", 'InvalidStateError');
        seek = effectEnd(a);
      }
    }
    if (seek !== null) st.holdTime = seek;
    let hasPendingReady = false;
    if (st.pendingTask === 'play') { setPending(a, null); hasPendingReady = true; }
    if (!hasPendingReady) newReady(a);
    setPending(a, 'pause');
    updateFinishedState(a, false, false);
  }
  function runPendingTask(a) {
    const st = a[S];
    const ready = timelineTime(st.timeline);
    if (ready === null) return;
    if (st.pendingTask === 'play') {
      if (st.holdTime !== null) {
        applyPendingRate(a);
        st.startTime = st.rate === 0 ? ready : ready - st.holdTime / st.rate;
        if (st.rate !== 0) st.holdTime = null;
      } else if (st.startTime !== null && st.pendingRate !== null) {
        const cur = (ready - st.startTime) * st.rate;
        applyPendingRate(a);
        if (st.rate === 0) st.holdTime = cur;
        else st.startTime = ready - cur / st.rate;
      }
    } else if (st.pendingTask === 'pause') {
      if (st.startTime !== null && st.holdTime === null) st.holdTime = (ready - st.startTime) * st.rate;
      applyPendingRate(a);
      st.startTime = null;
    } else return;
    setPending(a, null);
    resolveReady(a);
    updateFinishedState(a, false, false);
  }
  function resetPendingTasks(a) {
    const st = a[S];
    if (!st.pendingTask) return;
    setPending(a, null);
    applyPendingRate(a);
    if (st.readyPending) { st.readyPending = false; handled(st.ready); st.readyReject(abortError()); }
    st.ready = Promise.resolve(a);
    st.readyPending = false;
  }
  function updateFinishedState(a, didSeek, sync) {
    const st = a[S];
    const unconstrained = didSeek ? currentTimeOf(a) : currentTimeOf(a, true);
    const tt = timelineTime(st.timeline);
    if (unconstrained !== null && st.startTime !== null && !st.pendingTask) {
      const end = effectEnd(a);
      if (st.rate > 0 && unconstrained >= end) {
        st.holdTime = didSeek ? unconstrained : st.previousCurrentTime === null ? end : Math.max(st.previousCurrentTime, end);
      } else if (st.rate < 0 && unconstrained <= 0) {
        st.holdTime = didSeek ? unconstrained : st.previousCurrentTime === null ? 0 : Math.min(st.previousCurrentTime, 0);
      } else if (st.rate !== 0 && tt !== null) {
        if (didSeek && st.holdTime !== null) st.startTime = tt - st.holdTime / st.rate;
        st.holdTime = null;
      }
    }
    st.previousCurrentTime = currentTimeOf(a);
    const finished = playStateOf(a) === 'finished';
    if (finished && st.finishedState === 'pending') {
      if (sync) { st.finishNotify = null; finishNotification(a); }
      else if (!st.finishNotify) {
        const token = {};
        st.finishNotify = token;
        queueMicrotask(() => { if (st.finishNotify === token) { st.finishNotify = null; finishNotification(a); } });
      }
    }
    if (!finished && st.finishedState === 'resolved') newFinished(a);
    touch(a);
  }
  function finishNotification(a) {
    const st = a[S];
    if (playStateOf(a) !== 'finished') return;
    st.finishedState = 'resolved';
    st.finishedResolve(a);
    queueEvent(a, 'finish', currentTimeOf(a), timelineTime(st.timeline));
  }

  // Effect values -> engine; frame bookkeeping.
  function touch(a) {
    const st = a[S];
    if (st.external) { cssPush(a); return; }
    const ct = currentTimeOf(a);
    const runs = st.pendingTask !== null || (st.holdTime === null && st.startTime !== null && st.timeline !== null && st.rate !== 0);
    if (runs) ticking.add(a); else ticking.delete(a);
    if (runs || ct !== null) known.add(a); else known.delete(a);
    syncEffect(a, false);
    refreshActive();
  }
  function effectState(a) {
    const e = a[S].effect;
    if (!e) return null;
    return computeTiming(e[S].timing, currentTimeOf(a), a[S].rate < 0);
  }
  function framesForEngine(e) {
    const st = e[S];
    const offs = computedOffsets(st.frames);
    return st.frames.map((f, i) => {
      const row = [offs[i], f.easing, f.composite];
      for (const [k, v] of f.props) row.push(k, v);
      return row;
    });
  }
  function syncEffect(a, force) {
    const st = a[S];
    const e = st.effect;
    if (!e || st.external) return;
    const es = e[S];
    const target = es.target && !es.pseudo && es.target.isConnected ? es.target : null;
    const r = target ? effectState(a) : null;
    const progress = r && r.progress !== null && (st.replaceState !== 'removed' || force) ? r.progress : null;
    if (progress === null) {
      if (es.pushed) { N.animDrop(es.id); es.pushed = false; }
      return;
    }
    if (!es.pushed || es.pushedVersion !== es.version || es.pushedTarget !== target) {
      N.animFrames(es.id, target, framesForEngine(e));
      es.pushed = true; es.pushedVersion = es.version; es.pushedTarget = target;
    }
    N.animProgress(es.id, progress, r.iteration === Infinity ? 1e9 : r.iteration, st.seq, es.composite);
  }

  // Effect is current or in effect (getAnimations).
  function relevant(a) {
    const e = a[S].effect;
    if (!e) return false;
    const r = effectState(a);
    if (!r || r.phase === 'idle') return false;
    if (r.activeTime !== null) return true;
    const rate = a[S].rate;
    if (r.phase === 'before' && rate > 0) return true;
    if (r.phase === 'after' && rate < 0) return true;
    return r.phase === 'active' && playStateOf(a) !== 'finished';
  }

  function removeReplaced() {
    const replaceable = (a) => {
      const st = a[S];
      if (st.external || st.replaceState === 'removed' || st.timeline !== docTimeline) return false;
      if (playStateOf(a) !== 'finished') return false;
      const e = st.effect;
      if (!e || !e[S].target) return false;
      const r = effectState(a);
      return !!r && r.activeTime !== null;
    };
    const list = Array.from(known).filter(replaceable).sort((x, y) => x[S].seq - y[S].seq);
    if (list.length < 2) return;
    const removed = [];
    for (let i = 0; i < list.length; i++) {
      const a = list[i];
      if (a[S].replaceState !== 'active') continue;
      const e = a[S].effect[S];
      const props = effectProps(a[S].effect);
      if (!props.size) continue;
      const covered = new Set();
      for (let j = i + 1; j < list.length; j++) {
        const b = list[j][S].effect[S];
        if (b.target !== e.target || b.pseudo !== e.pseudo) continue;
        for (const p of effectProps(list[j][S].effect)) covered.add(p);
      }
      let all = true;
      for (const p of props) if (!covered.has(p)) { all = false; break; }
      if (all) removed.push(a);
    }
    for (const a of removed) {
      a[S].replaceState = 'removed';
      syncEffect(a, false);
      queueEvent(a, 'remove', currentTimeOf(a), timelineTime(a[S].timeline));
    }
  }

  // The frame hook.
  N.animSetHook(function update(phase) {
    if (phase === 0) {
      for (const a of Array.from(pendingSet)) runPendingTask(a);
      for (const a of Array.from(ticking)) updateFinishedState(a, false, false);
      for (const a of Array.from(cssAnims.values())) cssRefresh(a);
      removeReplaced();
      refreshActive();
      return;
    }
    const events = eventQueue;
    eventQueue = [];
    for (const ev of events) {
      const e = new AnimationPlaybackEvent(ev.type, { currentTime: ev.currentTime, timelineTime: ev.timelineTime });
      ev.target.dispatchEvent(e);
    }
    refreshActive();
  });

  class Animation extends g.EventTarget {
    constructor(effect = null, timeline) {
      super();
      if (effect !== null && !(effect instanceof AnimationEffect)) throw new TypeError("Failed to construct 'Animation': parameter 1 is not of type 'AnimationEffect'.");
      if (arguments.length < 2 || timeline === undefined) timeline = docTimeline;
      else if (timeline !== null && !(timeline instanceof AnimationTimeline)) throw new TypeError("Failed to construct 'Animation': parameter 2 is not of type 'AnimationTimeline'.");
      initState(this, timeline, false);
      if (effect) attachEffect(this, effect);
    }
    get id() { return this[S].id; }
    set id(v) { this[S].id = String(v); }
    get effect() { return this[S].effect; }
    set effect(e) {
      if (e !== null && !(e instanceof AnimationEffect)) throw new TypeError("Failed to set the 'effect' property on 'Animation': The provided value is not of type 'AnimationEffect'.");
      const st = this[S];
      if (e === st.effect) return;
      const old = st.effect;
      if (old) { if (old[S].pushed) { N.animDrop(old[S].id); old[S].pushed = false; } old[S].animation = null; st.effect = null; }
      if (e) attachEffect(this, e);
      updateFinishedState(this, false, false);
    }
    get timeline() { return this[S].timeline; }
    set timeline(t) {
      if (t !== null && !(t instanceof AnimationTimeline)) throw new TypeError("Failed to set the 'timeline' property on 'Animation': The provided value is not of type 'AnimationTimeline'.");
      const st = this[S];
      if (t === st.timeline) return;
      st.timeline = t;
      if (st.startTime !== null) st.holdTime = null;
      updateFinishedState(this, false, false);
    }
    get startTime() { return this[S].startTime; }
    set startTime(v) { setStartTime(this, v === null ? null : finiteDouble(v, 'startTime')); }
    get currentTime() { return currentTimeOf(this); }
    set currentTime(v) {
      if (v === null) { silentlySetCurrentTime(this, null); return; }
      setCurrentTime(this, finiteDouble(v, 'currentTime'));
    }
    get playbackRate() { return this[S].rate; }
    set playbackRate(r) {
      r = finiteDouble(r, 'playbackRate');
      const st = this[S];
      st.pendingRate = null;
      const previous = currentTimeOf(this);
      st.rate = r;
      if (previous !== null) setCurrentTime(this, previous);
      else touch(this);
    }
    get playState() { return playStateOf(this); }
    get replaceState() { return this[S].replaceState; }
    get pending() { return this[S].pendingTask !== null; }
    get ready() { return this[S].ready; }
    get finished() { return this[S].finished; }
    get overallProgress() {
      const e = effectEnd(this); const t = currentTimeOf(this);
      if (t === null || !this[S].effect) return null;
      if (e === 0) return t < 0 ? 0 : 1;
      return Math.max(0, Math.min(1, t / e));
    }
    play() { play(this, true); }
    pause() { pause(this); }
    finish() {
      const st = this[S];
      const rate = st.pendingRate !== null ? st.pendingRate : st.rate;
      const end = effectEnd(this);
      if (rate === 0) throw domError("Failed to execute 'finish' on 'Animation': Cannot finish Animation with a playbackRate of 0.", 'InvalidStateError');
      if (rate > 0 && end === Infinity) throw domError("Failed to execute 'finish' on 'Animation': Cannot finish Animation with an infinite target effect end.", 'InvalidStateError');
      applyPendingRate(this);
      const limit = st.rate > 0 ? end : 0;
      silentlySetCurrentTime(this, limit);
      const tt = timelineTime(st.timeline);
      if (st.startTime === null && tt !== null) st.startTime = tt - limit / st.rate;
      if (st.pendingTask === 'pause' && st.startTime !== null) { st.holdTime = null; setPending(this, null); resolveReady(this); }
      if (st.pendingTask === 'play' && st.startTime !== null) { setPending(this, null); resolveReady(this); }
      updateFinishedState(this, true, true);
    }
    cancel() {
      const st = this[S];
      if (playStateOf(this) !== 'idle') {
        resetPendingTasks(this);
        if (st.finishedState === 'pending') { handled(st.finished); st.finishedReject(abortError()); }
        st.finishNotify = null;
        newFinished(this);
        queueEvent(this, 'cancel', null, timelineTime(st.timeline));
      }
      st.holdTime = null;
      st.startTime = null;
      touch(this);
    }
    reverse() {
      const st = this[S];
      const tt = timelineTime(st.timeline);
      if (tt === null) throw domError("Failed to execute 'reverse' on 'Animation': Cannot reverse an animation with no active timeline", 'InvalidStateError');
      const original = st.pendingRate;
      st.pendingRate = -(st.pendingRate !== null ? st.pendingRate : st.rate);
      try { play(this, true); } catch (e) { st.pendingRate = original; throw e; }
    }
    updatePlaybackRate(r) {
      r = finiteDouble(r, 'playbackRate');
      const st = this[S];
      const previous = playStateOf(this);
      st.pendingRate = r;
      if (st.pendingTask) return;
      if (previous === 'idle' || previous === 'paused' || currentTimeOf(this) === null) { applyPendingRate(this); touch(this); }
      else if (previous === 'finished') {
        const unconstrained = currentTimeOf(this, true);
        const tt = timelineTime(st.timeline);
        st.startTime = r === 0 ? tt : tt - unconstrained / r;
        applyPendingRate(this);
        updateFinishedState(this, false, false);
      } else play(this, false);
    }
    persist() { this[S].replaceState = 'persisted'; syncEffect(this, false); }
    commitStyles() {
      const st = this[S];
      const e = st.effect;
      if (!e || !(e instanceof KeyframeEffect)) return;
      const es = e[S];
      const target = es.target;
      if (!target) return;
      if (es.pseudo) throw domError("Failed to execute 'commitStyles' on 'Animation': Animations with a pseudo-element target cannot be committed.", 'NoModificationAllowedError');
      if (!target.style) throw domError("Failed to execute 'commitStyles' on 'Animation': Target element does not have a style attribute.", 'NoModificationAllowedError');
      if (!target.isConnected || g.getComputedStyle(target).display === 'none') {
        throw domError("Failed to execute 'commitStyles' on 'Animation': Target element is not rendered.", 'InvalidStateError');
      }
      const props = Array.from(effectProps(e)).filter((p) => !p.startsWith('--'));
      if (!props.length) return;
      syncEffect(this, true);
      const values = N.animCommit(target, st.seq, props);
      syncEffect(this, false);
      if (!values) return;
      for (let i = 0; i + 1 < values.length; i += 2) target.style.setProperty(values[i], values[i + 1]);
    }
  }
  tag(Animation, 'Animation');
  N.installHandlers(Animation.prototype, ['finish', 'cancel', 'remove']);
  function initState(a, timeline, external) {
    hidden(a, S, {
      id: '', effect: null, timeline, startTime: null, holdTime: null, rate: 1, pendingRate: null, pendingTask: null,
      previousCurrentTime: null, replaceState: 'active', seq: external ? 0 : ++animSeq, finishNotify: null, external,
    });
    a[S].ready = Promise.resolve(a);
    a[S].readyPending = false;
    newFinished(a);
  }
  function attachEffect(a, e) {
    const prev = e[S].animation;
    if (prev && prev !== a) prev.effect = null;
    e[S].animation = a;
    a[S].effect = e;
  }

  class AnimationPlaybackEvent extends g.Event {
    constructor(type, init) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'AnimationPlaybackEvent': 1 argument required, but only 0 present.");
      init = init || {};
      super(type, init);
      const num = (v) => (v === undefined || v === null ? null : finiteDouble(v, 'time'));
      hidden(this, S, { currentTime: num(init.currentTime), timelineTime: num(init.timelineTime) });
    }
    get currentTime() { return this[S].currentTime; }
    get timelineTime() { return this[S].timelineTime; }
  }
  tag(AnimationPlaybackEvent, 'AnimationPlaybackEvent');

  // ------------------------------------- CSS animations and transitions
  // The engine runs them; these objects mirror and control them. One
  // object per engine animation id (same identity across calls).
  class CSSAnimation extends Animation {
    constructor() { super(); ILLEGAL(); }
    get animationName() { return this[S].name; }
  }
  tag(CSSAnimation, 'CSSAnimation');
  class CSSTransition extends Animation {
    constructor() { super(); ILLEGAL(); }
    get transitionProperty() { return this[S].name; }
  }
  tag(CSSTransition, 'CSSTransition');
  function cssObject(info) {
    let a = cssAnims.get(info.id);
    if (!a) {
      a = Reflect.construct(g.EventTarget, [], info.transition ? CSSTransition : CSSAnimation);
      initState(a, docTimeline, true);
      a[S].cssId = info.id;
      const effect = Object.create(KeyframeEffect.prototype);
      hidden(effect, S, { id: 0, target: info.target, pseudo: null, frames: [], timing: Object.assign({}, TIMING_DEFAULTS), composite: 'replace', iterationComposite: 'replace', animation: a, version: 1, pushed: false });
      a[S].effect = effect;
      cssAnims.set(info.id, a);
      refreshActive();
    }
    cssApply(a, info);
    return a;
  }
  const safeEasing = (e) => { try { return parseEasing(e).css; } catch (err) { return 'linear'; } };
  // Take the engine's state for `a`.
  function cssApply(a, info) {
    const st = a[S];
    st.info = info;
    st.name = info.name;
    st.startTime = info.startTime;
    st.holdTime = info.holdTime;
    st.rate = info.rate;
    const es = st.effect[S];
    es.target = info.target;
    es.timing = { delay: info.delay, direction: info.direction, duration: info.duration, easing: safeEasing(info.easing), endDelay: 0, fill: info.fill, iterationStart: 0, iterations: info.iterations };
    es.frames = info.keyframes.map((k) => {
      const props = new Map();
      for (let i = 2; i + 1 < k.length; i += 2) props.set(k[i], k[i + 1]);
      return { offset: k[0], easing: safeEasing(k[1]), composite: 'auto', props };
    });
  }
  // Refresh `a` from the engine; resolve its finished promise / queue its
  // finish event when it reached its end.
  function cssRefresh(a) {
    const st = a[S];
    if (!cssAnims.has(st.cssId)) return;
    const info = N.cssAnimation(st.cssId);
    const was = playStateOf(a);
    if (info && !info.ended) cssApply(a, info);
    else {
      cssAnims.delete(st.cssId);
      refreshActive();
      if (info && info.ended && st.startTime !== null) {
        // Ran to its end: finished, held at the end.
        st.holdTime = st.rate < 0 ? 0 : effectEnd(a);
      } else { st.startTime = null; st.holdTime = null; }
    }
    if (st.pendingTask) return;
    const now = playStateOf(a);
    if (now === 'finished' && st.finishedState === 'pending' && (was !== 'finished' || !st.seenFinished)) finishNotification(a);
    st.seenFinished = now === 'finished';
    if (now !== 'finished' && st.finishedState === 'resolved') newFinished(a);
  }
  function cssPush(a) {
    const st = a[S];
    if (!cssAnims.has(st.cssId)) return;
    N.cssAnimationSet(st.cssId, st.startTime, st.holdTime, st.rate);
    if (st.pendingTask) pendingSet.add(a);
    refreshActive();
  }
  // Read the engine's state before every script access.
  for (const C of [CSSAnimation, CSSTransition]) {
    const P = C.prototype;
    for (const k of ['currentTime', 'startTime', 'playbackRate', 'playState', 'pending', 'replaceState', 'ready', 'finished', 'overallProgress']) {
      const d = Object.getOwnPropertyDescriptor(Animation.prototype, k);
      const desc = { configurable: true, enumerable: true, get() { cssRefresh(this); return d.get.call(this); } };
      if (d.set) desc.set = function (v) { cssRefresh(this); d.set.call(this, v); };
      Object.defineProperty(P, k, desc);
    }
    for (const k of ['play', 'pause', 'finish', 'cancel', 'reverse', 'updatePlaybackRate']) {
      const f = Animation.prototype[k];
      hidden(P, k, { [k](...args) { cssRefresh(this); return f.apply(this, args); } }[k]);
    }
  }
  // Engine CSS animations / transitions of the main document, as objects.
  function cssList() {
    const infos = N.cssAnimations();
    if (!infos || !infos.length) return [];
    return infos.map(cssObject);
  }

  // ------------------------------------------------------ Element.animate
  const E = g.Element;
  hidden(E.prototype, 'animate', function animate(keyframes, options) {
    const effect = new KeyframeEffect(this, keyframes, options);
    let timeline = docTimeline;
    if (options !== null && typeof options === 'object' && options.timeline !== undefined) timeline = options.timeline;
    const a = new Animation(effect, timeline);
    if (options !== null && typeof options === 'object' && options.id !== undefined) a.id = options.id;
    a.play();
    return a;
  });
  // Composite order: transitions, CSS animations (by element tree order),
  // then script animations (creation order).
  function treeCmp(x, y) {
    if (x === y) return 0;
    return x.compareDocumentPosition(y) & 4 ? -1 : 1;
  }
  function collect(pred) {
    const css = cssList().filter((a) => {
      const t = a[S].effect[S].target;
      return t && pred(t, null) && playStateOf(a) !== 'idle' && relevant(a);
    });
    css.sort((x, y) => {
      const kx = x instanceof CSSTransition ? 0 : 1, ky = y instanceof CSSTransition ? 0 : 1;
      if (kx !== ky) return kx - ky;
      const c = treeCmp(x[S].effect[S].target, y[S].effect[S].target);
      if (c) return c;
      return x[S].info.order - y[S].info.order;
    });
    const script = Array.from(known).filter((a) => {
      const e = a[S].effect;
      return e && e[S].target && a[S].replaceState !== 'removed' && pred(e[S].target, e[S].pseudo) && relevant(a);
    }).sort((x, y) => x[S].seq - y[S].seq);
    return css.concat(script);
  }
  hidden(E.prototype, 'getAnimations', function getAnimations(options) {
    const subtree = !!(options && options.subtree);
    let pseudo = null;
    if (options && options.pseudoElement !== undefined && options.pseudoElement !== null) {
      pseudo = checkPseudo(options.pseudoElement);
    }
    if (pseudo !== null) return collect((t, p) => p === pseudo && t === this);
    return collect((t, p) => (t === this && p === null) || (subtree && (t === this || this.contains(t))));
  });
  hidden(g.Document.prototype, 'getAnimations', function getAnimations() {
    return collect((t) => t.isConnected && t.ownerDocument === this && t.getRootNode() === this);
  });
  if (g.ShadowRoot) hidden(g.ShadowRoot.prototype, 'getAnimations', function getAnimations() { return collect((t) => t.getRootNode() === this); });
  Object.defineProperty(g.Document.prototype, 'timeline', { get() { return docTimeline; }, configurable: true, enumerable: true });

  define('AnimationTimeline', AnimationTimeline);
  define('DocumentTimeline', DocumentTimeline);
  define('AnimationEffect', AnimationEffect);
  define('KeyframeEffect', KeyframeEffect);
  define('Animation', Animation);
  define('CSSAnimation', CSSAnimation);
  define('CSSTransition', CSSTransition);
  // An animation of a shadow-tree element's CSS animation projected onto a
  // pseudo-element host (CSS Animations 2): mirrors its source animation.
  const SHADOW_SRC = Symbol('shadowAnimation');
  class ShadowAnimation extends Animation {
    constructor(source, newTarget) {
      if (!(source instanceof Animation)) throw new TypeError("Failed to construct 'ShadowAnimation': parameter 1 is not of type 'Animation'.");
      super(null, source.timeline);
      Object.defineProperty(this, SHADOW_SRC, { value: { source, target: newTarget } });
    }
    get sourceAnimation() { return this[SHADOW_SRC].source; }
  }
  Object.defineProperty(ShadowAnimation.prototype, Symbol.toStringTag, { value: 'ShadowAnimation', configurable: true });
  define('ShadowAnimation', ShadowAnimation);
  define('AnimationPlaybackEvent', AnimationPlaybackEvent);
})();
