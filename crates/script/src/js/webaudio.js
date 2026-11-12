// Web Audio API basics on the native mixer: BaseAudioContext /
// AudioContext / OfflineAudioContext, AudioBuffer, AudioParam (values and
// simple automation), AudioNode connection graph, GainNode,
// AudioBufferSourceNode (start / stop / loop / playbackRate / onended),
// OscillatorNode, decodeAudioData and the destination. Sources play
// through native voices (`__rhNative.wa*`); the gain of a voice is the
// product of the GainNodes on its path to the destination.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  if (!N || !N.waVoiceCreate) return;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const S = Symbol('webaudio');
  const RATE = 48000;

  // ------------------------------------------------------------ AudioBuffer
  class AudioBuffer {
    constructor(options) {
      if (!options || typeof options !== 'object') throw new TypeError("Failed to construct 'AudioBuffer': 1 argument required, but only 0 present.");
      const ch = options.numberOfChannels === undefined ? 1 : options.numberOfChannels >>> 0;
      const len = options.length >>> 0;
      const rate = +options.sampleRate;
      if (ch < 1 || ch > 32) throw domError(`Failed to construct 'AudioBuffer': The number of channels provided (${ch}) is outside the range [1, 32].`, 'NotSupportedError');
      if (len < 1) throw domError(`Failed to construct 'AudioBuffer': The number of frames provided (${len}) is less than or equal to the minimum bound (0).`, 'NotSupportedError');
      if (!(rate >= 3000 && rate <= 768000)) throw domError(`Failed to construct 'AudioBuffer': The sample rate provided (${rate}) is outside the range [3000, 768000].`, 'NotSupportedError');
      Object.defineProperty(this, S, { value: { rate, len, data: Array.from({ length: ch }, () => new Float32Array(len)) } });
    }
    get sampleRate() { return this[S].rate; }
    get length() { return this[S].len; }
    get duration() { return this[S].len / this[S].rate; }
    get numberOfChannels() { return this[S].data.length; }
    getChannelData(c) {
      c = c >>> 0;
      if (c >= this[S].data.length) throw domError(`Failed to execute 'getChannelData' on 'AudioBuffer': channel index (${c}) exceeds number of channels (${this[S].data.length})`, 'IndexSizeError');
      return this[S].data[c];
    }
    copyFromChannel(dest, c, start = 0) { const src = this.getChannelData(c); start >>>= 0; dest.set(src.subarray(start, start + dest.length)); }
    copyToChannel(src, c, start = 0) { const d = this.getChannelData(c); start >>>= 0; d.set(src.subarray(0, Math.max(0, d.length - start)), start); }
  }
  tag(AudioBuffer, 'AudioBuffer');
  const makeBuffer = (channels, rate) => {
    const b = Object.create(AudioBuffer.prototype);
    Object.defineProperty(b, S, { value: { rate, len: channels[0] ? channels[0].length : 0, data: channels } });
    return b;
  };

  // ------------------------------------------------------------- AudioParam
  class AudioParam {
    constructor() { ILLEGAL(); }
    get value() { return this[S].value; }
    set value(v) { v = +v; if (!Number.isFinite(v)) throw new TypeError("Failed to set the 'value' property on 'AudioParam': The provided float value is non-finite."); this[S].value = Math.min(this[S].max, Math.max(this[S].min, v)); this[S].changed(); }
    get defaultValue() { return this[S].dflt; }
    get minValue() { return this[S].min; }
    get maxValue() { return this[S].max; }
    get automationRate() { return this[S].rateMode; }
    set automationRate(v) { if (v === 'a-rate' || v === 'k-rate') this[S].rateMode = v; }
    setValueAtTime(v, t) { at(this, t, () => { this.value = v; }); return this; }
    linearRampToValueAtTime(v, t) { ramp(this, v, t, false); return this; }
    exponentialRampToValueAtTime(v, t) { if (v === 0) throw new RangeError("Failed to execute 'exponentialRampToValueAtTime' on 'AudioParam': The float target value provided (0) should not be in the range (-1.40130e-45, 1.40130e-45)."); ramp(this, v, t, true); return this; }
    setTargetAtTime(v, t, c) { at(this, t, () => { ramp(this, v, this[S].ctx.currentTime + Math.max(0.001, 3 * c), false); }); return this; }
    setValueCurveAtTime(values, t, d) { const arr = Array.from(values); const n = arr.length; for (let i = 0; i < n; i++) at(this, t + d * i / Math.max(1, n - 1), () => { this.value = arr[i]; }); return this; }
    cancelScheduledValues() { for (const id of this[S].timers.splice(0)) clearTimeout(id); return this; }
    cancelAndHoldAtTime(t) { return this.cancelScheduledValues(t); }
  }
  tag(AudioParam, 'AudioParam');
  function newParam(ctx, value, min, max, changed) {
    const p = Object.create(AudioParam.prototype);
    Object.defineProperty(p, S, { value: { ctx, value, dflt: value, min, max, rateMode: 'a-rate', timers: [], changed: changed || (() => {}) } });
    return p;
  }
  function at(param, t, fn) {
    const delay = Math.max(0, (t - param[S].ctx.currentTime) * 1000);
    if (delay <= 0) { fn(); return; }
    param[S].timers.push(setTimeout(fn, delay));
  }
  function ramp(param, v, t, exp) {
    const ctx = param[S].ctx;
    const start = ctx.currentTime, from = param[S].value;
    const steps = Math.max(1, Math.min(50, Math.round((t - start) * 50)));
    for (let i = 1; i <= steps; i++) {
      const k = i / steps;
      const val = exp && from > 0 && v > 0 ? from * Math.pow(v / from, k) : from + (v - from) * k;
      at(param, start + (t - start) * k, () => { param[S].value = val; param[S].changed(); });
    }
  }

  // --------------------------------------------------------------- AudioNode
  class AudioNode extends EventTarget {
    constructor() { super(); ILLEGAL(); }
    get context() { return this[S].ctx; }
    get numberOfInputs() { return this[S].inputs; }
    get numberOfOutputs() { return this[S].outputs; }
    get channelCount() { return this[S].channelCount; }
    set channelCount(v) { this[S].channelCount = v >>> 0; }
    get channelCountMode() { return this[S].countMode; }
    set channelCountMode(v) { this[S].countMode = v; }
    get channelInterpretation() { return this[S].interp; }
    set channelInterpretation(v) { this[S].interp = v; }
    connect(dest, output = 0, input = 0) {
      if (dest instanceof AudioParam) { this[S].out.add(dest); return undefined; }
      if (!(dest instanceof AudioNode)) throw new TypeError("Failed to execute 'connect' on 'AudioNode': Overload resolution failed.");
      if (dest[S].ctx !== this[S].ctx) throw domError("Failed to execute 'connect' on 'AudioNode': cannot connect to an AudioNode belonging to a different audio context.", 'InvalidAccessError');
      this[S].out.add(dest);
      refreshGains(this[S].ctx);
      return dest;
    }
    disconnect(dest) {
      if (dest === undefined) this[S].out.clear(); else this[S].out.delete(dest);
      refreshGains(this[S].ctx);
    }
  }
  tag(AudioNode, 'AudioNode');
  function initNode(node, ctx, inputs, outputs) {
    Object.defineProperty(node, S, { value: { ctx, inputs, outputs, out: new Set(), channelCount: 2, countMode: 'max', interp: 'speakers' } });
    ctx[S].nodes.add(node);
    return node;
  }
  /// Effective gain from `node` to the destination (product along the
  /// strongest path; 0 when not connected).
  function pathGain(node, seen = new Set()) {
    if (node instanceof AudioDestinationNode) return 1;
    if (seen.has(node)) return 0;
    seen.add(node);
    const own = node instanceof GainNode ? node[S].gain.value : 1;
    let best = 0;
    for (const n of node[S].out) if (n instanceof AudioNode) best = Math.max(best, pathGain(n, seen));
    seen.delete(node);
    return own * best;
  }
  function refreshGains(ctx) {
    for (const v of ctx[S].voices) v.updateGain();
  }

  class AudioDestinationNode extends AudioNode {
    constructor() { super(); ILLEGAL(); }
    get maxChannelCount() { return 2; }
  }
  tag(AudioDestinationNode, 'AudioDestinationNode');

  class GainNode extends AudioNode {
    constructor(ctx, options) {
      const n = Object.create(new.target.prototype);
      checkCtx(ctx, 'GainNode');
      initNode(n, ctx, 1, 1);
      n[S].gain = newParam(ctx, options && options.gain !== undefined ? +options.gain : 1, -3.4028234663852886e38, 3.4028234663852886e38, () => refreshGains(ctx));
      return n;
    }
    get gain() { return this[S].gain; }
  }
  tag(GainNode, 'GainNode');

  class AudioScheduledSourceNode extends AudioNode {
    constructor() { super(); ILLEGAL(); }
  }
  tag(AudioScheduledSourceNode, 'AudioScheduledSourceNode');
  N.installHandlers(AudioScheduledSourceNode.prototype, ['ended']);

  // A playing source: a native voice fed with the buffer's samples.
  class Voice {
    constructor(src, ctx, frames, rate, channels, loop, loopStart, loopEnd) {
      this.src = src; this.ctx = ctx; this.id = N.waVoiceCreate(); this.frames = frames; this.rate = rate; this.channels = channels;
      this.loop = loop; this.loopStart = loopStart; this.loopEnd = loopEnd; this.pos = 0; this.pts = 0; this.done = false;
      ctx[S].voices.add(this);
      this.updateGain();
      N.waVoiceRate(this.id, src[S].rate ? src[S].rate.value : 1);
    }
    updateGain() { if (!this.done) N.waVoiceGain(this.id, pathGain(this.src)); }
    feed(from, to) {
      // Interleave [from, to) and queue it.
      const n = to - from;
      if (n <= 0) return;
      const out = new Float32Array(n * this.channels);
      for (let c = 0; c < this.channels; c++) { const d = this.frames[c]; for (let i = 0; i < n; i++) out[i * this.channels + c] = d[from + i]; }
      N.waVoicePush(this.id, out, this.channels, this.rate, this.pts, !this.loop);
      this.pts += n / this.rate;
    }
    pump() {
      if (this.done) return;
      const st = N.waVoiceState(this.id);
      if (!this.loop) return st && st.drained ? this.finish() : undefined;
      // Keep ~1 s queued when looping.
      if (st && st.buffered < 1) {
        const ls = Math.floor(this.loopStart * this.rate), le = this.loopEnd > 0 ? Math.floor(this.loopEnd * this.rate) : this.frames[0].length;
        this.feed(ls, Math.max(ls + 1, le));
      }
    }
    finish() {
      if (this.done) return;
      this.done = true;
      N.waVoiceDestroy(this.id);
      this.ctx[S].voices.delete(this);
      const src = this.src;
      queueMicrotask(() => src.dispatchEvent(new Event('ended')));
    }
  }

  class AudioBufferSourceNode extends AudioScheduledSourceNode {
    constructor(ctx, options) {
      const n = Object.create(new.target.prototype);
      checkCtx(ctx, 'AudioBufferSourceNode');
      initNode(n, ctx, 0, 1);
      const o = options || {};
      Object.assign(n[S], { buffer: o.buffer || null, loop: !!o.loop, loopStart: +o.loopStart || 0, loopEnd: +o.loopEnd || 0, started: false, voice: null });
      n[S].rate = newParam(ctx, o.playbackRate !== undefined ? +o.playbackRate : 1, -3.4e38, 3.4e38, () => { if (n[S].voice) N.waVoiceRate(n[S].voice.id, n[S].rate.value); });
      n[S].detune = newParam(ctx, o.detune !== undefined ? +o.detune : 0, -3.4e38, 3.4e38);
      return n;
    }
    get buffer() { return this[S].buffer; }
    set buffer(b) { if (b !== null && !(b instanceof AudioBuffer)) throw new TypeError("Failed to set the 'buffer' property on 'AudioBufferSourceNode': The provided value is not of type 'AudioBuffer'."); if (this[S].buffer && b) throw domError("Failed to set the 'buffer' property on 'AudioBufferSourceNode': Cannot set buffer to non-null after it has been already been set to a non-null buffer", 'InvalidStateError'); this[S].buffer = b; }
    get loop() { return this[S].loop; }
    set loop(v) { this[S].loop = !!v; if (this[S].voice) this[S].voice.loop = !!v; }
    get loopStart() { return this[S].loopStart; }
    set loopStart(v) { this[S].loopStart = +v; }
    get loopEnd() { return this[S].loopEnd; }
    set loopEnd(v) { this[S].loopEnd = +v; }
    get playbackRate() { return this[S].rate; }
    get detune() { return this[S].detune; }
    start(when = 0, offset = 0, duration) {
      const st = this[S];
      if (st.started) throw domError("Failed to execute 'start' on 'AudioBufferSourceNode': cannot call start more than once.", 'InvalidStateError');
      if (when < 0 || offset < 0) throw new RangeError("Failed to execute 'start' on 'AudioBufferSourceNode': The start time provided is negative.");
      st.started = true;
      const ctx = st.ctx;
      if (ctx[S].offline) return;
      const go = () => {
        if (st.stopped) return;
        const b = st.buffer;
        if (!b) { queueMicrotask(() => this.dispatchEvent(new Event('ended'))); return; }
        const rate = b.sampleRate, frames = b[S].data;
        const v = new Voice(this, ctx, frames, rate, frames.length, st.loop, st.loopStart, st.loopEnd);
        st.voice = v;
        const from = Math.min(frames[0].length, Math.floor(offset * rate));
        const to = duration !== undefined ? Math.min(frames[0].length, from + Math.floor(duration * rate)) : frames[0].length;
        v.feed(from, Math.max(from, to));
        N.waVoicePlay(v.id, ctx[S].state === 'running');
        schedulePump(ctx);
      };
      const delay = (when - ctx.currentTime) * 1000;
      if (delay > 1) setTimeout(go, delay); else go();
    }
    stop(when = 0) {
      const st = this[S];
      if (!st.started) throw domError("Failed to execute 'stop' on 'AudioBufferSourceNode': cannot call stop without calling start first.", 'InvalidStateError');
      const go = () => { st.stopped = true; if (st.voice) st.voice.finish(); };
      const delay = (when - st.ctx.currentTime) * 1000;
      if (delay > 1) setTimeout(go, delay); else go();
    }
  }
  tag(AudioBufferSourceNode, 'AudioBufferSourceNode');

  class OscillatorNode extends AudioScheduledSourceNode {
    constructor(ctx, options) {
      const n = Object.create(new.target.prototype);
      checkCtx(ctx, 'OscillatorNode');
      initNode(n, ctx, 0, 1);
      const o = options || {};
      n[S].type = o.type || 'sine';
      n[S].frequency = newParam(ctx, o.frequency !== undefined ? +o.frequency : 440, -24000, 24000);
      n[S].detune = newParam(ctx, o.detune !== undefined ? +o.detune : 0, -153600, 153600);
      return n;
    }
    get type() { return this[S].type; }
    set type(v) { if (['sine', 'square', 'sawtooth', 'triangle'].includes(v)) this[S].type = v; else if (v === 'custom') throw domError("Failed to set the 'type' property on 'OscillatorNode': 'type' cannot be set directly to 'custom'.  Use setPeriodicWave() to create a custom Oscillator type.", 'InvalidStateError'); }
    get frequency() { return this[S].frequency; }
    get detune() { return this[S].detune; }
    setPeriodicWave() { this[S].type = 'custom'; }
    start(when = 0) {
      const st = this[S];
      if (st.started) throw domError("Failed to execute 'start' on 'OscillatorNode': cannot call start more than once.", 'InvalidStateError');
      st.started = true;
      const ctx = st.ctx;
      if (ctx[S].offline) return;
      const go = () => {
        if (st.stopped) return;
        // One second of the waveform, looped.
        const f = st.frequency.value * Math.pow(2, st.detune.value / 1200);
        const len = RATE;
        const d = new Float32Array(len);
        for (let i = 0; i < len; i++) {
          const ph = (f * i / RATE) % 1;
          d[i] = st.type === 'square' ? (ph < 0.5 ? 1 : -1) : st.type === 'sawtooth' ? 2 * ph - 1 : st.type === 'triangle' ? 1 - 4 * Math.abs(ph - 0.5) : Math.sin(2 * Math.PI * ph);
        }
        const v = new Voice(this, ctx, [d], RATE, 1, true, 0, 0);
        st.voice = v;
        v.feed(0, len);
        N.waVoicePlay(v.id, ctx[S].state === 'running');
        schedulePump(ctx);
      };
      const delay = (when - ctx.currentTime) * 1000;
      if (delay > 1) setTimeout(go, delay); else go();
    }
    stop(when = 0) {
      const st = this[S];
      if (!st.started) throw domError("Failed to execute 'stop' on 'OscillatorNode': cannot call stop without calling start first.", 'InvalidStateError');
      const go = () => { st.stopped = true; if (st.voice) st.voice.finish(); };
      const delay = (when - st.ctx.currentTime) * 1000;
      if (delay > 1) setTimeout(go, delay); else go();
    }
  }
  tag(OscillatorNode, 'OscillatorNode');

  function schedulePump(ctx) {
    const s = ctx[S];
    if (s.pumpTimer) return;
    const tick = () => {
      s.pumpTimer = 0;
      for (const v of Array.from(s.voices)) v.pump();
      if (s.voices.size) s.pumpTimer = setTimeout(tick, 100);
    };
    s.pumpTimer = setTimeout(tick, 100);
  }

  function checkCtx(ctx, name) {
    if (!(ctx instanceof BaseAudioContext)) throw new TypeError(`Failed to construct '${name}': parameter 1 is not of type 'BaseAudioContext'.`);
  }

  // ---------------------------------------------------------- contexts
  class BaseAudioContext extends EventTarget {
    constructor() { super(); ILLEGAL(); }
    get destination() { return this[S].destination; }
    get sampleRate() { return this[S].rate; }
    get currentTime() {
      const s = this[S];
      return s.state === 'running' ? s.base + (performance.now() - s.since) / 1000 : s.base;
    }
    get state() { return this[S].state; }
    get listener() { return null; }
    get audioWorklet() { return undefined; }
    createGain() { return new GainNode(this); }
    createBufferSource() { return new AudioBufferSourceNode(this); }
    createOscillator() { return new OscillatorNode(this); }
    createBuffer(channels, length, rate) { return new AudioBuffer({ numberOfChannels: channels, length, sampleRate: rate }); }
    decodeAudioData(data, ok, fail) {
      const ctx = this;
      return new Promise((resolve, reject) => {
        if (!(data instanceof ArrayBuffer)) { const e = new TypeError("Failed to execute 'decodeAudioData' on 'BaseAudioContext': parameter 1 is not of type 'ArrayBuffer'."); if (fail) fail(e); reject(e); return; }
        setTimeout(() => {
          const r = N.waDecode(data, ctx[S].rate);
          if (typeof r === 'string') { const e = domError(r, 'EncodingError'); if (typeof fail === 'function') fail(e); reject(e); return; }
          const b = makeBuffer(r.channels.map((ab) => new Float32Array(ab)), r.rate);
          if (typeof ok === 'function') ok(b);
          resolve(b);
        }, 0);
      });
    }
  }
  tag(BaseAudioContext, 'BaseAudioContext');
  N.installHandlers(BaseAudioContext.prototype, ['statechange']);

  function initContext(c, rate, state) {
    Object.defineProperty(c, S, { value: { rate, state, base: 0, since: performance.now(), voices: new Set(), nodes: new Set(), pumpTimer: 0, destination: null } });
    const d = Object.create(AudioDestinationNode.prototype);
    initNode(d, c, 1, 0);
    c[S].destination = d;
  }
  function setState(c, state) {
    const s = c[S];
    if (s.state === state) return;
    if (s.state === 'running') s.base += (performance.now() - s.since) / 1000;
    s.since = performance.now();
    s.state = state;
    for (const v of s.voices) N.waVoicePlay(v.id, state === 'running');
    queueMicrotask(() => c.dispatchEvent(new Event('statechange')));
  }

  class AudioContext extends BaseAudioContext {
    constructor(options) {
      const c = Object.create(new.target.prototype);
      const rate = options && options.sampleRate ? +options.sampleRate : RATE;
      initContext(c, rate, 'suspended');
      // Autoplay policy: running after a user gesture (or RH_AUTOPLAY).
      const ua = g.navigator && g.navigator.userActivation;
      if ((ua && ua.hasBeenActive) || N.mediaAutoplayEnv()) setState(c, 'running');
      return c;
    }
    get baseLatency() { return 0.01; }
    get outputLatency() { return 0.02; }
    get sinkId() { return ''; }
    resume() { if (this[S].state === 'closed') return Promise.reject(domError('Cannot resume a closed AudioContext.', 'InvalidStateError')); setState(this, 'running'); return Promise.resolve(); }
    suspend() { if (this[S].state === 'closed') return Promise.reject(domError('Cannot suspend a closed AudioContext.', 'InvalidStateError')); setState(this, 'suspended'); return Promise.resolve(); }
    close() {
      if (this[S].state === 'closed') return Promise.reject(domError('Cannot close a closed AudioContext.', 'InvalidStateError'));
      for (const v of Array.from(this[S].voices)) v.finish();
      setState(this, 'closed');
      return Promise.resolve();
    }
    getOutputTimestamp() { return { contextTime: this.currentTime, performanceTime: performance.now() }; }
    setSinkId() { return Promise.resolve(); }
    createMediaElementSource() { throw domError("Failed to execute 'createMediaElementSource' on 'AudioContext': not supported.", 'NotSupportedError'); }
  }
  tag(AudioContext, 'AudioContext');

  // Offline rendering: mixes AudioBufferSourceNodes / oscillators through
  // gains into a buffer (no effects).
  class OfflineAudioContext extends BaseAudioContext {
    constructor(a, b, c) {
      const o = typeof a === 'object' ? a : { numberOfChannels: a, length: b, sampleRate: c };
      const ctx = Object.create(new.target.prototype);
      initContext(ctx, +o.sampleRate || RATE, 'suspended');
      ctx[S].offline = { channels: o.numberOfChannels || 1, length: o.length >>> 0 };
      return ctx;
    }
    get length() { return this[S].offline.length; }
    startRendering() {
      const ctx = this;
      const { channels, length } = ctx[S].offline;
      const out = Array.from({ length: channels }, () => new Float32Array(length));
      for (const n of ctx[S].nodes) {
        const st = n[S];
        if (!st.started) continue;
        const gain = pathGain(n);
        if (n instanceof AudioBufferSourceNode && st.buffer) {
          const src = st.buffer[S].data;
          for (let c = 0; c < channels; c++) { const d = src[Math.min(c, src.length - 1)]; for (let i = 0; i < Math.min(length, d.length); i++) out[c][i] += d[i] * gain; }
        } else if (n instanceof OscillatorNode) {
          const f = st.frequency.value;
          for (let i = 0; i < length; i++) { const v = Math.sin(2 * Math.PI * f * i / ctx[S].rate) * gain; for (let c = 0; c < channels; c++) out[c][i] += v; }
        }
      }
      const buf = makeBuffer(out, ctx[S].rate);
      setState(ctx, 'closed');
      return new Promise((resolve) => setTimeout(() => { ctx.dispatchEvent(Object.assign(new Event('complete'), { renderedBuffer: buf })); resolve(buf); }, 0));
    }
    resume() { return Promise.resolve(); }
    suspend() { return Promise.resolve(); }
  }
  tag(OfflineAudioContext, 'OfflineAudioContext');
  N.installHandlers(OfflineAudioContext.prototype, ['complete']);

  // Node constructors taking the context first (`new GainNode(ctx)`)
  // already work; the AudioNode bases are not constructible.
  for (const [n, C] of [['AudioBuffer', AudioBuffer], ['AudioParam', AudioParam], ['AudioNode', AudioNode], ['AudioDestinationNode', AudioDestinationNode], ['GainNode', GainNode], ['AudioScheduledSourceNode', AudioScheduledSourceNode], ['AudioBufferSourceNode', AudioBufferSourceNode], ['OscillatorNode', OscillatorNode], ['BaseAudioContext', BaseAudioContext], ['AudioContext', AudioContext], ['OfflineAudioContext', OfflineAudioContext]]) define(n, C);
  if (!g.webkitAudioContext) define('webkitAudioContext', AudioContext);
})();
