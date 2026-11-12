// HTMLMediaElement playback (load / resource selection, readyState and
// networkState transitions, the event sequence, play() promises, autoplay
// policy, seeking, TimeRanges), Media Source Extensions (MediaSource,
// SourceBuffer, SourceBufferList) and navigator.mediaCapabilities, on top
// of the native media pipeline (`__rhNative.media*`, `ms*`, `sb*`).
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  if (!N || !N.mediaCreate || !g.HTMLMediaElement) return;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const acc = (C, name, get, set) => Object.defineProperty(C.prototype, name, { get, set, configurable: true, enumerable: true });
  const method = (C, name, fn) => Object.defineProperty(C.prototype, name, { value: fn, writable: true, configurable: true });
  const resolveURL = (v) => { try { return new URL(v, document.baseURI).href; } catch (e) { return null; } };
  const debug = (m) => N.mediaDebug(m);
  const ME = g.HTMLMediaElement;
  const NETWORK_EMPTY = 0, NETWORK_IDLE = 1, NETWORK_LOADING = 2, NETWORK_NO_SOURCE = 3;
  const HAVE_NOTHING = 0, HAVE_METADATA = 1, HAVE_CURRENT_DATA = 2, HAVE_FUTURE_DATA = 3, HAVE_ENOUGH_DATA = 4;

  // ------------------------------------------------------------ TimeRanges
  // html.js defines the interfaces; instances made here carry their
  // ranges under our own key.
  const TR = g.TimeRanges;
  const TRS = Symbol('ranges');
  {
    const d = (k) => Object.getOwnPropertyDescriptor(TR.prototype, k);
    const len = d('length'), start = d('start'), end = d('end');
    const check = (r, i) => { if (!(i >= 0 && i < r.length)) throw domError(`The index provided (${i}) is greater than or equal to the maximum bound (${r.length}).`, 'IndexSizeError'); };
    Object.defineProperty(TR.prototype, 'length', { get() { return this[TRS] ? this[TRS].length : len.get.call(this); }, configurable: true, enumerable: true });
    Object.defineProperty(TR.prototype, 'start', { value(i) { if (!this[TRS]) return start.value.call(this, i); i = Number(i) >>> 0 === Number(i) ? Number(i) : Number(i); check(this[TRS], i); return this[TRS][i][0]; }, writable: true, configurable: true, enumerable: true });
    Object.defineProperty(TR.prototype, 'end', { value(i) { if (!this[TRS]) return end.value.call(this, i); check(this[TRS], i); return this[TRS][i][1]; }, writable: true, configurable: true, enumerable: true });
  }
  const timeRanges = (list) => { const t = Object.create(TR.prototype); Object.defineProperty(t, TRS, { value: (list || []).map((r) => [r[0], r[1]]) }); return t; };
  const MEDIA_ERR_ABORTED = 1, MEDIA_ERR_NETWORK = 2, MEDIA_ERR_DECODE = 3, MEDIA_ERR_SRC_NOT_SUPPORTED = 4;
  const MErr = g.MediaError;
  const MES = Symbol('mediaError');
  {
    const code = Object.getOwnPropertyDescriptor(MErr.prototype, 'code');
    const msg = Object.getOwnPropertyDescriptor(MErr.prototype, 'message');
    Object.defineProperty(MErr.prototype, 'code', { get() { return this[MES] ? this[MES].code : code.get.call(this); }, configurable: true, enumerable: true });
    Object.defineProperty(MErr.prototype, 'message', { get() { return this[MES] ? this[MES].message : msg.get.call(this); }, configurable: true, enumerable: true });
  }
  const mediaError = (code, message) => { const e = Object.create(MErr.prototype); Object.defineProperty(e, MES, { value: { code, message: message || '' } }); return e; };

  // ---------------------------------------------------------------- state
  const states = new WeakMap();
  const active = new Set();
  function st(el) {
    let s = states.get(el);
    if (!s) {
      s = {
        id: 0, gen: 0, ms: null, networkState: NETWORK_EMPTY, readyState: HAVE_NOTHING, paused: true, ended: false, seeking: false,
        error: null, currentSrc: '', time: 0, duration: NaN, defaultStart: 0, rate: 1, defaultRate: 1, volume: 1, muted: null,
        pending: [], autoplaying: true, seekSerial: 0, seekTarget: 0, lastTimeupdate: 0, lastProgress: 0, waiting: false,
        loadeddata: false, played: [], playStart: null, sources: null, srcIdx: 0, deferred: false, width: 0, height: 0, status: null,
        lastTime: 0, buffered: [], seekable: [], idleSent: false, preloadNone: false,
      };
      states.set(el, s);
    }
    return s;
  }
  const isMuted = (el, s) => (s.muted === null ? el.hasAttribute('muted') : s.muted);

  // Media element tasks: run in order, dropped when the element reloads.
  const tasks = [];
  let taskTimer = 0;
  function queue(el, fn, keep) {
    const s = st(el);
    tasks.push([el, keep ? -1 : s.gen, fn]);
    if (!taskTimer) taskTimer = setTimeout(runTasks, 0);
  }
  function runTasks() {
    taskTimer = 0;
    const list = tasks.splice(0);
    for (const [el, gen, fn] of list) {
      if (gen !== -1 && st(el).gen !== gen) continue;
      try { fn(); } catch (e) { console.error(e); }
    }
  }
  const fire = (el, type) => el.dispatchEvent(new Event(type));
  const fireQueued = (el, type) => queue(el, () => fire(el, type), type === 'volumechange' || type === 'ratechange');

  // ----------------------------------------------------------- play promises
  function takePending(s) { return s.pending.splice(0); }
  function resolvePlay(el, s) {
    const p = takePending(s);
    if (p.length) queue(el, () => p.forEach((x) => x.resolve()));
  }
  function rejectPlay(el, s, name, msg) {
    const p = takePending(s);
    if (p.length) queue(el, () => p.forEach((x) => x.reject(domError(msg, name))));
  }
  // "Notify about playing": fire playing and resolve pending promises.
  function notifyPlaying(el, s) {
    const p = takePending(s);
    queue(el, () => { fire(el, 'playing'); p.forEach((x) => x.resolve()); });
  }

  // --------------------------------------------------------------- polling
  let pollTimer = 0;
  function wake() { if (!pollTimer && active.size) pollTimer = setTimeout(poll, 30); }
  function needsPolling(el, s) {
    return s.id && (!s.paused || s.seeking || s.networkState === NETWORK_LOADING || s.readyState < HAVE_ENOUGH_DATA || !s.idleSent);
  }
  function poll() {
    pollTimer = 0;
    let again = false, fast = false;
    for (const el of active) {
      const s = st(el);
      if (!s.id) { active.delete(el); continue; }
      try {
        update(el, s);
        syncUi(el, s);
        if (el.localName === 'video' && s.status && s.status.hasVideo && N.mediaFrame(s.id, el)) s.frames = (s.frames || 0) + 1;
      } catch (e) { console.error(e); }
      if (needsPolling(el, s)) again = true;
      if (el.localName === 'video' && s.status && s.status.hasVideo && !s.paused) fast = true;
    }
    if (again) pollTimer = setTimeout(poll, fast ? 16 : 50);
  }

  function setReady(el, s, ready) {
    const old = s.readyState;
    if (ready === old) return;
    if (old === HAVE_NOTHING && ready >= HAVE_METADATA) {
      s.readyState = HAVE_METADATA;
      if (s.status) { s.width = s.status.width; s.height = s.status.height; }
      queue(el, () => {
        fire(el, 'durationchange');
        if (el.localName === 'video' && (s.width || s.height)) fire(el, 'resize');
        fire(el, 'loadedmetadata');
      });
      if (s.defaultStart > 0) { const t = s.defaultStart; s.defaultStart = 0; seek(el, s, t); }
    }
    if (ready >= HAVE_CURRENT_DATA && old < HAVE_CURRENT_DATA && !s.loadeddata) {
      s.loadeddata = true;
      fireQueued(el, 'loadeddata');
    }
    s.readyState = ready;
    if (ready >= HAVE_FUTURE_DATA && old < HAVE_FUTURE_DATA) {
      fireQueued(el, 'canplay');
      if (!s.paused) { s.waiting = false; notifyPlaying(el, s); }
    }
    if (ready === HAVE_ENOUGH_DATA && old < HAVE_ENOUGH_DATA) {
      if (old < HAVE_FUTURE_DATA) { /* canplay queued above */ }
      fireQueued(el, 'canplaythrough');
      // Autoplay.
      if (s.autoplaying && s.paused && el.hasAttribute('autoplay') && allowedToPlay(el, s)) {
        s.paused = false;
        s.autoplaying = false;
        fireQueued(el, 'play');
        startPlayer(s);
        notifyPlaying(el, s);
      }
    }
    if (ready < old && ready <= HAVE_CURRENT_DATA && !s.paused && old >= HAVE_FUTURE_DATA && !s.ended) {
      s.waiting = true;
      queue(el, () => { fire(el, 'timeupdate'); fire(el, 'waiting'); });
    }
  }

  function update(el, s) {
    const stt = N.mediaStatus(s.id);
    if (!stt) return;
    s.status = stt;
    s.buffered = stt.buffered;
    s.seekable = stt.seekable;
    if (stt.error && !s.error) {
      if (s.readyState === HAVE_NOTHING) return sourceFailed(el, s, stt.error[0] === MEDIA_ERR_NETWORK ? MEDIA_ERR_SRC_NOT_SUPPORTED : stt.error[0], stt.error[1]);
      s.error = mediaError(stt.error[0], stt.error[1]);
      s.networkState = NETWORK_IDLE;
      debug(`error ${stt.error[0]}: ${stt.error[1]}`);
      fireQueued(el, 'error');
      return;
    }
    const dur = s.ms ? s.ms[MSS].duration : stt.duration;
    if (stt.ready >= HAVE_METADATA && !(dur === s.duration || (Number.isNaN(dur) && Number.isNaN(s.duration)))) {
      const had = s.readyState >= HAVE_METADATA;
      s.duration = dur;
      if (had) fireQueued(el, 'durationchange');
    }
    if (el.localName === 'video' && s.readyState >= HAVE_METADATA && (stt.width !== s.width || stt.height !== s.height)) {
      s.width = stt.width; s.height = stt.height;
      fireQueued(el, 'resize');
    }
    // Seek completion.
    if (s.seeking && !stt.seeking && stt.seekSerial !== s.seekSerial) {
      s.seekSerial = stt.seekSerial;
      s.seeking = false;
      s.time = s.seekTarget;
      queue(el, () => { fire(el, 'timeupdate'); fire(el, 'seeked'); });
    }
    if (!s.seeking) s.time = stt.time;
    setReady(el, s, s.seeking ? Math.min(stt.ready, HAVE_METADATA) || HAVE_METADATA : stt.ready);
    // Network state.
    const now = performance.now();
    if (s.networkState === NETWORK_LOADING) {
      if (now - s.lastProgress > 350) { s.lastProgress = now; fireQueued(el, 'progress'); }
      if (stt.idle) { s.networkState = NETWORK_IDLE; s.idleSent = true; queue(el, () => { fire(el, 'progress'); fire(el, 'suspend'); }); }
    } else if (stt.idle) s.idleSent = true;
    if (!s.paused && !s.seeking) {
      if (now - s.lastTimeupdate >= 250) { s.lastTimeupdate = now; fireQueued(el, 'timeupdate'); }
      if (stt.ended) reachedEnd(el, s);
    }
  }

  function reachedEnd(el, s) {
    if (el.hasAttribute('loop')) {
      seek(el, s, 0);
      N.mediaPlay(s.id);
      return;
    }
    s.ended = true;
    recordPlayed(s);
    const wasPlaying = !s.paused;
    s.paused = true;
    if (wasPlaying) rejectPlay(el, s, 'AbortError', 'The play() request was interrupted by end of playback.');
    queue(el, () => {
      fire(el, 'timeupdate');
      if (wasPlaying) fire(el, 'pause');
      fire(el, 'ended');
    });
  }

  function recordPlayed(s) {
    if (s.playStart === null) return;
    const a = s.playStart, b = s.time;
    s.playStart = null;
    if (b > a) {
      s.played.push([a, b]);
      s.played.sort((x, y) => x[0] - y[0]);
      const out = [];
      for (const r of s.played) { const l = out[out.length - 1]; if (l && r[0] <= l[1]) l[1] = Math.max(l[1], r[1]); else out.push(r); }
      s.played = out;
    }
  }

  function startPlayer(s) {
    if (!s.id) return;
    N.mediaVolume(s.id, s.volume, 0);
    N.mediaPlay(s.id);
    s.playStart = s.time;
    wake();
  }

  // ------------------------------------------------- load / resource selection
  function abortLoad(el, s) {
    s.gen++;
    if (s.id) { N.mediaDestroy(s.id); s.id = 0; }
    if (s.ms) { detachSource(s.ms); s.ms = null; }
    active.delete(el);
  }

  function loadAlgorithm(el) {
    const s = st(el);
    const wasState = s.networkState;
    abortLoad(el, s);
    rejectPlay(el, s, 'AbortError', 'The play() request was interrupted by a new load request.');
    if (wasState === NETWORK_LOADING || wasState === NETWORK_IDLE) fireQueued(el, 'abort');
    if (wasState !== NETWORK_EMPTY) {
      fireQueued(el, 'emptied');
      s.networkState = NETWORK_EMPTY;
      s.readyState = HAVE_NOTHING;
      if (!s.paused) s.paused = true;
      s.seeking = false;
      s.time = 0;
      if (!Number.isNaN(s.duration)) { s.duration = NaN; fireQueued(el, 'durationchange'); }
    }
    s.rate = s.defaultRate;
    s.error = null;
    s.autoplaying = true;
    s.ended = false;
    s.loadeddata = false;
    s.played = [];
    s.playStart = null;
    s.buffered = [];
    s.seekable = [];
    s.idleSent = false;
    s.status = null;
    s.width = s.height = 0;
    selectResource(el, s);
  }

  function selectResource(el, s) {
    s.networkState = NETWORK_NO_SOURCE;
    const gen = s.gen;
    // Await a stable state.
    queueMicrotask(() => {
      if (s.gen !== gen) return;
      let mode = null;
      if (s.srcObject) mode = 'object';
      else if (el.hasAttribute('src')) mode = 'attribute';
      else if (Array.from(el.children).some((c) => c.localName === 'source')) mode = 'children';
      if (!mode) { s.networkState = NETWORK_EMPTY; return; }
      s.networkState = NETWORK_LOADING;
      fireQueued(el, 'loadstart');
      if (mode === 'object') return startFetch(el, s, '', s.srcObject);
      if (mode === 'attribute') {
        const raw = el.getAttribute('src');
        const url = raw ? resolveURL(raw) : null;
        if (!url) return sourceFailed(el, s, MEDIA_ERR_SRC_NOT_SUPPORTED, 'MEDIA_ELEMENT_ERROR: Empty src attribute');
        return startFetch(el, s, url, null);
      }
      s.sources = Array.from(el.children).filter((c) => c.localName === 'source');
      s.srcIdx = 0;
      tryNextSource(el, s);
    });
  }

  function tryNextSource(el, s) {
    while (s.sources && s.srcIdx < s.sources.length) {
      const src = s.sources[s.srcIdx++];
      const raw = src.getAttribute('src');
      const type = src.getAttribute('type');
      const media = src.getAttribute('media');
      if (raw === null || raw === '') { queue(el, () => fire(src, 'error')); continue; }
      if (type && N.mediaCanPlayType(type) === '') { queue(el, () => fire(src, 'error')); continue; }
      if (media && g.matchMedia && !g.matchMedia(media).matches) continue;
      const url = resolveURL(raw);
      if (!url) { queue(el, () => fire(src, 'error')); continue; }
      s.currentSource = src;
      return startFetch(el, s, url, null);
    }
    // Nothing playable: wait for more <source> children.
    s.networkState = NETWORK_NO_SOURCE;
    s.sources = null;
    rejectPlay(el, s, 'NotSupportedError', 'The element has no supported sources.');
  }

  function startFetch(el, s, url, object) {
    s.currentSrc = url;
    const ms = object instanceof MediaSource ? object : (url && blobSources.get(url)) || null;
    if (ms) {
      if (ms[MSS].readyState !== 'closed') return sourceFailed(el, s, MEDIA_ERR_SRC_NOT_SUPPORTED, 'MediaSource already attached');
      s.ms = ms;
      attachSource(ms, el);
      const id = N.mediaCreate('', ms[MSS].id);
      if (typeof id === 'string') return sourceFailed(el, s, MEDIA_ERR_SRC_NOT_SUPPORTED, id);
      s.id = id;
    } else {
      if (object) return sourceFailed(el, s, MEDIA_ERR_SRC_NOT_SUPPORTED, 'Unsupported srcObject');
      // preload=none: wait for play() (or a later load).
      const preload = (el.getAttribute('preload') || '').toLowerCase();
      if (preload === 'none' && !el.hasAttribute('autoplay') && !s.forceLoad) {
        s.preloadNone = true;
        s.networkState = NETWORK_IDLE;
        fireQueued(el, 'suspend');
        return;
      }
      s.preloadNone = false;
      const id = N.mediaCreate(url, null);
      if (typeof id === 'string') return sourceFailed(el, s, MEDIA_ERR_SRC_NOT_SUPPORTED, id);
      s.id = id;
    }
    debug(`load ${url || '(object)'} -> player ${s.id}`);
    N.mediaVolume(s.id, s.volume, isMuted(el, s) ? 1 : 0);
    N.mediaRate(s.id, s.rate);
    // play() may have run before the resource was selected.
    if (!s.paused) { N.mediaPlay(s.id); s.playStart = s.time; }
    active.add(el);
    s.lastProgress = performance.now();
    wake();
  }

  function sourceFailed(el, s, code, message) {
    debug(`source failed (${code}): ${message}`);
    if (s.id) { N.mediaDestroy(s.id); s.id = 0; }
    active.delete(el);
    if (s.sources) {
      const src = s.currentSource;
      if (src) queue(el, () => fire(src, 'error'));
      return tryNextSource(el, s);
    }
    s.error = mediaError(code, message);
    s.networkState = NETWORK_NO_SOURCE;
    fireQueued(el, 'error');
    rejectPlay(el, s, 'NotSupportedError', 'Failed to load because no supported source was found.');
  }

  // ------------------------------------------------------------------ seek
  function seek(el, s, t) {
    if (s.readyState === HAVE_NOTHING) { s.defaultStart = t; return; }
    t = Number(t);
    if (!Number.isFinite(t)) t = 0;
    const d = s.ms ? s.ms[MSS].duration : s.duration;
    if (Number.isFinite(d) && t > d) t = d;
    if (t < 0) t = 0;
    recordPlayed(s);
    s.seeking = true;
    s.ended = false;
    s.seekTarget = t;
    s.time = t;
    if (!s.paused) s.playStart = t;
    if (s.id) N.mediaSeek(s.id, t);
    fireQueued(el, 'seeking');
    wake();
  }

  // -------------------------------------------------------- autoplay policy
  function allowedToPlay(el, s) {
    if (isMuted(el, s) || s.volume === 0) return true;
    if (N.mediaAutoplayEnv()) return true;
    const ua = g.navigator && g.navigator.userActivation;
    return !!(ua && ua.hasBeenActive);
  }

  // ------------------------------------------------------------ properties
  for (const k of ['paused', 'ended', 'duration', 'currentTime', 'volume', 'muted', 'playbackRate', 'defaultPlaybackRate', 'readyState', 'networkState', 'seeking', 'error', 'buffered', 'played', 'seekable', 'currentSrc', 'srcObject', 'defaultMuted', 'preservesPitch']) delete ME.prototype[k];
  acc(ME, 'networkState', function () { return st(this).networkState; });
  acc(ME, 'readyState', function () { return st(this).readyState; });
  acc(ME, 'paused', function () { return st(this).paused; });
  acc(ME, 'ended', function () { const s = st(this); return s.ended && !this.hasAttribute('loop'); });
  acc(ME, 'seeking', function () { return st(this).seeking; });
  acc(ME, 'error', function () { return st(this).error; });
  acc(ME, 'currentSrc', function () { return st(this).currentSrc; });
  acc(ME, 'duration', function () { const s = st(this); if (s.readyState === HAVE_NOTHING) return NaN; return s.ms ? s.ms[MSS].duration : s.duration; });
  acc(ME, 'currentTime', function () {
    const s = st(this);
    if (s.seeking || !s.id) return s.readyState === HAVE_NOTHING ? s.defaultStart : s.time;
    if (!s.paused && !s.ended) { const t = N.mediaTime(s.id); if (typeof t === 'number' && !Number.isNaN(t)) s.time = t; }
    return s.time;
  }, function (v) { seek(this, st(this), +v); });
  acc(ME, 'volume', function () { return st(this).volume; }, function (v) {
    v = +v;
    if (!Number.isFinite(v)) throw new TypeError("Failed to set the 'volume' property on 'HTMLMediaElement': The provided double value is non-finite.");
    if (!(v >= 0 && v <= 1)) throw domError(`The volume provided (${v}) is outside the range [0, 1].`, 'IndexSizeError');
    const s = st(this);
    if (s.volume === v) return;
    s.volume = v;
    if (s.id) N.mediaVolume(s.id, v, isMuted(this, s) ? 1 : 0);
    fireQueued(this, 'volumechange');
    wake();
  });
  acc(ME, 'muted', function () { return isMuted(this, st(this)); }, function (v) {
    const s = st(this);
    v = !!v;
    if (isMuted(this, s) === v) { s.muted = v; return; }
    s.muted = v;
    if (s.id) N.mediaVolume(s.id, s.volume, v ? 1 : 0);
    fireQueued(this, 'volumechange');
    wake();
  });
  acc(ME, 'defaultMuted', function () { return this.hasAttribute('muted'); }, function (v) { this.toggleAttribute('muted', !!v); });
  acc(ME, 'playbackRate', function () { return st(this).rate; }, function (v) {
    v = +v;
    if (!Number.isFinite(v)) throw new TypeError("Failed to set the 'playbackRate' property on 'HTMLMediaElement': The provided double value is non-finite.");
    if (v !== 0 && (Math.abs(v) < 0.0625 || Math.abs(v) > 16)) throw domError(`The provided playback rate (${v}) is not in the supported playback range.`, 'NotSupportedError');
    const s = st(this);
    if (s.rate === v) return;
    s.rate = v;
    if (s.id) N.mediaRate(s.id, v > 0 ? v : 1);
    fireQueued(this, 'ratechange');
  });
  acc(ME, 'defaultPlaybackRate', function () { return st(this).defaultRate; }, function (v) {
    v = +v;
    const s = st(this);
    if (s.defaultRate === v) return;
    s.defaultRate = v;
    fireQueued(this, 'ratechange');
  });
  acc(ME, 'preservesPitch', function () { const s = st(this); return s.preservesPitch !== false; }, function (v) { st(this).preservesPitch = !!v; });
  acc(ME, 'buffered', function () {
    const s = st(this);
    if (s.ms) return timeRanges(s.ms[MSS].info().buffered);
    if (s.id) { const x = N.mediaStatus(s.id); if (x) s.buffered = x.buffered; }
    return timeRanges(s.buffered);
  });
  acc(ME, 'seekable', function () {
    const s = st(this);
    if (s.readyState === HAVE_NOTHING) return timeRanges([]);
    if (s.ms) {
      const m = s.ms[MSS];
      if (Number.isFinite(m.duration)) return timeRanges([[0, m.duration]]);
      if (m.duration === Infinity) {
        if (m.liveRange) return timeRanges([m.liveRange]);
        const b = m.info().buffered;
        return timeRanges(b.length ? [[0, b[b.length - 1][1]]] : []);
      }
      return timeRanges([]);
    }
    return timeRanges(s.seekable);
  });
  acc(ME, 'played', function () {
    const s = st(this);
    const list = s.played.slice();
    if (s.playStart !== null && s.time > s.playStart) list.push([s.playStart, s.time]);
    return timeRanges(list);
  });
  acc(ME, 'srcObject', function () { return st(this).srcObject || null; }, function (v) { const s = st(this); s.srcObject = v || null; loadAlgorithm(this); });
  method(ME, 'canPlayType', function (type) { return N.mediaCanPlayType(String(type)); });
  method(ME, 'load', function () { const s = st(this); s.forceLoad = false; loadAlgorithm(this); });
  method(ME, 'play', function () {
    const el = this;
    const s = st(el);
    if (!allowedToPlay(el, s)) {
      return Promise.reject(domError("play() failed because the user didn't interact with the document first.", 'NotAllowedError'));
    }
    if (s.error && s.error.code === MEDIA_ERR_SRC_NOT_SUPPORTED) {
      return Promise.reject(domError('The element has no supported sources.', 'NotSupportedError'));
    }
    const p = new Promise((resolve, reject) => s.pending.push({ resolve, reject }));
    if (s.networkState === NETWORK_EMPTY) loadAlgorithm(el);
    else if (s.preloadNone && !s.id) { s.forceLoad = true; const url = s.currentSrc; s.networkState = NETWORK_LOADING; startFetch(el, s, url, null); }
    if (s.ended && s.playbackRate !== -1) { seek(el, s, 0); }
    s.autoplaying = false;
    if (s.paused) {
      s.paused = false;
      s.ended = false;
      fireQueued(el, 'play');
      if (s.readyState <= HAVE_CURRENT_DATA) { s.waiting = true; fireQueued(el, 'waiting'); }
      else notifyPlaying(el, s);
      startPlayer(s);
    } else if (s.readyState >= HAVE_FUTURE_DATA) {
      resolvePlay(el, s);
    }
    wake();
    return p;
  });
  method(ME, 'pause', function () {
    const el = this;
    const s = st(el);
    if (s.networkState === NETWORK_EMPTY) loadAlgorithm(el);
    s.autoplaying = false;
    if (!s.paused) {
      if (s.id) { N.mediaPause(s.id); const t = N.mediaTime(s.id); if (typeof t === 'number') s.time = t; }
      recordPlayed(s);
      s.paused = true;
      queue(el, () => { fire(el, 'timeupdate'); fire(el, 'pause'); });
      rejectPlay(el, s, 'AbortError', 'The play() request was interrupted by a call to pause().');
      wake();
    }
  });
  method(ME, 'fastSeek', function (t) { seek(this, st(this), +t); });
  if (g.HTMLVideoElement) {
    const V = g.HTMLVideoElement;
    acc(V, 'videoWidth', function () { const s = st(this); return s.readyState >= HAVE_METADATA ? s.width : 0; });
    acc(V, 'videoHeight', function () { const s = st(this); return s.readyState >= HAVE_METADATA ? s.height : 0; });
    method(V, 'getVideoPlaybackQuality', function () {
      const s = st(this);
      if (s.id && s.status && s.status.videoCodec === 'av1') {
        const [shown, dropped] = N.mediaVideoStats(s.id);
        return { creationTime: performance.now(), droppedVideoFrames: dropped, totalVideoFrames: shown + dropped, corruptedVideoFrames: 0 };
      }
      const frames = Math.floor((s.time || 0) * 30);
      return { creationTime: performance.now(), droppedVideoFrames: 0, totalVideoFrames: frames, corruptedVideoFrames: 0 };
    });
  }

  // `src` changes, <source> insertion and removal from the document reach
  // us through the native mutation hook.
  N.mediaSetMutationHook(function (el, what) {
    const s = st(el);
    if (what === 'src') {
      if (el.hasAttribute('src') || s.networkState !== NETWORK_EMPTY) loadAlgorithm(el);
    } else if (what === 'source') {
      if (s.networkState === NETWORK_EMPTY) loadAlgorithm(el);
      else if (s.networkState === NETWORK_NO_SOURCE && !s.id && !s.ms) {
        s.sources = Array.from(el.children).filter((c) => c.localName === 'source');
        s.networkState = NETWORK_LOADING;
        tryNextSource(el, s);
      }
    } else if (what === 'inserted') {
      if (s.networkState === NETWORK_EMPTY && (el.hasAttribute('src') || el.querySelector('source'))) loadAlgorithm(el);
    } else if (what === 'removed') {
      queueMicrotask(() => { if (!el.isConnected && !s.paused) el.pause(); });
    }
  });
  // Elements created by the parser.
  const scan = () => { for (const el of document.querySelectorAll('audio, video')) { const s = st(el); if (s.networkState === NETWORK_EMPTY && (el.hasAttribute('src') || el.querySelector('source'))) loadAlgorithm(el); } };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', scan); else queueMicrotask(scan);

  // ============================================================ MSE
  const MSS = Symbol('mediaSource');
  const SBS = Symbol('sourceBuffer');
  const blobSources = new Map();

  class SourceBufferList extends EventTarget {
    constructor() { super(); ILLEGAL(); }
    get length() { return this[SBS].length; }
    [Symbol.iterator]() { return this[SBS][Symbol.iterator](); }
  }
  tag(SourceBufferList, 'SourceBufferList');
  N.installHandlers(SourceBufferList.prototype, ['addsourcebuffer', 'removesourcebuffer']);
  function newList() {
    const arr = [];
    const l = Object.create(SourceBufferList.prototype);
    Object.defineProperty(l, SBS, { value: arr });
    return new Proxy(l, {
      get(t, k) { if (typeof k === 'string' && /^\d+$/.test(k)) return arr[+k]; return Reflect.get(t, k, t); },
      has(t, k) { if (typeof k === 'string' && /^\d+$/.test(k)) return +k < arr.length; return Reflect.has(t, k); },
    });
  }

  class MediaSource extends EventTarget {
    constructor() {
      super();
      const m = {
        id: N.msCreate(), readyState: 'closed', duration: NaN, el: null, liveRange: null,
        buffers: newList(), active: newList(),
        info() { return N.msInfo(this.id) || { buffered: [], highest: 0 }; },
      };
      Object.defineProperty(this, MSS, { value: m });
    }
    static isTypeSupported(type) { return N.msIsTypeSupported(String(type)); }
    static get canConstructInDedicatedWorker() { return false; }
    get sourceBuffers() { return this[MSS].buffers; }
    get activeSourceBuffers() { return this[MSS].active; }
    get readyState() { return this[MSS].readyState; }
    get duration() { return this[MSS].readyState === 'closed' ? NaN : this[MSS].duration; }
    set duration(v) {
      v = +v;
      if (Number.isNaN(v) || v < 0) throw new TypeError(`Failed to set the 'duration' property on 'MediaSource': The value provided (${v}) is negative or NaN.`);
      const m = this[MSS];
      if (m.readyState !== 'open') throw domError("Failed to set the 'duration' property on 'MediaSource': The MediaSource's readyState is not 'open'.", 'InvalidStateError');
      if (m.buffers[SBS].some((b) => b[SBS].updating)) throw domError("Failed to set the 'duration' property on 'MediaSource': The 'updating' attribute is true on one or more of this MediaSource's SourceBuffers.", 'InvalidStateError');
      durationChange(this, v);
    }
    addSourceBuffer(type) {
      type = String(type);
      const m = this[MSS];
      if (type === '') throw new TypeError("Failed to execute 'addSourceBuffer' on 'MediaSource': The type provided is empty.");
      if (!MediaSource.isTypeSupported(type)) throw domError(`Failed to execute 'addSourceBuffer' on 'MediaSource': The type provided ('${type}') is unsupported.`, 'NotSupportedError');
      if (m.readyState !== 'open') throw domError("Failed to execute 'addSourceBuffer' on 'MediaSource': The MediaSource's readyState is not 'open'.", 'InvalidStateError');
      const idx = N.msAddSourceBuffer(m.id, type);
      if (idx === null) throw domError(`Failed to execute 'addSourceBuffer' on 'MediaSource': The type provided ('${type}') is unsupported.`, 'NotSupportedError');
      const sb = Object.create(SourceBuffer.prototype);
      Object.defineProperty(sb, SBS, { value: { ms: this, idx, updating: false, mode: /audio\/(mpeg|aac)/.test(type) ? 'sequence' : 'segments', timestampOffset: 0, appendWindowStart: 0, appendWindowEnd: Infinity, type, removed: false, generateTimestamps: /audio\/(mpeg|aac)/.test(type), pendingAbort: null } });
      m.buffers[SBS].push(sb);
      debug(`addSourceBuffer(${type}) -> ${idx}`);
      queueTask(() => m.buffers.dispatchEvent(new Event('addsourcebuffer')));
      return sb;
    }
    removeSourceBuffer(sb) {
      const m = this[MSS];
      const i = m.buffers[SBS].indexOf(sb);
      if (i < 0) throw domError("Failed to execute 'removeSourceBuffer' on 'MediaSource': The SourceBuffer provided is not contained in this MediaSource.", 'NotFoundError');
      const b = sb[SBS];
      if (b.updating) { b.updating = false; queueTask(() => { sb.dispatchEvent(new Event('abort')); sb.dispatchEvent(new Event('updateend')); }); }
      N.msRemoveSourceBuffer(m.id, b.idx);
      b.removed = true;
      m.buffers[SBS].splice(i, 1);
      const j = m.active[SBS].indexOf(sb);
      if (j >= 0) { m.active[SBS].splice(j, 1); queueTask(() => m.active.dispatchEvent(new Event('removesourcebuffer'))); }
      queueTask(() => m.buffers.dispatchEvent(new Event('removesourcebuffer')));
    }
    endOfStream(error) {
      const m = this[MSS];
      if (m.readyState !== 'open') throw domError("Failed to execute 'endOfStream' on 'MediaSource': The MediaSource's readyState is not 'open'.", 'InvalidStateError');
      if (m.buffers[SBS].some((b) => b[SBS].updating)) throw domError("Failed to execute 'endOfStream' on 'MediaSource': The 'updating' attribute is true on one or more of this MediaSource's SourceBuffers.", 'InvalidStateError');
      endOfStream(this, error);
    }
    setLiveSeekableRange(start, end) {
      const m = this[MSS];
      if (m.readyState !== 'open') throw domError("Failed to execute 'setLiveSeekableRange' on 'MediaSource': The MediaSource's readyState is not 'open'.", 'InvalidStateError');
      if (start < 0 || start > end) throw new TypeError("Failed to execute 'setLiveSeekableRange' on 'MediaSource': The start provided is outside the range.");
      m.liveRange = [+start, +end];
    }
    clearLiveSeekableRange() {
      const m = this[MSS];
      if (m.readyState !== 'open') throw domError("Failed to execute 'clearLiveSeekableRange' on 'MediaSource': The MediaSource's readyState is not 'open'.", 'InvalidStateError');
      m.liveRange = null;
    }
  }
  tag(MediaSource, 'MediaSource');
  N.installHandlers(MediaSource.prototype, ['sourceopen', 'sourceended', 'sourceclose']);

  const queueTask = (fn) => setTimeout(fn, 0);

  function durationChange(ms, v) {
    const m = ms[MSS];
    if (m.duration === v) return;
    const highest = m.info().highest;
    if (v < highest) throw domError("Failed to set the 'duration' property on 'MediaSource': The value provided is less than the highest presentation timestamp of buffered data.", 'InvalidStateError');
    m.duration = v;
    N.msSet(m.id, 'duration', v);
    const el = m.el;
    if (el) {
      const s = st(el);
      if (s.readyState >= HAVE_METADATA) { s.duration = v; fireQueued(el, 'durationchange'); }
    }
  }

  function endOfStream(ms, error) {
    const m = ms[MSS];
    m.readyState = 'ended';
    queueTask(() => ms.dispatchEvent(new Event('sourceended')));
    if (error === undefined || error === null || error === '') {
      const h = m.info().highest;
      if (h > 0 && (!(m.duration === h))) { const d = m.duration; m.duration = h; N.msSet(m.id, 'duration', h); if (m.el && d !== h) fireQueued(m.el, 'durationchange'); }
      N.msSet(m.id, 'ended', 1);
    } else if (m.el) {
      const el = m.el;
      const s = st(el);
      const code = error === 'network' ? MEDIA_ERR_NETWORK : MEDIA_ERR_DECODE;
      if (s.readyState === HAVE_NOTHING) sourceFailed(el, s, MEDIA_ERR_SRC_NOT_SUPPORTED, `MediaSource endOfStream('${error}')`);
      else { s.error = mediaError(code, `MediaSource endOfStream('${error}')`); fireQueued(el, 'error'); }
    }
  }

  function attachSource(ms, el) {
    const m = ms[MSS];
    m.el = el;
    m.readyState = 'open';
    N.msSet(m.id, 'ended', 0);
    queueTask(() => ms.dispatchEvent(new Event('sourceopen')));
  }

  function detachSource(ms) {
    const m = ms[MSS];
    m.readyState = 'closed';
    m.duration = NaN;
    m.el = null;
    for (const sb of m.buffers[SBS].slice()) { N.msRemoveSourceBuffer(m.id, sb[SBS].idx); sb[SBS].removed = true; }
    m.buffers[SBS].length = 0;
    m.active[SBS].length = 0;
    queueTask(() => { ms.dispatchEvent(new Event('sourceclose')); });
  }

  class SourceBuffer extends EventTarget {
    constructor() { super(); ILLEGAL(); }
    get mode() { return this[SBS].mode; }
    set mode(v) {
      const b = this[SBS];
      if (v !== 'segments' && v !== 'sequence') throw new TypeError(`Failed to set the 'mode' property on 'SourceBuffer': The provided value '${v}' is not a valid enum value of type AppendMode.`);
      checkUsable(this, 'mode', true);
      if (b.generateTimestamps && v === 'segments') throw new TypeError("Failed to set the 'mode' property on 'SourceBuffer': The mode value provided (segments) is invalid for a byte stream format that uses generated timestamps.");
      if (N.sbParsing(b.ms[MSS].id, b.idx)) throw domError("Failed to set the 'mode' property on 'SourceBuffer': The SourceBuffer is in the middle of parsing a media segment.", 'InvalidStateError');
      b.mode = v;
      N.sbSet(b.ms[MSS].id, b.idx, 'mode', v === 'sequence' ? 1 : 0);
    }
    get updating() { return this[SBS].updating; }
    get buffered() {
      const b = this[SBS];
      if (b.removed) throw domError("Failed to read the 'buffered' property from 'SourceBuffer': This SourceBuffer has been removed from the parent media source.", 'InvalidStateError');
      return timeRanges(N.sbBuffered(b.ms[MSS].id, b.idx));
    }
    get timestampOffset() { return this[SBS].timestampOffset; }
    set timestampOffset(v) {
      const b = this[SBS];
      checkUsable(this, 'timestampOffset', true);
      if (N.sbParsing(b.ms[MSS].id, b.idx)) throw domError("Failed to set the 'timestampOffset' property on 'SourceBuffer': The SourceBuffer is in the middle of parsing a media segment.", 'InvalidStateError');
      b.timestampOffset = +v;
      N.sbSet(b.ms[MSS].id, b.idx, 'timestampOffset', +v);
    }
    get appendWindowStart() { return this[SBS].appendWindowStart; }
    set appendWindowStart(v) {
      const b = this[SBS];
      checkUsable(this, 'appendWindowStart', false);
      v = +v;
      if (v < 0 || v >= b.appendWindowEnd) throw new TypeError(`Failed to set the 'appendWindowStart' property on 'SourceBuffer': The value provided (${v}) is outside the range [0, ${b.appendWindowEnd}).`);
      b.appendWindowStart = v;
      N.sbSet(b.ms[MSS].id, b.idx, 'appendWindow', b.appendWindowStart, b.appendWindowEnd);
    }
    get appendWindowEnd() { return this[SBS].appendWindowEnd; }
    set appendWindowEnd(v) {
      const b = this[SBS];
      checkUsable(this, 'appendWindowEnd', false);
      v = +v;
      if (Number.isNaN(v)) throw new TypeError("Failed to set the 'appendWindowEnd' property on 'SourceBuffer': The value provided is NaN.");
      if (v <= b.appendWindowStart) throw new TypeError(`Failed to set the 'appendWindowEnd' property on 'SourceBuffer': The value provided (${v}) is less than or equal to the start (${b.appendWindowStart}).`);
      b.appendWindowEnd = v;
      N.sbSet(b.ms[MSS].id, b.idx, 'appendWindow', b.appendWindowStart, v === Infinity ? NaN : v);
    }
    get audioTracks() { return this[SBS].audioTracks || (this[SBS].audioTracks = []); }
    get videoTracks() { return this[SBS].videoTracks || (this[SBS].videoTracks = []); }
    get textTracks() { return []; }
    appendBuffer(data) {
      const sb = this;
      const b = sb[SBS];
      if (!(data instanceof ArrayBuffer) && !ArrayBuffer.isView(data) && !(typeof SharedArrayBuffer !== 'undefined' && data instanceof SharedArrayBuffer)) throw new TypeError("Failed to execute 'appendBuffer' on 'SourceBuffer': The provided value is not of type '(ArrayBuffer or ArrayBufferView)'.");
      checkUsable(sb, 'appendBuffer', true, 'Failed to execute');
      const m = b.ms[MSS];
      if (m.readyState === 'ended') { m.readyState = 'open'; N.msSet(m.id, 'ended', 0); queueTask(() => b.ms.dispatchEvent(new Event('sourceopen'))); }
      const r = N.sbAppend(m.id, b.idx, data);
      if (r.quota) throw domError("Failed to execute 'appendBuffer' on 'SourceBuffer': The SourceBuffer is full, and cannot free space to append additional buffers.", 'QuotaExceededError');
      b.updating = true;
      queueTask(() => {
        if (b.removed) return;
        sb.dispatchEvent(new Event('updatestart'));
        if (r.error) {
          debug(`append error: ${r.error}`);
          N.sbAbort(m.id, b.idx);
          b.updating = false;
          sb.dispatchEvent(new Event('error'));
          sb.dispatchEvent(new Event('updateend'));
          if (m.readyState === 'open') endOfStream(b.ms, 'decode');
          return;
        }
        for (const init of r.inits) initSegmentReceived(sb, init);
        b.timestampOffset = r.timestampOffset;
        b.updating = false;
        sb.dispatchEvent(new Event('update'));
        sb.dispatchEvent(new Event('updateend'));
        if (m.el) wake();
      });
    }
    abort() {
      const b = this[SBS];
      const m = b.ms[MSS];
      if (b.removed || !m.buffers[SBS].includes(this)) throw domError("Failed to execute 'abort' on 'SourceBuffer': This SourceBuffer has been removed from the parent media source.", 'InvalidStateError');
      if (m.readyState !== 'open') throw domError("Failed to execute 'abort' on 'SourceBuffer': The MediaSource's readyState is not 'open'.", 'InvalidStateError');
      if (b.removing) throw domError("Failed to execute 'abort' on 'SourceBuffer': removal is in progress.", 'InvalidStateError');
      if (b.updating) {
        // The append already ran; report it aborted.
        b.updating = false;
        const sb = this;
        queueTask(() => { sb.dispatchEvent(new Event('abort')); sb.dispatchEvent(new Event('updateend')); });
      }
      N.sbAbort(m.id, b.idx);
      b.appendWindowStart = 0;
      b.appendWindowEnd = Infinity;
      N.sbSet(m.id, b.idx, 'appendWindow', 0, NaN);
    }
    changeType(type) {
      const b = this[SBS];
      type = String(type);
      if (type === '') throw new TypeError("Failed to execute 'changeType' on 'SourceBuffer': The type provided is empty.");
      checkUsable(this, 'changeType', true, 'Failed to execute');
      if (!MediaSource.isTypeSupported(type)) throw domError(`Failed to execute 'changeType' on 'SourceBuffer': The type provided ('${type}') is not supported.`, 'NotSupportedError');
      N.sbChangeType(b.ms[MSS].id, b.idx, type);
      b.type = type;
      b.generateTimestamps = /audio\/(mpeg|aac)/.test(type);
      if (b.generateTimestamps) { b.mode = 'sequence'; N.sbSet(b.ms[MSS].id, b.idx, 'mode', 1); }
    }
    remove(start, end) {
      const sb = this;
      const b = sb[SBS];
      checkUsable(sb, 'remove', true, 'Failed to execute');
      const m = b.ms[MSS];
      start = +start; end = +end;
      if (Number.isNaN(m.duration) || start < 0 || start > m.duration) throw new TypeError(`Failed to execute 'remove' on 'SourceBuffer': The start provided (${start}) is outside the range (0, ${m.duration}).`);
      if (!(end > start) || Number.isNaN(end)) throw new TypeError(`Failed to execute 'remove' on 'SourceBuffer': The end value provided (${end}) must be greater than the start value provided (${start}).`);
      if (m.readyState === 'ended') { m.readyState = 'open'; N.msSet(m.id, 'ended', 0); queueTask(() => b.ms.dispatchEvent(new Event('sourceopen'))); }
      b.updating = true;
      b.removing = true;
      queueTask(() => {
        if (b.removed) return;
        sb.dispatchEvent(new Event('updatestart'));
        N.sbRemove(m.id, b.idx, start, end);
        b.updating = false;
        b.removing = false;
        sb.dispatchEvent(new Event('update'));
        sb.dispatchEvent(new Event('updateend'));
      });
    }
  }
  tag(SourceBuffer, 'SourceBuffer');
  N.installHandlers(SourceBuffer.prototype, ['updatestart', 'update', 'updateend', 'error', 'abort']);

  function checkUsable(sb, what, needOpen, verb) {
    const b = sb[SBS];
    const pre = verb ? `${verb} '${what}' on 'SourceBuffer'` : `Failed to set the '${what}' property on 'SourceBuffer'`;
    if (b.removed) throw domError(`${pre}: This SourceBuffer has been removed from the parent media source.`, 'InvalidStateError');
    if (b.updating) throw domError(`${pre}: This SourceBuffer is still processing an 'appendBuffer' or 'remove' operation.`, 'InvalidStateError');
    if (needOpen && b.ms[MSS].readyState === 'closed') throw domError(`${pre}: The MediaSource's readyState is not 'open'.`, 'InvalidStateError');
  }

  function initSegmentReceived(sb, init) {
    const b = sb[SBS];
    const m = b.ms[MSS];
    debug(`init segment: ${init.tracks.map((t) => `${t.kind}:${t.codec}${t.kind === 'video' ? ` ${t.width}x${t.height}` : ` ${t.sampleRate}Hz/${t.channels}ch`}`).join(', ')}${init.duration ? ` duration ${init.duration}` : ''}`);
    if (Number.isNaN(m.duration)) {
      const d = init.duration && init.duration > 0 ? init.duration : Infinity;
      m.duration = d;
      N.msSet(m.id, 'duration', d);
    }
    if (!b.firstInit) {
      b.firstInit = true;
      if (!m.active[SBS].includes(sb)) {
        m.active[SBS].push(sb);
        queueTask(() => m.active.dispatchEvent(new Event('addsourcebuffer')));
      }
    }
    if (m.el) wake();
  }

  // URL.createObjectURL(mediaSource) → remember the object for the element.
  {
    const U = g.URL;
    const create = U.createObjectURL;
    const revoke = U.revokeObjectURL;
    U.createObjectURL = function createObjectURL(obj) {
      const url = create.call(this, obj);
      if (obj instanceof MediaSource) blobSources.set(url, obj);
      return url;
    };
    U.revokeObjectURL = function revokeObjectURL(url) { blobSources.delete(String(url)); return revoke.call(this, url); };
  }

  define('MediaSource', MediaSource);
  define('SourceBuffer', SourceBuffer);
  define('SourceBufferList', SourceBufferList);
  if (!g.WebKitMediaSource) { /* no prefixed alias */ }

  // ------------------------------------------------- navigator.mediaCapabilities
  const nav = g.navigator;
  if (nav) {
    const cap = (config, kind) => {
      if (!config || typeof config !== 'object') return Promise.reject(new TypeError(`Failed to execute '${kind}' on 'MediaCapabilities': The provided value is not of type 'MediaDecodingConfiguration'.`));
      let ok = true;
      const types = [];
      if (config.audio && config.audio.contentType) types.push(config.audio.contentType);
      if (config.video && config.video.contentType) types.push(config.video.contentType);
      if (!types.length) return Promise.reject(new TypeError(`Failed to execute '${kind}' on 'MediaCapabilities': The configuration requires an audio or video configuration.`));
      for (const t of types) {
        if (config.type === 'media-source' || kind === 'encodingInfo') ok = ok && MediaSource.isTypeSupported(t);
        else ok = ok && N.mediaCanPlayType(t) !== '';
      }
      if (kind === 'encodingInfo') ok = false;
      return Promise.resolve({ supported: ok, smooth: ok, powerEfficient: ok, keySystemAccess: null, configuration: config });
    };
    const MC = g.MediaCapabilities || class MediaCapabilities { constructor() { ILLEGAL(); } };
    method(MC, 'decodingInfo', function (c) { return cap(c, 'decodingInfo'); });
    method(MC, 'encodingInfo', function (c) { return cap(c, 'encodingInfo'); });
    tag(MC, 'MediaCapabilities');
    define('MediaCapabilities', MC);
    if (!('mediaCapabilities' in nav)) {
      const inst = Object.create(MC.prototype);
      Object.defineProperty(Object.getPrototypeOf(nav), 'mediaCapabilities', { get() { return inst; }, configurable: true, enumerable: true });
    }
  }

  // ------------------------------------------------------- native controls
  // The painted controls (layout's paint_media_controls) show this state.
  function syncUi(el, s) {
    const controls = el.hasAttribute('controls') || el.localName === 'audio';
    const paused = s.paused || s.ended;
    const t = s.time || 0;
    const d = s.ms ? s.ms[MSS].duration : s.duration;
    let bend = 0;
    for (const r of s.buffered || []) if (r[0] <= t + 0.5 && r[1] > bend) bend = r[1];
    const vol = s.volume, muted = isMuted(el, s);
    const key = controls ? `${paused}|${Math.floor(t)}|${d}|${muted}|${vol}|${Math.floor(bend)}` : `${paused}`;
    if (key === s.uiKey) return;
    s.uiKey = key;
    N.mediaSetUi(el, paused, t, d, muted, vol, bend);
  }
  // Clicks on the painted controls.
  g.addEventListener('click', (e) => {
    const el = e.target;
    if (!e.isTrusted || !(el instanceof ME) || !el.hasAttribute('controls')) return;
    const r = el.getBoundingClientRect();
    const x = e.clientX - r.left, y = e.clientY - r.top;
    const audio = el.localName === 'audio';
    const h = audio ? Math.min(r.height, 54) : Math.min(40, r.height);
    const toggle = () => { if (el.paused || el.ended) el.play().catch(() => {}); else el.pause(); };
    if (y < r.height - h) { if (!audio) toggle(); return; }
    const tlx = 124, tle = r.width - 44;
    if (x < 40) toggle();
    else if (x >= r.width - 36) el.muted = !el.muted;
    else if (x >= tlx - 6 && x <= tle + 6 && tle > tlx) { const d = el.duration; if (Number.isFinite(d)) el.currentTime = Math.max(0, Math.min(1, (x - tlx) / (tle - tlx))) * d; }
  }, true);

  // Native controls in the page (GUI clicks) call these.
  g.__rhMediaControl = function (el, action, value) {
    if (!(el instanceof ME)) return;
    const s = st(el);
    if (action === 'toggle') { if (s.paused || s.ended) { el.play().catch(() => {}); } else el.pause(); }
    else if (action === 'seek') { const d = el.duration; if (Number.isFinite(d)) el.currentTime = Math.max(0, Math.min(1, value)) * d; }
    else if (action === 'mute') el.muted = !el.muted;
    else if (action === 'volume') { el.volume = Math.max(0, Math.min(1, value)); if (el.muted && value > 0) el.muted = false; }
  };
  // UI state of an element's native controls.
  g.__rhMediaUi = function (el) {
    const s = st(el);
    return { paused: s.paused || s.ended, time: el.currentTime, duration: el.duration, muted: el.muted, volume: s.volume, buffered: s.buffered };
  };
})();
