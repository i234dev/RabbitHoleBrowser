// WHATWG Streams (https://streams.spec.whatwg.org/): ReadableStream (default
// and byte sources, readers, tee, pipeTo/pipeThrough, async iteration,
// from), WritableStream, TransformStream, queuing strategies; plus
// TextEncoderStream / TextDecoderStream and CompressionStream /
// DecompressionStream (deflate, deflate-raw, gzip).
(function () {
  'use strict';
  const g = globalThis;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('stream');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const queueTask = (f) => Promise.resolve().then(f);
  const isAbort = (signal) => signal && signal.aborted;
  // A rejected promise that is already marked as handled.
  const rejectedHandled = (e) => { let rej; const p = new Promise((_, r) => { rej = r; }); p.catch(() => {}); rej(e); return p; };

  // ------------------------------------------------------ queuing strategies
  class CountQueuingStrategy {
    constructor(init) { if (!init || init.highWaterMark === undefined) throw new TypeError("Failed to construct 'CountQueuingStrategy': required member highWaterMark is undefined."); hidden(this, S, +init.highWaterMark); }
    get highWaterMark() { return this[S]; }
    get size() { return function size() { return 1; }; }
  }
  class ByteLengthQueuingStrategy {
    constructor(init) { if (!init || init.highWaterMark === undefined) throw new TypeError("Failed to construct 'ByteLengthQueuingStrategy': required member highWaterMark is undefined."); hidden(this, S, +init.highWaterMark); }
    get highWaterMark() { return this[S]; }
    get size() { return function size(chunk) { return chunk.byteLength; }; }
  }
  tag(CountQueuingStrategy, 'CountQueuingStrategy');
  tag(ByteLengthQueuingStrategy, 'ByteLengthQueuingStrategy');

  // ------------------------------------------------------------ ReadableStream
  class ReadableStream {
    constructor(source = {}, strategy = {}) {
      source = source === null ? {} : source;
      hidden(this, S, { state: 'readable', reader: null, storedError: undefined, disturbed: false, controller: null });
      const type = source.type === undefined ? undefined : String(source.type);
      if (type === 'bytes') {
        const c = Object.create(ReadableByteStreamController.prototype);
        setupController(this, c, source, strategy.highWaterMark === undefined ? 0 : +strategy.highWaterMark, () => 1, true, +source.autoAllocateChunkSize || 0);
      } else if (type !== undefined) {
        throw new TypeError(`Failed to construct 'ReadableStream': The provided value '${type}' is not a valid enum value of type ReadableStreamType.`);
      } else {
        const c = Object.create(ReadableStreamDefaultController.prototype);
        const size = strategy.size || (() => 1);
        setupController(this, c, source, strategy.highWaterMark === undefined ? 1 : +strategy.highWaterMark, size, false, 0);
      }
    }
    static from(iterable) {
      const it = iterable[Symbol.asyncIterator] ? iterable[Symbol.asyncIterator]() : iterable[Symbol.iterator]();
      return new ReadableStream({
        async pull(c) { const r = await it.next(); if (r.done) c.close(); else c.enqueue(r.value); },
        async cancel(reason) { if (it.return) await it.return(reason); },
      }, { highWaterMark: 0 });
    }
    get locked() { return !!this[S].reader; }
    cancel(reason) {
      if (this[S].reader) return Promise.reject(new TypeError('Cannot cancel a locked stream'));
      return cancelStream(this, reason);
    }
    getReader(options = {}) {
      const mode = options && options.mode;
      if (mode === 'byob') {
        if (!(this[S].controller instanceof ReadableByteStreamController)) throw new TypeError("Failed to execute 'getReader' on 'ReadableStream': Cannot get a BYOB reader for a stream that does not have a byte source.");
        return new ReadableStreamBYOBReader(this);
      }
      if (mode !== undefined) throw new TypeError(`Failed to execute 'getReader' on 'ReadableStream': The provided value '${mode}' is not a valid enum value of type ReadableStreamReaderMode.`);
      return new ReadableStreamDefaultReader(this);
    }
    pipeThrough(transform, options = {}) {
      if (this.locked) throw new TypeError('Cannot pipe a locked stream');
      if (transform.writable.locked) throw new TypeError('Cannot pipe to a locked stream');
      const p = this.pipeTo(transform.writable, options);
      p.catch(() => {});
      return transform.readable;
    }
    pipeTo(dest, options = {}) {
      if (!(dest instanceof WritableStream)) return Promise.reject(new TypeError("Failed to execute 'pipeTo' on 'ReadableStream': parameter 1 is not of type 'WritableStream'."));
      if (this.locked) return Promise.reject(new TypeError('Cannot pipe a locked stream'));
      if (dest.locked) return Promise.reject(new TypeError('Cannot pipe to a locked stream'));
      const { preventClose, preventAbort, preventCancel, signal } = options || {};
      const reader = this.getReader();
      const writer = dest.getWriter();
      return new Promise((resolve, reject) => {
        let done = false;
        const finish = (err, isErr) => {
          if (done) return;
          done = true;
          if (signal) signal.removeEventListener('abort', onAbort);
          reader.releaseLock();
          writer.releaseLock();
          if (isErr) reject(err); else resolve();
        };
        const onAbort = () => {
          const reason = signal.reason;
          const acts = [];
          if (!preventAbort) acts.push(dest[S].state === 'writable' ? writer.abort(reason) : Promise.resolve());
          if (!preventCancel) acts.push(this[S].state === 'readable' ? reader.cancel(reason) : Promise.resolve());
          Promise.all(acts).then(() => finish(reason, true), (e) => finish(e, true));
        };
        if (signal) { if (signal.aborted) { onAbort(); return; } signal.addEventListener('abort', onAbort); }
        const loop = () => {
          if (done) return;
          writer.ready.then(() => reader.read()).then(({ value, done: end }) => {
            if (done) return;
            if (end) {
              if (!preventClose) writer.close().then(() => finish(), (e) => finish(e, true));
              else finish();
              return;
            }
            writer.write(value).catch(() => {});
            loop();
          }, (e) => {
            if (done) return;
            if (!preventAbort) writer.abort(e).then(() => finish(e, true), () => finish(e, true));
            else finish(e, true);
          });
        };
        writer.closed.catch((e) => { if (!done) { if (!preventCancel) reader.cancel(e).catch(() => {}); finish(e, true); } });
        loop();
      });
    }
    tee() {
      const reader = this.getReader();
      let canceled = [false, false], reasons = [], resolveCancel;
      const cancelPromise = new Promise((r) => { resolveCancel = r; });
      const branches = [];
      let pulling = false;
      const pull = () => {
        if (pulling) return Promise.resolve();
        pulling = true;
        return reader.read().then(({ value, done }) => {
          pulling = false;
          if (done) { for (let i = 0; i < 2; i++) if (!canceled[i]) branches[i][S].controller.close(); resolveCancel(); return; }
          for (let i = 0; i < 2; i++) if (!canceled[i]) {
            const v = i === 1 && value instanceof Uint8Array ? new Uint8Array(value) : value;
            branches[i][S].controller.enqueue(v);
          }
        }, (e) => { pulling = false; for (let i = 0; i < 2; i++) branches[i][S].controller.error(e); resolveCancel(); });
      };
      for (let i = 0; i < 2; i++) {
        branches.push(new ReadableStream({
          pull,
          cancel: (reason) => {
            canceled[i] = true; reasons[i] = reason;
            if (canceled[0] && canceled[1]) { const r = reader.cancel(reasons); resolveCancel(r); }
            return cancelPromise;
          },
        }));
      }
      reader.closed.catch((e) => { for (const b of branches) if (b[S].state === 'readable') b[S].controller.error(e); });
      return branches;
    }
    values(options = {}) { return makeAsyncIterator(this, !!(options && options.preventCancel)); }
    [Symbol.asyncIterator](options) { return this.values(options); }
  }
  tag(ReadableStream, 'ReadableStream');

  function makeAsyncIterator(stream, preventCancel) {
    const reader = stream.getReader();
    const it = Object.create(AsyncIteratorProto);
    hidden(it, S, { reader, preventCancel, done: false });
    return it;
  }
  const AsyncIteratorProto = Object.create(Object.getPrototypeOf(Object.getPrototypeOf((async function* () {})())), {
    next: { value: function () { const st = this[S]; if (st.done) return Promise.resolve({ value: undefined, done: true }); return st.reader.read().then((r) => { if (r.done) { st.done = true; st.reader.releaseLock(); } return r; }, (e) => { st.done = true; st.reader.releaseLock(); throw e; }); }, writable: true, configurable: true },
    return: { value: function (value) { const st = this[S]; if (st.done) return Promise.resolve({ value, done: true }); st.done = true; if (!st.preventCancel) { const p = st.reader.cancel(value); st.reader.releaseLock(); return p.then(() => ({ value, done: true })); } st.reader.releaseLock(); return Promise.resolve({ value, done: true }); }, writable: true, configurable: true },
  });

  function setupController(stream, c, source, hwm, size, bytes, autoAllocate) {
    if (Number.isNaN(hwm) || hwm < 0) throw new RangeError('highWaterMark must be a non-negative number');
    hidden(c, S, { stream, queue: [], queueSize: 0, hwm, size, started: false, closeRequested: false, pulling: false, pullAgain: false, source, bytes, autoAllocate, pendingByob: [] });
    stream[S].controller = c;
    let start;
    try { start = source.start ? source.start.call(source, c) : undefined; } catch (e) { throw e; }
    Promise.resolve(start).then(() => { c[S].started = true; callPull(c); }, (e) => errorStream(stream, e));
  }
  function desiredSize(c) {
    const st = c[S].stream[S].state;
    if (st === 'errored') return null;
    if (st === 'closed') return 0;
    return c[S].hwm - c[S].queueSize;
  }
  function shouldPull(c) {
    const s = c[S], stream = s.stream;
    if (!s.started || s.closeRequested || stream[S].state !== 'readable') return false;
    const r = stream[S].reader;
    if (r && r[S].requests.length > 0) return true;
    return desiredSize(c) > 0;
  }
  function callPull(c) {
    const s = c[S];
    if (!shouldPull(c)) return;
    if (s.pulling) { s.pullAgain = true; return; }
    s.pulling = true;
    let p;
    try { p = s.source.pull ? s.source.pull.call(s.source, c) : undefined; } catch (e) { errorStream(s.stream, e); return; }
    Promise.resolve(p).then(() => { s.pulling = false; if (s.pullAgain) { s.pullAgain = false; callPull(c); } }, (e) => errorStream(s.stream, e));
  }
  function enqueue(c, chunk) {
    const s = c[S], stream = s.stream;
    if (s.closeRequested || stream[S].state !== 'readable') throw new TypeError('Cannot enqueue a chunk into a closed or errored stream');
    const reader = stream[S].reader;
    if (reader && reader[S].requests.length > 0) {
      const req = reader[S].requests.shift();
      if (reader instanceof ReadableStreamBYOBReader) fillByob(req, chunk);
      else req.resolve({ value: chunk, done: false });
    } else {
      let sz = 1;
      try { sz = s.bytes ? chunk.byteLength : s.size(chunk); } catch (e) { errorStream(stream, e); throw e; }
      s.queue.push({ chunk, size: sz });
      s.queueSize += sz;
    }
    callPull(c);
  }
  function fillByob(req, chunk) {
    const view = req.view;
    const src = new Uint8Array(chunk.buffer || chunk, chunk.byteOffset || 0, chunk.byteLength);
    const n = Math.min(view.byteLength, src.byteLength);
    new Uint8Array(view.buffer, view.byteOffset, n).set(src.subarray(0, n));
    const Ctor = view.constructor;
    req.resolve({ value: new Ctor(view.buffer, view.byteOffset, n / (Ctor.BYTES_PER_ELEMENT || 1)), done: false });
    return src.byteLength - n;
  }
  function closeController(c) {
    const s = c[S];
    if (s.closeRequested || s.stream[S].state !== 'readable') throw new TypeError('Cannot close a closed or errored stream');
    s.closeRequested = true;
    if (s.queue.length === 0) closeStream(s.stream);
  }
  function closeStream(stream) {
    stream[S].state = 'closed';
    const r = stream[S].reader;
    if (!r) return;
    for (const req of r[S].requests) req.resolve({ value: undefined, done: true });
    r[S].requests = [];
    r[S].resolveClosed();
  }
  function errorStream(stream, e) {
    if (stream[S].state !== 'readable') return;
    stream[S].state = 'errored';
    stream[S].storedError = e;
    const c = stream[S].controller;
    if (c) { c[S].queue = []; c[S].queueSize = 0; }
    const r = stream[S].reader;
    if (!r) return;
    for (const req of r[S].requests) req.reject(e);
    r[S].requests = [];
    r[S].rejectClosed(e);
  }
  function cancelStream(stream, reason) {
    stream[S].disturbed = true;
    if (stream[S].state === 'closed') return Promise.resolve();
    if (stream[S].state === 'errored') return Promise.reject(stream[S].storedError);
    closeStream(stream);
    const c = stream[S].controller;
    c[S].queue = []; c[S].queueSize = 0;
    let p;
    try { p = c[S].source.cancel ? c[S].source.cancel.call(c[S].source, reason) : undefined; } catch (e) { return Promise.reject(e); }
    return Promise.resolve(p).then(() => undefined);
  }

  class ReadableStreamDefaultController {
    constructor() { ILLEGAL(); }
    get desiredSize() { return desiredSize(this); }
    close() { closeController(this); }
    enqueue(chunk) { enqueue(this, chunk); }
    error(e) { errorStream(this[S].stream, e); }
  }
  tag(ReadableStreamDefaultController, 'ReadableStreamDefaultController');
  class ReadableByteStreamController {
    constructor() { ILLEGAL(); }
    get byobRequest() {
      const r = this[S].stream[S].reader;
      if (!(r instanceof ReadableStreamBYOBReader) || !r[S].requests.length) return null;
      const req = r[S].requests[0];
      const ctrl = this;
      const b = Object.create(ReadableStreamBYOBRequest.prototype);
      hidden(b, S, { req, ctrl });
      return b;
    }
    get desiredSize() { return desiredSize(this); }
    close() { closeController(this); }
    enqueue(chunk) {
      if (!ArrayBuffer.isView(chunk)) throw new TypeError("Failed to execute 'enqueue' on 'ReadableByteStreamController': parameter 1 is not of type 'ArrayBufferView'.");
      enqueue(this, new Uint8Array(chunk.buffer.slice(chunk.byteOffset, chunk.byteOffset + chunk.byteLength)));
    }
    error(e) { errorStream(this[S].stream, e); }
  }
  tag(ReadableByteStreamController, 'ReadableByteStreamController');
  class ReadableStreamBYOBRequest {
    constructor() { ILLEGAL(); }
    get view() { return this[S].req.view; }
    respond(bytesWritten) {
      const { req, ctrl } = this[S];
      const r = ctrl[S].stream[S].reader;
      r[S].requests.shift();
      const v = req.view;
      const Ctor = v.constructor;
      req.resolve({ value: new Ctor(v.buffer, v.byteOffset, bytesWritten / (Ctor.BYTES_PER_ELEMENT || 1)), done: false });
    }
    respondWithNewView(view) {
      const { req, ctrl } = this[S];
      ctrl[S].stream[S].reader[S].requests.shift();
      req.resolve({ value: view, done: false });
    }
  }
  tag(ReadableStreamBYOBRequest, 'ReadableStreamBYOBRequest');

  class ReaderBase {
    get closed() { return this[S].closed; }
    cancel(reason) { if (!this[S].stream) return Promise.reject(new TypeError('This readable stream reader has been released and cannot be used to cancel its previous owner stream')); return cancelStream(this[S].stream, reason); }
    releaseLock() {
      const st = this[S];
      if (!st.stream) return;
      const e = new TypeError('Reader was released');
      for (const req of st.requests) req.reject(e);
      st.requests = [];
      if (st.stream[S].state === 'readable') st.rejectClosed(e);
      else st.closed = rejectedHandled(e);
      st.stream[S].reader = null;
      st.stream = null;
    }
  }
  function initReader(r, stream) {
    if (stream[S].reader) throw new TypeError('ReadableStreamDefaultReader constructor can only accept readable streams that are not yet locked to a reader');
    const st = { stream, requests: [] };
    st.closed = new Promise((res, rej) => { st.resolveClosed = res; st.rejectClosed = rej; });
    st.closed.catch(() => {});
    hidden(r, S, st);
    stream[S].reader = r;
    if (stream[S].state === 'closed') st.resolveClosed();
    if (stream[S].state === 'errored') st.rejectClosed(stream[S].storedError);
  }
  function read(r, view) {
    const st = r[S];
    if (!st.stream) return Promise.reject(new TypeError('This readable stream reader has been released and cannot be used to read from its previous owner stream'));
    const stream = st.stream;
    stream[S].disturbed = true;
    if (stream[S].state === 'closed') return Promise.resolve({ value: view ? new view.constructor(view.buffer, view.byteOffset, 0) : undefined, done: true });
    if (stream[S].state === 'errored') return Promise.reject(stream[S].storedError);
    const c = stream[S].controller;
    if (c[S].queue.length > 0) {
      const item = c[S].queue[0];
      let value;
      if (view) {
        const left = new Promise((resolve) => { const rest = fillByob({ view, resolve }, item.chunk); if (rest > 0) { item.chunk = item.chunk.subarray(item.chunk.byteLength - rest); item.size = rest; c[S].queueSize -= item.chunk.byteLength - rest; } else { c[S].queue.shift(); c[S].queueSize -= item.size; } });
        if (c[S].closeRequested && c[S].queue.length === 0) closeStream(stream); else callPull(c);
        return left;
      }
      c[S].queue.shift();
      c[S].queueSize -= item.size;
      value = item.chunk;
      if (c[S].closeRequested && c[S].queue.length === 0) closeStream(stream); else callPull(c);
      return Promise.resolve({ value, done: false });
    }
    return new Promise((resolve, reject) => { st.requests.push({ resolve, reject, view }); callPull(c); });
  }
  class ReadableStreamDefaultReader extends ReaderBase {
    constructor(stream) {
      super();
      if (!(stream instanceof ReadableStream)) throw new TypeError("Failed to construct 'ReadableStreamDefaultReader': parameter 1 is not of type 'ReadableStream'.");
      initReader(this, stream);
    }
    read() { return read(this, null); }
  }
  tag(ReadableStreamDefaultReader, 'ReadableStreamDefaultReader');
  class ReadableStreamBYOBReader extends ReaderBase {
    constructor(stream) {
      super();
      if (!(stream instanceof ReadableStream) || !(stream[S].controller instanceof ReadableByteStreamController)) throw new TypeError("Failed to construct 'ReadableStreamBYOBReader': Cannot construct a ReadableStreamBYOBReader for a stream not constructed with a byte source.");
      initReader(this, stream);
    }
    read(view, options) {
      if (!ArrayBuffer.isView(view) || view.byteLength === 0) return Promise.reject(new TypeError("Failed to execute 'read' on 'ReadableStreamBYOBReader': This readable stream reader cannot read into an empty view."));
      void options;
      return read(this, view);
    }
  }
  tag(ReadableStreamBYOBReader, 'ReadableStreamBYOBReader');
  // The base class is an implementation detail.
  for (const C of [ReadableStreamDefaultReader, ReadableStreamBYOBReader]) {
    for (const k of ['closed', 'cancel', 'releaseLock']) Object.defineProperty(C.prototype, k, Object.getOwnPropertyDescriptor(ReaderBase.prototype, k));
    Object.setPrototypeOf(C.prototype, Object.prototype);
  }

  // ------------------------------------------------------------ WritableStream
  class WritableStream {
    constructor(sink = {}, strategy = {}) {
      sink = sink === null ? {} : sink;
      if (sink.type !== undefined) throw new RangeError("Failed to construct 'WritableStream': Invalid type is specified");
      hidden(this, S, { state: 'writable', writer: null, storedError: undefined, queue: [], inFlight: false, closeRequested: false, sink, hwm: strategy.highWaterMark === undefined ? 1 : +strategy.highWaterMark, size: strategy.size || (() => 1), queueSize: 0, started: false });
      const c = Object.create(WritableStreamDefaultController.prototype);
      const ac = new AbortController();
      hidden(c, S, { stream: this, abort: ac });
      this[S].controller = c;
      let start;
      start = sink.start ? sink.start.call(sink, c) : undefined;
      Promise.resolve(start).then(() => { this[S].started = true; advance(this); }, (e) => wsError(this, e));
    }
    get locked() { return !!this[S].writer; }
    abort(reason) { if (this.locked) return Promise.reject(new TypeError('Cannot abort a locked stream')); return wsAbort(this, reason); }
    close() { if (this.locked) return Promise.reject(new TypeError('Cannot close a locked stream')); return wsClose(this); }
    getWriter() { return new WritableStreamDefaultWriter(this); }
  }
  tag(WritableStream, 'WritableStream');
  function wsDesired(ws) { const s = ws[S]; return s.state === 'errored' || s.state === 'erroring' ? null : s.state === 'closed' ? 0 : s.hwm - s.queueSize; }
  function updateReady(ws) {
    const w = ws[S].writer;
    if (!w) return;
    const d = wsDesired(ws);
    if (d !== null && d <= 0 && !w[S].backpressure) { w[S].backpressure = true; w[S].ready = new Promise((r) => { w[S].resolveReady = r; }); }
    else if ((d === null || d > 0) && w[S].backpressure) { w[S].backpressure = false; w[S].resolveReady(); }
  }
  function advance(ws) {
    const s = ws[S];
    if (!s.started || s.inFlight || s.state !== 'writable' && s.state !== 'erroring') return;
    if (s.state === 'erroring') { finishErroring(ws); return; }
    if (s.queue.length === 0) {
      if (s.closeRequested && !s.closing) {
        s.closing = true;
        s.inFlight = true;
        Promise.resolve().then(() => (s.sink.close ? s.sink.close.call(s.sink) : undefined)).then(() => {
          s.inFlight = false; s.state = 'closed';
          s.closeResolve();
          if (s.writer) s.writer[S].resolveClosed();
        }, (e) => { s.inFlight = false; wsError(ws, e); s.closeReject(e); });
      }
      return;
    }
    const { chunk, resolve, reject, size } = s.queue.shift();
    s.inFlight = true;
    Promise.resolve().then(() => (s.sink.write ? s.sink.write.call(s.sink, chunk, s.controller) : undefined)).then(() => {
      s.inFlight = false;
      s.queueSize -= size;
      resolve();
      updateReady(ws);
      advance(ws);
    }, (e) => { s.inFlight = false; reject(e); wsError(ws, e); });
  }
  function wsError(ws, e) {
    const s = ws[S];
    if (s.state === 'errored' || s.state === 'closed') return;
    s.state = 'errored';
    s.storedError = e;
    for (const q of s.queue) q.reject(e);
    s.queue = [];
    if (s.closeReject && !s.closing) s.closeReject(e);
    const w = s.writer;
    if (w) { w[S].rejectClosed(e); if (!w[S].backpressure) { w[S].ready = rejectedHandled(e); } else w[S].resolveReady(); }
  }
  function finishErroring(ws) { wsError(ws, ws[S].pendingAbortReason); }
  function wsAbort(ws, reason) {
    const s = ws[S];
    if (s.state === 'closed' || s.state === 'errored') return Promise.resolve();
    s.controller[S].abort.abort(reason);
    s.pendingAbortReason = reason;
    const p = Promise.resolve().then(() => (s.sink.abort ? s.sink.abort.call(s.sink, reason) : undefined));
    wsError(ws, reason);
    return p.then(() => undefined);
  }
  function wsClose(ws) {
    const s = ws[S];
    if (s.state === 'closed' || s.state === 'errored' || s.closeRequested) return Promise.reject(new TypeError('Cannot close a stream that is already closed or is closing'));
    s.closeRequested = true;
    const p = new Promise((res, rej) => { s.closeResolve = res; s.closeReject = rej; });
    advance(ws);
    return p;
  }
  class WritableStreamDefaultController {
    constructor() { ILLEGAL(); }
    get signal() { return this[S].abort.signal; }
    error(e) { wsError(this[S].stream, e); }
  }
  tag(WritableStreamDefaultController, 'WritableStreamDefaultController');
  class WritableStreamDefaultWriter {
    constructor(stream) {
      if (!(stream instanceof WritableStream)) throw new TypeError("Failed to construct 'WritableStreamDefaultWriter': parameter 1 is not of type 'WritableStream'.");
      if (stream[S].writer) throw new TypeError('This stream has already been locked for exclusive writing by another writer');
      const st = { stream, backpressure: false, ready: Promise.resolve() };
      st.closed = new Promise((res, rej) => { st.resolveClosed = res; st.rejectClosed = rej; });
      st.closed.catch(() => {});
      hidden(this, S, st);
      stream[S].writer = this;
      if (stream[S].state === 'closed') st.resolveClosed();
      if (stream[S].state === 'errored') { st.rejectClosed(stream[S].storedError); st.ready = rejectedHandled(stream[S].storedError); }
      updateReady(stream);
    }
    get closed() { return this[S].closed; }
    get ready() { return this[S].ready; }
    get desiredSize() { if (!this[S].stream) throw new TypeError('This writable stream writer has been released and cannot be used to get the desired size'); return wsDesired(this[S].stream); }
    abort(reason) { if (!this[S].stream) return Promise.reject(new TypeError('released writer')); return wsAbort(this[S].stream, reason); }
    close() { if (!this[S].stream) return Promise.reject(new TypeError('released writer')); return wsClose(this[S].stream); }
    releaseLock() {
      const st = this[S];
      if (!st.stream) return;
      const e = new TypeError('Writer was released');
      st.rejectClosed(e);
      st.closed = rejectedHandled(e);
      st.stream[S].writer = null;
      st.stream = null;
    }
    write(chunk) {
      const ws = this[S].stream;
      if (!ws) return Promise.reject(new TypeError('This writable stream writer has been released and cannot be used to write to its previous owner stream'));
      const s = ws[S];
      if (s.state === 'errored') return Promise.reject(s.storedError);
      if (s.closeRequested || s.state === 'closed') return Promise.reject(new TypeError('Cannot write to a closing or closed stream'));
      let size = 1;
      try { size = s.size(chunk); } catch (e) { wsError(ws, e); return Promise.reject(e); }
      const p = new Promise((resolve, reject) => s.queue.push({ chunk, resolve, reject, size }));
      s.queueSize += size;
      updateReady(ws);
      advance(ws);
      return p;
    }
  }
  tag(WritableStreamDefaultWriter, 'WritableStreamDefaultWriter');

  // ----------------------------------------------------------- TransformStream
  class TransformStream {
    constructor(transformer = {}, writableStrategy = {}, readableStrategy = {}) {
      transformer = transformer === null ? {} : transformer;
      if (transformer.readableType !== undefined) throw new RangeError("Failed to construct 'TransformStream': Invalid readableType is specified");
      if (transformer.writableType !== undefined) throw new RangeError("Failed to construct 'TransformStream': Invalid writableType is specified");
      const c = Object.create(TransformStreamDefaultController.prototype);
      let readableController;
      let startResolve;
      const started = new Promise((r) => { startResolve = r; });
      const readable = new ReadableStream({
        start(rc) { readableController = rc; },
        pull() { return undefined; },
        cancel(reason) { if (transformer.cancel) return transformer.cancel.call(transformer, reason); return undefined; },
      }, readableStrategy.highWaterMark === undefined ? { highWaterMark: 0 } : readableStrategy);
      hidden(c, S, { readable: () => readableController, ts: this });
      const writable = new WritableStream({
        start() { return started; },
        write(chunk) {
          if (transformer.transform) return transformer.transform.call(transformer, chunk, c);
          c.enqueue(chunk);
          return undefined;
        },
        close() {
          return Promise.resolve(transformer.flush ? transformer.flush.call(transformer, c) : undefined).then(() => { try { readableController.close(); } catch (e) {} });
        },
        abort(reason) { readableController.error(reason); if (transformer.cancel) return transformer.cancel.call(transformer, reason); return undefined; },
      }, writableStrategy);
      hidden(this, S, { readable, writable, controller: c });
      const start = transformer.start ? transformer.start.call(transformer, c) : undefined;
      Promise.resolve(start).then(() => startResolve(), (e) => { readableController.error(e); startResolve(); });
    }
    get readable() { return this[S].readable; }
    get writable() { return this[S].writable; }
  }
  tag(TransformStream, 'TransformStream');
  class TransformStreamDefaultController {
    constructor() { ILLEGAL(); }
    get desiredSize() { return this[S].readable().desiredSize; }
    enqueue(chunk) { this[S].readable().enqueue(chunk); }
    error(e) { this[S].readable().error(e); wsError(this[S].ts[S].writable, e); }
    terminate() { try { this[S].readable().close(); } catch (e) {} wsError(this[S].ts[S].writable, new TypeError('The transform stream has been terminated')); }
  }
  tag(TransformStreamDefaultController, 'TransformStreamDefaultController');

  // ------------------------------------------------------- encoding streams
  class TextEncoderStream {
    constructor() {
      const enc = new g.TextEncoder();
      let pending = '';
      const ts = new TransformStream({
        transform(chunk, c) {
          let s = pending + String(chunk);
          pending = '';
          const last = s.charCodeAt(s.length - 1);
          if (last >= 0xd800 && last <= 0xdbff) { pending = s.slice(-1); s = s.slice(0, -1); }
          if (s) c.enqueue(enc.encode(s));
        },
        flush(c) { if (pending) c.enqueue(new Uint8Array([0xef, 0xbf, 0xbd])); },
      });
      hidden(this, S, ts);
    }
    get encoding() { return 'utf-8'; }
    get readable() { return this[S].readable; }
    get writable() { return this[S].writable; }
  }
  tag(TextEncoderStream, 'TextEncoderStream');
  class TextDecoderStream {
    constructor(label = 'utf-8', options = {}) {
      const dec = new g.TextDecoder(label, options);
      const ts = new TransformStream({
        transform(chunk, c) { const s = dec.decode(chunk, { stream: true }); if (s) c.enqueue(s); },
        flush(c) { const s = dec.decode(); if (s) c.enqueue(s); },
      });
      hidden(this, S, { ts, dec });
    }
    get encoding() { return this[S].dec.encoding; }
    get fatal() { return this[S].dec.fatal; }
    get ignoreBOM() { return this[S].dec.ignoreBOM; }
    get readable() { return this[S].ts.readable; }
    get writable() { return this[S].ts.writable; }
  }
  tag(TextDecoderStream, 'TextDecoderStream');

  // ------------------------------------------------- compression streams
  // Compression and decompression run natively (zlib / DEFLATE / gzip).
  const N = g.__rhNative;
  const FORMATS = ['deflate', 'deflate-raw', 'gzip'];
  const toBytes = (chunk) => {
    if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk);
    if (ArrayBuffer.isView(chunk)) return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
    throw new TypeError('The provided value is not of type \'(ArrayBuffer or ArrayBufferView)\'');
  };
  const concat = (parts) => { const n = parts.reduce((s, p) => s + p.length, 0); const o = new Uint8Array(n); let i = 0; for (const p of parts) { o.set(p, i); i += p.length; } return o; };
  class CompressionStream {
    constructor(format) {
      if (!FORMATS.includes(format)) throw new TypeError(`Failed to construct 'CompressionStream': Unsupported compression format: '${format}'`);
      const parts = [];
      hidden(this, S, new TransformStream({
        transform(chunk) { parts.push(toBytes(chunk).slice()); },
        flush(c) {
          c.enqueue(new Uint8Array(N.deflate(concat(parts), format)));
        },
      }));
    }
    get readable() { return this[S].readable; }
    get writable() { return this[S].writable; }
  }
  tag(CompressionStream, 'CompressionStream');
  class DecompressionStream {
    constructor(format) {
      if (!FORMATS.includes(format)) throw new TypeError(`Failed to construct 'DecompressionStream': Unsupported compression format: '${format}'`);
      const parts = [];
      hidden(this, S, new TransformStream({
        transform(chunk) { parts.push(toBytes(chunk).slice()); },
        flush(c) {
          c.enqueue(new Uint8Array(N.inflate(concat(parts), format)));
        },
      }));
    }
    get readable() { return this[S].readable; }
    get writable() { return this[S].writable; }
  }
  tag(DecompressionStream, 'DecompressionStream');

  // ---------------------------------------------------------------- exports
  define('ReadableStream', ReadableStream);
  define('ReadableStreamDefaultReader', ReadableStreamDefaultReader);
  define('ReadableStreamBYOBReader', ReadableStreamBYOBReader);
  define('ReadableStreamDefaultController', ReadableStreamDefaultController);
  define('ReadableByteStreamController', ReadableByteStreamController);
  define('ReadableStreamBYOBRequest', ReadableStreamBYOBRequest);
  define('WritableStream', WritableStream);
  define('WritableStreamDefaultWriter', WritableStreamDefaultWriter);
  define('WritableStreamDefaultController', WritableStreamDefaultController);
  define('TransformStream', TransformStream);
  define('TransformStreamDefaultController', TransformStreamDefaultController);
  define('ByteLengthQueuingStrategy', ByteLengthQueuingStrategy);
  define('CountQueuingStrategy', CountQueuingStrategy);
  define('TextEncoderStream', TextEncoderStream);
  define('TextDecoderStream', TextDecoderStream);
  define('CompressionStream', CompressionStream);
  define('DecompressionStream', DecompressionStream);
  hidden(g, '__rhStreams', { isDisturbed: (s) => !!(s && s[S] && s[S].disturbed) });
  void isAbort; void queueTask;
})();
