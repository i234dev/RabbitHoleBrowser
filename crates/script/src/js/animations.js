// Web Animations: AnimationTimeline / DocumentTimeline, AnimationEffect /
// KeyframeEffect (timing model: delay, endDelay, iterations, direction,
// fill, playbackRate, computed timing), Animation (play/pause/finish/
// cancel/reverse/updatePlaybackRate/persist, ready/finished promises,
// finish/cancel/remove events), AnimationPlaybackEvent, Element.animate and
// getAnimations. Effects are timed but not composited into rendering.
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
  const now = () => g.performance.now();
  const live = new Set();

  class AnimationTimeline {
    constructor() { ILLEGAL(); }
    get currentTime() { return Math.max(0, now() - this[S].origin); }
    get duration() { return null; }
  }
  tag(AnimationTimeline, 'AnimationTimeline');
  class DocumentTimeline extends AnimationTimeline {
    constructor(options = {}) { const t = Object.create(new.target ? new.target.prototype : DocumentTimeline.prototype); hidden(t, S, { origin: +options.originTime || 0 }); return t; }
  }
  tag(DocumentTimeline, 'DocumentTimeline');
  const docTimeline = Object.create(DocumentTimeline.prototype);
  hidden(docTimeline, S, { origin: 0 });

  // --------------------------------------------------------- timing model
  const TIMING_DEFAULTS = { delay: 0, endDelay: 0, fill: 'auto', iterationStart: 0, iterations: 1, duration: 'auto', direction: 'normal', easing: 'linear', playbackRate: 1 };
  function normTiming(options) {
    const t = Object.assign({}, TIMING_DEFAULTS);
    if (typeof options === 'number') t.duration = options;
    else if (options && typeof options === 'object') for (const k of Object.keys(TIMING_DEFAULTS)) if (options[k] !== undefined) t[k] = options[k];
    if (t.duration !== 'auto' && (typeof t.duration !== 'number' || t.duration < 0 || Number.isNaN(t.duration))) throw new TypeError("Failed to execute 'animate' on 'Element': duration must be non-negative or auto.");
    if (t.iterations < 0 || Number.isNaN(t.iterations)) throw new TypeError("Failed to execute 'animate' on 'Element': iterationCount must be non-negative.");
    return t;
  }
  class AnimationEffect {
    constructor() { ILLEGAL(); }
    getTiming() { return Object.assign({}, this[S].timing); }
    getComputedTiming() {
      const t = this[S].timing, a = this[S].animation;
      const duration = t.duration === 'auto' ? 0 : t.duration;
      const active = duration * t.iterations;
      const end = Math.max(t.delay + active + t.endDelay, 0);
      const local = a ? a.currentTime : null;
      let progress = null, iteration = null;
      if (local !== null) {
        const atime = local - t.delay;
        const inActive = atime >= 0 && atime < active;
        const before = atime < 0, after = atime >= active;
        const fill = t.fill === 'auto' ? 'none' : t.fill;
        if (inActive || (before && (fill === 'backwards' || fill === 'both')) || (after && (fill === 'forwards' || fill === 'both'))) {
          const clamped = Math.max(0, Math.min(active, atime));
          const overall = duration ? clamped / duration + t.iterationStart : t.iterationStart + (after ? t.iterations : 0);
          iteration = Math.floor(overall);
          let simple = overall - iteration;
          if (simple === 0 && after && t.iterations > 0 && overall > 0) { simple = 1; iteration -= 1; }
          const dir = t.direction === 'reverse' || (t.direction === 'alternate' && iteration % 2 === 1) || (t.direction === 'alternate-reverse' && iteration % 2 === 0);
          progress = dir ? 1 - simple : simple;
        }
      }
      return Object.assign(this.getTiming(), { duration, activeDuration: active, endTime: end, localTime: local, progress, currentIteration: iteration, fill: t.fill === 'auto' ? 'none' : t.fill });
    }
    updateTiming(timing = {}) { this[S].timing = normTiming(Object.assign({}, this[S].timing, timing)); const a = this[S].animation; if (a) schedule(a); }
  }
  tag(AnimationEffect, 'AnimationEffect');
  function normKeyframes(kf) {
    if (kf === null || kf === undefined) return [];
    if (Array.isArray(kf)) return kf.map((f) => Object.assign({}, f));
    // Property-indexed form.
    const keys = Object.keys(kf).filter((k) => !['offset', 'easing', 'composite'].includes(k));
    const n = Math.max(0, ...keys.map((k) => (Array.isArray(kf[k]) ? kf[k].length : 1)));
    const out = [];
    for (let i = 0; i < n; i++) {
      const f = {};
      for (const k of keys) { const v = Array.isArray(kf[k]) ? kf[k][i] : i === 0 ? kf[k] : undefined; if (v !== undefined) f[k] = v; }
      out.push(f);
    }
    return out;
  }
  class KeyframeEffect extends AnimationEffect {
    constructor(target, keyframes, options) {
      const e = Object.create(new.target ? new.target.prototype : KeyframeEffect.prototype);
      if (target instanceof KeyframeEffect) {
        hidden(e, S, { target: target[S].target, frames: target[S].frames.map((f) => Object.assign({}, f)), timing: Object.assign({}, target[S].timing), composite: target[S].composite, pseudo: target[S].pseudo, animation: null });
        return e;
      }
      hidden(e, S, { target: target || null, frames: normKeyframes(keyframes), timing: normTiming(options), composite: (options && options.composite) || 'replace', pseudo: (options && options.pseudoElement) || null, animation: null });
      return e;
    }
    get target() { return this[S].target; }
    set target(v) { this[S].target = v; }
    get pseudoElement() { return this[S].pseudo; }
    set pseudoElement(v) { this[S].pseudo = v; }
    get composite() { return this[S].composite; }
    set composite(v) { if (['replace', 'add', 'accumulate'].includes(v)) this[S].composite = v; }
    get iterationComposite() { return 'replace'; }
    getKeyframes() {
      const f = this[S].frames;
      return f.map((k, i) => Object.assign({ offset: k.offset ?? null, computedOffset: k.offset ?? (f.length === 1 ? 1 : i / (f.length - 1)), easing: k.easing || 'linear', composite: k.composite || 'auto' }, k));
    }
    setKeyframes(kf) { this[S].frames = normKeyframes(kf); }
  }
  tag(KeyframeEffect, 'KeyframeEffect');

  // ------------------------------------------------------------ Animation
  let animId = 0;
  class Animation extends g.EventTarget {
    constructor(effect = null, timeline = docTimeline) {
      super();
      hidden(this, S, { id: '', effect, timeline: timeline === undefined ? docTimeline : timeline, startTime: null, holdTime: null, rate: 1, state: 'idle', replaceState: 'active', pendingPlay: false, pendingPause: false, timer: null, seq: ++animId });
      if (effect) effect[S].animation = this;
      newReady(this, true);
      newFinished(this);
    }
    get id() { return this[S].id; }
    set id(v) { this[S].id = String(v); }
    get effect() { return this[S].effect; }
    set effect(e) { if (this[S].effect) this[S].effect[S].animation = null; this[S].effect = e; if (e) e[S].animation = this; schedule(this); }
    get timeline() { return this[S].timeline; }
    set timeline(t) { this[S].timeline = t; }
    get startTime() { return this[S].startTime; }
    set startTime(v) { const st = this[S]; if (v === null) { st.startTime = null; st.holdTime = null; } else { st.startTime = +v; st.holdTime = null; st.state = 'running'; } schedule(this); }
    get currentTime() {
      const st = this[S];
      if (st.holdTime !== null) return st.holdTime;
      if (st.startTime === null || !st.timeline) return null;
      return (st.timeline.currentTime - st.startTime) * st.rate;
    }
    set currentTime(v) {
      const st = this[S];
      if (v === null) throw new TypeError("Failed to set the 'currentTime' property on 'Animation': currentTime cannot be set to null.");
      v = +v;
      if (st.state === 'paused' || st.startTime === null) st.holdTime = v;
      else st.startTime = st.timeline.currentTime - v / st.rate;
      schedule(this);
    }
    get playbackRate() { return this[S].rate; }
    set playbackRate(r) { const t = this.currentTime; this[S].rate = +r; if (t !== null) this.currentTime = t; }
    get playState() {
      const st = this[S];
      if (st.state === 'idle' || (this.currentTime === null && !st.pendingPlay)) return 'idle';
      if (st.state === 'paused' || st.pendingPause) return 'paused';
      const end = endTime(this);
      const t = this.currentTime;
      if (t !== null && ((st.rate > 0 && t >= end) || (st.rate < 0 && t <= 0))) return 'finished';
      return 'running';
    }
    get replaceState() { return this[S].replaceState; }
    get pending() { return this[S].pendingPlay || this[S].pendingPause; }
    get ready() { return this[S].ready; }
    get finished() { return this[S].finished; }
    get overallProgress() { const e = endTime(this); const t = this.currentTime; return t === null || !e ? null : Math.max(0, Math.min(1, t / e)); }
    play() {
      const st = this[S];
      const end = endTime(this);
      let t = this.currentTime;
      if (st.rate > 0 && (t === null || t < 0 || t >= end)) t = 0;
      else if (st.rate < 0 && (t === null || t <= 0 || t > end)) t = end;
      if (t === null) t = 0;
      if (st.state === 'finished-resolved') newFinished(this);
      st.state = 'running';
      st.holdTime = null;
      st.pendingPlay = true;
      live.add(this);
      // Becomes ready on the next frame.
      requestAnimationFrame(() => {
        if (!st.pendingPlay) return;
        st.pendingPlay = false;
        st.startTime = st.timeline.currentTime - t / st.rate;
        st.readyResolve(this);
        schedule(this);
      });
    }
    pause() {
      const st = this[S];
      const t = this.currentTime ?? (st.rate < 0 ? endTime(this) : 0);
      st.state = 'paused'; st.holdTime = t; st.startTime = null; st.pendingPlay = false;
      clearTimeout(st.timer);
      st.pendingPause = true;
      requestAnimationFrame(() => { st.pendingPause = false; st.readyResolve(this); });
    }
    finish() {
      const st = this[S];
      if (st.rate === 0 || (st.rate > 0 && endTime(this) === Infinity)) throw domError("Failed to execute 'finish' on 'Animation': Cannot finish Animation with a playbackRate of 0 or an infinite end.", 'InvalidStateError');
      const limit = st.rate > 0 ? endTime(this) : 0;
      st.state = 'running'; st.pendingPlay = false; st.holdTime = null;
      st.startTime = st.timeline.currentTime - limit / st.rate;
      st.readyResolve(this);
      checkFinished(this, true);
    }
    cancel() {
      const st = this[S];
      if (st.state === 'idle') return;
      clearTimeout(st.timer);
      st.readyReject(domError('The user aborted a request.', 'AbortError'));
      st.finishedReject(domError('The user aborted a request.', 'AbortError'));
      newReady(this, true);
      newFinished(this);
      st.state = 'idle'; st.startTime = null; st.holdTime = null; st.pendingPlay = false; st.pendingPause = false;
      live.delete(this);
      const ev = new AnimationPlaybackEvent('cancel', { currentTime: null, timelineTime: st.timeline ? st.timeline.currentTime : null });
      setTimeout(() => this.dispatchEvent(ev), 0);
    }
    reverse() { this.updatePlaybackRate(-this[S].rate); this.play(); }
    updatePlaybackRate(r) { this.playbackRate = r; }
    persist() { this[S].replaceState = 'persisted'; }
    commitStyles() {
      const e = this[S].effect;
      if (!e || !e[S].target || !e[S].target.style) return;
      const ct = e.getComputedTiming();
      if (ct.progress === null) return;
      const frames = e[S].frames;
      if (!frames.length) return;
      const f = frames[ct.progress >= 0.5 ? frames.length - 1 : 0];
      for (const [k, v] of Object.entries(f)) if (!['offset', 'easing', 'composite'].includes(k)) e[S].target.style[k] = v;
    }
  }
  tag(Animation, 'Animation');
  N.installHandlers(Animation.prototype, ['finish', 'cancel', 'remove']);
  function newReady(a, resolved) {
    const st = a[S];
    st.ready = new Promise((res, rej) => { st.readyResolve = res; st.readyReject = rej; });
    st.ready.catch(() => {});
    if (resolved) st.readyResolve(a);
  }
  function newFinished(a) {
    const st = a[S];
    st.finished = new Promise((res, rej) => { st.finishedResolve = res; st.finishedReject = rej; });
    st.finished.catch(() => {});
  }
  function endTime(a) { const e = a[S].effect; return e ? e.getComputedTiming().endTime : 0; }
  function schedule(a) {
    const st = a[S];
    clearTimeout(st.timer);
    if (st.state !== 'running' || st.startTime === null || st.rate === 0) return;
    const end = endTime(a);
    if (end === Infinity) return;
    const t = a.currentTime;
    const remaining = st.rate > 0 ? (end - t) / st.rate : t / -st.rate;
    st.timer = setTimeout(() => checkFinished(a, false), Math.max(0, remaining));
  }
  function checkFinished(a, sync) {
    const st = a[S];
    if (a.playState !== 'finished' || st.state === 'finished-resolved') return;
    const t = a.currentTime;
    st.holdTime = t;
    st.state = 'finished-resolved';
    live.delete(a);
    const ev = new AnimationPlaybackEvent('finish', { currentTime: t, timelineTime: st.timeline ? st.timeline.currentTime : null });
    // The promise resolves now (finish()) or at the end of the task; the
    // event is always queued.
    if (sync) st.finishedResolve(a); else Promise.resolve().then(() => st.finishedResolve(a));
    setTimeout(() => a.dispatchEvent(ev), 0);
  }
  // playState treats the resolved state like running-at-end.
  const origPlayState = Object.getOwnPropertyDescriptor(Animation.prototype, 'playState').get;
  Object.defineProperty(Animation.prototype, 'playState', { get() { return this[S].state === 'finished-resolved' ? 'finished' : origPlayState.call(this); }, configurable: true, enumerable: true });

  class AnimationPlaybackEvent extends g.Event {
    constructor(type, init = {}) { super(type, init); hidden(this, S, { currentTime: init.currentTime ?? null, timelineTime: init.timelineTime ?? null }); }
    get currentTime() { return this[S].currentTime; }
    get timelineTime() { return this[S].timelineTime; }
  }
  tag(AnimationPlaybackEvent, 'AnimationPlaybackEvent');

  // ------------------------------------------------------ Element.animate
  const E = g.Element;
  hidden(E.prototype, 'animate', function animate(keyframes, options) {
    const effect = new KeyframeEffect(this, keyframes, options);
    const a = new Animation(effect, options && options.timeline !== undefined ? options.timeline : docTimeline);
    if (options && options.id) a.id = options.id;
    a.play();
    return a;
  });
  const animsOf = (pred) => Array.from(live).filter((a) => { const e = a[S].effect; return e && e[S].target && pred(e[S].target); }).sort((x, y) => x[S].seq - y[S].seq);
  hidden(E.prototype, 'getAnimations', function getAnimations(options = {}) { return animsOf((t) => t === this || (options.subtree && this.contains(t))); });
  hidden(g.Document.prototype, 'getAnimations', function getAnimations() { return animsOf((t) => t.isConnected); });
  if (g.ShadowRoot) hidden(g.ShadowRoot.prototype, 'getAnimations', function getAnimations() { return animsOf((t) => t.getRootNode() === this); });
  Object.defineProperty(g.Document.prototype, 'timeline', { get() { return docTimeline; }, configurable: true, enumerable: true });

  define('AnimationTimeline', AnimationTimeline);
  define('DocumentTimeline', DocumentTimeline);
  define('AnimationEffect', AnimationEffect);
  define('KeyframeEffect', KeyframeEffect);
  define('Animation', Animation);
  // An animation of a shadow-tree element's CSS animation projected onto a
  // pseudo-element host (CSS Animations 2): mirrors its source animation.
  class ShadowAnimation extends Animation {
    constructor(source, newTarget) {
      if (!(source instanceof Animation)) throw new TypeError("Failed to construct 'ShadowAnimation': parameter 1 is not of type 'Animation'.");
      super(source.effect, source.timeline);
      Object.defineProperty(this, SHADOW_SRC, { value: { source, target: newTarget } });
    }
    get sourceAnimation() { return this[SHADOW_SRC].source; }
  }
  const SHADOW_SRC = Symbol('shadowAnimation');
  Object.defineProperty(ShadowAnimation.prototype, Symbol.toStringTag, { value: 'ShadowAnimation', configurable: true });
  define('ShadowAnimation', ShadowAnimation);
  define('AnimationPlaybackEvent', AnimationPlaybackEvent);
})();
