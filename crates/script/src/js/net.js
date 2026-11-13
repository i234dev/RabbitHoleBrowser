// Fetch / XHR / File API / Encoding / WebSocket / EventSource pieces in
// JavaScript on top of the native fetch, XMLHttpRequest, Blob and
// TextEncoder/Decoder bindings.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('net');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const domError = (msg, name) => new g.DOMException(msg, name);
  const defaults = (C, map) => {
    if (!C) return;
    for (const [k, v] of Object.entries(map)) {
      if (Object.prototype.hasOwnProperty.call(C.prototype, k)) continue;
      Object.defineProperty(C.prototype, k, { get() { return typeof v === 'function' ? v.call(this) : v; }, configurable: true, enumerable: true });
    }
  };
  const method = (C, name, fn, force) => { if (C && (force || !Object.prototype.hasOwnProperty.call(C.prototype, name))) hidden(C.prototype, name, fn); };

  // ---------------------------------------------------------------- Blob
  const Blob = g.Blob;
  defaults(Blob, { size: 0, type: '' });
  method(Blob, 'bytes', function () { return this.arrayBuffer().then((b) => new Uint8Array(b)); });
  method(Blob, 'stream', function () {
    const blob = this;
    return new g.ReadableStream({ type: 'bytes', start(c) { return blob.arrayBuffer().then((b) => { if (b.byteLength) c.enqueue(new Uint8Array(b)); c.close(); }); } });
  }, true);
  method(Blob, 'textStream', function () { return this.stream().pipeThrough(new g.TextDecoderStream()); });

  // File: a Blob with a name and modification time.
  class File extends Blob {
    constructor(bits, name, options = {}) {
      if (arguments.length < 2) throw new TypeError(`Failed to construct 'File': 2 arguments required, but only ${arguments.length} present.`);
      super(bits, options);
      Object.setPrototypeOf(this, new.target.prototype);
      hidden(this, S, { name: String(name).replace(/\//g, ':'), lastModified: options.lastModified !== undefined ? Number(options.lastModified) : Date.now(), relPath: '' });
    }
    get name() { return this[S] ? this[S].name : ''; }
    get lastModified() { return this[S] ? this[S].lastModified : 0; }
    get lastModifiedDate() { return new Date(this.lastModified); }
    get webkitRelativePath() { return this[S] ? this[S].relPath : ''; }
  }
  tag(File, 'File');
  define('File', File);
  N.registerProto('File', File.prototype);
  class FileList {
    constructor() { ILLEGAL(); }
    get length() { return this[S].length; }
    item(i) { return this[S][i] ?? null; }
    [Symbol.iterator]() { return this[S][Symbol.iterator](); }
  }
  tag(FileList, 'FileList');
  define('FileList', FileList);
  hidden(g, '__rhFileList', (files) => { const l = Object.create(FileList.prototype); hidden(l, S, Array.from(files)); l[S].forEach((f, i) => Object.defineProperty(l, i, { value: f, enumerable: true })); return l; });
  if (g.HTMLInputElement) {
    const d = Object.getOwnPropertyDescriptor(g.HTMLInputElement.prototype, 'files');
    const lists = new WeakMap();
    Object.defineProperty(g.HTMLInputElement.prototype, 'files', {
      get() { if (this.type !== 'file') return null; let l = lists.get(this); if (!l) { l = g.__rhFileList([]); lists.set(this, l); } return l; },
      set(v) { if (this.type === 'file' && v instanceof FileList) lists.set(this, v); },
      configurable: true, enumerable: true,
    });
    void d;
  }

  // ----------------------------------------------------------- FileReader
  const FR_STATES = { EMPTY: 0, LOADING: 1, DONE: 2 };
  class FileReader extends g.EventTarget {
    constructor() { super(); hidden(this, S, { state: 0, result: null, error: null, gen: 0 }); }
    get readyState() { return this[S].state; }
    get result() { return this[S].result; }
    get error() { return this[S].error; }
    readAsArrayBuffer(blob) { read(this, blob, 'buffer'); }
    readAsBinaryString(blob) { read(this, blob, 'binary'); }
    readAsText(blob, encoding) { read(this, blob, 'text', encoding); }
    readAsDataURL(blob) { read(this, blob, 'dataurl'); }
    abort() {
      const st = this[S];
      if (st.state === 0 || st.state === 2) { st.result = null; return; }
      st.gen++;
      st.state = 2; st.result = null;
      fire(this, 'abort'); fire(this, 'loadend');
    }
  }
  for (const [k, v] of Object.entries(FR_STATES)) { Object.defineProperty(FileReader, k, { value: v, enumerable: true }); Object.defineProperty(FileReader.prototype, k, { value: v, enumerable: true }); }
  tag(FileReader, 'FileReader');
  N.installHandlers(FileReader.prototype, ['loadstart', 'progress', 'load', 'abort', 'error', 'loadend']);
  const fire = (target, type, init) => target.dispatchEvent(new g.ProgressEvent(type, init || {}));
  function read(fr, blob, kind, encoding) {
    if (!(blob instanceof Blob)) throw new TypeError(`Failed to execute 'readAs...' on 'FileReader': parameter 1 is not of type 'Blob'.`);
    const st = fr[S];
    if (st.state === 1) throw domError("Failed to execute 'readAs...' on 'FileReader': The object is already busy reading Blobs.", 'InvalidStateError');
    st.state = 1; st.result = null; st.error = null;
    const gen = ++st.gen;
    setTimeout(() => {
      if (gen !== st.gen) return;
      fire(fr, 'loadstart');
      blob.arrayBuffer().then((buf) => {
        if (gen !== st.gen) return;
        const bytes = new Uint8Array(buf);
        let result;
        switch (kind) {
          case 'buffer': result = buf; break;
          case 'binary': { let s = ''; for (let i = 0; i < bytes.length; i += 8192) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 8192)); result = s; break; }
          case 'text': {
            // File API "package data" for text: the encoding argument, else
            // the type's charset, else UTF-8; a byte order mark wins.
            const known = (l) => { try { return new g.TextDecoder(l).encoding; } catch (e) { return null; } };
            let enc = encoding !== undefined ? known(String(encoding)) : null;
            if (!enc) { const m = /;\s*charset=["']?([^;"'\s]+)/i.exec(blob.type || ''); if (m) enc = known(m[1]); }
            if (!enc) enc = 'utf-8';
            if (bytes[0] === 0xEF && bytes[1] === 0xBB && bytes[2] === 0xBF) enc = 'utf-8';
            else if (bytes[0] === 0xFE && bytes[1] === 0xFF) enc = 'utf-16be';
            else if (bytes[0] === 0xFF && bytes[1] === 0xFE) enc = 'utf-16le';
            result = new g.TextDecoder(enc).decode(bytes);
            break;
          }
          default: {
            let s = '';
            for (let i = 0; i < bytes.length; i += 8192) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 8192));
            result = `data:${blob.type || 'application/octet-stream'};base64,${g.btoa(s)}`;
          }
        }
        fire(fr, 'progress', { lengthComputable: true, loaded: bytes.length, total: bytes.length });
        st.state = 2; st.result = result;
        fire(fr, 'load', { lengthComputable: true, loaded: bytes.length, total: bytes.length });
        if (st.state === 2) fire(fr, 'loadend', { lengthComputable: true, loaded: bytes.length, total: bytes.length });
      }, (e) => {
        if (gen !== st.gen) return;
        st.state = 2; st.error = domError(String(e && e.message || e), 'NotReadableError');
        fire(fr, 'error'); fire(fr, 'loadend');
      });
    }, 0);
  }
  define('FileReader', FileReader);
  class FileReaderSync {
    constructor() { ILLEGAL(); }
  }
  void FileReaderSync;

  // --------------------------------------------------------- TextEncoder
  const TE = g.TextEncoder, TD = g.TextDecoder;
  defaults(TE, { encoding: 'utf-8' });
  method(TE, 'encodeInto', function (src, dest) {
    if (!(dest instanceof Uint8Array)) throw new TypeError("Failed to execute 'encodeInto' on 'TextEncoder': parameter 2 is not of type 'Uint8Array'.");
    src = String(src);
    let read = 0, written = 0;
    for (const ch of src) {
      const bytes = this.encode(ch);
      if (written + bytes.length > dest.length) break;
      dest.set(bytes, written);
      written += bytes.length;
      read += ch.length;
    }
    return { read, written };
  });
  if (TD) {
    const ctor = TD;
    const labels = new WeakMap();
    const Wrapped = function TextDecoder(label = 'utf-8', options = {}) {
      if (!new.target) throw new TypeError("Failed to construct 'TextDecoder': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
      const norm = normalizeLabel(label);
      if (!norm) throw new RangeError(`Failed to construct 'TextDecoder': The encoding label provided ('${label}') is invalid.`);
      const d = new ctor(norm, options);
      Object.setPrototypeOf(d, new.target.prototype);
      labels.set(d, { encoding: norm, fatal: !!(options && options.fatal), ignoreBOM: !!(options && options.ignoreBOM) });
      return d;
    };
    Wrapped.prototype = ctor.prototype;
    Object.defineProperty(ctor.prototype, 'constructor', { value: Wrapped, writable: true, configurable: true });
    defaults(ctor, { encoding: function () { return (labels.get(this) || { encoding: 'utf-8' }).encoding; }, fatal: function () { return !!(labels.get(this) || {}).fatal; }, ignoreBOM: function () { return !!(labels.get(this) || {}).ignoreBOM; } });
    define('TextDecoder', Wrapped);
  }
  function normalizeLabel(label) {
    const l = String(label).trim().toLowerCase();
    const MAP = {
      'utf-8': ['unicode-1-1-utf-8', 'unicode11utf8', 'unicode20utf8', 'utf-8', 'utf8', 'x-unicode20utf8'],
      'utf-16le': ['csunicode', 'iso-10646-ucs-2', 'ucs-2', 'unicode', 'unicodefeff', 'utf-16', 'utf-16le'],
      'utf-16be': ['unicodefffe', 'utf-16be'],
      'windows-1252': ['ansi_x3.4-1968', 'ascii', 'cp1252', 'cp819', 'csisolatin1', 'ibm819', 'iso-8859-1', 'iso-ir-100', 'iso8859-1', 'iso88591', 'iso_8859-1', 'iso_8859-1:1987', 'l1', 'latin1', 'us-ascii', 'windows-1252', 'x-cp1252'],
      'iso-8859-2': ['csisolatin2', 'iso-8859-2', 'iso-ir-101', 'iso8859-2', 'iso88592', 'iso_8859-2', 'l2', 'latin2'],
      'iso-8859-15': ['csisolatin9', 'iso-8859-15', 'iso8859-15', 'iso885915', 'iso_8859-15', 'l9'],
      'windows-1251': ['cp1251', 'windows-1251', 'x-cp1251'],
      'koi8-r': ['cskoi8r', 'koi', 'koi8', 'koi8-r', 'koi8_r'],
      'shift_jis': ['csshiftjis', 'ms932', 'ms_kanji', 'shift-jis', 'shift_jis', 'sjis', 'windows-31j', 'x-sjis'],
      'euc-jp': ['cseucpkdfmtjapanese', 'euc-jp', 'x-euc-jp'],
      'gbk': ['chinese', 'csgb2312', 'csiso58gb231280', 'gb2312', 'gb_2312', 'gb_2312-80', 'gbk', 'iso-ir-58', 'x-gbk'],
      'gb18030': ['gb18030'],
      'big5': ['big5', 'big5-hkscs', 'cn-big5', 'csbig5', 'x-x-big5'],
      'euc-kr': ['cseuckr', 'csksc56011987', 'euc-kr', 'iso-ir-149', 'korean', 'ks_c_5601-1987', 'ks_c_5601-1989', 'ksc5601', 'ksc_5601', 'windows-949'],
    };
    for (const [name, list] of Object.entries(MAP)) if (list.includes(l)) return name;
    return /^(windows-12\d\d|iso-8859-\d+|ibm866|macintosh|x-mac-cyrillic|koi8-u|iso-2022-jp|x-user-defined)$/.test(l) ? l : null;
  }

  // ------------------------------------------------------------------ URL
  defaults(g.URL, { username: function () { const m = /^[a-z][a-z0-9+.-]*:\/\/([^@/?#]*)@/i.exec(this.href); return m ? decodeURIComponent(m[1].split(':')[0]) && m[1].split(':')[0] : ''; }, password: function () { const m = /^[a-z][a-z0-9+.-]*:\/\/([^@/?#]*)@/i.exec(this.href); return m && m[1].includes(':') ? m[1].slice(m[1].indexOf(':') + 1) : ''; } });

  // ------------------------------------------------------- Request/Response
  const used = new WeakSet();
  const bodies = new WeakMap();
  // `new Response(readableStream)`: the stream is the body; reading the
  // response drains it (chunks: BufferSource or string).
  const streamBodies = new WeakMap();
  const isStream = (b) => !!(b && typeof b === 'object' && g.ReadableStream && b instanceof g.ReadableStream);
  async function drain(stream) {
    const reader = stream.getReader();
    const parts = [];
    let n = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      let u8;
      if (typeof value === 'string') u8 = new TextEncoder().encode(value);
      else if (value instanceof ArrayBuffer) u8 = new Uint8Array(value);
      else if (ArrayBuffer.isView(value)) u8 = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
      else throw new TypeError('Failed to read the body: the stream yielded a chunk that is not a BufferSource');
      parts.push(u8);
      n += u8.length;
    }
    const out = new Uint8Array(n);
    let o = 0;
    for (const p of parts) { out.set(p, o); o += p.length; }
    return out;
  }
  if (g.Response) {
    const NativeResponse = g.Response;
    const Response = function Response(body, init) {
      if (!new.target) throw new TypeError("Failed to construct 'Response': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
      if (!isStream(body)) return arguments.length ? new NativeResponse(...arguments) : new NativeResponse();
      if (body.locked) throw new TypeError("Failed to construct 'Response': Response body object should not be disturbed or locked");
      const r = new NativeResponse(null, init);
      streamBodies.set(r, body);
      return r;
    };
    Response.prototype = NativeResponse.prototype;
    Object.defineProperty(NativeResponse.prototype, 'constructor', { value: Response, writable: true, configurable: true });
    for (const k of Object.getOwnPropertyNames(NativeResponse)) {
      if (!['length', 'name', 'prototype', 'arguments', 'caller'].includes(k)) Object.defineProperty(Response, k, Object.getOwnPropertyDescriptor(NativeResponse, k));
    }
    define('Response', Response);
    const origClone = NativeResponse.prototype.clone;
    if (typeof origClone === 'function') {
      hidden(NativeResponse.prototype, 'clone', function clone() {
        const sb = streamBodies.get(this);
        if (!sb) return origClone.call(this);
        if (used.has(this)) throw new TypeError("Failed to execute 'clone' on 'Response': Response body is already used");
        const [a, b] = sb.tee();
        streamBodies.set(this, a);
        const c = origClone.call(this);
        streamBodies.set(c, b);
        return c;
      });
    }
  }
  for (const C of [g.Request, g.Response]) {
    if (!C) continue;
    for (const m of ['text', 'json', 'arrayBuffer', 'blob', 'formData', 'bytes']) {
      const orig = C.prototype[m];
      if (typeof orig !== 'function') continue;
      hidden(C.prototype, m, function () {
        if (used.has(this)) return Promise.reject(new TypeError(`Failed to execute '${m}' on '${C.name}': body stream already read`));
        used.add(this);
        const sb = streamBodies.get(this);
        if (sb) {
          const headers = this.headers;
          return drain(sb).then((bytes) => {
            const tmp = new g.Response(bytes, { headers });
            return m === 'bytes' ? orig.call(tmp).then((b) => new Uint8Array(b)) : orig.call(tmp);
          });
        }
        if (m === 'bytes') return orig.call(this).then((b) => new Uint8Array(b));
        return orig.apply(this, arguments);
      });
    }
    Object.defineProperty(C.prototype, 'bodyUsed', { get() { return used.has(this); }, configurable: true, enumerable: true });
    Object.defineProperty(C.prototype, 'body', {
      get() {
        if (streamBodies.has(this)) return streamBodies.get(this);
        if (bodies.has(this)) return bodies.get(this);
        const self = this;
        if (C === g.Request && (this.method === 'GET' || this.method === 'HEAD')) return null;
        const orig = C.prototype.arrayBuffer;
        const stream = new g.ReadableStream({
          type: 'bytes',
          start(c) {
            return Promise.resolve().then(() => self.clone()).then((cl) => Object.getPrototypeOf(C.prototype) && orig.call(cl)).then((b) => { used.add(self); if (b && b.byteLength) c.enqueue(new Uint8Array(b)); c.close(); }, (e) => c.error(e));
          },
        });
        bodies.set(this, stream);
        return stream;
      }, configurable: true, enumerable: true,
    });
    method(C, 'textStream', function () { const b = this.body; return b ? b.pipeThrough(new g.TextDecoderStream()) : null; });
  }
  defaults(g.Response, { ok: function () { return this.status >= 200 && this.status < 300; }, status: 200, statusText: '', type: 'default', url: '', redirected: false, headers: () => new g.Headers() });
  defaults(g.Request, { method: 'GET', url: '', headers: () => new g.Headers(), destination: '', referrer: 'about:client', referrerPolicy: '', mode: 'cors', credentials: 'same-origin', cache: 'default', redirect: 'follow', integrity: '', keepalive: false, isReloadNavigation: false, isHistoryNavigation: false, signal: () => new AbortController().signal, duplex: 'half', targetAddressSpace: 'unknown' });
  if (g.Response && !g.Response.redirect) {
    g.Response.redirect = function redirect(url, status = 302) {
      if (![301, 302, 303, 307, 308].includes(status)) throw new RangeError("Failed to execute 'redirect' on 'Response': Invalid status code");
      const u = new URL(url, document.baseURI).href;
      return new g.Response(null, { status, headers: { Location: u } });
    };
  }

  // Abort rejects with the original reason. Transport cancellation remains an
  // environment concern; dropping the native SW receiver cancels interception.
  const fetchNative = g.fetch;
  hidden(g, 'fetch', function fetch(input, init) {
    let signal;
    try {
      const fromInit = init ? init.signal : undefined;
      signal = fromInit !== undefined ? fromInit : (input instanceof g.Request ? input.signal : undefined);
      if (signal !== undefined && signal !== null && !(signal instanceof g.AbortSignal)) throw new TypeError('Invalid fetch signal.');
      if (signal && signal.aborted) return Promise.reject(signal.reason);
    } catch (e) { return Promise.reject(e); }
    if (!signal) return fetchNative.call(this, input, init);
    return new Promise((resolve, reject) => {
      let nativePromise, done = false;
      const abort = () => {
        if (done) return;
        done = true;
        signal.removeEventListener('abort', abort);
        if (nativePromise) N.cancelFetch(nativePromise);
        reject(signal.reason);
      };
      signal.addEventListener('abort', abort, { once: true });
      if (signal.aborted) { abort(); return; }
      try { nativePromise = fetchNative.call(this, input, init, signal); }
      catch (e) { done = true; signal.removeEventListener('abort', abort); reject(e); return; }
      if (done) { N.cancelFetch(nativePromise); nativePromise.catch(() => {}); return; }
      nativePromise.then(response => {
        if (done) return;
        done = true; signal.removeEventListener('abort', abort); resolve(response);
      }, error => {
        if (done) return;
        done = true; signal.removeEventListener('abort', abort); reject(error);
      });
    });
  });

  // fetchLater (deferred fetching): sent immediately on pagehide is not
  // possible here; requests are queued and sent when the page unloads.
  class FetchLaterResult { constructor() { ILLEGAL(); } get activated() { return this[S].activated; } }
  tag(FetchLaterResult, 'FetchLaterResult');
  define('FetchLaterResult', FetchLaterResult);
  const later = [];
  hidden(g, 'fetchLater', function fetchLater(input, init = {}) {
    const req = new g.Request(input, init);
    if (init.activateAfter !== undefined && init.activateAfter < 0) throw new RangeError("Failed to execute 'fetchLater' on 'Window': activateAfter must be non-negative.");
    const r = Object.create(FetchLaterResult.prototype);
    hidden(r, S, { activated: false });
    const fire = () => { if (r[S].activated) return; r[S].activated = true; g.fetch(req).catch(() => {}); };
    later.push(fire);
    if (init.activateAfter !== undefined) setTimeout(fire, init.activateAfter);
    if (init.signal) init.signal.addEventListener('abort', () => { r[S].activated = true; });
    return r;
  });
  g.addEventListener('pagehide', () => { for (const f of later) f(); });

  // ------------------------------------------------------------------ XHR
  class XMLHttpRequestEventTarget extends g.EventTarget { constructor() { super(); ILLEGAL(); } }
  tag(XMLHttpRequestEventTarget, 'XMLHttpRequestEventTarget');
  N.installHandlers(XMLHttpRequestEventTarget.prototype, ['loadstart', 'progress', 'abort', 'error', 'load', 'timeout', 'loadend']);
  define('XMLHttpRequestEventTarget', XMLHttpRequestEventTarget);
  if (g.XMLHttpRequest) {
    for (const k of ['onloadstart', 'onprogress', 'onabort', 'onerror', 'onload', 'ontimeout', 'onloadend']) delete g.XMLHttpRequest.prototype[k];
    Object.setPrototypeOf(g.XMLHttpRequest.prototype, XMLHttpRequestEventTarget.prototype);
    Object.setPrototypeOf(g.XMLHttpRequest, XMLHttpRequestEventTarget);
    for (const [k, v] of Object.entries({ UNSENT: 0, OPENED: 1, HEADERS_RECEIVED: 2, LOADING: 3, DONE: 4 })) if (!Object.prototype.hasOwnProperty.call(g.XMLHttpRequest, k)) Object.defineProperty(g.XMLHttpRequest, k, { value: v, enumerable: true });
    defaults(g.XMLHttpRequest, { responseType: '', timeout: 0, withCredentials: false, upload: null });
  }
  if (g.XMLHttpRequestUpload) {
    Object.setPrototypeOf(g.XMLHttpRequestUpload.prototype, XMLHttpRequestEventTarget.prototype);
    Object.setPrototypeOf(g.XMLHttpRequestUpload, XMLHttpRequestEventTarget);
  }
  // ProgressEvent init members.
  defaults(g.ProgressEvent, { lengthComputable: false, loaded: 0, total: 0 });

  // ------------------------------------------------------------ WebSocket
  // Connections go through the network stack's WebSocket client
  // (HostEnv::open_websocket); the event loop polls them and hands events
  // to `socketEvent`. Without a client, connections fail the way an
  // unreachable server does (error + close 1006), asynchronously.
  const WS_STATES = { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 };
  const byteLen = (data) => (typeof data === 'string' ? new TextEncoder().encode(data).length : (data.byteLength || data.size || 0));
  function failSocket(ws) {
    setTimeout(() => {
      const st = ws[S];
      if (st.state === 3) return;
      st.state = 3;
      ws.dispatchEvent(new Event('error'));
      ws.dispatchEvent(new g.CloseEvent('close', { wasClean: false, code: 1006, reason: '' }));
    }, 0);
  }
  class WebSocket extends g.EventTarget {
    constructor(url, protocols = []) {
      super();
      if (arguments.length < 1) throw new TypeError("Failed to construct 'WebSocket': 1 argument required, but only 0 present.");
      let u;
      try { u = new URL(url, document.baseURI); } catch (e) { throw domError(`Failed to construct 'WebSocket': The URL '${url}' is invalid.`, 'SyntaxError'); }
      if (u.protocol === 'http:') u.protocol = 'ws:';
      if (u.protocol === 'https:') u.protocol = 'wss:';
      if (u.protocol !== 'ws:' && u.protocol !== 'wss:') throw domError(`Failed to construct 'WebSocket': The URL's scheme must be either 'http', 'https', 'ws', or 'wss'. '${u.protocol}' is not allowed.`, 'SyntaxError');
      if (u.hash) throw domError("Failed to construct 'WebSocket': The URL contains a fragment identifier ('" + u.hash + "'). Fragment identifiers are not allowed in WebSocket URLs.", 'SyntaxError');
      const list = typeof protocols === 'string' ? [protocols] : Array.from(protocols, String);
      if (new Set(list).size !== list.length) throw domError("Failed to construct 'WebSocket': The subprotocol is duplicated.", 'SyntaxError');
      for (const p of list) if (!/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(p)) throw domError(`Failed to construct 'WebSocket': The subprotocol '${p}' is invalid.`, 'SyntaxError');
      hidden(this, S, { url: u.href, state: 0, binaryType: 'blob', protocol: '', extensions: '', buffered: 0, id: null });
      const id = N.wsOpen(this, u.href, list);
      if (typeof id === 'number') this[S].id = id;
      else failSocket(this);
    }
    get url() { return this[S].url; }
    get readyState() { return this[S].state; }
    get bufferedAmount() {
      const st = this[S];
      const info = st.id !== null && st.state === 1 ? N.connInfo(st.id) : null;
      return st.buffered + (info ? info.bufferedAmount : 0);
    }
    get extensions() { return this[S].extensions; }
    get protocol() { return this[S].protocol; }
    get binaryType() { return this[S].binaryType; }
    set binaryType(v) { if (v === 'blob' || v === 'arraybuffer') this[S].binaryType = v; }
    send(data) {
      if (arguments.length < 1) throw new TypeError("Failed to execute 'send' on 'WebSocket': 1 argument required, but only 0 present.");
      const st = this[S];
      if (st.state === 0) throw domError("Failed to execute 'send' on 'WebSocket': Still in CONNECTING state.", 'InvalidStateError');
      const isText = !(data instanceof ArrayBuffer || ArrayBuffer.isView(data) || data instanceof g.Blob);
      if (isText) data = String(data);
      // After close() the data only counts towards bufferedAmount.
      if (st.state > 1 || st.id === null) { st.buffered += byteLen(data); return; }
      if (data instanceof DataView) data = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
      N.wsSend(st.id, data, isText);
    }
    close(code, reason) {
      if (code !== undefined) {
        code = Number(code) & 0xffff;
        if (code !== 1000 && !(code >= 3000 && code <= 4999)) throw domError(`Failed to execute 'close' on 'WebSocket': The close code must be either 1000, or between 3000 and 4999. ${code} is neither.`, 'InvalidAccessError');
      }
      if (reason !== undefined && new TextEncoder().encode(String(reason)).length > 123) throw domError("Failed to execute 'close' on 'WebSocket': The message must not be greater than 123 bytes.", 'SyntaxError');
      const st = this[S];
      if (st.state >= 2) return;
      st.state = 2;
      if (st.id !== null) N.connClose(st.id, code === undefined ? null : code, reason === undefined ? '' : String(reason));
    }
  }
  for (const [k, v] of Object.entries(WS_STATES)) { Object.defineProperty(WebSocket, k, { value: v, enumerable: true }); Object.defineProperty(WebSocket.prototype, k, { value: v, enumerable: true }); }
  tag(WebSocket, 'WebSocket');
  N.installHandlers(WebSocket.prototype, ['open', 'error', 'close', 'message']);
  define('WebSocket', WebSocket);

  // ---------------------------------------------------------- EventSource
  // Server-sent events through the network stack's streaming client
  // (HostEnv::open_event_source), which handles reconnection and
  // Last-Event-ID. Without one, the stream is fetched: the response is
  // parsed when it completes, then re-established after the retry delay.
  class EventSource extends g.EventTarget {
    constructor(url, init = {}) {
      super();
      if (arguments.length < 1) throw new TypeError("Failed to construct 'EventSource': 1 argument required, but only 0 present.");
      let u;
      try { u = new URL(url, document.baseURI); } catch (e) { throw domError(`Failed to construct 'EventSource': Cannot open an EventSource to '${url}'. The URL is invalid.`, 'SyntaxError'); }
      const creds = !!(init && init.withCredentials);
      hidden(this, S, { url: u.href, state: 0, withCredentials: creds, lastId: '', retry: 3000, ctrl: null, id: null });
      const id = N.sseOpen(this, u.href, creds);
      if (typeof id === 'number') this[S].id = id;
      else if (id === null) connect(this);
      else setTimeout(() => { if (this[S].state !== 2) { this[S].state = 2; this.dispatchEvent(new Event('error')); } }, 0);
    }
    get url() { return this[S].url; }
    get withCredentials() { return this[S].withCredentials; }
    get readyState() { return this[S].state; }
    close() {
      const st = this[S];
      st.state = 2;
      if (st.ctrl) st.ctrl.abort();
      if (st.id !== null) { N.connClose(st.id); st.id = null; }
    }
  }
  for (const [k, v] of Object.entries({ CONNECTING: 0, OPEN: 1, CLOSED: 2 })) { Object.defineProperty(EventSource, k, { value: v, enumerable: true }); Object.defineProperty(EventSource.prototype, k, { value: v, enumerable: true }); }
  tag(EventSource, 'EventSource');
  N.installHandlers(EventSource.prototype, ['open', 'message', 'error']);

  // Events from native connections: hook(obj, type, ...args).
  function socketEvent(obj, type, a, b, c, d) {
    const st = obj[S];
    if (obj instanceof WebSocket) {
      switch (type) {
        case 'open':
          if (st.state !== 0) return;
          st.state = 1; st.protocol = a || ''; st.extensions = b || '';
          obj.dispatchEvent(new Event('open'));
          return;
        case 'message':
        case 'binary': {
          if (st.state !== 1) return;
          const data = type === 'message' ? a : st.binaryType === 'arraybuffer' ? a : new g.Blob([a]);
          obj.dispatchEvent(new g.MessageEvent('message', { data, origin: new URL(st.url).origin }));
          return;
        }
        case 'error':
          if (st.state === 3) return;
          obj.dispatchEvent(new Event('error'));
          return;
        case 'close':
          st.state = 3; st.id = null;
          obj.dispatchEvent(new g.CloseEvent('close', { wasClean: !!c, code: a, reason: b || '' }));
          return;
      }
      return;
    }
    if (obj instanceof EventSource) {
      if (st.state === 2) return;
      switch (type) {
        case 'open':
          st.state = 1;
          obj.dispatchEvent(new Event('open'));
          return;
        case 'sse':
          st.lastId = c;
          obj.dispatchEvent(new g.MessageEvent(a || 'message', { data: b, origin: d || new URL(st.url).origin, lastEventId: c }));
          return;
        case 'error':
          // b: the client reconnects by itself.
          st.state = b ? 0 : 2;
          if (!b) st.id = null;
          obj.dispatchEvent(new Event('error'));
          return;
      }
    }
  }
  N.setSocketHook(socketEvent);
  function connect(es) {
    const st = es[S];
    if (st.state === 2) return;
    st.ctrl = new AbortController();
    const headers = { Accept: 'text/event-stream', 'Cache-Control': 'no-cache' };
    if (st.lastId) headers['Last-Event-ID'] = st.lastId;
    g.fetch(st.url, { headers, credentials: st.withCredentials ? 'include' : 'same-origin', signal: st.ctrl.signal }).then((r) => {
      if (st.state === 2) return;
      if (!r.ok || !/^text\/event-stream/i.test(r.headers.get('content-type') || '')) { st.state = 2; es.dispatchEvent(new Event('error')); return; }
      st.state = 1;
      es.dispatchEvent(new Event('open'));
      return r.text().then((text) => {
        parseEvents(es, text);
        if (st.state === 2) return;
        st.state = 0;
        es.dispatchEvent(new Event('error'));
        setTimeout(() => connect(es), st.retry);
      });
    }, () => {
      if (st.state === 2) return;
      st.state = 0;
      es.dispatchEvent(new Event('error'));
      setTimeout(() => connect(es), st.retry);
    });
  }
  function parseEvents(es, text) {
    const st = es[S];
    let data = '', type = '', id = null;
    for (const line of text.replace(/\r\n?/g, '\n').split('\n')) {
      if (line === '') {
        if (id !== null) st.lastId = id;
        if (data) {
          const ev = new g.MessageEvent(type || 'message', { data: data.endsWith('\n') ? data.slice(0, -1) : data, origin: new URL(st.url).origin, lastEventId: st.lastId });
          es.dispatchEvent(ev);
        }
        data = ''; type = ''; id = null;
        continue;
      }
      if (line[0] === ':') continue;
      const i = line.indexOf(':');
      const field = i < 0 ? line : line.slice(0, i);
      let value = i < 0 ? '' : line.slice(i + 1);
      if (value[0] === ' ') value = value.slice(1);
      if (field === 'event') type = value;
      else if (field === 'data') data += value + '\n';
      else if (field === 'id' && !value.includes('\0')) id = value;
      else if (field === 'retry' && /^\d+$/.test(value)) st.retry = +value;
    }
  }
  define('EventSource', EventSource);
})();
