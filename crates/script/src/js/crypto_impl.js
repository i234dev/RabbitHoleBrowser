// SubtleCrypto algorithms (Web Cryptography Level 2, Secure Curves and
// Modern Algorithms). Compiled on first use by crypto.js; evaluates to a
// factory receiving the natives and the CryptoKey internal-slot accessors.
(function (env) {
  'use strict';
  const { N, makeKey, record, random, isSubtle } = env;
  const g = globalThis;
  const err = (name, msg) => new g.DOMException(msg, name);
  const dataError = (msg) => err('DataError', msg || 'The key data is invalid');
  const opError = (msg) => err('OperationError', msg || 'The operation failed');
  const accessError = (msg) => err('InvalidAccessError', msg || 'The requested operation is not valid for the provided key');
  const syntaxError = (msg) => err('SyntaxError', msg || 'Cannot create a key using the specified key usages');
  const notSupported = (msg) => err('NotSupportedError', msg || 'The operation is not supported');
  const defer = g.setTimeout;

  // ---------------------------------------------------------------- bytes
  const bytesOf = (data) => new Uint8Array(N.cryptoCopyBuffer(data));
  const ab = (u8) => u8.slice().buffer;
  const concat = (...parts) => {
    const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    let o = 0;
    for (const p of parts) { out.set(p, o); o += p.length; }
    return out;
  };
  const same = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);
  function b64u(bytes) {
    let s = '';
    for (let i = 0; i < bytes.length; i += 8192) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 8192));
    return g.btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }
  function unb64u(text) {
    if (typeof text !== 'string' || !/^[A-Za-z0-9_-]*$/.test(text) || text.length % 4 === 1) throw dataError('Invalid base64url data');
    const t = text.replace(/-/g, '+').replace(/_/g, '/');
    const bin = g.atob(t + '==='.slice((t.length + 3) % 4));
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  // A base64url key part of exactly `size` bytes in canonical form.
  function keyPart(text, size, error) {
    let bytes;
    try { bytes = unb64u(text); } catch (e) { throw error(); }
    if ((size !== undefined && bytes.length !== size) || b64u(bytes) !== text) throw error();
    return bytes;
  }
  const randomBytes = (n) => { const out = new Uint8Array(n); for (let i = 0; i < n; i += 65536) random(out.subarray(i, i + 65536)); return out; };

  // ---------------------------------------------------- Web IDL conversions
  function str(v) { return `${v}`; }
  const FORMATS = ['raw', 'spki', 'pkcs8', 'jwk', 'raw-public', 'raw-private', 'raw-seed', 'raw-secret'];
  const USAGES = ['encrypt', 'decrypt', 'sign', 'verify', 'deriveKey', 'deriveBits', 'wrapKey', 'unwrapKey',
    'encapsulateKey', 'encapsulateBits', 'decapsulateKey', 'decapsulateBits'];
  function enumOf(v, list, type) {
    const s = str(v);
    if (!list.includes(s)) throw new TypeError(`The provided value '${s}' is not a valid enum value of type ${type}.`);
    return s;
  }
  function sequence(v, convert) {
    if (v === null || (typeof v !== 'object' && typeof v !== 'function')) throw new TypeError('The provided value cannot be converted to a sequence.');
    const method = v[Symbol.iterator];
    if (typeof method !== 'function') throw new TypeError('The object must have a callable @@iterator property.');
    const out = [];
    const it = method.call(v);
    for (;;) {
      const step = it.next();
      if (step.done) break;
      out.push(convert(step.value));
    }
    return out;
  }
  const usageList = (v) => sequence(v, (u) => enumOf(u, USAGES, 'KeyUsage'));
  function enforceRange(v, max) {
    const n = +v;
    if (!Number.isFinite(n)) throw new TypeError('Value is not a finite number.');
    const x = Math.trunc(n);
    if (x < 0 || x > max) throw new TypeError('Value is outside the accepted range.');
    return x + 0;
  }
  const abLength = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get;
  const typedBuffer = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Uint8Array.prototype), 'buffer').get;
  const typedTag = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag).get;
  const viewBuffer = Object.getOwnPropertyDescriptor(DataView.prototype, 'buffer').get;
  function isArrayBuffer(v) { try { abLength.call(v); return true; } catch (e) { return false; } }
  // 'buffer' / 'view' for (non-shared) BufferSource values, null otherwise.
  function bufferKind(v) {
    if (v === null || typeof v !== 'object') return null;
    if (isArrayBuffer(v)) return 'buffer';
    let backing;
    try { backing = typedBuffer.call(v); } catch (e) { try { backing = viewBuffer.call(v); } catch (e2) { /* not a view */ } }
    if (backing !== undefined) {
      if (!isArrayBuffer(backing)) throw new TypeError('The provided ArrayBufferView value must not be shared.');
      return 'view';
    }
    if (Object.prototype.toString.call(v) === '[object SharedArrayBuffer]') throw new TypeError('The provided ArrayBuffer value must not be shared.');
    return null;
  }
  function bufferSource(v) {
    if (bufferKind(v) === null) throw new TypeError("The provided value is not of type '(ArrayBuffer or ArrayBufferView)'.");
    return v;
  }
  function cryptoKey(v) {
    if (!record(v)) throw new TypeError("The provided value is not of type 'CryptoKey'.");
    return v;
  }
  const algorithmIdentifier = (v) => (v !== null && (typeof v === 'object' || typeof v === 'function')) ? v : str(v);
  const JWK_MEMBERS = [['alg', 's'], ['crv', 's'], ['d', 's'], ['dp', 's'], ['dq', 's'], ['e', 's'], ['ext', 'b'], ['k', 's'],
    ['key_ops', 'ss'], ['kty', 's'], ['n', 's'], ['oth', 'o'], ['p', 's'], ['priv', 's'], ['pub', 's'], ['q', 's'], ['qi', 's'],
    ['use', 's'], ['x', 's'], ['y', 's']];
  function jsonWebKey(v) {
    if (v !== undefined && v !== null && typeof v !== 'object' && typeof v !== 'function') throw new TypeError('The provided value is not of type JsonWebKey.');
    const out = { __proto__: null };
    if (v === undefined || v === null) return out;
    for (const [m, t] of JWK_MEMBERS) {
      const x = v[m];
      if (x === undefined) continue;
      out[m] = t === 's' ? str(x) : t === 'b' ? !!x : t === 'ss' ? sequence(x, str) : sequence(x, (o) => o);
    }
    return out;
  }

  // ------------------------------------------------ algorithm normalization
  const BUF = 1, ULONG = 2, USHORT = 3, OCTET = 4, HASH = 5, BIGINT = 6, KEY = 7, STR = 8;
  const DICT = {
    none: [],
    AesCbcParams: [['iv', BUF, 1]],
    AesCtrParams: [['counter', BUF, 1], ['length', OCTET, 1]],
    AesGcmParams: [['additionalData', BUF, 0], ['iv', BUF, 1], ['tagLength', OCTET, 0]],
    AesKeyGenParams: [['length', USHORT, 1]],
    HmacParams: [['hash', HASH, 1], ['length', ULONG, 0]],
    RsaHashedKeyGenParams: [['modulusLength', ULONG, 1], ['publicExponent', BIGINT, 1], ['hash', HASH, 1]],
    RsaHashedImportParams: [['hash', HASH, 1]],
    RsaPssParams: [['saltLength', ULONG, 1]],
    RsaOaepParams: [['label', BUF, 0]],
    EcdsaParams: [['hash', HASH, 1]],
    EcKeyParams: [['namedCurve', STR, 1]],
    EcdhKeyDeriveParams: [['public', KEY, 1]],
    HkdfParams: [['hash', HASH, 1], ['info', BUF, 1], ['salt', BUF, 1]],
    Pbkdf2Params: [['hash', HASH, 1], ['iterations', ULONG, 1], ['salt', BUF, 1]],
    ContextParams: [['context', BUF, 0]],
    CShakeParams: [['customization', BUF, 0], ['functionName', BUF, 0], ['outputLength', ULONG, 1]],
    TurboShakeParams: [['domainSeparation', OCTET, 0], ['outputLength', ULONG, 1]],
    KangarooTwelveParams: [['customization', BUF, 0], ['outputLength', ULONG, 1]],
    KmacKeyParams: [['length', ULONG, 0]],
    KmacParams: [['customization', BUF, 0], ['outputLength', ULONG, 1]],
    Argon2Params: [['associatedData', BUF, 0], ['memory', ULONG, 1], ['nonce', BUF, 1], ['parallelism', ULONG, 1],
      ['passes', ULONG, 1], ['secretValue', BUF, 0], ['version', OCTET, 0]],
  };
  const REGISTRY = {};
  function register(ops, names, dict) {
    for (const op of ops.split(',')) {
      const table = REGISTRY[op] || (REGISTRY[op] = new Map());
      for (const name of names) table.set(name.toUpperCase(), [name, DICT[dict]]);
    }
  }
  const RSA = ['RSASSA-PKCS1-v1_5', 'RSA-PSS', 'RSA-OAEP'];
  const EC = ['ECDSA', 'ECDH'];
  const OKP = ['Ed25519', 'Ed448', 'X25519', 'X448'];
  const AES = ['AES-CTR', 'AES-CBC', 'AES-GCM', 'AES-KW', 'AES-OCB'];
  const MLKEM = ['ML-KEM-512', 'ML-KEM-768', 'ML-KEM-1024'];
  const HYBRID = ['MLKEM768-P256', 'MLKEM768-X25519', 'MLKEM1024-P384'];
  const MLDSA = ['ML-DSA-44', 'ML-DSA-65', 'ML-DSA-87'];
  const KMAC = ['KMAC128', 'KMAC256'];
  const ARGON2 = ['Argon2d', 'Argon2i', 'Argon2id'];
  const SHA = ['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512'];
  const SHA3 = ['SHA3-256', 'SHA3-384', 'SHA3-512'];
  register('encrypt,decrypt', ['RSA-OAEP'], 'RsaOaepParams');
  register('encrypt,decrypt', ['AES-CTR'], 'AesCtrParams');
  register('encrypt,decrypt', ['AES-CBC'], 'AesCbcParams');
  register('encrypt,decrypt', ['AES-GCM', 'AES-OCB', 'ChaCha20-Poly1305'], 'AesGcmParams');
  register('sign,verify', ['RSASSA-PKCS1-v1_5', 'HMAC', 'Ed25519'], 'none');
  register('sign,verify', ['RSA-PSS'], 'RsaPssParams');
  register('sign,verify', ['ECDSA'], 'EcdsaParams');
  register('sign,verify', ['Ed448', ...MLDSA], 'ContextParams');
  register('sign,verify', KMAC, 'KmacParams');
  register('digest', [...SHA, ...SHA3], 'none');
  register('digest', ['cSHAKE128', 'cSHAKE256'], 'CShakeParams');
  register('digest', ['TurboSHAKE128', 'TurboSHAKE256'], 'TurboShakeParams');
  register('digest', ['KT128', 'KT256'], 'KangarooTwelveParams');
  register('generateKey', RSA, 'RsaHashedKeyGenParams');
  register('generateKey', EC, 'EcKeyParams');
  register('generateKey', AES, 'AesKeyGenParams');
  register('generateKey', ['HMAC'], 'HmacParams');
  register('generateKey', [...OKP, ...MLDSA, ...MLKEM, ...HYBRID, 'ChaCha20-Poly1305'], 'none');
  register('generateKey', KMAC, 'KmacKeyParams');
  register('deriveBits', ['ECDH', 'X25519', 'X448'], 'EcdhKeyDeriveParams');
  register('deriveBits', ['HKDF'], 'HkdfParams');
  register('deriveBits', ['PBKDF2'], 'Pbkdf2Params');
  register('deriveBits', ARGON2, 'Argon2Params');
  register('importKey', RSA, 'RsaHashedImportParams');
  register('importKey', EC, 'EcKeyParams');
  register('importKey', ['HMAC'], 'HmacParams');
  register('importKey', [...AES, 'HKDF', 'PBKDF2', ...OKP, ...MLDSA, ...MLKEM, ...HYBRID, 'ChaCha20-Poly1305', ...ARGON2], 'none');
  register('importKey', KMAC, 'KmacKeyParams');
  register('exportKey', [...RSA, ...EC, ...AES, 'HMAC', ...OKP, ...MLDSA, ...MLKEM, ...HYBRID, 'ChaCha20-Poly1305', ...KMAC], 'none');
  register('get key length', AES, 'AesKeyGenParams');
  register('get key length', ['HMAC'], 'HmacParams');
  register('get key length', ['HKDF', 'PBKDF2', 'ChaCha20-Poly1305', ...ARGON2], 'none');
  register('get key length', KMAC, 'KmacKeyParams');
  register('wrapKey,unwrapKey', ['AES-KW'], 'none');
  register('encapsulate,decapsulate,get shared key length', [...MLKEM, ...HYBRID], 'none');

  const asciiUpper = (s) => s.replace(/[a-z]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 32));
  function normalize(alg, op) {
    if (alg === null || (typeof alg !== 'object' && typeof alg !== 'function')) alg = { name: str(alg) };
    const nameValue = alg.name;
    if (nameValue === undefined) throw new TypeError("Failed to read the 'name' property from 'Algorithm': Required member is undefined.");
    const name = str(nameValue);
    const entry = REGISTRY[op] && REGISTRY[op].get(asciiUpper(name));
    if (!entry) throw notSupported('Algorithm: Unrecognized name');
    const [registered, dict] = entry;
    // The name member is read once (as engines do); the other members of
    // the desired dictionary type are read in order below.
    const out = { name: registered };
    for (const [member, type, required] of dict) {
      const v = alg[member];
      if (v === undefined) {
        if (required) throw new TypeError(`Failed to read the '${member}' property: Required member is undefined.`);
        continue;
      }
      switch (type) {
        case BUF: out[member] = bufferSource(v); break;
        case ULONG: out[member] = enforceRange(v, 0xffffffff); break;
        case USHORT: out[member] = enforceRange(v, 0xffff); break;
        case OCTET: out[member] = enforceRange(v, 0xff); break;
        case HASH: out[member] = algorithmIdentifier(v); break;
        case BIGINT:
          if (bufferKind(v) !== 'view' || typedTag.call(v) !== 'Uint8Array') throw new TypeError("The provided value is not of type 'Uint8Array'.");
          out[member] = v;
          break;
        case KEY: out[member] = cryptoKey(v); break;
        case STR: out[member] = str(v); break;
      }
    }
    for (const [member, type] of dict) {
      if (!(member in out)) continue;
      if (type === BUF) out[member] = bytesOf(out[member]);
      else if (type === HASH) out[member] = normalize(out[member], 'digest');
      else if (type === BIGINT) out[member] = out[member].slice();
    }
    return out;
  }
  const registered = (op, name) => !!(REGISTRY[op] && REGISTRY[op].has(asciiUpper(name)));

  // ------------------------------------------------------------ key helpers
  const dedupe = (usages) => [...new Set(usages)];
  function checkUsages(usages, allowed) {
    for (const u of usages) if (!allowed.includes(u)) throw syntaxError();
  }
  const key = (type, algorithm, raw) => ({ type, algorithm, raw, extractable: false, usages: [] });
  function finish(r, extractable, usages) {
    r.extractable = !!extractable;
    r.usages = dedupe(usages);
    return makeKey(r);
  }
  function jwkCommon(jwk, usages, extractable, use, error = dataError) {
    if (usages.length && jwk.use !== undefined && jwk.use !== use) throw error('Invalid JWK "use" member');
    if (jwk.key_ops !== undefined) {
      const ops = jwk.key_ops;
      if (new Set(ops).size !== ops.length || usages.some((u) => !ops.includes(u))) throw error('Invalid JWK "key_ops" member');
    }
    if (jwk.ext === false && extractable) throw error('The JWK is not extractable');
  }
  const jwkOf = (data) => {
    if (!data || data.__jwk !== true) throw dataError('A JsonWebKey is required');
    return data.value;
  };
  const isJwkFormat = (format) => format === 'jwk';

  // ----------------------------------------------------------------- DER
  function derField(bytes, start, tag, error) {
    if (bytes[start++] !== tag || start >= bytes.length + (tag === 5 ? 1 : 0)) throw error();
    let length = bytes[start++];
    if (length === undefined) throw error();
    if (length & 128) {
      const count = length & 127;
      if (!count || count > 4 || bytes[start] === 0 || start + count > bytes.length) throw error();
      length = 0;
      for (let i = 0; i < count; ++i) length = length * 256 + bytes[start++];
      if (length < 128) throw error();
    }
    if (start + length > bytes.length) throw error();
    return { data: bytes.subarray(start, start + length), end: start + length };
  }
  function der(tag, body) {
    const len = [];
    for (let n = body.length; n; n = Math.floor(n / 256)) len.unshift(n & 255);
    return concat(Uint8Array.of(tag, ...(body.length < 128 ? [body.length] : [128 | len.length, ...len])), body);
  }
  // SubjectPublicKeyInfo / PrivateKeyInfo with an OID-only AlgorithmIdentifier.
  function encodeSpki(oid, publicKey) { return der(0x30, concat(der(0x30, oid), der(3, concat(Uint8Array.of(0), publicKey)))); }
  function encodePkcs8(oid, privateKey) { return der(0x30, concat(Uint8Array.of(2, 1, 0), der(0x30, oid), der(4, privateKey))); }
  function parseSpki(bytes, oid, error) {
    const outer = derField(bytes, 0, 0x30, error);
    if (outer.end !== bytes.length) throw error();
    const alg = derField(outer.data, 0, 0x30, error);
    if (!same(alg.data, oid)) throw error();
    const key = derField(outer.data, alg.end, 3, error);
    if (key.end !== outer.data.length || key.data[0] !== 0) throw error();
    return key.data.subarray(1);
  }
  function parsePkcs8(bytes, oid, error) {
    const outer = derField(bytes, 0, 0x30, error);
    if (outer.end !== bytes.length) throw error();
    const version = derField(outer.data, 0, 2, error);
    if (version.data.length !== 1 || version.data[0] > 1) throw error();
    const alg = derField(outer.data, version.end, 0x30, error);
    if (!same(alg.data, oid)) throw error();
    const key = derField(outer.data, alg.end, 4, error);
    if (key.end !== outer.data.length) {
      // Optional [0] attributes and [1] public key.
      let pos = key.end;
      while (pos < outer.data.length) {
        const t = outer.data[pos];
        if (t !== 0xa0 && t !== 0x81 && t !== 0xa1) throw error();
        pos = derField(outer.data, pos, t, error).end;
      }
    }
    return key.data;
  }

  // ---------------------------------------------------------- hash helpers
  const HASH_INFO = { 'SHA-1': [64, 20], 'SHA-256': [64, 32], 'SHA-384': [128, 48], 'SHA-512': [128, 64] };
  function macHash(hash) {
    if (!HASH_INFO[hash.name]) throw notSupported(`${hash.name} is not supported here`);
    return hash.name;
  }
  const hmac = (hash, keyBytes, msg) => new Uint8Array(N.cryptoHmac(hash, keyBytes, msg));

  // ------------------------------------------------------------------ AES
  const SBOX = new Uint8Array(256), INV = new Uint8Array(256);
  (() => {
    let p = 1, q = 1;
    do {
      p = p ^ ((p << 1) & 0xff) ^ (p & 0x80 ? 0x1b : 0);
      q ^= q << 1; q ^= q << 2; q ^= q << 4; q &= 0xff; if (q & 0x80) q ^= 0x09;
      const x = q ^ ((q << 1) | (q >> 7)) & 0xff ^ ((q << 2) | (q >> 6)) & 0xff ^ ((q << 3) | (q >> 5)) & 0xff ^ ((q << 4) | (q >> 4)) & 0xff;
      SBOX[p] = (x ^ 0x63) & 0xff;
    } while (p !== 1);
    SBOX[0] = 0x63;
    for (let i = 0; i < 256; i++) INV[SBOX[i]] = i;
  })();
  const xt = (a) => ((a << 1) ^ (a & 0x80 ? 0x1b : 0)) & 0xff;
  const mul = (a, b) => { let r = 0; while (b) { if (b & 1) r ^= a; a = xt(a); b >>= 1; } return r; };
  function expandKey(k) {
    const nk = k.length / 4, nr = nk + 6, w = new Uint8Array(16 * (nr + 1));
    w.set(k);
    let rcon = 1;
    for (let i = nk; i < 4 * (nr + 1); i++) {
      let t = w.slice((i - 1) * 4, i * 4);
      if (i % nk === 0) { t = new Uint8Array([SBOX[t[1]] ^ rcon, SBOX[t[2]], SBOX[t[3]], SBOX[t[0]]]); rcon = xt(rcon); }
      else if (nk > 6 && i % nk === 4) t = t.map((b) => SBOX[b]);
      for (let j = 0; j < 4; j++) w[i * 4 + j] = w[(i - nk) * 4 + j] ^ t[j];
    }
    return { w, nr };
  }
  function encBlock(ks, input) {
    const s = new Uint8Array(input);
    const add = (r) => { for (let i = 0; i < 16; i++) s[i] ^= ks.w[r * 16 + i]; };
    add(0);
    for (let r = 1; r <= ks.nr; r++) {
      for (let i = 0; i < 16; i++) s[i] = SBOX[s[i]];
      const t = s.slice();
      for (let c = 0; c < 4; c++) for (let rr = 0; rr < 4; rr++) s[c * 4 + rr] = t[((c + rr) % 4) * 4 + rr];
      if (r !== ks.nr) {
        for (let c = 0; c < 4; c++) {
          const a = s.slice(c * 4, c * 4 + 4);
          s[c * 4] = xt(a[0]) ^ xt(a[1]) ^ a[1] ^ a[2] ^ a[3];
          s[c * 4 + 1] = a[0] ^ xt(a[1]) ^ xt(a[2]) ^ a[2] ^ a[3];
          s[c * 4 + 2] = a[0] ^ a[1] ^ xt(a[2]) ^ xt(a[3]) ^ a[3];
          s[c * 4 + 3] = xt(a[0]) ^ a[0] ^ a[1] ^ a[2] ^ xt(a[3]);
        }
      }
      add(r);
    }
    return s;
  }
  function decBlock(ks, input) {
    const s = new Uint8Array(input);
    const add = (r) => { for (let i = 0; i < 16; i++) s[i] ^= ks.w[r * 16 + i]; };
    add(ks.nr);
    for (let r = ks.nr - 1; r >= 0; r--) {
      const t = s.slice();
      for (let c = 0; c < 4; c++) for (let rr = 0; rr < 4; rr++) s[((c + rr) % 4) * 4 + rr] = t[c * 4 + rr];
      for (let i = 0; i < 16; i++) s[i] = INV[s[i]];
      add(r);
      if (r !== 0) {
        for (let c = 0; c < 4; c++) {
          const a = s.slice(c * 4, c * 4 + 4);
          s[c * 4] = mul(a[0], 14) ^ mul(a[1], 11) ^ mul(a[2], 13) ^ mul(a[3], 9);
          s[c * 4 + 1] = mul(a[0], 9) ^ mul(a[1], 14) ^ mul(a[2], 11) ^ mul(a[3], 13);
          s[c * 4 + 2] = mul(a[0], 13) ^ mul(a[1], 9) ^ mul(a[2], 14) ^ mul(a[3], 11);
          s[c * 4 + 3] = mul(a[0], 11) ^ mul(a[1], 13) ^ mul(a[2], 9) ^ mul(a[3], 14);
        }
      }
    }
    return s;
  }
  function cbc(k, iv, data, decrypt) {
    if (iv.length !== 16) throw opError('The provided IV is not 16 bytes long.');
    const ks = expandKey(k);
    if (!decrypt) {
      const padLen = 16 - (data.length % 16);
      const p = new Uint8Array(data.length + padLen); p.set(data); p.fill(padLen, data.length);
      const out = new Uint8Array(p.length);
      let prev = iv;
      for (let i = 0; i < p.length; i += 16) { const b = p.slice(i, i + 16); for (let j = 0; j < 16; j++) b[j] ^= prev[j]; prev = encBlock(ks, b); out.set(prev, i); }
      return out;
    }
    if (data.length % 16 || !data.length) throw opError('The provided data is not a multiple of the block size.');
    const out = new Uint8Array(data.length);
    let prev = iv;
    for (let i = 0; i < data.length; i += 16) { const c = data.slice(i, i + 16); const b = decBlock(ks, c); for (let j = 0; j < 16; j++) b[j] ^= prev[j]; out.set(b, i); prev = c; }
    const n = out[out.length - 1];
    if (n < 1 || n > 16 || out.slice(out.length - n).some((x) => x !== n)) throw opError('The decryption failed (bad padding).');
    return out.slice(0, out.length - n);
  }
  function ctr(k, counter, data, length) {
    if (counter.length !== 16) throw opError('The counter must be 16 bytes long.');
    if (!length || length > 128) throw opError('Invalid AES-CTR counter length.');
    if (length < 53 && Math.ceil(data.length / 16) > 2 ** length) throw opError('AES-CTR counter would repeat');
    const ks = expandKey(k);
    const block = new Uint8Array(counter);
    const out = new Uint8Array(data.length);
    for (let i = 0; i < data.length; i += 16) {
      const ksb = encBlock(ks, block);
      for (let j = 0; j < 16 && i + j < data.length; j++) out[i + j] = data[i + j] ^ ksb[j];
      for (let kk = 15, bits = length; bits > 0; --kk, bits -= 8) {
        const mask = (1 << Math.min(bits, 8)) - 1;
        const low = (block[kk] + 1) & mask;
        block[kk] = (block[kk] & ~mask) | low;
        if (low) break;
      }
    }
    return out;
  }
  function aesKw(k, data, unwrap) {
    if (data.length % 8 || data.length < (unwrap ? 24 : 16)) throw opError('AES-KW data must be a multiple of 8 bytes.');
    const ks = expandKey(k);
    const IV = [0xa6, 0xa6, 0xa6, 0xa6, 0xa6, 0xa6, 0xa6, 0xa6];
    const xorT = (a, t) => { a[7] ^= t & 255; a[6] ^= (t >> 8) & 255; a[5] ^= (t >> 16) & 255; a[4] ^= (t >>> 24) & 255; };
    if (!unwrap) {
      const n = data.length / 8;
      let a = new Uint8Array(IV);
      const r = []; for (let i = 0; i < n; i++) r.push(data.slice(i * 8, i * 8 + 8));
      for (let j = 0; j < 6; j++) for (let i = 0; i < n; i++) {
        const b = encBlock(ks, concat(a, r[i]));
        a = b.slice(0, 8); xorT(a, n * j + i + 1);
        r[i] = b.slice(8);
      }
      return concat(a, ...r);
    }
    const n = data.length / 8 - 1;
    let a = data.slice(0, 8);
    const r = []; for (let i = 0; i < n; i++) r.push(data.slice(8 + i * 8, 16 + i * 8));
    for (let j = 5; j >= 0; j--) for (let i = n - 1; i >= 0; i--) {
      const aa = new Uint8Array(a); xorT(aa, n * j + i + 1);
      const b = decBlock(ks, concat(aa, r[i]));
      a = b.slice(0, 8); r[i] = b.slice(8);
    }
    if (a.some((x, i) => x !== IV[i])) throw opError('AES-KW integrity check failed.');
    return concat(...r);
  }

  const ALG = { __proto__: null };
  const SECRET_USAGES = ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey'];

  // Symmetric keys imported from raw-secret (and "raw" for the Level 2
  // algorithms) or an "oct" JWK.
  function importOct(format, keyData, extractable, usages, use, rawAllowed, jwkAlg) {
    if (format === 'raw-secret' || (format === 'raw' && rawAllowed)) return bytesOf(keyData);
    if (format !== 'jwk') throw notSupported(`Unsupported key format '${format}'`);
    const jwk = jwkOf(keyData);
    if (jwk.kty !== 'oct') throw dataError('The JWK "kty" member was not "oct"');
    if (jwk.k === undefined) throw dataError('The JWK "k" member is missing');
    const bytes = unb64u(jwk.k);
    jwkAlg(jwk, bytes);
    jwkCommon(jwk, usages, extractable, use);
    return bytes;
  }
  function exportOct(format, r, rawAllowed, alg) {
    if (format === 'raw-secret' || (format === 'raw' && rawAllowed)) return ab(r.raw);
    if (format === 'jwk') {
      const jwk = { kty: 'oct', k: b64u(r.raw) };
      if (alg) jwk.alg = alg;
      jwk.key_ops = r.usages.slice();
      jwk.ext = r.extractable;
      return jwk;
    }
    throw notSupported(`Unsupported key format '${format}'`);
  }

  for (const name of AES) {
    const suffix = name.slice(4);
    const allowed = name === 'AES-KW' ? ['wrapKey', 'unwrapKey'] : SECRET_USAGES;
    const level2 = name !== 'AES-OCB';
    ALG[name] = {
      generate(a, extractable, usages) {
        checkUsages(usages, allowed);
        if (![128, 192, 256].includes(a.length)) throw opError('AES key length must be 128, 192 or 256 bits.');
        return key('secret', { name, length: a.length }, randomBytes(a.length / 8));
      },
      importKey(format, data, a, extractable, usages) {
        checkUsages(usages, allowed);
        const bytes = importOct(format, data, extractable, usages, 'enc', level2, (jwk, b) => {
          if (![16, 24, 32].includes(b.length)) throw dataError('AES key data must be 128, 192 or 256 bits');
          if (jwk.alg !== undefined && jwk.alg !== `A${b.length * 8}${suffix}`) throw dataError('The JWK "alg" member was inconsistent');
        });
        if (![16, 24, 32].includes(bytes.length)) throw dataError('AES key data must be 128, 192 or 256 bits');
        return key('secret', { name, length: bytes.length * 8 }, bytes);
      },
      exportKey: (format, r) => exportOct(format, r, level2, `A${r.raw.length * 8}${suffix}`),
      keyLength(a) {
        if (![128, 192, 256].includes(a.length)) throw opError('AES key length must be 128, 192 or 256 bits.');
        return a.length;
      },
    };
  }
  ALG['AES-CBC'].encrypt = (a, r, data) => cbc(r.raw, a.iv, data, false);
  ALG['AES-CBC'].decrypt = (a, r, data) => cbc(r.raw, a.iv, data, true);
  ALG['AES-CTR'].encrypt = ALG['AES-CTR'].decrypt = (a, r, data) => ctr(r.raw, a.counter, data, a.length);
  function gcm(a, r, data, decrypt) {
    const tag = a.tagLength === undefined ? 128 : a.tagLength;
    if (![32, 64, 96, 104, 112, 120, 128].includes(tag)) throw opError('Invalid tag length.');
    if (decrypt && data.length < tag / 8) throw opError('The ciphertext is shorter than the tag.');
    return new Uint8Array(N.aesGcm(r.raw, a.iv, data, a.additionalData || new Uint8Array(0), tag / 8, decrypt));
  }
  ALG['AES-GCM'].encrypt = (a, r, data) => gcm(a, r, data, false);
  ALG['AES-GCM'].decrypt = (a, r, data) => gcm(a, r, data, true);
  function ocb(a, r, data, decrypt) {
    if (!a.iv.length || a.iv.length > 15) throw opError('The AES-OCB nonce must be 1 to 15 bytes.');
    const tag = a.tagLength === undefined ? 128 : a.tagLength;
    if (![64, 96, 128].includes(tag)) throw opError('Invalid tag length.');
    if (decrypt && data.length < tag / 8) throw opError('The ciphertext is shorter than the tag.');
    return new Uint8Array(N.aesOcb(r.raw, a.iv, data, a.additionalData || new Uint8Array(0), tag / 8, decrypt));
  }
  ALG['AES-OCB'].encrypt = (a, r, data) => ocb(a, r, data, false);
  ALG['AES-OCB'].decrypt = (a, r, data) => ocb(a, r, data, true);
  ALG['AES-KW'].wrap = (a, r, data) => aesKw(r.raw, data, false);
  ALG['AES-KW'].unwrap = (a, r, data) => aesKw(r.raw, data, true);

  ALG['ChaCha20-Poly1305'] = {
    generate(a, extractable, usages) {
      checkUsages(usages, SECRET_USAGES);
      return key('secret', { name: 'ChaCha20-Poly1305' }, randomBytes(32));
    },
    importKey(format, data, a, extractable, usages) {
      checkUsages(usages, SECRET_USAGES);
      const bytes = importOct(format, data, extractable, usages, 'enc', false, (jwk, b) => {
        if (b.length !== 32) throw dataError('ChaCha20-Poly1305 requires a 256-bit key');
        if (jwk.alg !== undefined && jwk.alg !== 'C20P') throw dataError('The JWK "alg" member was inconsistent');
      });
      if (bytes.length !== 32) throw dataError('ChaCha20-Poly1305 requires a 256-bit key');
      return key('secret', { name: 'ChaCha20-Poly1305' }, bytes);
    },
    exportKey: (format, r) => exportOct(format, r, false, 'C20P'),
    keyLength: () => 256,
  };
  function chacha(a, r, data, decrypt) {
    if (a.iv.length !== 12) throw opError('A 12-byte IV is required');
    if (a.tagLength !== undefined && a.tagLength !== 128) throw opError('Invalid tag length.');
    if (decrypt && data.length < 16) throw opError('The ciphertext is shorter than the tag.');
    return new Uint8Array(N.chachaPoly1305(r.raw, a.iv, data, a.additionalData || new Uint8Array(0), decrypt));
  }
  ALG['ChaCha20-Poly1305'].encrypt = (a, r, data) => chacha(a, r, data, false);
  ALG['ChaCha20-Poly1305'].decrypt = (a, r, data) => chacha(a, r, data, true);

  // ----------------------------------------------------------------- HMAC
  ALG.HMAC = {
    generate(a, extractable, usages) {
      checkUsages(usages, ['sign', 'verify']);
      const hash = macHash(a.hash);
      let length;
      if (a.length === undefined) length = HASH_INFO[hash][0] * 8;
      else if (a.length) length = a.length;
      else throw opError('HMAC key length must not be zero');
      const bytes = randomBytes(Math.ceil(length / 8));
      if (length % 8) bytes[bytes.length - 1] &= 0xff << (8 - length % 8);
      return key('secret', { name: 'HMAC', length, hash: { name: hash } }, bytes);
    },
    importKey(format, data, a, extractable, usages) {
      checkUsages(usages, ['sign', 'verify']);
      const hash = macHash(a.hash);
      const bytes = importOct(format, data, extractable, usages, 'sig', true, (jwk) => {
        if (jwk.alg !== undefined && jwk.alg !== 'HS' + hash.slice(4)) throw dataError('The JWK "alg" member was inconsistent');
      });
      let length = bytes.length * 8;
      if (!length) throw dataError('HMAC key data must not be empty');
      if (a.length !== undefined) {
        if (a.length > length || a.length <= length - 8) throw dataError('The HMAC key length is inconsistent with the key data');
        length = a.length;
      }
      return key('secret', { name: 'HMAC', length, hash: { name: hash } }, bytes);
    },
    exportKey: (format, r) => exportOct(format, r, true, 'HS' + r.algorithm.hash.name.slice(4)),
    sign: (a, r, data) => hmac(r.algorithm.hash.name, r.raw, data),
    verify: (a, r, sig, data) => N.cryptoHmac(r.algorithm.hash.name, r.raw, data, sig),
    keyLength(a) {
      if (a.length === undefined) return HASH_INFO[macHash(a.hash)][0] * 8;
      if (a.length) return a.length;
      throw new TypeError('HMAC key length must not be zero');
    },
  };

  // ------------------------------------------------------------------ KMAC
  for (const name of KMAC) {
    const level = name === 'KMAC128' ? 128 : 256;
    const jwkName = name === 'KMAC128' ? 'K128' : 'K256';
    ALG[name] = {
      generate(a, extractable, usages) {
        checkUsages(usages, ['sign', 'verify']);
        const length = a.length === undefined ? level : a.length;
        if (length % 8) throw notSupported('KMAC key lengths must be a multiple of 8 bits');
        return key('secret', { name, length }, randomBytes(length / 8));
      },
      importKey(format, data, a, extractable, usages) {
        checkUsages(usages, ['sign', 'verify']);
        const bytes = importOct(format, data, extractable, usages, 'sig', false, (jwk) => {
          if (jwk.alg !== undefined && jwk.alg !== jwkName) throw dataError('The JWK "alg" member was inconsistent');
        });
        let length = bytes.length * 8;
        if (a.length !== undefined) {
          if (a.length > length || a.length <= length - 8) throw dataError('The KMAC key length is inconsistent with the key data');
          if (a.length % 8) throw notSupported('KMAC key lengths must be a multiple of 8 bits');
          length = a.length;
        }
        return key('secret', { name, length }, bytes);
      },
      exportKey: (format, r) => exportOct(format, r, false, jwkName),
      sign(a, r, data) {
        if (a.outputLength % 8) throw notSupported('KMAC output lengths must be a multiple of 8 bits');
        return new Uint8Array(N.cryptoKmac(level, r.raw, data, a.outputLength / 8, a.customization || null));
      },
      verify(a, r, sig, data) {
        const mac = this.sign(a, r, data);
        let diff = mac.length ^ sig.length;
        for (let i = 0; i < Math.min(mac.length, sig.length); i++) diff |= mac[i] ^ sig[i];
        return diff === 0;
      },
      keyLength: (a) => a.length === undefined ? level : a.length,
    };
  }

  // ------------------------------------------------------ PBKDF2 / HKDF / Argon2
  function importDerivation(name, rawFormats) {
    return function (format, data, a, extractable, usages) {
      if (!rawFormats.includes(format)) throw notSupported(`Unsupported key format '${format}' for ${name}`);
      checkUsages(usages, ['deriveKey', 'deriveBits']);
      if (extractable) throw syntaxError(`${name} keys are not extractable.`);
      return key('secret', { name }, bytesOf(data));
    };
  }
  ALG.HKDF = {
    importKey: importDerivation('HKDF', ['raw', 'raw-secret']),
    keyLength: () => null,
    deriveBits(a, r, length) {
      if (length === null || length % 8) throw opError('The length must be a multiple of 8.');
      const hash = macHash(a.hash);
      const n = length / 8, hl = HASH_INFO[hash][1];
      if (!n) return new Uint8Array(0);
      if (n > 255 * hl) throw opError('HKDF output exceeds 255 blocks.');
      const prk = hmac(hash, a.salt.length ? a.salt : new Uint8Array(HASH_INFO[hash][0]), r.raw);
      const out = new Uint8Array(n);
      let t = new Uint8Array(0);
      for (let i = 1, off = 0; off < n; i++) {
        t = hmac(hash, prk, concat(t, a.info, Uint8Array.of(i)));
        out.set(t.subarray(0, Math.min(hl, n - off)), off);
        off += hl;
      }
      return out;
    },
  };
  ALG.PBKDF2 = {
    importKey: importDerivation('PBKDF2', ['raw', 'raw-secret']),
    keyLength: () => null,
    deriveBits(a, r, length) {
      if (length === null || length % 8) throw opError('The length must be a multiple of 8.');
      if (!a.iterations) throw opError('PBKDF2 iterations must be greater than 0.');
      const hash = macHash(a.hash);
      if (!length) return new Uint8Array(0);
      const op = N.pbkdfStart(hash, r.raw, a.salt, a.iterations, length / 8);
      const output = N.pbkdfStep(op);
      if (output) return new Uint8Array(output);
      return new Promise((resolve, reject) => {
        function next() {
          try {
            const result = N.pbkdfStep(op);
            if (result) resolve(new Uint8Array(result)); else defer(next, 0);
          } catch (error) { reject(error); }
        }
        defer(next, 0);
      });
    },
  };
  ARGON2.forEach((name, type) => {
    ALG[name] = {
      importKey: importDerivation(name, ['raw-secret']),
      keyLength: () => null,
      deriveBits(a, r, length) {
        if (length === null || length < 32 || length % 8) throw opError('Invalid Argon2 output length');
        if (a.version !== undefined && a.version !== 0x13) throw opError('Unsupported Argon2 version');
        if (!a.parallelism || a.parallelism > 0xffffff) throw opError('Invalid Argon2 parallelism');
        if (a.memory < 8 * a.parallelism) throw opError('Invalid Argon2 memory size');
        if (!a.passes) throw opError('Invalid Argon2 pass count');
        return new Uint8Array(N.argon2(type, r.raw, a.nonce, a.secretValue || null, a.associatedData || null,
          a.parallelism, a.memory, a.passes, length / 8));
      },
    };
  });

  // ------------------------------------------------------------------ RSA
  const rsaNumbers = ['n', 'e', 'd', 'p', 'q', 'dp', 'dq', 'qi'];
  const rsaIdentifier = Uint8Array.of(6, 9, 42, 134, 72, 134, 247, 13, 1, 1, 1, 5, 0);
  const rsaError = () => dataError('Invalid RSA key encoding');
  const rsaUsages = (name, priv) => name === 'RSA-OAEP' ? (priv ? ['decrypt', 'unwrapKey'] : ['encrypt', 'wrapKey']) : (priv ? ['sign'] : ['verify']);
  function rsaJwa(name, hash) {
    const size = hash.slice(4);
    if (name === 'RSA-OAEP') return hash === 'SHA-1' ? name : name + '-' + size;
    return (name === 'RSA-PSS' ? 'PS' : 'RS') + size;
  }
  function rsaIntegers(bytes, priv) {
    const seq = derField(bytes, 0, 48, rsaError);
    if (seq.end !== bytes.length) throw rsaError();
    let pos = 0;
    if (priv) {
      const version = derField(seq.data, pos, 2, rsaError);
      if (version.data.length !== 1 || version.data[0] !== 0) throw rsaError();
      pos = version.end;
    }
    const parts = [];
    for (let i = 0; i < (priv ? 8 : 2); ++i) {
      const integer = derField(seq.data, pos, 2, rsaError), b = integer.data;
      if (!b.length || b[0] >= 128 || (b[0] === 0 && (b.length === 1 || b[1] < 128))) throw rsaError();
      parts.push(b[0] === 0 ? b.subarray(1) : b);
      pos = integer.end;
    }
    if (pos !== seq.data.length) throw rsaError();
    return parts;
  }
  function rsaJwkDer(jwk, priv) {
    const parts = priv ? [Uint8Array.of(2, 1, 0)] : [];
    for (const k of rsaNumbers.slice(0, priv ? 8 : 2)) {
      const text = jwk[k];
      if (typeof text !== 'string' || !/^[A-Za-z0-9_-]+$/.test(text)) throw rsaError();
      const v = unb64u(text);
      if (!v.length || !v[0] || b64u(v) !== text) throw rsaError();
      parts.push(der(2, v[0] >= 128 ? concat(Uint8Array.of(0), v) : v));
    }
    return der(48, concat(...parts));
  }
  function unpackRsa(bytes, priv) {
    const outer = derField(bytes, 0, 48, rsaError);
    if (outer.end !== bytes.length) throw rsaError();
    let pos = 0;
    if (priv) {
      const version = derField(outer.data, pos, 2, rsaError);
      if (version.data.length !== 1 || version.data[0] !== 0) throw rsaError();
      pos = version.end;
    }
    const alg = derField(outer.data, pos, 48, rsaError);
    if (![11, 13].includes(alg.data.length) || !alg.data.every((b, i) => b === rsaIdentifier[i])) throw rsaError();
    const field = derField(outer.data, alg.end, priv ? 4 : 3, rsaError);
    if (!priv && field.data[0] !== 0) throw rsaError();
    if (field.end !== outer.data.length && (!priv || derField(outer.data, field.end, 160, rsaError).end !== outer.data.length)) throw rsaError();
    return priv ? field.data : field.data.subarray(1);
  }
  function rsaKey(name, hash, handle, priv) {
    const parts = rsaIntegers(new Uint8Array(N.rsaExport(handle, false)), false);
    const modulusLength = 8 * (parts[0].length - 1) + 32 - Math.clz32(parts[0][0]);
    return key(priv ? 'private' : 'public', { name, modulusLength, publicExponent: parts[1].slice(), hash: { name: hash } }, handle);
  }
  function rsaParams(bits, exponent) {
    let first = 0;
    while (first < exponent.length && exponent[first] === 0) first++;
    const e = exponent.subarray(first);
    return bits >= 512 && bits <= 16384 && bits % 128 === 0 && e.length && e.length <= 4 && (e[e.length - 1] & 1) && !(e.length === 1 && e[0] === 1);
  }
  for (const name of RSA) {
    ALG[name] = {
      generate(a, extractable, usages) {
        const priv = rsaUsages(name, true), pub = rsaUsages(name, false);
        checkUsages(usages, [...pub, ...priv]);
        const hash = macHash(a.hash);
        if (!rsaParams(a.modulusLength, a.publicExponent)) throw opError('Unsupported RSA generation parameters');
        const privateUsages = usages.filter((u) => priv.includes(u)), publicUsages = usages.filter((u) => pub.includes(u));
        // An empty private usage list fails after generation: skip the work.
        if (!privateUsages.length) throw syntaxError('A private key requires a usage');
        const job = N.rsaGenerate(a.modulusLength, a.publicExponent, true);
        return new Promise((resolve, reject) => {
          function poll() {
            try {
              const handle = N.rsaGenerated(job);
              if (!handle) { defer(poll, 1); return; }
              const publicHandle = N.rsaImport(N.rsaExport(handle, false), false, true);
              const publicKey = rsaKey(name, hash, publicHandle, false);
              publicKey.extractable = true; publicKey.usages = publicUsages;
              const privateKey = rsaKey(name, hash, handle, true);
              privateKey.extractable = extractable; privateKey.usages = privateUsages;
              resolve({ publicKey, privateKey });
            } catch (e) { reject(e); }
          }
          defer(poll, 1);
        });
      },
      importKey(format, data, a, extractable, usages) {
        const hash = macHash(a.hash);
        if (!['spki', 'pkcs8', 'jwk'].includes(format)) throw notSupported('Unsupported RSA key format');
        let priv, encoded;
        if (format === 'jwk') {
          const jwk = jwkOf(data);
          priv = jwk.d !== undefined;
          checkUsages(usages, rsaUsages(name, priv));
          if (jwk.kty !== 'RSA') throw rsaError();
          if (jwk.alg !== undefined && jwk.alg !== rsaJwa(name, hash)) throw rsaError();
          jwkCommon(jwk, usages, extractable, name === 'RSA-OAEP' ? 'enc' : 'sig', rsaError);
          if (jwk.oth !== undefined) throw rsaError();
          encoded = rsaJwkDer(jwk, priv);
        } else {
          priv = format === 'pkcs8';
          checkUsages(usages, rsaUsages(name, priv));
          encoded = unpackRsa(bytesOf(data), priv);
        }
        return rsaKey(name, hash, N.rsaImport(encoded, priv, true), priv);
      },
      exportKey(format, r) {
        const priv = r.type === 'private';
        if (!['spki', 'pkcs8', 'jwk'].includes(format)) throw notSupported('Unsupported RSA key format');
        if (format !== 'jwk' && (format === 'pkcs8') !== priv) throw accessError('Wrong RSA key type for this format');
        const bytes = new Uint8Array(N.rsaExport(r.raw, priv));
        if (format === 'spki') return ab(encodeSpki(rsaIdentifier, bytes));
        if (format === 'pkcs8') return ab(der(48, concat(Uint8Array.of(2, 1, 0), der(48, rsaIdentifier), der(4, bytes))));
        const parts = rsaIntegers(bytes, priv), alg = r.algorithm;
        const jwk = { kty: 'RSA', alg: rsaJwa(alg.name, alg.hash.name), key_ops: r.usages.slice(), ext: true };
        parts.forEach((v, i) => { jwk[rsaNumbers[i]] = b64u(v); });
        return jwk;
      },
      publicKey: (r) => rsaKey(name, r.algorithm.hash.name, N.rsaImport(N.rsaExport(r.raw, false), false, true), false),
    };
    if (name === 'RSA-OAEP') {
      const oaep = (op) => function (a, r, data) {
        if (r.type !== (op === 'encrypt' ? 'public' : 'private')) throw accessError();
        return new Uint8Array(N.rsaOperate(r.raw, op, r.algorithm.hash.name, null, data, a.label || new Uint8Array(0)));
      };
      ALG[name].encrypt = oaep('encrypt');
      ALG[name].decrypt = oaep('decrypt');
    } else {
      const signature = (op) => function (a, r, data, sig) {
        if (r.type !== (op === 'sign' ? 'private' : 'public')) throw accessError();
        const hash = r.algorithm.hash.name;
        const salt = name === 'RSA-PSS' ? a.saltLength : null;
        const digest = N.cryptoDigest(hash, data);
        const result = N.rsaOperate(r.raw, op, hash, salt, digest, op === 'sign' ? new Uint8Array(0) : sig);
        return op === 'sign' ? new Uint8Array(result) : result;
      };
      ALG[name].sign = signature('sign');
      ALG[name].verify = (a, r, sig, data) => signature('verify')(a, r, data, sig);
    }
  }
  // ------------------------------------------------------------------- EC
  const ecCurves = {
    __proto__: null,
    'P-256': { size: 32, oid: [0x2a, 0x86, 0x48, 0xce, 0x3d, 3, 1, 7], jwa: 'ES256' },
    'P-384': { size: 48, oid: [0x2b, 0x81, 4, 0, 0x22], jwa: 'ES384' },
    'P-521': { size: 66, oid: [0x2b, 0x81, 4, 0, 0x23], jwa: 'ES512' },
  };
  const ecError = () => dataError('Invalid elliptic curve key encoding');
  function ecCurve(a) {
    if (!ecCurves[a.namedCurve]) throw notSupported('Unsupported named curve');
    return a.namedCurve;
  }
  const ecAlgIdentifier = (curve) => Uint8Array.of(6, 7, 0x2a, 0x86, 0x48, 0xce, 0x3d, 2, 1, 6, ecCurves[curve].oid.length, ...ecCurves[curve].oid);
  function parseEcDer(bytes, curve, priv) {
    const outer = derField(bytes, 0, 0x30, ecError);
    if (outer.end !== bytes.length) throw ecError();
    let start = 0;
    if (priv) {
      const version = derField(outer.data, 0, 2, ecError);
      if (version.data.length !== 1 || version.data[0] !== 0) throw ecError();
      start = version.end;
    }
    const alg = derField(outer.data, start, 0x30, ecError), want = ecAlgIdentifier(curve);
    if (!same(alg.data, want)) throw ecError();
    const k = derField(outer.data, alg.end, priv ? 4 : 3, ecError);
    if (priv) return bytes;
    if (k.end !== outer.data.length || k.data[0] !== 0) throw ecError();
    return k.data.slice(1);
  }
  const encodeEcSpki = (curve, point) => der(0x30, concat(der(0x30, ecAlgIdentifier(curve)), der(3, concat(Uint8Array.of(0), point))));
  const ecPart = (value, size) => {
    if (typeof value !== 'string' || value.length !== Math.ceil(size * 4 / 3)) throw ecError();
    return keyPart(value, size, ecError);
  };
  const ecPrivateFromHandle = (handle) => ({ handle, point: new Uint8Array(N.ecdhExport(handle, 'public')) });
  for (const name of EC) {
    const signing = name === 'ECDSA';
    const privateOps = signing ? ['sign'] : ['deriveBits', 'deriveKey'];
    const publicOps = signing ? ['verify'] : [];
    ALG[name] = {
      generate(a, extractable, usages) {
        checkUsages(usages, [...privateOps, ...publicOps]);
        const namedCurve = ecCurve(a);
        const handle = N.ecdhKey(namedCurve, 'generate', null, true, signing);
        const privateKey = key('private', { name, namedCurve }, ecPrivateFromHandle(handle));
        privateKey.extractable = extractable; privateKey.usages = usages.filter((u) => privateOps.includes(u));
        const publicKey = key('public', { name, namedCurve }, privateKey.raw.point);
        publicKey.extractable = true; publicKey.usages = usages.filter((u) => publicOps.includes(u));
        return { publicKey, privateKey };
      },
      importKey(format, data, a, extractable, usages) {
        const curve = ecCurve(a), size = ecCurves[curve].size;
        let priv;
        if (format === 'jwk') priv = jwkOf(data).d !== undefined;
        else if (format === 'pkcs8') priv = true;
        else if (['spki', 'raw', 'raw-public'].includes(format)) priv = false;
        else throw notSupported('Unsupported EC import format');
        checkUsages(usages, priv ? privateOps : publicOps);
        let point, handle;
        if (format === 'jwk') {
          const jwk = jwkOf(data);
          if (jwk.kty !== 'EC') throw ecError();
          jwkCommon(jwk, usages, extractable, signing ? 'sig' : 'enc', ecError);
          if (signing && jwk.alg !== undefined && jwk.alg !== ecCurves[curve].jwa) throw ecError();
          if (jwk.crv !== curve) throw ecError();
          point = concat(Uint8Array.of(4), ecPart(jwk.x, size), ecPart(jwk.y, size));
          if (priv) {
            handle = N.ecdhKey(curve, 'scalar', ecPart(jwk.d, size), true, signing);
            if (!same(new Uint8Array(N.ecdhExport(handle, 'public')), point)) throw ecError();
          }
        } else if (format === 'pkcs8') {
          handle = N.ecdhKey(curve, 'pkcs8', parseEcDer(bytesOf(data), curve, true), true, signing);
        } else if (format === 'spki') {
          point = parseEcDer(bytesOf(data), curve, false);
        } else {
          point = bytesOf(data);
        }
        if (priv) return key('private', { name, namedCurve: curve }, ecPrivateFromHandle(handle));
        return key('public', { name, namedCurve: curve }, new Uint8Array(N.ecdhValidate(curve, point)));
      },
      exportKey(format, r) {
        const curve = r.algorithm.namedCurve, priv = r.type === 'private';
        if (format === 'pkcs8') {
          if (!priv) throw accessError('The EC key type cannot be exported in this format');
          return N.ecdhExport(r.raw.handle, format);
        }
        const point = priv ? r.raw.point : r.raw;
        if (format === 'raw' || format === 'raw-public' || format === 'spki') {
          if (priv) throw accessError('The EC key type cannot be exported in this format');
          return format === 'spki' ? ab(encodeEcSpki(curve, point)) : ab(point);
        }
        if (format === 'jwk') {
          const split = 1 + ecCurves[curve].size;
          const jwk = { kty: 'EC', crv: curve, x: b64u(point.subarray(1, split)), y: b64u(point.subarray(split)) };
          if (priv) jwk.d = b64u(new Uint8Array(N.ecdhExport(r.raw.handle, 'scalar')));
          jwk.key_ops = r.usages.slice();
          jwk.ext = r.extractable;
          return jwk;
        }
        throw notSupported('Unsupported EC export format');
      },
      publicKey: (r) => key('public', r.algorithm, r.raw.point),
    };
  }
  ALG.ECDSA.sign = (a, r, data) => {
    if (r.type !== 'private') throw accessError();
    return new Uint8Array(N.ecdsaSign(r.raw.handle, N.cryptoDigest(macHash(a.hash), data)));
  };
  ALG.ECDSA.verify = (a, r, sig, data) => {
    if (r.type !== 'public') throw accessError();
    return N.ecdsaVerify(encodeEcSpki(r.algorithm.namedCurve, r.raw), N.cryptoDigest(macHash(a.hash), data), sig);
  };
  function agreementBits(secret, length) {
    if (length === null) return secret;
    if (length > secret.length * 8) throw opError('Requested length exceeds the shared secret');
    const out = secret.slice(0, Math.ceil(length / 8));
    if (length % 8) out[out.length - 1] &= 0xff << (8 - length % 8);
    return out;
  }
  function peerKey(a, r) {
    const peer = record(a.public);
    if (r.type !== 'private') throw accessError();
    if (peer.type !== 'public') throw accessError('The public member must be a public key');
    if (peer.algorithm.name !== r.algorithm.name) throw accessError('Key agreement requires keys of the same algorithm');
    if (peer.algorithm.namedCurve !== r.algorithm.namedCurve) throw accessError('Key agreement requires keys on the same curve');
    return peer;
  }
  ALG.ECDH.deriveBits = (a, r, length) => {
    const peer = peerKey(a, r);
    return agreementBits(new Uint8Array(N.ecdhDerive(r.raw.handle, peer.raw)), length);
  };

  // -------------------------------------- Ed25519 / Ed448 / X25519 / X448
  const OKP_INFO = {
    Ed25519: { size: 32, oid: 0x70, sign: true },
    Ed448: { size: 57, oid: 0x71, sign: true },
    X25519: { size: 32, oid: 0x6e, sign: false },
    X448: { size: 56, oid: 0x6f, sign: false },
  };
  const okpError = () => dataError('Invalid key encoding');
  function okpPublic(name, secret) {
    switch (name) {
      case 'Ed25519': return new Uint8Array(N.ed25519KeyBytes(N.ed25519Key(secret, null, false), false));
      case 'X25519': return new Uint8Array(N.x25519KeyBytes(N.x25519Key(secret, null, false), false));
      case 'Ed448': return new Uint8Array(N.ed448Public(secret));
      default: return new Uint8Array(N.x448(secret, null));
    }
  }
  for (const name of OKP) {
    const info = OKP_INFO[name];
    const privateOps = info.sign ? ['sign'] : ['deriveBits', 'deriveKey'];
    const publicOps = info.sign ? ['verify'] : [];
    const oid = Uint8Array.of(6, 3, 0x2b, 0x65, info.oid);
    const privateKey = (secret, pub) => key('private', { name }, { secret, pub: pub || okpPublic(name, secret) });
    ALG[name] = {
      generate(a, extractable, usages) {
        checkUsages(usages, [...privateOps, ...publicOps]);
        const priv = privateKey(randomBytes(info.size));
        priv.extractable = extractable; priv.usages = usages.filter((u) => privateOps.includes(u));
        const pub = key('public', { name }, priv.raw.pub);
        pub.extractable = true; pub.usages = usages.filter((u) => publicOps.includes(u));
        return { publicKey: pub, privateKey: priv };
      },
      importKey(format, data, a, extractable, usages) {
        let priv;
        if (format === 'jwk') priv = jwkOf(data).d !== undefined;
        else if (format === 'pkcs8' || format === 'raw-seed') priv = true;
        else if (['spki', 'raw', 'raw-public'].includes(format)) priv = false;
        else throw notSupported('Unsupported key format');
        checkUsages(usages, priv ? privateOps : publicOps);
        if (format === 'jwk') {
          const jwk = jwkOf(data);
          if (jwk.kty !== 'OKP') throw okpError();
          if (jwk.crv !== name) throw okpError();
          if (info.sign && jwk.alg !== undefined && jwk.alg !== 'EdDSA' && jwk.alg !== name) throw okpError();
          jwkCommon(jwk, usages, extractable, info.sign ? 'sig' : 'enc', okpError);
          const pub = keyPart(jwk.x, info.size, okpError);
          if (!priv) return key('public', { name }, pub);
          const secret = keyPart(jwk.d, info.size, okpError);
          const r = privateKey(secret);
          if (!same(r.raw.pub, pub)) throw okpError();
          return r;
        }
        let bytes = bytesOf(data);
        if (format === 'spki') {
          bytes = parseSpki(bytes, oid, okpError);
        } else if (format === 'pkcs8') {
          const inner = parsePkcs8(bytes, oid, okpError);
          const octets = derField(inner, 0, 4, okpError);
          if (octets.end !== inner.length) throw okpError();
          bytes = octets.data;
        }
        if (bytes.length !== info.size) throw okpError();
        return priv ? privateKey(bytes.slice()) : key('public', { name }, bytes.slice());
      },
      exportKey(format, r) {
        const priv = r.type === 'private';
        const pub = priv ? r.raw.pub : r.raw;
        if (format === 'jwk') {
          const jwk = { kty: 'OKP', crv: name, x: b64u(pub) };
          if (info.sign) jwk.alg = name;
          if (priv) jwk.d = b64u(r.raw.secret);
          jwk.key_ops = r.usages.slice();
          jwk.ext = r.extractable;
          return jwk;
        }
        if ((format === 'raw' || format === 'raw-public' || format === 'spki') && !priv) {
          return format === 'spki' ? ab(der(0x30, concat(der(0x30, oid), der(3, concat(Uint8Array.of(0), pub))))) : ab(pub);
        }
        if (format === 'raw-seed' && priv) return ab(r.raw.secret);
        if (format === 'pkcs8' && priv) return ab(encodePkcs8(oid, der(4, r.raw.secret)));
        if (['raw', 'raw-public', 'spki', 'raw-seed', 'pkcs8'].includes(format)) throw accessError('The key type cannot be exported in this format');
        throw notSupported('Unsupported key format');
      },
      publicKey: (r) => key('public', { name }, r.raw.pub),
    };
  }
  ALG.Ed25519.sign = (a, r, data) => {
    if (r.type !== 'private') throw accessError();
    return new Uint8Array(N.ed25519Sign(N.ed25519Key(r.raw.secret, null, false), data));
  };
  ALG.Ed25519.verify = (a, r, sig, data) => {
    if (r.type !== 'public') throw accessError();
    return N.ed25519Verify(r.raw, data, sig);
  };
  ALG.Ed448.sign = (a, r, data) => {
    if (a.context && a.context.length > 255) throw opError('The context must be at most 255 bytes');
    if (r.type !== 'private') throw accessError();
    return new Uint8Array(N.ed448Sign(r.raw.secret, data, a.context || null));
  };
  ALG.Ed448.verify = (a, r, sig, data) => {
    if (a.context && a.context.length > 255) throw opError('The context must be at most 255 bytes');
    if (r.type !== 'public') throw accessError();
    return N.ed448Verify(r.raw, data, sig, a.context || null);
  };
  function montgomery(name) {
    return (a, r, length) => {
      const peer = peerKey(a, r);
      const secret = name === 'X25519' ? new Uint8Array(N.x25519Derive(N.x25519Key(r.raw.secret, null, false), peer.raw))
        : new Uint8Array(N.x448(r.raw.secret, peer.raw));
      if (secret.every((b) => b === 0)) throw opError('The peer contributed no key material');
      return agreementBits(secret, length);
    };
  }
  ALG.X25519.deriveBits = montgomery('X25519');
  ALG.X448.deriveBits = montgomery('X448');

  // ------------------------------------- ML-KEM, hybrid KEMs and ML-DSA
  const PQ = {
    'ML-KEM-512': { oid: 0x401, seed: 64, pub: 800, kem: true },
    'ML-KEM-768': { oid: 0x402, seed: 64, pub: 1184, kem: true },
    'ML-KEM-1024': { oid: 0x403, seed: 64, pub: 1568, kem: true },
    'MLKEM768-P256': { seed: 32, pub: 1249, kem: true },
    'MLKEM768-X25519': { seed: 32, pub: 1216, kem: true },
    'MLKEM1024-P384': { seed: 32, pub: 1665, kem: true },
    'ML-DSA-44': { oid: 0x311, seed: 32, pub: 1312 },
    'ML-DSA-65': { oid: 0x312, seed: 32, pub: 1952 },
    'ML-DSA-87': { oid: 0x313, seed: 32, pub: 2592 },
  };
  for (const name of Object.keys(PQ)) {
    const info = PQ[name];
    const privateOps = info.kem ? ['decapsulateKey', 'decapsulateBits'] : ['sign'];
    const publicOps = info.kem ? ['encapsulateKey', 'encapsulateBits'] : ['verify'];
    const oid = info.oid && Uint8Array.of(6, 9, 0x60, 0x86, 0x48, 1, 0x65, 3, 4, info.oid >> 8, info.oid & 0xff);
    const formats = oid ? ['spki', 'pkcs8', 'raw-public', 'raw-seed', 'jwk'] : ['raw-public', 'raw-seed', 'jwk'];
    const privateKey = (seed) => key('private', { name }, { seed, pub: new Uint8Array(N.pqPublic(name, seed)) });
    ALG[name] = {
      generate(a, extractable, usages) {
        checkUsages(usages, [...privateOps, ...publicOps]);
        const priv = privateKey(randomBytes(info.seed));
        priv.extractable = extractable; priv.usages = usages.filter((u) => privateOps.includes(u));
        const pub = key('public', { name }, priv.raw.pub);
        pub.extractable = true; pub.usages = usages.filter((u) => publicOps.includes(u));
        return { publicKey: pub, privateKey: priv };
      },
      importKey(format, data, a, extractable, usages) {
        if (!formats.includes(format)) throw notSupported('Unsupported key format');
        if (format === 'jwk') {
          const jwk = jwkOf(data);
          const priv = jwk.priv !== undefined;
          checkUsages(usages, priv ? privateOps : publicOps);
          if (jwk.kty !== 'AKP') throw dataError('The JWK "kty" member was not "AKP"');
          if (jwk.alg !== name) throw dataError('The JWK "alg" member was inconsistent');
          jwkCommon(jwk, usages, extractable, info.kem ? 'enc' : 'sig');
          if (priv) {
            const r = privateKey(keyPart(jwk.priv, info.seed, dataError));
            if (jwk.pub !== b64u(r.raw.pub)) throw dataError('The JWK "pub" member does not match the private key');
            return r;
          }
          return key('public', { name }, keyPart(jwk.pub, info.pub, dataError));
        }
        const priv = format === 'pkcs8' || format === 'raw-seed';
        checkUsages(usages, priv ? privateOps : publicOps);
        let bytes = bytesOf(data);
        if (format === 'spki') {
          bytes = parseSpki(bytes, oid, dataError);
        } else if (format === 'pkcs8') {
          const inner = parsePkcs8(bytes, oid, dataError);
          if (inner[0] === 0x04) throw notSupported('Expanded private keys are not supported');
          if (inner[0] === 0x30) {
            // seed and expanded key: the seed defines the key; the expanded
            // form must belong to it.
            const both = derField(inner, 0, 0x30, dataError);
            if (both.end !== inner.length) throw dataError();
            const seed = derField(both.data, 0, 4, dataError);
            const expanded = derField(both.data, seed.end, 4, dataError);
            if (expanded.end !== both.data.length || seed.data.length !== info.seed) throw dataError();
            bytes = seed.data;
          } else {
            const seed = derField(inner, 0, 0x80, dataError);
            if (seed.end !== inner.length) throw dataError();
            bytes = seed.data;
          }
          if (bytes.length !== info.seed) throw dataError('Invalid seed length');
        } else if (format === 'raw-seed') {
          if (bytes.length !== info.seed) throw dataError('Invalid seed length');
        } else if (!oid && bytes.length !== info.pub) {
          throw dataError('Invalid public key length');
        }
        return priv ? privateKey(bytes.slice()) : key('public', { name }, bytes.slice());
      },
      exportKey(format, r) {
        const priv = r.type === 'private';
        if (!formats.includes(format)) throw notSupported('Unsupported key format');
        if (format === 'jwk') {
          const jwk = { kty: 'AKP', alg: name, pub: b64u(priv ? r.raw.pub : r.raw) };
          if (priv) jwk.priv = b64u(r.raw.seed);
          jwk.key_ops = r.usages.slice();
          jwk.ext = r.extractable;
          return jwk;
        }
        if ((format === 'spki' || format === 'raw-public') !== !priv) throw accessError('The key type cannot be exported in this format');
        if (format === 'raw-public') return ab(r.raw);
        if (format === 'raw-seed') return ab(r.raw.seed);
        if (format === 'spki') return ab(encodeSpki(oid, r.raw));
        return ab(encodePkcs8(oid, der(0x80, r.raw.seed)));
      },
      publicKey: (r) => key('public', { name }, r.raw.pub),
    };
    if (info.kem) {
      ALG[name].encapsulate = (a, r) => {
        if (r.type !== 'public') throw accessError();
        const out = new Uint8Array(N.pqEncaps(name, r.raw));
        return { sharedKey: out.slice(out.length - 32), ciphertext: out.slice(0, out.length - 32) };
      };
      ALG[name].decapsulate = (a, r, ct) => {
        if (r.type !== 'private') throw accessError();
        return new Uint8Array(N.pqDecaps(name, r.raw.seed, ct));
      };
      ALG[name].sharedKeyLength = () => 256;
    } else {
      ALG[name].sign = (a, r, data) => {
        if (a.context && a.context.length > 255) throw opError('The context must be at most 255 bytes');
        if (r.type !== 'private') throw accessError();
        return new Uint8Array(N.pqSign(name, r.raw.seed, data, a.context || new Uint8Array(0)));
      };
      ALG[name].verify = (a, r, sig, data) => {
        if (a.context && a.context.length > 255) throw opError('The context must be at most 255 bytes');
        if (r.type !== 'public') throw accessError();
        return N.pqVerify(name, r.raw, data, sig, a.context || new Uint8Array(0));
      };
    }
  }

  // --------------------------------------------------------------- digests
  const DIGEST = { __proto__: null };
  for (const name of SHA) DIGEST[name] = (a, data) => new Uint8Array(N.cryptoDigest(name, data));
  for (const name of SHA3) DIGEST[name] = (a, data) => new Uint8Array(N.cryptoKeccak(name, data, 0));
  for (const name of ['cSHAKE128', 'cSHAKE256']) {
    DIGEST[name] = (a, data) => {
      if (a.outputLength % 8) throw notSupported('The output length must be a multiple of 8 bits');
      return new Uint8Array(N.cryptoKeccak(name, data, a.outputLength / 8, a.functionName || null, a.customization || null));
    };
  }
  for (const name of ['TurboSHAKE128', 'TurboSHAKE256']) {
    DIGEST[name] = (a, data) => {
      if (!a.outputLength || a.outputLength % 8) throw opError('The output length must be a positive multiple of 8 bits');
      const d = a.domainSeparation === undefined ? 0x1f : a.domainSeparation;
      if (d < 1 || d > 0x7f) throw opError('Invalid domain separation byte');
      return new Uint8Array(N.cryptoKeccak(name, data, a.outputLength / 8, d));
    };
  }
  for (const name of ['KT128', 'KT256']) {
    DIGEST[name] = (a, data) => {
      if (!a.outputLength || a.outputLength % 8) throw opError('The output length must be a positive multiple of 8 bits');
      return new Uint8Array(N.cryptoKeccak(name, data, a.outputLength / 8, a.customization || null));
    };
  }

  // --------------------------------------------------------------- methods
  const toBuffer = (v) => v instanceof Uint8Array ? ab(v) : v;
  function settle(value, map) {
    if (value && typeof value.then === 'function') return value.then(map);
    return map(value);
  }
  const algorithmOf = (name) => {
    const handler = ALG[name];
    if (!handler) throw notSupported();
    return handler;
  };
  function checkKeyFor(r, name, usage) {
    if (r.algorithm.name !== name) throw accessError('The requested operation is not valid for the provided key');
    if (!r.usages.includes(usage)) throw accessError(`The key does not allow the '${usage}' operation`);
  }
  function emptyUsageCheck(r) {
    if ((r.type === 'secret' || r.type === 'private') && !r.usages.length) throw syntaxError('Usages cannot be empty when creating a key.');
  }
  function importOp(format, keyData, normalized, extractable, usages) {
    const handler = algorithmOf(normalized.name);
    if (!handler.importKey) throw notSupported();
    const r = handler.importKey(format, keyData, normalized, extractable, usages);
    r.extractable = extractable;
    r.usages = dedupe(usages);
    emptyUsageCheck(r);
    return makeKey(r);
  }
  function exportOp(format, k) {
    const r = record(k);
    if (!registered('exportKey', r.algorithm.name)) throw notSupported('The key algorithm does not support export');
    if (!r.extractable) throw accessError('key is not extractable');
    return ALG[r.algorithm.name].exportKey(format, r);
  }
  function keyDataFor(format, keyData) {
    if (format === 'jwk') {
      if (keyData.__jwk !== true) throw new TypeError('A JsonWebKey is required for the jwk format');
      return keyData;
    }
    if (keyData.__jwk === true) throw new TypeError('A BufferSource is required for this key format');
    return bytesOf(keyData.value);
  }
  // keyData union: BufferSource or JsonWebKey.
  function keyDataArg(v) {
    if (bufferKind(v) !== null) return { __jwk: false, value: v };
    return { __jwk: true, value: jsonWebKey(v) };
  }
  function parseJwk(bytes) {
    let parsed;
    try { parsed = JSON.parse(new TextDecoder().decode(bytes)); } catch (e) { throw dataError('The unwrapped key is not a valid JWK'); }
    if (parsed === null || typeof parsed !== 'object') throw dataError('The unwrapped key is not a valid JWK');
    const jwk = jsonWebKey(parsed);
    if (jwk.kty === undefined) throw dataError('The JWK "kty" member is missing');
    return { __jwk: true, value: jwk };
  }

  const ARITY = { encrypt: 3, decrypt: 3, sign: 3, verify: 4, digest: 2, generateKey: 3, deriveKey: 5, deriveBits: 2,
    importKey: 5, exportKey: 2, wrapKey: 4, unwrapKey: 7, encapsulateKey: 5, encapsulateBits: 2, decapsulateKey: 6,
    decapsulateBits: 3, getPublicKey: 2 };
  const METHODS = {
    encrypt: (alg, k, data) => cipherOp('encrypt', alg, k, data),
    decrypt: (alg, k, data) => cipherOp('decrypt', alg, k, data),
    sign(algorithm, k, data) {
      const a = normalize(algorithm, 'sign');
      const bytes = bytesOf(data), r = record(k);
      checkKeyFor(r, a.name, 'sign');
      return settle(ALG[a.name].sign(a, r, bytes), toBuffer);
    },
    verify(algorithm, k, signature, data) {
      const a = normalize(algorithm, 'verify');
      const sig = bytesOf(signature), bytes = bytesOf(data), r = record(k);
      checkKeyFor(r, a.name, 'verify');
      return ALG[a.name].verify(a, r, sig, bytes);
    },
    digest(algorithm, data) {
      const a = normalize(algorithm, 'digest');
      return ab(DIGEST[a.name](a, bytesOf(data)));
    },
    generateKey(algorithm, extractable, usages) {
      const a = normalize(algorithm, 'generateKey');
      const result = algorithmOf(a.name).generate(a, extractable, usages);
      return settle(result, (res) => {
        if (res.privateKey) {
          res.privateKey.usages = dedupe(res.privateKey.usages);
          res.publicKey.usages = dedupe(res.publicKey.usages);
          emptyUsageCheck(res.privateKey);
          return { publicKey: makeKey(res.publicKey), privateKey: makeKey(res.privateKey) };
        }
        res.extractable = extractable;
        res.usages = dedupe(usages);
        emptyUsageCheck(res);
        return makeKey(res);
      });
    },
    deriveKey(algorithm, baseKey, derivedKeyType, extractable, usages) {
      const a = normalize(algorithm, 'deriveBits');
      const importAlg = normalize(derivedKeyType, 'importKey');
      const lengthAlg = normalize(derivedKeyType, 'get key length');
      const r = record(baseKey);
      checkKeyFor(r, a.name, 'deriveKey');
      const length = algorithmOf(lengthAlg.name).keyLength(lengthAlg);
      return settle(ALG[a.name].deriveBits(a, r, length), (secret) =>
        importOp('raw-secret', secret, importAlg, extractable, usages));
    },
    deriveBits(algorithm, baseKey, length) {
      const a = normalize(algorithm, 'deriveBits');
      const r = record(baseKey);
      checkKeyFor(r, a.name, 'deriveBits');
      return settle(ALG[a.name].deriveBits(a, r, length), toBuffer);
    },
    importKey(format, keyData, algorithm, extractable, usages) {
      const a = normalize(algorithm, 'importKey');
      const data = keyDataFor(format, keyData);
      return importOp(format, data, a, extractable, usages);
    },
    exportKey: (format, k) => exportOp(format, k),
    wrapKey(format, k, wrappingKey, wrapAlgorithm) {
      let a, wrap = true;
      try { a = normalize(wrapAlgorithm, 'wrapKey'); } catch (e) { a = normalize(wrapAlgorithm, 'encrypt'); wrap = false; }
      checkKeyFor(record(wrappingKey), a.name, 'wrapKey');
      const exported = exportOp(format, k);
      const bytes = format === 'jwk' ? new TextEncoder().encode(JSON.stringify(exported)) : new Uint8Array(exported);
      const handler = ALG[a.name];
      return toBuffer(wrap ? handler.wrap(a, record(wrappingKey), bytes) : handler.encrypt(a, record(wrappingKey), bytes));
    },
    unwrapKey(format, wrappedKey, unwrappingKey, unwrapAlgorithm, unwrappedKeyAlgorithm, extractable, usages) {
      let a, unwrap = true;
      try { a = normalize(unwrapAlgorithm, 'unwrapKey'); } catch (e) { a = normalize(unwrapAlgorithm, 'decrypt'); unwrap = false; }
      const importAlg = normalize(unwrappedKeyAlgorithm, 'importKey');
      const wrapped = bytesOf(wrappedKey);
      const r = record(unwrappingKey);
      checkKeyFor(r, a.name, 'unwrapKey');
      const handler = ALG[a.name];
      const bytes = unwrap ? handler.unwrap(a, r, wrapped) : handler.decrypt(a, r, wrapped);
      const data = format === 'jwk' ? parseJwk(bytes) : bytes;
      return importOp(format, data, importAlg, extractable, usages);
    },
    encapsulateKey(algorithm, k, sharedKeyAlgorithm, extractable, usages) {
      const a = normalize(algorithm, 'encapsulate');
      const importAlg = normalize(sharedKeyAlgorithm, 'importKey');
      const r = record(k);
      checkKeyFor(r, a.name, 'encapsulateKey');
      const bits = ALG[a.name].encapsulate(a, r);
      return { sharedKey: importOp('raw-secret', bits.sharedKey, importAlg, extractable, usages), ciphertext: ab(bits.ciphertext) };
    },
    encapsulateBits(algorithm, k) {
      const a = normalize(algorithm, 'encapsulate');
      const r = record(k);
      checkKeyFor(r, a.name, 'encapsulateBits');
      const bits = ALG[a.name].encapsulate(a, r);
      return { sharedKey: ab(bits.sharedKey), ciphertext: ab(bits.ciphertext) };
    },
    decapsulateKey(algorithm, k, ciphertext, sharedKeyAlgorithm, extractable, usages) {
      const a = normalize(algorithm, 'decapsulate');
      const importAlg = normalize(sharedKeyAlgorithm, 'importKey');
      const ct = bytesOf(ciphertext);
      const r = record(k);
      checkKeyFor(r, a.name, 'decapsulateKey');
      const secret = ALG[a.name].decapsulate(a, r, ct);
      return importOp('raw-secret', secret, importAlg, extractable, usages);
    },
    decapsulateBits(algorithm, k, ciphertext) {
      const a = normalize(algorithm, 'decapsulate');
      const ct = bytesOf(ciphertext);
      const r = record(k);
      checkKeyFor(r, a.name, 'decapsulateBits');
      return ab(ALG[a.name].decapsulate(a, r, ct));
    },
    getPublicKey(k, usages) {
      const r = record(k);
      const handler = ALG[r.algorithm.name];
      if (!handler || !handler.publicKey) throw notSupported('The algorithm cannot derive a public key');
      if (r.type !== 'private') throw accessError('A private key is required');
      const publicOps = ['encrypt', 'verify', 'wrapKey', 'encapsulateKey', 'encapsulateBits'];
      checkUsages(usages, publicOps.filter((u) => supportsUsage(r.algorithm.name, u)));
      const pub = handler.publicKey(r);
      pub.algorithm = r.algorithm;
      pub.extractable = true;
      pub.usages = dedupe(usages);
      return makeKey(pub);
    },
  };
  function supportsUsage(name, usage) {
    if (RSA.includes(name)) return rsaUsages(name, false).includes(usage);
    if (name === 'ECDSA' || name === 'Ed25519' || name === 'Ed448' || MLDSA.includes(name)) return usage === 'verify';
    if (MLKEM.includes(name) || HYBRID.includes(name)) return usage === 'encapsulateKey' || usage === 'encapsulateBits';
    return false;
  }
  function cipherOp(op, algorithm, k, data) {
    const a = normalize(algorithm, op);
    const bytes = bytesOf(data), r = record(k);
    checkKeyFor(r, a.name, op);
    return toBuffer(ALG[a.name][op](a, r, bytes));
  }

  // Web IDL argument conversion for each method, in argument order.
  const CONVERT = {
    encrypt: [algorithmIdentifier, cryptoKey, bufferSource],
    decrypt: [algorithmIdentifier, cryptoKey, bufferSource],
    sign: [algorithmIdentifier, cryptoKey, bufferSource],
    verify: [algorithmIdentifier, cryptoKey, bufferSource, bufferSource],
    digest: [algorithmIdentifier, bufferSource],
    generateKey: [algorithmIdentifier, (v) => !!v, usageList],
    deriveKey: [algorithmIdentifier, cryptoKey, algorithmIdentifier, (v) => !!v, usageList],
    deriveBits: [algorithmIdentifier, cryptoKey, (v) => v === undefined || v === null ? null : enforceRange(v, 0xffffffff)],
    importKey: [(v) => enumOf(v, FORMATS, 'KeyFormat'), keyDataArg, algorithmIdentifier, (v) => !!v, usageList],
    exportKey: [(v) => enumOf(v, FORMATS, 'KeyFormat'), cryptoKey],
    wrapKey: [(v) => enumOf(v, FORMATS, 'KeyFormat'), cryptoKey, cryptoKey, algorithmIdentifier],
    unwrapKey: [(v) => enumOf(v, FORMATS, 'KeyFormat'), bufferSource, cryptoKey, algorithmIdentifier, algorithmIdentifier, (v) => !!v, usageList],
    encapsulateKey: [algorithmIdentifier, cryptoKey, algorithmIdentifier, (v) => !!v, usageList],
    encapsulateBits: [algorithmIdentifier, cryptoKey],
    decapsulateKey: [algorithmIdentifier, cryptoKey, bufferSource, algorithmIdentifier, (v) => !!v, usageList],
    decapsulateBits: [algorithmIdentifier, cryptoKey, bufferSource],
    getPublicKey: [cryptoKey, usageList],
  };
  function method(name, self, args) {
    try {
      if (!isSubtle(self)) throw new TypeError("Illegal invocation: 'this' is not a SubtleCrypto");
      if (args.length < ARITY[name]) throw new TypeError(`Failed to execute '${name}' on 'SubtleCrypto': ${ARITY[name]} arguments required, but only ${args.length} present.`);
      const converters = CONVERT[name];
      const values = converters.map((convert, i) => convert(args[i]));
      return Promise.resolve(METHODS[name](...values));
    } catch (e) {
      return Promise.reject(e);
    }
  }

  // ------------------------------------------------------------- supports
  const OPERATIONS = ['encrypt', 'decrypt', 'sign', 'verify', 'digest', 'generateKey', 'deriveKey', 'deriveBits', 'importKey',
    'exportKey', 'wrapKey', 'unwrapKey', 'encapsulateKey', 'encapsulateBits', 'decapsulateKey', 'decapsulateBits', 'getPublicKey'];
  // Run the steps of an operation that do not need a key or data.
  function supportedSteps(op, a, length) {
    const name = a.name;
    switch (op) {
      case 'encrypt': case 'decrypt':
        if (name === 'AES-CBC') return a.iv.length === 16;
        if (name === 'AES-CTR') return a.counter.length === 16 && a.length > 0 && a.length <= 128;
        if (name === 'AES-GCM') return a.tagLength === undefined || [32, 64, 96, 104, 112, 120, 128].includes(a.tagLength);
        if (name === 'AES-OCB') return a.iv.length > 0 && a.iv.length <= 15 && (a.tagLength === undefined || [64, 96, 128].includes(a.tagLength));
        if (name === 'ChaCha20-Poly1305') return a.iv.length === 12 && (a.tagLength === undefined || a.tagLength === 128);
        return true;
      case 'sign': case 'verify':
        if (a.context && a.context.length > 255) return false;
        if (KMAC.includes(name)) return a.outputLength % 8 === 0;
        if (name === 'ECDSA') return !!HASH_INFO[a.hash.name];
        return true;
      case 'digest':
        if (name.startsWith('cSHAKE')) return a.outputLength % 8 === 0;
        if (name.startsWith('TurboSHAKE')) {
          const d = a.domainSeparation === undefined ? 0x1f : a.domainSeparation;
          return a.outputLength > 0 && a.outputLength % 8 === 0 && d >= 1 && d <= 0x7f;
        }
        if (name.startsWith('KT')) return a.outputLength > 0 && a.outputLength % 8 === 0;
        return true;
      case 'generateKey':
        try { settle(ALG[name].generate(a, false, []), () => {}); } catch (e) { return e.name === 'SyntaxError'; }
        return true;
      case 'importKey':
        if (a.hash && !HASH_INFO[a.hash.name]) return false;
        if (name === 'HMAC' && a.length === 0) return false;
        if (EC.includes(name)) return !!ecCurves[a.namedCurve];
        if (KMAC.includes(name)) return a.length === undefined || a.length % 8 === 0;
        return true;
      case 'deriveBits':
        if (name === 'HKDF' || name === 'PBKDF2') {
          if (!HASH_INFO[a.hash.name] || length === null || length % 8) return false;
          return name === 'HKDF' ? length <= 255 * HASH_INFO[a.hash.name][1] * 8 : a.iterations > 0;
        }
        if (ARGON2.includes(name)) {
          return length !== null && length >= 32 && length % 8 === 0 && (a.version === undefined || a.version === 0x13) &&
            a.parallelism > 0 && a.parallelism <= 0xffffff && a.memory >= 8 * a.parallelism && a.passes > 0;
        }
        {
          const peer = record(a.public);
          if (peer.type !== 'public' || peer.algorithm.name !== name) return false;
          if (length !== null && name !== 'ECDH' && length > (name === 'X448' ? 448 : 256)) return false;
          if (length !== null && name === 'ECDH') return length <= ecCurves[peer.algorithm.namedCurve].size * 8;
          return true;
        }
      case 'get key length':
        try { algorithmOf(name).keyLength(a); } catch (e) { return false; }
        return true;
      default:
        return true;
    }
  }
  function checkSupport(op, alg, length) {
    if (op === 'encapsulateKey' || op === 'encapsulateBits') op = 'encapsulate';
    if (op === 'decapsulateKey' || op === 'decapsulateBits') op = 'decapsulate';
    if (op === 'getPublicKey') {
      let a;
      try { a = normalize(alg, 'exportKey'); } catch (e) { return false; }
      return !!(ALG[a.name] && ALG[a.name].publicKey);
    }
    let a;
    try { a = normalize(alg, op); } catch (e) {
      if (op === 'wrapKey') return checkSupport('encrypt', alg, length);
      if (op === 'unwrapKey') return checkSupport('decrypt', alg, length);
      return false;
    }
    try { return supportedSteps(op, a, length); } catch (e) { return false; }
  }
  function supports(args) {
    if (args.length < 2) throw new TypeError(`Failed to execute 'supports' on 'SubtleCrypto': 2 arguments required, but only ${args.length} present.`);
    const operation = str(args[0]);
    const algorithm = algorithmIdentifier(args[1]);
    const extra = args[2];
    const additional = extra !== null && extra !== undefined && (typeof extra === 'object' || typeof extra === 'function' || typeof extra === 'string');
    if (!additional) {
      const length = extra === undefined || extra === null ? null : enforceRange(extra, 0xffffffff);
      if (!OPERATIONS.includes(operation)) return false;
      return checkSupport(operation, algorithm, length);
    }
    const additionalAlgorithm = algorithmIdentifier(extra);
    if (!OPERATIONS.includes(operation)) return false;
    let op = operation, length = null;
    if ((op === 'deriveKey' || op === 'unwrapKey') && !checkSupport('importKey', additionalAlgorithm, null)) return false;
    if (op === 'wrapKey' && !checkSupport('exportKey', additionalAlgorithm, null)) return false;
    if (op === 'encapsulateKey' || op === 'decapsulateKey') {
      let a, imported;
      try { a = normalize(algorithm, 'get shared key length'); } catch (e) { return false; }
      const sharedLength = ALG[a.name].sharedKeyLength();
      try { imported = normalize(additionalAlgorithm, 'importKey'); } catch (e) { return false; }
      if (!supportedSteps('importKey', imported, null)) return false;
      try { ALG[imported.name].importKey('raw-secret', new Uint8Array(sharedLength / 8), imported, false, []); } catch (e) { return false; }
    }
    if (op === 'deriveKey') {
      if (!checkSupport('get key length', additionalAlgorithm, null)) return false;
      const lengthAlg = normalize(additionalAlgorithm, 'get key length');
      length = algorithmOf(lengthAlg.name).keyLength(lengthAlg);
      op = 'deriveBits';
    }
    return checkSupport(op, algorithm, length);
  }

  // -------------------------------------------------------- serialization
  function serialize(r) {
    let raw = r.raw, kind = 'bytes';
    const name = r.algorithm.name;
    if (RSA.includes(name)) { raw = new Uint8Array(N.rsaExport(r.raw, r.type === 'private')); kind = 'rsa'; }
    else if (EC.includes(name) && r.type === 'private') { raw = new Uint8Array(N.ecdhExport(r.raw.handle, 'pkcs8')); kind = 'ec'; }
    else if (r.type === 'private' && (OKP.includes(name) || PQ[name])) { raw = r.raw.secret || r.raw.seed; kind = 'seed'; }
    const algorithm = { ...r.algorithm };
    if (algorithm.publicExponent) algorithm.publicExponent = Array.from(algorithm.publicExponent);
    return { type: r.type, extractable: r.extractable, usages: r.usages.slice(), algorithm, kind, raw: Array.from(raw) };
  }
  function deserialize(d) {
    const algorithm = { ...d.algorithm };
    if (algorithm.publicExponent) algorithm.publicExponent = Uint8Array.from(algorithm.publicExponent);
    const name = algorithm.name, bytes = Uint8Array.from(d.raw);
    let raw = bytes;
    if (d.kind === 'rsa') raw = N.rsaImport(bytes, d.type === 'private', true);
    else if (d.kind === 'ec') raw = ecPrivateFromHandle(N.ecdhKey(algorithm.namedCurve, 'pkcs8', bytes, true, name === 'ECDSA'));
    else if (d.kind === 'seed') raw = OKP.includes(name) ? { secret: bytes, pub: okpPublic(name, bytes) } : { seed: bytes, pub: new Uint8Array(N.pqPublic(name, bytes)) };
    return makeKey({ type: d.type, extractable: d.extractable, algorithm, usages: d.usages, raw });
  }

  return { method, supports, serialize, deserialize };
})
