// IndexedDB 3.0, functional in memory for the page's lifetime: factory
// (open / deleteDatabase / databases / cmp), versioned upgrades,
// transactions with auto-commit and abort/rollback, object stores (in-line
// and out-of-line keys, key generators), indexes (unique, multiEntry),
// cursors, key ranges, requests and events.
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('idb');
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const err = (name, msg) => new g.DOMException(msg, name);
  const task = (f) => setTimeout(f, 0);
  const make = (C, st) => { const o = Object.create(C.prototype); hidden(o, S, st); return o; };

  // ------------------------------------------------------------------ keys
  function keyType(k) {
    if (typeof k === 'number') return Number.isNaN(k) ? null : 1;
    if (k instanceof Date) return Number.isNaN(k.getTime()) ? null : 2;
    if (typeof k === 'string') return 3;
    if (k instanceof ArrayBuffer || ArrayBuffer.isView(k)) return 4;
    if (Array.isArray(k)) return 5;
    return null;
  }
  function toKey(v, seen = []) {
    const t = keyType(v);
    if (t === null) throw err('DataError', 'The parameter is not a valid key.');
    if (t === 2) return new Date(v.getTime());
    if (t === 4) return new Uint8Array(ArrayBuffer.isView(v) ? v.buffer.slice(v.byteOffset, v.byteOffset + v.byteLength) : v.slice(0)).buffer;
    if (t === 5) {
      if (seen.includes(v)) throw err('DataError', 'The parameter is not a valid key.');
      return v.map((x) => toKey(x, seen.concat([v])));
    }
    return v;
  }
  const validKey = (v) => { try { toKey(v); return true; } catch (e) { return false; } };
  function cmp(a, b) {
    const ta = keyType(a), tb = keyType(b);
    if (ta !== tb) return ta > tb ? 1 : -1;
    switch (ta) {
      case 1: case 3: return a === b ? 0 : a < b ? -1 : 1;
      case 2: return a.getTime() === b.getTime() ? 0 : a.getTime() < b.getTime() ? -1 : 1;
      case 4: {
        const x = new Uint8Array(a), y = new Uint8Array(b);
        for (let i = 0; i < Math.min(x.length, y.length); i++) if (x[i] !== y[i]) return x[i] < y[i] ? -1 : 1;
        return x.length === y.length ? 0 : x.length < y.length ? -1 : 1;
      }
      default: {
        for (let i = 0; i < Math.min(a.length, b.length); i++) { const c = cmp(a[i], b[i]); if (c) return c; }
        return a.length === b.length ? 0 : a.length < b.length ? -1 : 1;
      }
    }
  }
  function evalPath(value, path) {
    if (Array.isArray(path)) return path.map((p) => evalPath(value, p));
    if (path === '') return value;
    let v = value;
    for (const part of path.split('.')) {
      if (v === null || v === undefined) return undefined;
      if (typeof v === 'string' && part === 'length') { v = v.length; continue; }
      if (!(typeof v === 'object') || !(part in Object(v))) return undefined;
      v = v[part];
    }
    return v;
  }
  function injectKey(value, path, key) {
    const parts = path.split('.');
    let o = value;
    for (let i = 0; i < parts.length - 1; i++) {
      if (o[parts[i]] === undefined) o[parts[i]] = {};
      o = o[parts[i]];
    }
    o[parts[parts.length - 1]] = key;
  }
  const validPath = (p) => (Array.isArray(p) ? p.length > 0 && p.every((x) => typeof x === 'string' && validPath(x)) : p === '' || /^[A-Za-z_$][\w$]*(\.[A-Za-z_$][\w$]*)*$/.test(p));

  // ------------------------------------------------------------- key ranges
  class IDBKeyRange {
    constructor() { ILLEGAL(); }
    get lower() { return this[S].lower; }
    get upper() { return this[S].upper; }
    get lowerOpen() { return this[S].lowerOpen; }
    get upperOpen() { return this[S].upperOpen; }
    includes(key) {
      key = toKey(key);
      const st = this[S];
      if (st.lower !== undefined) { const c = cmp(key, st.lower); if (c < 0 || (c === 0 && st.lowerOpen)) return false; }
      if (st.upper !== undefined) { const c = cmp(key, st.upper); if (c > 0 || (c === 0 && st.upperOpen)) return false; }
      return true;
    }
    static only(v) { const k = toKey(v); return range(k, k, false, false); }
    static lowerBound(v, open = false) { return range(toKey(v), undefined, !!open, true); }
    static upperBound(v, open = false) { return range(undefined, toKey(v), true, !!open); }
    static bound(l, u, lo = false, uo = false) {
      l = toKey(l); u = toKey(u);
      const c = cmp(l, u);
      if (c > 0 || (c === 0 && (lo || uo))) throw err('DataError', 'The lower key is greater than the upper key.');
      return range(l, u, !!lo, !!uo);
    }
  }
  tag(IDBKeyRange, 'IDBKeyRange');
  function range(lower, upper, lowerOpen, upperOpen) { return make(IDBKeyRange, { lower, upper, lowerOpen, upperOpen }); }
  function toRange(q) {
    if (q === undefined || q === null) return null;
    if (q instanceof IDBKeyRange) return q;
    const k = toKey(q);
    return range(k, k, false, false);
  }

  // ------------------------------------------------------------ data model
  // dbs: name -> { name, version, stores: Map(name -> store) }
  // store: { name, keyPath, autoIncrement, gen, records: [{key, value}], indexes: Map }
  // index: { name, keyPath, unique, multiEntry, records: [{key, primaryKey}] }
  const dbs = new Map();
  const connections = new Map();
  const deepCopy = (v) => g.structuredClone(v);
  function lowerBoundIdx(list, key, field = 'key') {
    let lo = 0, hi = list.length;
    while (lo < hi) { const mid = (lo + hi) >> 1; if (cmp(list[mid][field], key) < 0) lo = mid + 1; else hi = mid; }
    return lo;
  }
  function indexEntries(idx, value, primaryKey) {
    const k = evalPath(value, idx.keyPath);
    if (k === undefined) return [];
    if (idx.multiEntry && Array.isArray(k)) {
      const out = [];
      for (const x of k) if (validKey(x) && !out.some((o) => cmp(o, x) === 0)) out.push(toKey(x));
      return out.map((key) => ({ key, primaryKey }));
    }
    return validKey(k) ? [{ key: toKey(k), primaryKey }] : [];
  }
  function indexInsert(idx, entries) {
    for (const e of entries) {
      let i = lowerBoundIdx(idx.records, e.key);
      while (i < idx.records.length && cmp(idx.records[i].key, e.key) === 0 && cmp(idx.records[i].primaryKey, e.primaryKey) < 0) i++;
      idx.records.splice(i, 0, e);
    }
  }
  function indexRemovePrimary(idx, primaryKey) { idx.records = idx.records.filter((r) => cmp(r.primaryKey, primaryKey) !== 0); }

  // ---------------------------------------------------------- requests
  class IDBRequest extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get result() { if (this[S].readyState !== 'done') throw err('InvalidStateError', "Failed to read the 'result' property from 'IDBRequest': The request has not finished."); return this[S].result; }
    get error() { if (this[S].readyState !== 'done') throw err('InvalidStateError', "Failed to read the 'error' property from 'IDBRequest': The request has not finished."); return this[S].error; }
    get source() { return this[S].source; }
    get transaction() { return this[S].transaction; }
    get readyState() { return this[S].readyState; }
  }
  tag(IDBRequest, 'IDBRequest');
  N.installHandlers(IDBRequest.prototype, ['success', 'error']);
  class IDBOpenDBRequest extends IDBRequest {}
  tag(IDBOpenDBRequest, 'IDBOpenDBRequest');
  N.installHandlers(IDBOpenDBRequest.prototype, ['blocked', 'upgradeneeded']);
  const newRequest = (C, source, tx) => make(C, { result: undefined, error: null, source, transaction: tx, readyState: 'pending' });
  function succeed(req, result) {
    const st = req[S];
    st.readyState = 'done'; st.result = result; st.error = null;
    const tx = st.transaction;
    if (tx) tx[S].active = true;
    try { req.dispatchEvent(new Event('success')); } catch (e) { if (tx) abortTx(tx, e); }
    if (tx) { tx[S].active = false; checkCommit(tx); }
  }
  function fail(req, e) {
    const st = req[S];
    st.readyState = 'done'; st.result = undefined; st.error = e;
    const tx = st.transaction;
    if (tx) tx[S].active = true;
    const ev = new Event('error', { bubbles: true, cancelable: true });
    const notPrevented = req.dispatchEvent(ev);
    if (tx) {
      tx[S].active = false;
      if (notPrevented) abortTx(tx, e); else checkCommit(tx);
    }
  }

  // ------------------------------------------------------- transactions
  class IDBTransaction extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get objectStoreNames() { return g.__rhDOMStringList(this[S].scope.slice().sort()); }
    get mode() { return this[S].mode; }
    get durability() { return this[S].durability; }
    get db() { return this[S].db; }
    get error() { return this[S].error; }
    objectStore(name) {
      const st = this[S];
      if (st.finished) throw err('InvalidStateError', "Failed to execute 'objectStore' on 'IDBTransaction': The transaction has finished.");
      if (!st.scope.includes(name)) throw err('NotFoundError', "Failed to execute 'objectStore' on 'IDBTransaction': The specified object store was not found.");
      let s = st.storeObjs.get(name);
      if (!s) { s = make(IDBObjectStore, { name, tx: this, db: st.db }); st.storeObjs.set(name, s); }
      return s;
    }
    commit() { const st = this[S]; if (st.finished) throw err('InvalidStateError', 'The transaction has finished.'); st.commitRequested = true; checkCommit(this); }
    abort() { if (this[S].finished || this[S].committing) throw err('InvalidStateError', "Failed to execute 'abort' on 'IDBTransaction': The transaction has already been committed or aborted."); abortTx(this, null); }
  }
  tag(IDBTransaction, 'IDBTransaction');
  N.installHandlers(IDBTransaction.prototype, ['abort', 'complete', 'error']);
  function newTx(db, scope, mode, durability) {
    const tx = make(IDBTransaction, { db, scope, mode, durability: durability || 'default', active: true, finished: false, committing: false, pending: 0, queue: [], storeObjs: new Map(), error: null, snapshot: null, commitRequested: false });
    // Rollback snapshot.
    const data = dbs.get(db[S].name);
    tx[S].snapshot = mode === 'readonly' ? null : snapshotDb(data);
    db[S].txs.add(tx);
    // Deactivate at the end of the current task; auto-commit when idle.
    task(() => { tx[S].active = false; checkCommit(tx); });
    return tx;
  }
  function snapshotDb(data) {
    return { version: data.version, stores: new Map(Array.from(data.stores, ([n, s]) => [n, { name: s.name, keyPath: s.keyPath, autoIncrement: s.autoIncrement, gen: s.gen, records: s.records.slice(), indexes: new Map(Array.from(s.indexes, ([k, i]) => [k, Object.assign({}, i, { records: i.records.slice() })])) }])) };
  }
  function schedule(tx, req, op) {
    const st = tx[S];
    st.pending++;
    task(() => {
      st.pending--;
      if (st.finished) return;
      let result;
      try { result = op(); } catch (e) { fail(req, e instanceof g.DOMException ? e : err('DataError', String(e && e.message || e))); return; }
      succeed(req, result);
    });
  }
  function checkCommit(tx) {
    const st = tx[S];
    if (st.finished || st.active || st.pending > 0) return;
    st.committing = true;
    task(() => {
      if (st.finished || st.pending > 0) { st.committing = false; return; }
      st.finished = true;
      st.db[S].txs.delete(tx);
      tx.dispatchEvent(new Event('complete'));
      const r = st.onFinish; if (r) r(true);
      processQueue();
    });
  }
  function abortTx(tx, e) {
    const st = tx[S];
    if (st.finished) return;
    st.finished = true;
    st.error = e;
    if (st.snapshot) {
      const data = dbs.get(st.db[S].name);
      if (data) { data.stores = st.snapshot.stores; data.version = st.snapshot.version; }
    }
    st.db[S].txs.delete(tx);
    task(() => {
      tx.dispatchEvent(new Event('abort', { bubbles: true }));
      const r = st.onFinish; if (r) r(false);
      processQueue();
    });
  }
  function processQueue() {}

  // ------------------------------------------------------------- database
  class IDBDatabase extends g.EventTarget {
    constructor() { super(); ILLEGAL(); }
    get name() { return this[S].name; }
    get version() { return this[S].version; }
    get objectStoreNames() { const d = dbs.get(this[S].name); return g.__rhDOMStringList(d && !this[S].closed ? Array.from(d.stores.keys()).sort() : []); }
    transaction(names, mode = 'readonly', options = {}) {
      const st = this[S];
      if (st.closed) throw err('InvalidStateError', "Failed to execute 'transaction' on 'IDBDatabase': The database connection is closing.");
      if (st.upgradeTx && !st.upgradeTx[S].finished) throw err('InvalidStateError', "Failed to execute 'transaction' on 'IDBDatabase': A version change transaction is running.");
      const list = typeof names === 'string' ? [names] : Array.from(names);
      if (!list.length) throw err('InvalidAccessError', "Failed to execute 'transaction' on 'IDBDatabase': The storeNames parameter was empty.");
      if (mode !== 'readonly' && mode !== 'readwrite') throw new TypeError(`Failed to execute 'transaction' on 'IDBDatabase': The provided value '${mode}' is not a valid enum value of type IDBTransactionMode.`);
      const data = dbs.get(st.name);
      for (const n of list) if (!data.stores.has(n)) throw err('NotFoundError', "Failed to execute 'transaction' on 'IDBDatabase': One of the specified object stores was not found.");
      return newTx(this, Array.from(new Set(list)), mode, options.durability);
    }
    createObjectStore(name, options = {}) {
      const tx = versionTx(this, 'createObjectStore');
      name = String(name);
      const data = dbs.get(this[S].name);
      if (data.stores.has(name)) throw err('ConstraintError', "Failed to execute 'createObjectStore' on 'IDBDatabase': An object store with the specified name already exists.");
      const keyPath = options.keyPath === undefined || options.keyPath === null ? null : options.keyPath;
      if (keyPath !== null && !validPath(keyPath)) throw err('SyntaxError', "Failed to execute 'createObjectStore' on 'IDBDatabase': The keyPath option is not a valid key path.");
      if (options.autoIncrement && (keyPath === '' || Array.isArray(keyPath))) throw err('InvalidAccessError', "Failed to execute 'createObjectStore' on 'IDBDatabase': The autoIncrement option was set but the keyPath option was empty or an array.");
      data.stores.set(name, { name, keyPath, autoIncrement: !!options.autoIncrement, gen: 1, records: [], indexes: new Map() });
      tx[S].scope.push(name);
      return tx.objectStore(name);
    }
    deleteObjectStore(name) {
      const tx = versionTx(this, 'deleteObjectStore');
      const data = dbs.get(this[S].name);
      if (!data.stores.delete(String(name))) throw err('NotFoundError', "Failed to execute 'deleteObjectStore' on 'IDBDatabase': The specified object store was not found.");
      tx[S].scope = tx[S].scope.filter((n) => n !== name);
      tx[S].storeObjs.delete(name);
    }
    close() { this[S].closed = true; const set = connections.get(this[S].name); if (set) set.delete(this); }
  }
  tag(IDBDatabase, 'IDBDatabase');
  N.installHandlers(IDBDatabase.prototype, ['abort', 'close', 'error', 'versionchange']);
  function versionTx(db, what) {
    const tx = db[S].upgradeTx;
    if (!tx || tx[S].finished) throw err('InvalidStateError', `Failed to execute '${what}' on 'IDBDatabase': The database is not running a version change transaction.`);
    if (!tx[S].active) throw err('TransactionInactiveError', `Failed to execute '${what}' on 'IDBDatabase': The transaction is not active.`);
    return tx;
  }

  // ------------------------------------------------------------ factory
  class IDBFactory {
    constructor() { ILLEGAL(); }
    open(name, version) {
      if (arguments.length < 1) throw new TypeError("Failed to execute 'open' on 'IDBFactory': 1 argument required, but only 0 present.");
      name = String(name);
      if (version !== undefined) {
        version = Number(version);
        if (!(version >= 1) || !Number.isFinite(version) || version > Number.MAX_SAFE_INTEGER) throw new TypeError("Failed to execute 'open' on 'IDBFactory': The optional version provided (" + version + ") is not a valid integer version.");
        version = Math.floor(version);
      }
      const req = newRequest(IDBOpenDBRequest, null, null);
      task(() => openDb(req, name, version));
      return req;
    }
    deleteDatabase(name) {
      name = String(name);
      const req = newRequest(IDBOpenDBRequest, null, null);
      task(() => {
        const old = dbs.get(name);
        const oldVersion = old ? old.version : 0;
        for (const c of connections.get(name) || []) c.dispatchEvent(new g.IDBVersionChangeEvent('versionchange', { oldVersion, newVersion: null }));
        dbs.delete(name);
        connections.delete(name);
        req[S].readyState = 'done'; req[S].result = undefined;
        req.dispatchEvent(new g.IDBVersionChangeEvent('success', { oldVersion, newVersion: null }));
      });
      return req;
    }
    databases() { return Promise.resolve(Array.from(dbs.values()).map((d) => ({ name: d.name, version: d.version }))); }
    cmp(a, b) { return cmp(toKey(a), toKey(b)); }
  }
  tag(IDBFactory, 'IDBFactory');
  function openDb(req, name, version) {
    let data = dbs.get(name);
    const oldVersion = data ? data.version : 0;
    if (version === undefined) version = data ? data.version : 1;
    if (data && version < data.version) { fail(req, err('VersionError', `The requested version (${version}) is less than the existing version (${data.version}).`)); return; }
    if (!data) { data = { name, version: 0, stores: new Map() }; dbs.set(name, data); }
    const db = make(IDBDatabase, { name, version: data.version, closed: false, txs: new Set(), upgradeTx: null });
    if (!connections.has(name)) connections.set(name, new Set());
    if (version > data.version) {
      for (const c of connections.get(name)) if (!c[S].closed) c.dispatchEvent(new g.IDBVersionChangeEvent('versionchange', { oldVersion, newVersion: version }));
      if (Array.from(connections.get(name)).some((c) => !c[S].closed)) req.dispatchEvent(new g.IDBVersionChangeEvent('blocked', { oldVersion, newVersion: version }));
      connections.get(name).add(db);
      data.version = version;
      db[S].version = version;
      const tx = make(IDBTransaction, { db, scope: Array.from(data.stores.keys()), mode: 'versionchange', durability: 'default', active: true, finished: false, committing: false, pending: 0, storeObjs: new Map(), error: null, snapshot: null, commitRequested: false });
      tx[S].snapshot = snapshotDb(data);
      tx[S].snapshot.version = oldVersion;
      db[S].upgradeTx = tx;
      db[S].txs.add(tx);
      req[S].transaction = tx;
      req[S].readyState = 'done';
      req[S].result = db;
      tx[S].onFinish = (ok) => {
        req[S].transaction = null;
        db[S].upgradeTx = null;
        if (ok) { req.dispatchEvent(new Event('success')); }
        else {
          db[S].closed = true;
          connections.get(name).delete(db);
          if (oldVersion === 0) dbs.delete(name);
          req[S].result = undefined;
          req[S].error = err('AbortError', 'Version change transaction was aborted in upgradeneeded event handler.');
          req.dispatchEvent(new Event('error', { bubbles: true, cancelable: true }));
        }
      };
      try { req.dispatchEvent(new g.IDBVersionChangeEvent('upgradeneeded', { oldVersion, newVersion: version })); } catch (e) { abortTx(tx, e); return; }
      tx[S].active = false;
      checkCommit(tx);
      return;
    }
    connections.get(name).add(db);
    req[S].readyState = 'done';
    req[S].result = db;
    req.dispatchEvent(new Event('success'));
  }

  // ------------------------------------------------------- object stores
  function storeData(store) {
    const st = store[S];
    const tx = st.tx;
    if (tx[S].finished) throw err('TransactionInactiveError', 'The transaction has finished.');
    const data = dbs.get(st.db[S].name);
    const s = data && data.stores.get(st.name);
    if (!s) throw err('InvalidStateError', 'The object store has been deleted.');
    return s;
  }
  function checkActive(tx, what) { if (!tx[S].active) throw err('TransactionInactiveError', `Failed to execute '${what}': The transaction is not active.`); }
  function checkWritable(tx, what) { if (tx[S].mode === 'readonly') throw err('ReadOnlyError', `Failed to execute '${what}': The transaction is read-only.`); }
  function recordsIn(list, r, field = 'key') {
    if (!r) return list.slice();
    const st = r[S];
    let i = st.lower === undefined ? 0 : lowerBoundIdx(list, st.lower, field);
    const out = [];
    for (; i < list.length; i++) {
      const k = list[i][field];
      if (st.lower !== undefined && st.lowerOpen && cmp(k, st.lower) === 0) continue;
      if (st.upper !== undefined) { const c = cmp(k, st.upper); if (c > 0 || (c === 0 && st.upperOpen)) break; }
      out.push(list[i]);
    }
    return out;
  }
  function putRecord(s, value, key, noOverwrite) {
    value = deepCopy(value);
    if (s.keyPath !== null) {
      if (key !== undefined) throw err('DataError', 'The object store uses in-line keys and the key parameter was provided.');
      let k = evalPath(value, s.keyPath);
      if (k === undefined) {
        if (!s.autoIncrement) throw err('DataError', 'Evaluating the object store\'s key path did not yield a value.');
        k = s.gen++;
        injectKey(value, s.keyPath, k);
      } else { k = toKey(k); if (s.autoIncrement && typeof k === 'number' && k >= s.gen) s.gen = Math.floor(k) + 1; }
      key = k;
    } else if (key === undefined) {
      if (!s.autoIncrement) throw err('DataError', 'The object store uses out-of-line keys and has no key generator and the key parameter was not provided.');
      key = s.gen++;
    } else { key = toKey(key); if (s.autoIncrement && typeof key === 'number' && key >= s.gen) s.gen = Math.floor(key) + 1; }
    const i = lowerBoundIdx(s.records, key);
    const exists = i < s.records.length && cmp(s.records[i].key, key) === 0;
    if (exists && noOverwrite) throw err('ConstraintError', 'Key already exists in the object store.');
    // Unique index constraints.
    for (const idx of s.indexes.values()) {
      if (!idx.unique) continue;
      for (const e of indexEntries(idx, value, key)) {
        const j = lowerBoundIdx(idx.records, e.key);
        if (j < idx.records.length && cmp(idx.records[j].key, e.key) === 0 && cmp(idx.records[j].primaryKey, key) !== 0) throw err('ConstraintError', `Unable to add key to index '${idx.name}': at least one key does not satisfy the uniqueness requirements.`);
      }
    }
    if (exists) { s.records[i] = { key, value }; for (const idx of s.indexes.values()) indexRemovePrimary(idx, key); }
    else s.records.splice(i, 0, { key, value });
    for (const idx of s.indexes.values()) indexInsert(idx, indexEntries(idx, value, key));
    return key;
  }
  class IDBObjectStore {
    constructor() { ILLEGAL(); }
    get name() { return this[S].name; }
    set name(v) {
      const tx = versionTx(this[S].db, 'name');
      const data = dbs.get(this[S].db[S].name);
      v = String(v);
      if (v === this[S].name) return;
      if (data.stores.has(v)) throw err('ConstraintError', 'An object store with the specified name already exists.');
      const s = data.stores.get(this[S].name);
      data.stores.delete(this[S].name); s.name = v; data.stores.set(v, s);
      tx[S].scope = tx[S].scope.map((n) => (n === this[S].name ? v : n));
      this[S].name = v;
    }
    get keyPath() { const s = storeData(this); return Array.isArray(s.keyPath) ? s.keyPath.slice() : s.keyPath; }
    get indexNames() { return g.__rhDOMStringList(Array.from(storeData(this).indexes.keys()).sort()); }
    get transaction() { return this[S].tx; }
    get autoIncrement() { return storeData(this).autoIncrement; }
    put(value, key) { return this._write('put', value, key, false); }
    add(value, key) { return this._write('add', value, key, true); }
    _write(what, value, key, noOverwrite) {
      const tx = this[S].tx; checkActive(tx, what); checkWritable(tx, what);
      const s = storeData(this);
      if (s.keyPath !== null && key !== undefined) throw err('DataError', `Failed to execute '${what}' on 'IDBObjectStore': The object store uses in-line keys and the key parameter was provided.`);
      if (key !== undefined) key = toKey(key);
      const copy = deepCopy(value);
      const req = newRequest(IDBRequest, this, tx);
      schedule(tx, req, () => putRecord(s, copy, key, noOverwrite));
      return req;
    }
    delete(query) {
      const tx = this[S].tx; checkActive(tx, 'delete'); checkWritable(tx, 'delete');
      const s = storeData(this); const r = toRange(query);
      if (!r) throw err('DataError', 'No key or key range specified.');
      const req = newRequest(IDBRequest, this, tx);
      schedule(tx, req, () => { for (const rec of recordsIn(s.records, r)) { s.records.splice(s.records.indexOf(rec), 1); for (const idx of s.indexes.values()) indexRemovePrimary(idx, rec.key); } return undefined; });
      return req;
    }
    clear() {
      const tx = this[S].tx; checkActive(tx, 'clear'); checkWritable(tx, 'clear');
      const s = storeData(this);
      const req = newRequest(IDBRequest, this, tx);
      schedule(tx, req, () => { s.records = []; for (const idx of s.indexes.values()) idx.records = []; return undefined; });
      return req;
    }
    get(query) { return this._read('get', query, (s, r) => { const rec = recordsIn(s.records, r)[0]; return rec ? deepCopy(rec.value) : undefined; }, true); }
    getKey(query) { return this._read('getKey', query, (s, r) => { const rec = recordsIn(s.records, r)[0]; return rec ? rec.key : undefined; }, true); }
    getAll(query, count) { return this._read('getAll', query, (s, r) => limit(recordsIn(s.records, r), count).map((x) => deepCopy(x.value))); }
    getAllKeys(query, count) { return this._read('getAllKeys', query, (s, r) => limit(recordsIn(s.records, r), count).map((x) => x.key)); }
    getAllRecords(options = {}) { return this._read('getAllRecords', options.query, (s, r) => { let l = recordsIn(s.records, r); if (options.direction === 'prev' || options.direction === 'prevunique') l = l.reverse(); return limit(l, options.count).map((x) => make(IDBRecord, { key: x.key, primaryKey: x.key, value: deepCopy(x.value) })); }); }
    count(query) { return this._read('count', query, (s, r) => recordsIn(s.records, r).length); }
    _read(what, query, fn, needKey) {
      const tx = this[S].tx; checkActive(tx, what);
      const s = storeData(this);
      const r = toRange(query);
      if (needKey && !r) throw err('DataError', `Failed to execute '${what}' on 'IDBObjectStore': No key or key range specified.`);
      const req = newRequest(IDBRequest, this, tx);
      schedule(tx, req, () => fn(s, r));
      return req;
    }
    openCursor(query, direction = 'next') { return openCursor(this, query, direction, false); }
    openKeyCursor(query, direction = 'next') { return openCursor(this, query, direction, true); }
    index(name) {
      const s = storeData(this);
      if (this[S].tx[S].finished) throw err('InvalidStateError', 'The transaction has finished.');
      if (!s.indexes.has(name)) throw err('NotFoundError', "Failed to execute 'index' on 'IDBObjectStore': The specified index was not found.");
      return make(IDBIndex, { name, store: this });
    }
    createIndex(name, keyPath, options = {}) {
      versionTx(this[S].db, 'createIndex');
      const s = storeData(this);
      name = String(name);
      if (s.indexes.has(name)) throw err('ConstraintError', "Failed to execute 'createIndex' on 'IDBObjectStore': An index with the specified name already exists.");
      if (!validPath(keyPath)) throw err('SyntaxError', "Failed to execute 'createIndex' on 'IDBObjectStore': The keyPath argument contains an invalid key path.");
      if (Array.isArray(keyPath) && options.multiEntry) throw err('InvalidAccessError', "Failed to execute 'createIndex' on 'IDBObjectStore': The keyPath argument was an array and the multiEntry option is true.");
      const idx = { name, keyPath, unique: !!options.unique, multiEntry: !!options.multiEntry, records: [] };
      for (const rec of s.records) {
        const entries = indexEntries(idx, rec.value, rec.key);
        if (idx.unique) for (const e of entries) if (idx.records.some((x) => cmp(x.key, e.key) === 0)) { const tx = this[S].tx; task(() => abortTx(tx, err('ConstraintError', 'Unique index constraint failed.'))); }
        indexInsert(idx, entries);
      }
      s.indexes.set(name, idx);
      return make(IDBIndex, { name, store: this });
    }
    deleteIndex(name) {
      versionTx(this[S].db, 'deleteIndex');
      if (!storeData(this).indexes.delete(String(name))) throw err('NotFoundError', "Failed to execute 'deleteIndex' on 'IDBObjectStore': The specified index was not found.");
    }
  }
  tag(IDBObjectStore, 'IDBObjectStore');
  const limit = (l, count) => (count === undefined || count === 0 ? l : l.slice(0, count >>> 0));

  class IDBIndex {
    constructor() { ILLEGAL(); }
    get name() { return this[S].name; }
    set name(v) {
      versionTx(this[S].store[S].db, 'name');
      const s = storeData(this[S].store);
      v = String(v);
      if (v === this[S].name) return;
      if (s.indexes.has(v)) throw err('ConstraintError', 'An index with the specified name already exists.');
      const idx = s.indexes.get(this[S].name); s.indexes.delete(this[S].name); idx.name = v; s.indexes.set(v, idx); this[S].name = v;
    }
    get objectStore() { return this[S].store; }
    get keyPath() { const k = idxData(this).keyPath; return Array.isArray(k) ? k.slice() : k; }
    get multiEntry() { return idxData(this).multiEntry; }
    get unique() { return idxData(this).unique; }
    get(query) { return this._read('get', query, (s, idx, r) => { const e = recordsIn(idx.records, r)[0]; if (!e) return undefined; const rec = s.records[lowerBoundIdx(s.records, e.primaryKey)]; return deepCopy(rec.value); }, true); }
    getKey(query) { return this._read('getKey', query, (s, idx, r) => { const e = recordsIn(idx.records, r)[0]; return e ? e.primaryKey : undefined; }, true); }
    getAll(query, count) { return this._read('getAll', query, (s, idx, r) => limit(recordsIn(idx.records, r), count).map((e) => deepCopy(s.records[lowerBoundIdx(s.records, e.primaryKey)].value))); }
    getAllKeys(query, count) { return this._read('getAllKeys', query, (s, idx, r) => limit(recordsIn(idx.records, r), count).map((e) => e.primaryKey)); }
    getAllRecords(options = {}) { return this._read('getAllRecords', options.query, (s, idx, r) => limit(recordsIn(idx.records, r), options.count).map((e) => make(IDBRecord, { key: e.key, primaryKey: e.primaryKey, value: deepCopy(s.records[lowerBoundIdx(s.records, e.primaryKey)].value) }))); }
    count(query) { return this._read('count', query, (s, idx, r) => recordsIn(idx.records, r).length); }
    _read(what, query, fn, needKey) {
      const store = this[S].store, tx = store[S].tx;
      checkActive(tx, what);
      const s = storeData(store), idx = idxData(this), r = toRange(query);
      if (needKey && !r) throw err('DataError', `Failed to execute '${what}' on 'IDBIndex': No key or key range specified.`);
      const req = newRequest(IDBRequest, this, tx);
      schedule(tx, req, () => fn(s, idx, r));
      return req;
    }
    openCursor(query, direction = 'next') { return openCursor(this, query, direction, false); }
    openKeyCursor(query, direction = 'next') { return openCursor(this, query, direction, true); }
  }
  tag(IDBIndex, 'IDBIndex');
  function idxData(index) { const s = storeData(index[S].store); const idx = s.indexes.get(index[S].name); if (!idx) throw err('InvalidStateError', 'The index has been deleted.'); return idx; }

  class IDBRecord { constructor() { ILLEGAL(); } get key() { return this[S].key; } get primaryKey() { return this[S].primaryKey; } get value() { return this[S].value; } }
  tag(IDBRecord, 'IDBRecord');

  // --------------------------------------------------------------- cursors
  class IDBCursor {
    constructor() { ILLEGAL(); }
    get source() { return this[S].source; }
    get direction() { return this[S].direction; }
    get key() { return this[S].key; }
    get primaryKey() { return this[S].primaryKey; }
    get request() { return this[S].request; }
    advance(count) {
      count = Number(count) >>> 0;
      if (count === 0) throw new TypeError("Failed to execute 'advance' on 'IDBCursor': A count argument with value 0 (zero) was supplied, must be greater than 0.");
      this._step(count, undefined, undefined);
    }
    continue(key) { this._step(1, key === undefined ? undefined : toKey(key), undefined); }
    continuePrimaryKey(key, primaryKey) { this._step(1, toKey(key), toKey(primaryKey)); }
    _step(count, target, targetPrimary) {
      const st = this[S];
      const tx = st.tx;
      checkActive(tx, 'continue');
      if (!st.gotValue) throw err('InvalidStateError', "Failed to execute 'continue' on 'IDBCursor': The cursor is being iterated or has iterated past its end.");
      st.gotValue = false;
      const req = st.request;
      req[S].readyState = 'pending';
      schedule(tx, req, () => iterate(this, count, target, targetPrimary));
    }
    update(value) {
      const st = this[S];
      checkActive(st.tx, 'update'); checkWritable(st.tx, 'update');
      if (st.keyOnly || !st.gotValue) throw err('InvalidStateError', "Failed to execute 'update' on 'IDBCursor': The cursor is a key cursor or is being iterated.");
      const store = st.source instanceof IDBIndex ? st.source[S].store : st.source;
      const s = storeData(store);
      const copy = deepCopy(value);
      if (s.keyPath !== null && cmp(toKey(evalPath(copy, s.keyPath)), st.primaryKey) !== 0) throw err('DataError', 'The effective object store of this cursor uses in-line keys and evaluating the key path of the value parameter results in a different value than the cursor\'s effective key.');
      const req = newRequest(IDBRequest, this, st.tx);
      schedule(st.tx, req, () => putRecord(s, copy, s.keyPath === null ? st.primaryKey : undefined, false));
      return req;
    }
    delete() {
      const st = this[S];
      checkActive(st.tx, 'delete'); checkWritable(st.tx, 'delete');
      if (st.keyOnly || !st.gotValue) throw err('InvalidStateError', "Failed to execute 'delete' on 'IDBCursor': The cursor is a key cursor or is being iterated.");
      const store = st.source instanceof IDBIndex ? st.source[S].store : st.source;
      return store.delete(st.primaryKey);
    }
  }
  tag(IDBCursor, 'IDBCursor');
  class IDBCursorWithValue extends IDBCursor { get value() { return this[S].value; } }
  tag(IDBCursorWithValue, 'IDBCursorWithValue');
  function openCursor(source, query, direction, keyOnly) {
    if (!['next', 'nextunique', 'prev', 'prevunique'].includes(direction)) throw new TypeError(`The provided value '${direction}' is not a valid enum value of type IDBCursorDirection.`);
    const store = source instanceof IDBIndex ? source[S].store : source;
    const tx = store[S].tx;
    checkActive(tx, 'openCursor');
    storeData(store);
    const r = toRange(query);
    const req = newRequest(IDBRequest, source, tx);
    const cur = make(keyOnly ? IDBCursor : IDBCursorWithValue, { source, direction, range: r, keyOnly, tx, request: req, key: undefined, primaryKey: undefined, value: undefined, gotValue: false, position: undefined, objectPosition: undefined });
    schedule(tx, req, () => iterate(cur, 1, undefined, undefined));
    return req;
  }
  function iterate(cur, count, target, targetPrimary) {
    const st = cur[S];
    const isIndex = st.source instanceof IDBIndex;
    const store = isIndex ? st.source[S].store : st.source;
    const s = storeData(store);
    const list = isIndex ? recordsIn(idxData(st.source).records, st.range).map((e) => ({ key: e.key, primaryKey: e.primaryKey })) : recordsIn(s.records, st.range).map((e) => ({ key: e.key, primaryKey: e.key }));
    const forward = st.direction.startsWith('next');
    const unique = st.direction.endsWith('unique');
    const ordered = forward ? list : list.slice().reverse();
    let found = null;
    for (const e of ordered) {
      if (st.position !== undefined) {
        const c = cmp(e.key, st.position);
        if (forward ? c < 0 : c > 0) continue;
        if (c === 0) {
          if (unique) continue;
          const pc = cmp(e.primaryKey, st.objectPosition);
          if (forward ? pc <= 0 : pc >= 0) continue;
        }
      }
      if (target !== undefined) { const c = cmp(e.key, target); if (forward ? c < 0 : c > 0) continue; if (targetPrimary !== undefined && c === 0) { const pc = cmp(e.primaryKey, targetPrimary); if (forward ? pc < 0 : pc > 0) continue; } }
      if (unique && found === null && st.position !== undefined && cmp(e.key, st.position) === 0) continue;
      if (--count > 0) { st.position = e.key; st.objectPosition = e.primaryKey; continue; }
      found = e;
      break;
    }
    if (!found) { st.key = undefined; st.primaryKey = undefined; st.value = undefined; st.gotValue = false; return null; }
    if (unique && !forward) {
      // prevunique: the first record (lowest primary key) of that key.
      const first = list.find((e) => cmp(e.key, found.key) === 0);
      found = first;
    }
    st.position = found.key; st.objectPosition = found.primaryKey;
    st.key = found.key; st.primaryKey = found.primaryKey;
    if (!st.keyOnly) { const rec = s.records[lowerBoundIdx(s.records, found.primaryKey)]; st.value = rec ? deepCopy(rec.value) : undefined; }
    st.gotValue = true;
    return cur;
  }

  // ------------------------------------------------------------- exports
  define('IDBFactory', IDBFactory);
  define('IDBDatabase', IDBDatabase);
  define('IDBTransaction', IDBTransaction);
  define('IDBObjectStore', IDBObjectStore);
  define('IDBIndex', IDBIndex);
  define('IDBCursor', IDBCursor);
  define('IDBCursorWithValue', IDBCursorWithValue);
  define('IDBKeyRange', IDBKeyRange);
  define('IDBRequest', IDBRequest);
  define('IDBOpenDBRequest', IDBOpenDBRequest);
  define('IDBRecord', IDBRecord);
  const factory = make(IDBFactory, {});
  Object.defineProperty(g, 'indexedDB', { get() { return factory; }, configurable: true, enumerable: true });
})();
