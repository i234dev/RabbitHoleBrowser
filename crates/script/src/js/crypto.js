// Web Cryptography: Crypto / CryptoKey / SubtleCrypto with SHA-1/256/384/
// 512 digests, HMAC, AES-CBC / AES-CTR / AES-GCM / AES-KW, PBKDF2, HKDF
// (generate/import/export raw+jwk, sign/verify, encrypt/decrypt, derive,
// wrap/unwrap). Ed25519 signing and verification use native key objects.
(function () {
  'use strict';
  const g = globalThis;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const keySlots = new WeakMap();
  const keyInfo = WeakMap.prototype.get.bind(keySlots);
  const recordKey = WeakMap.prototype.set.bind(keySlots);
  const hasKey = WeakMap.prototype.has.bind(keySlots);
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const err = (name, msg) => new g.DOMException(msg, name);
  const native = g.crypto;
  const nativeRandom = native.getRandomValues.bind(native);
  const nativeUUID = native.randomUUID.bind(native);

  // ------------------------------------------------------------- hashes
  const N = g.__rhNative, defer = g.setTimeout;
  const bytesOf = data => new Uint8Array(N.cryptoCopyBuffer(data));
  function unsigned(value, maximum) {
    if (value === undefined) throw new TypeError('Missing algorithm parameter');
    const number = Math.trunc(+value);
    if (!Number.isFinite(number) || number < 0 || number > maximum) throw new TypeError('Algorithm parameter out of range');
    return number;
  }
  const HASHES = { 'SHA-1': [64,20], 'SHA-256': [64,32], 'SHA-384': [128,48], 'SHA-512': [128,64] };
  function hashName(h) {
    const n = (typeof h === 'string' ? h : h && h.name || '').toUpperCase();
    if (!HASHES[n]) throw err('NotSupportedError', `Unrecognized algorithm name`);
    return n;
  }
  const hmac = (hash, key, msg) => new Uint8Array(N.cryptoHmac(hash, key, msg));
  // ---------------------------------------------------------------- AES
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
  function expandKey(key) {
    const nk = key.length / 4, nr = nk + 6, w = new Uint8Array(16 * (nr + 1));
    w.set(key);
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
  function cbc(key, iv, data, decrypt) {
    const ks = expandKey(key);
    if (iv.length !== 16) throw err('OperationError', 'The provided IV is not 16 bytes long.');
    if (!decrypt) {
      const padLen = 16 - (data.length % 16);
      const p = new Uint8Array(data.length + padLen); p.set(data); p.fill(padLen, data.length);
      const out = new Uint8Array(p.length);
      let prev = iv;
      for (let i = 0; i < p.length; i += 16) { const b = p.slice(i, i + 16); for (let j = 0; j < 16; j++) b[j] ^= prev[j]; prev = encBlock(ks, b); out.set(prev, i); }
      return out;
    }
    if (data.length % 16 || !data.length) throw err('OperationError', 'The provided data is not a multiple of the block size.');
    const out = new Uint8Array(data.length);
    let prev = iv;
    for (let i = 0; i < data.length; i += 16) { const c = data.slice(i, i + 16); const b = decBlock(ks, c); for (let j = 0; j < 16; j++) b[j] ^= prev[j]; out.set(b, i); prev = c; }
    const n = out[out.length - 1];
    if (n < 1 || n > 16 || out.slice(out.length - n).some((x) => x !== n)) throw err('OperationError', 'The decryption failed (bad padding).');
    return out.slice(0, out.length - n);
  }
  function ctr(key, counter, data, length) {
    if (Math.ceil(data.length / 16) > 2 ** length) throw err('OperationError', 'AES-CTR counter would repeat');
    const ks = expandKey(key);
    const block = new Uint8Array(counter);
    const out = new Uint8Array(data.length);
    for (let i = 0; i < data.length; i += 16) {
      const ksb = encBlock(ks, block);
      for (let j = 0; j < 16 && i + j < data.length; j++) out[i + j] = data[i + j] ^ ksb[j];
      // Increment the rightmost `length` bits.
      for (let k = 15, bits = length; bits > 0; --k, bits -= 8) {
        const mask = (1 << Math.min(bits, 8)) - 1;
        const low = (block[k] + 1) & mask;
        block[k] = (block[k] & ~mask) | low;
        if (low) break;
      }
    }
    return out;
  }
  function aesKw(key, data, unwrap) {
    const ks = expandKey(key);
    const IV = [0xa6, 0xa6, 0xa6, 0xa6, 0xa6, 0xa6, 0xa6, 0xa6];
    if (data.length % 8) throw err('OperationError', 'AES-KW data must be a multiple of 8 bytes.');
    if (!unwrap) {
      const n = data.length / 8;
      let a = new Uint8Array(IV);
      const r = []; for (let i = 0; i < n; i++) r.push(data.slice(i * 8, i * 8 + 8));
      for (let j = 0; j < 6; j++) for (let i = 0; i < n; i++) {
        const b = encBlock(ks, new Uint8Array([...a, ...r[i]]));
        const t = n * j + i + 1;
        a = b.slice(0, 8); a[7] ^= t & 255; a[6] ^= (t >> 8) & 255; a[5] ^= (t >> 16) & 255; a[4] ^= (t >> 24) & 255;
        r[i] = b.slice(8);
      }
      return new Uint8Array([...a, ...r.flatMap((x) => Array.from(x))]);
    }
    const n = data.length / 8 - 1;
    let a = data.slice(0, 8);
    const r = []; for (let i = 0; i < n; i++) r.push(data.slice(8 + i * 8, 16 + i * 8));
    for (let j = 5; j >= 0; j--) for (let i = n - 1; i >= 0; i--) {
      const t = n * j + i + 1;
      const aa = new Uint8Array(a); aa[7] ^= t & 255; aa[6] ^= (t >> 8) & 255; aa[5] ^= (t >> 16) & 255; aa[4] ^= (t >> 24) & 255;
      const b = decBlock(ks, new Uint8Array([...aa, ...r[i]]));
      a = b.slice(0, 8); r[i] = b.slice(8);
    }
    if (a.some((x, i) => x !== IV[i])) throw err('OperationError', 'AES-KW integrity check failed.');
    return new Uint8Array(r.flatMap((x) => Array.from(x)));
  }

  // ------------------------------------------------------------ CryptoKey
  class CryptoKey {
    constructor() { ILLEGAL(); }
    get type() { return keyInfo(this).type; }
    get extractable() { return keyInfo(this).extractable; }
    get algorithm() { return keyInfo(this).publicAlgorithm; }
    get usages() { return keyInfo(this).usages; }
  }
  tag(CryptoKey, 'CryptoKey');
  function newKey(type, extractable, algorithm, usages, raw) {
    const k = Object.create(CryptoKey.prototype);
    const publicAlgorithm = { ...algorithm };
    if (algorithm.publicExponent) publicAlgorithm.publicExponent = algorithm.publicExponent.slice();
    if (algorithm.hash) publicAlgorithm.hash = { ...algorithm.hash };
    recordKey(k, { type, extractable: !!extractable, algorithm, publicAlgorithm,
      usages: Object.freeze(Array.from(new Set(usages))), raw });
    return k;
  }

  const normAlg = a => typeof a === 'object' && a !== null ? a : { name: `${a}` };
  const UNSUPPORTED = ['ED448', 'X448'];
  function algName(a) {
    const id = normAlg(a);
    const name = id.name;
    if (name === undefined) throw new TypeError('An algorithm name is required');
    const n = String(name).toUpperCase();
    const map = { 'RSASSA-PKCS1-V1_5':'RSASSA-PKCS1-v1_5', 'RSA-PSS':'RSA-PSS', 'RSA-OAEP':'RSA-OAEP', 'CHACHA20-POLY1305': 'ChaCha20-Poly1305', ECDSA: 'ECDSA', ECDH: 'ECDH', ED25519: 'Ed25519', X25519: 'X25519', 'AES-CBC': 'AES-CBC', 'AES-CTR': 'AES-CTR', 'AES-GCM': 'AES-GCM', 'AES-KW': 'AES-KW', HMAC: 'HMAC', PBKDF2: 'PBKDF2', HKDF: 'HKDF', 'SHA-1': 'SHA-1', 'SHA-256': 'SHA-256', 'SHA-384': 'SHA-384', 'SHA-512': 'SHA-512' };
    if (map[n]) return map[n];
    if (UNSUPPORTED.includes(n)) throw err('NotSupportedError', `Algorithm: ${n} is not supported.`);
    throw err('NotSupportedError', 'Algorithm: Unrecognized name');
  }
  const b64u = (bytes) => g.btoa(String.fromCharCode(...bytes)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  const unb64u = (s) => { const t = String(s).replace(/-/g, '+').replace(/_/g, '/'); const bin = g.atob(t + '==='.slice((t.length + 3) % 4)); return Uint8Array.from(bin, (c) => c.charCodeAt(0)); };
  const usageCheck = (usages, allowed) => { for (const u of usages) if (!allowed.includes(u)) throw err('SyntaxError', `Cannot create a key using the specified key usages.`); };

  // RFC 8410 encodes the 32-byte public value in a BIT STRING and the
  // private seed in a nested OCTET STRING. Algorithm parameters are absent.
  function octetCurve(oid, key, keyBytes, privateUsages, publicUsages, use) {
    return { key, keyBytes, privateUsages, publicUsages, use,
      spki: Uint8Array.of(0x30,42,0x30,5,6,3,0x2b,0x65,oid,3,33,0),
      pkcs8: Uint8Array.of(0x30,46,2,1,0,0x30,5,6,3,0x2b,0x65,oid,4,34,4,32) };
  }
  const okp = {
    __proto__: null,
    Ed25519: octetCurve(0x70, N.ed25519Key, N.ed25519KeyBytes, ['sign'], ['verify'], 'sig'),
    X25519: octetCurve(0x6e, N.x25519Key, N.x25519KeyBytes, ['deriveBits', 'deriveKey'], [], 'enc')
  };
  const keyDataError = () => err('DataError', 'Invalid octet curve key encoding');
  function decodeKeyPart(text) {
    if (typeof text !== 'string' || !/^[A-Za-z0-9_-]{43}$/.test(text)) throw keyDataError();
    const raw = unb64u(text);
    if (raw.length !== 32 || b64u(raw) !== text) throw keyDataError();
    return raw;
  }
  function encodeKeyDer(prefix, raw) {
    const result = new Uint8Array(prefix.length + raw.length);
    result.set(prefix); result.set(raw, prefix.length);
    return result.buffer;
  }
  function importOctetKey(name, format, data, extractable, usages) {
    const curve = okp[name];
    const jwk = format === 'jwk';
    const privateKey = format === 'pkcs8' || format === 'raw-seed' || (jwk && data?.d !== undefined);
    usageCheck(usages, privateKey ? curve.privateUsages : curve.publicUsages);
    if (privateKey && !usages.length) throw err('SyntaxError', 'A private key requires a usage');
    let raw, publicBytes;
    if (jwk) {
      if (!data || data.kty !== 'OKP' || data.crv !== name ||
          (name === 'Ed25519' && data.alg !== undefined && data.alg !== 'EdDSA' && data.alg !== 'Ed25519') ||
          (usages.length && data.use !== undefined && data.use !== curve.use) ||
          (data.ext === false && extractable)) throw keyDataError();
      if (data.key_ops !== undefined) {
        const ops = Array.from(data.key_ops);
        if (new Set(ops).size !== ops.length || usages.some(u => !ops.includes(u))) throw keyDataError();
      }
      publicBytes = decodeKeyPart(data.x);
      raw = privateKey ? decodeKeyPart(data.d) : publicBytes;
    } else {
      raw = bytesOf(data);
      if (format === 'pkcs8' || format === 'spki') {
        const prefix = privateKey ? curve.pkcs8 : curve.spki;
        if (raw.length !== prefix.length + 32 || !prefix.every((v, i) => raw[i] === v)) throw keyDataError();
        raw = raw.subarray(prefix.length);
      } else if (!['raw', 'raw-public', 'raw-seed'].includes(format)) {
        throw err('NotSupportedError', 'Unsupported octet curve key format');
      }
      if (raw.length !== 32) throw keyDataError();
    }
    const material = privateKey ? curve.key(raw, publicBytes, !!extractable) : raw;
    return newKey(privateKey ? 'private' : 'public', extractable, { name }, usages, material);
  }
  function exportOctetKey(format, k) {
    const name = k.algorithm.name, curve = okp[name];
    const priv = k.type === 'private';
    if (format === 'jwk') {
      const publicBytes = priv ? new Uint8Array(curve.keyBytes(k.raw, false)) : k.raw;
      const jwk = { kty:'OKP', crv:name, x:b64u(publicBytes), ext:true, key_ops:k.usages.slice() };
      if (name === 'Ed25519') jwk.alg = name;
      if (priv) jwk.d = b64u(new Uint8Array(curve.keyBytes(k.raw, true)));
      return jwk;
    }
    if ((format === 'raw' || format === 'raw-public') && !priv) return k.raw.slice().buffer;
    if (format === 'raw-seed' && priv) return curve.keyBytes(k.raw, true);
    if (format === 'spki' && !priv) return encodeKeyDer(curve.spki, k.raw);
    if (format === 'pkcs8' && priv) return encodeKeyDer(curve.pkcs8, new Uint8Array(curve.keyBytes(k.raw, true)));
    throw err('InvalidAccessError', 'The key type cannot be exported in this format');
  }

  const ecCurves = {
    __proto__: null,
    'P-256': {size:32, oid:[0x2a,0x86,0x48,0xce,0x3d,3,1,7]},
    'P-384': {size:48, oid:[0x2b,0x81,4,0,0x22]},
    'P-521': {size:66, oid:[0x2b,0x81,4,0,0x23]},
  };
  const ecError = () => err('DataError','Invalid elliptic curve key encoding');
  function ecCurve(alg) {
    const curve = normAlg(alg).namedCurve;
    if (curve === undefined) throw new TypeError('An EC namedCurve is required');
    const name = `${curve}`;
    if (!ecCurves[name]) throw err('NotSupportedError','Unsupported named curve');
    return name;
  }
  function ecDigest(alg, message) {
    const hash = normAlg(alg).hash;
    if (hash === undefined) throw new TypeError('ECDSA requires a hash algorithm');
    return N.cryptoDigest(hashName(hash), message);
  }
  function ecPart(value, size) {
    if (typeof value !== 'string' || value.length !== Math.ceil(size * 4 / 3) ||
        !/^[A-Za-z0-9_-]+$/.test(value)) throw ecError();
    const bytes = unb64u(value);
    if (bytes.length !== size || b64u(bytes) !== value) throw ecError();
    return bytes;
  }
  // EC public key SPKI (RFC 5480) uses a named-curve OID and a BIT STRING.
  // PKCS #8 uses the same AlgorithmIdentifier, followed by an OCTET STRING.
  function ecDerField(bytes, start, tag) {
    if (bytes[start++] !== tag || start >= bytes.length) throw ecError();
    let length = bytes[start++];
    if (length & 128) {
      const count = length & 127;
      if (!count || count > 4 || bytes[start] === 0 || start + count > bytes.length) throw ecError();
      length = 0;
      for (let i = 0; i < count; ++i) length = length * 256 + bytes[start++];
      if (length < 128) throw ecError();
    }
    if (start + length > bytes.length) throw ecError();
    return {data: bytes.subarray(start,start + length), end:start + length};
  }
  function ecAlgIdentifier(curve) {
    const oid = ecCurves[curve].oid;
    return Uint8Array.of(6,7,0x2a,0x86,0x48,0xce,0x3d,2,1,6,oid.length,...oid);
  }
  function parseEcDer(bytes, curve, priv) {
    const outer = ecDerField(bytes,0,0x30);
    if (outer.end !== bytes.length) throw ecError();
    let start = 0;
    if (priv) {
      const version = ecDerField(outer.data,0,2);
      if (version.data.length !== 1 || version.data[0] !== 0) throw ecError();
      start = version.end;
    }
    const alg = ecDerField(outer.data,start,0x30), want = ecAlgIdentifier(curve);
    if (alg.data.length !== want.length || !want.every((b,i)=>b === alg.data[i])) throw ecError();
    const key = ecDerField(outer.data,alg.end,priv ? 4 : 3);
    if (priv) return bytes; // Native decoder checks the private key and optional attributes.
    if (key.end !== outer.data.length || key.data[0] !== 0) throw ecError();
    return key.data.slice(1);
  }
  function encodeEcSpki(curve, bytes) {
    function field(tag, body) {
      const len = body.length;
      return [tag, ...(len < 128 ? [len] : [0x81,len]), ...body];
    }
    return Uint8Array.from(field(0x30,[...field(0x30,ecAlgIdentifier(curve)),...field(3,[0,...bytes])])).buffer;
  }
  function importEc(name, format, data, alg, extractable, usages) {
    const curve = ecCurve(alg), size = ecCurves[curve].size;
    const priv = format === 'pkcs8' || (format === 'jwk' && data?.d !== undefined);
    const signing = name === 'ECDSA';
    usageCheck(usages, priv ? (signing ? ['sign'] : ['deriveBits','deriveKey']) : (signing ? ['verify'] : []));
    if (priv && !usages.length) throw err('SyntaxError','A private key requires a usage');
    let point, handle;
    if (format === 'jwk') {
      if (data?.kty !== 'EC' || data.crv !== curve || (usages.length && data.use !== undefined && data.use !== (signing ? 'sig' : 'enc')) ||
          (signing && data.alg !== undefined && data.alg !== ({'P-256':'ES256','P-384':'ES384','P-521':'ES512'})[curve]) ||
          (data.ext === false && extractable)) throw ecError();
      if (data.key_ops !== undefined) {
        const ops = Array.from(data.key_ops);
        if (new Set(ops).size !== ops.length || usages.some(u => !ops.includes(u))) throw ecError();
      }
      point = Uint8Array.of(4,...ecPart(data.x,size),...ecPart(data.y,size));
      if (priv) {
        handle = N.ecdhKey(curve,'scalar',ecPart(data.d,size),!!extractable,signing);
        const actual = new Uint8Array(N.ecdhExport(handle,'public'));
        if (!actual.every((b,i)=>b === point[i])) throw ecError();
      }
    } else if (format === 'pkcs8') {
      handle = N.ecdhKey(curve,'pkcs8',parseEcDer(bytesOf(data),curve,true),!!extractable,signing);
    } else if (format === 'spki') {
      point = parseEcDer(bytesOf(data),curve,false);
    } else if (format === 'raw' || format === 'raw-public') {
      point = bytesOf(data);
    } else throw err('NotSupportedError','Unsupported EC import format');
    if (!priv) N.ecdhValidate(curve,point);
    return newKey(priv ? 'private' : 'public',extractable,{name,namedCurve:curve},usages,priv ? handle : point.slice());
  }
  function exportEc(format, k) {
    const curve = k.algorithm.namedCurve, priv = k.type === 'private';
    if (format === 'pkcs8' && priv) return N.ecdhExport(k.raw,format);
    const point = priv ? new Uint8Array(N.ecdhExport(k.raw,'public')) : k.raw;
    if (!priv && (format === 'raw' || format === 'raw-public')) return point.slice().buffer;
    if (!priv && format === 'spki') return encodeEcSpki(curve,point);
    if (format === 'jwk') {
      const split = 1 + ecCurves[curve].size;
      const jwk = {kty:'EC',crv:curve,x:b64u(point.subarray(1,split)),y:b64u(point.subarray(split)),ext:true,key_ops:k.usages.slice()};
      if (priv) jwk.d = b64u(new Uint8Array(N.ecdhExport(k.raw,'scalar')));
      return jwk;
    }
    throw err('InvalidAccessError','The EC key type cannot be exported in this format');
  }

  const rsaAlgorithms = new Set(['RSASSA-PKCS1-v1_5','RSA-PSS','RSA-OAEP']);
  const rsaNumbers = ['n','e','d','p','q','dp','dq','qi'];
  const rsaIdentifier = [6,9,42,134,72,134,247,13,1,1,1,5,0];
  const rsaError = () => err('DataError','Invalid RSA key encoding');
  function rsaUsages(name, priv) {
    if (name === 'RSA-OAEP') return priv ? ['decrypt','unwrapKey'] : ['encrypt','wrapKey'];
    return priv ? ['sign'] : ['verify'];
  }
  function rsaJwa(name, hash) {
    const size = hash.slice(4);
    if (name === 'RSA-OAEP') return hash === 'SHA-1' ? name : name+'-'+size;
    return (name === 'RSA-PSS' ? 'PS' : 'RS')+size;
  }
  function derField(tag, data) {
    const length = [];
    for (let n=data.length; n; n=Math.floor(n/256)) length.unshift(n & 255);
    return Uint8Array.from([tag,...(data.length < 128 ? [data.length] : [128|length.length,...length]),...data]);
  }
  function rsaIntegers(der, priv) {
    const seq = ecDerField(der,0,48);
    if (seq.end !== der.length) throw rsaError();
    let pos = 0;
    if (priv) {
      const version=ecDerField(seq.data,pos,2);
      if (version.data.length !== 1 || version.data[0] !== 0) throw rsaError();
      pos=version.end;
    }
    const parts = [];
    for (let i=0; i<(priv ? 8 : 2); ++i) {
      const integer=ecDerField(seq.data,pos,2), bytes=integer.data;
      if (!bytes.length || bytes[0]>=128 || (bytes[0]===0 && (bytes.length===1 || bytes[1]<128))) throw rsaError();
      parts.push(bytes[0]===0 ? bytes.subarray(1) : bytes);
      pos=integer.end;
    }
    if (pos !== seq.data.length) throw rsaError();
    return parts;
  }
  function rsaJwkDer(jwk, priv) {
    const bytes = priv ? [2,1,0] : [];
    for (const key of rsaNumbers.slice(0,priv ? 8 : 2)) {
      const text=jwk[key];
      if (typeof text!=='string' || !/^[A-Za-z0-9_-]+$/.test(text)) throw rsaError();
      const v=unb64u(text);
      if (!v.length || !v[0] || b64u(v)!==text) throw rsaError();
      bytes.push(...derField(2, v[0]>=128 ? [0,...v] : v));
    }
    return derField(48,bytes);
  }
  function rsaContainer(key, priv) {
    const alg=derField(48,rsaIdentifier);
    return derField(48,priv ? [2,1,0,...alg,...derField(4,key)] : [...alg,...derField(3,[0,...key])]).buffer;
  }
  function unpackRsa(der, priv) {
    const outer=ecDerField(der,0,48);
    if (outer.end !== der.length) throw rsaError();
    let pos=0;
    if (priv) {
      const version=ecDerField(outer.data,pos,2);
      if (version.data.length!==1 || version.data[0]!==0) throw rsaError();
      pos=version.end;
    }
    const alg=ecDerField(outer.data,pos,48);
    if (![11,13].includes(alg.data.length) || !alg.data.every((b,i)=>b===rsaIdentifier[i])) throw rsaError();
    const field=ecDerField(outer.data,alg.end,priv ? 4 : 3);
    if (!priv && field.data[0]!==0) throw rsaError();
    if (field.end !== outer.data.length && (!priv || ecDerField(outer.data,field.end,160).end !== outer.data.length)) throw rsaError();
    return priv ? field.data : field.data.subarray(1);
  }
  function rsaKey(name, hash, handle, extractable, usages, priv) {
    const parts=rsaIntegers(new Uint8Array(N.rsaExport(handle,false)),false);
    const modulusLength=8*(parts[0].length-1)+32-Math.clz32(parts[0][0]);
    return newKey(priv ? 'private' : 'public',extractable,
      {name,hash:{name:hash},modulusLength,publicExponent:parts[1].slice()},usages,handle);
  }
  function importRsa(name, format, data, alg, extractable, usages) {
    const h=normAlg(alg).hash;
    if (h===undefined) throw new TypeError('RSA requires a hash algorithm');
    const hash=hashName(h);
    if (!['spki','pkcs8','jwk'].includes(format)) throw err('NotSupportedError','Unsupported RSA key format');
    const priv=format==='pkcs8' || (format==='jwk' && data?.d !== undefined);
    usageCheck(usages,rsaUsages(name,priv));
    let encoded;
    if (format==='jwk') {
      if (data?.kty!=='RSA' || (data.alg!==undefined && data.alg!==rsaJwa(name,hash)) ||
          (usages.length && data.use!==undefined && data.use!==(name==='RSA-OAEP' ? 'enc' : 'sig')) ||
          (data.ext===false && extractable) || data.oth!==undefined) throw rsaError();
      if (data.key_ops!==undefined) {
        const ops=Array.from(data.key_ops);
        if (new Set(ops).size!==ops.length || usages.some(u=>!ops.includes(u))) throw rsaError();
      }
      encoded=rsaJwkDer(data,priv);
    } else encoded=unpackRsa(bytesOf(data),priv);
    const handle=N.rsaImport(encoded,priv,!!extractable);
    if (priv && !usages.length) throw err('SyntaxError','A private key requires a usage');
    return rsaKey(name,hash,handle,extractable,usages,priv);
  }
  function exportRsa(format,k) {
    const priv=k.type==='private';
    if (!['spki','pkcs8','jwk'].includes(format)) throw err('NotSupportedError','Unsupported RSA key format');
    if (format!=='jwk' && (format==='pkcs8')!==priv) throw err('InvalidAccessError','Wrong RSA key type for this format');
    const der=new Uint8Array(N.rsaExport(k.raw,priv));
    if (format!=='jwk') return rsaContainer(der,priv);
    const parts=rsaIntegers(der,priv), a=k.algorithm;
    const jwk={kty:'RSA',alg:rsaJwa(a.name,a.hash.name),key_ops:k.usages.slice(),ext:true};
    parts.forEach((v,i)=>{jwk[rsaNumbers[i]]=b64u(v);});
    return jwk;
  }
  function generateRsa(name,alg,extractable,usages) {
    const a=normAlg(alg), hashValue=a.hash;
    if (hashValue===undefined) throw new TypeError('RSA requires a hash algorithm');
    const hash=hashName(hashValue), bits=unsigned(a.modulusLength,0xffffffff), e=a.publicExponent;
    const exponent=N.rsaExponent(e);
    const priv=rsaUsages(name,true), pub=rsaUsages(name,false);
    usageCheck(usages,[...pub,...priv]);
    const privateUsages=usages.filter(u=>priv.includes(u)), publicUsages=usages.filter(u=>pub.includes(u));
    if (!privateUsages.length) throw err('SyntaxError','A private key requires a usage');
    const job=N.rsaGenerate(bits,exponent,!!extractable);
    return new Promise((resolve,reject) => {
      function poll() {
        try {
          const handle=N.rsaGenerated(job);
          if (!handle) { defer(poll,1); return; }
          const publicHandle=N.rsaImport(N.rsaExport(handle,false),false,true);
          resolve({publicKey:rsaKey(name,hash,publicHandle,true,publicUsages,false),
            privateKey:rsaKey(name,hash,handle,extractable,privateUsages,true)});
        } catch (e) { reject(e); }
      }
      defer(poll,1);
    });
  }
  function rsaSignature(k,op,alg,msg,sig) {
    const hash=k.algorithm.hash.name;
    const salt=k.algorithm.name==='RSA-PSS' ? unsigned(normAlg(alg).saltLength,0xffffffff) : null;
    const digest=N.cryptoDigest(hash,bytesOf(msg));
    return N.rsaOperate(k.raw,op,hash,salt,digest,
      op==='sign' ? new Uint8Array(0) : bytesOf(sig));
  }

  function importSecret(format, keyData, alg, extractable, usages) {
    const name = algName(alg);
    if (rsaAlgorithms.has(name)) return importRsa(name,format,keyData,alg,extractable,Array.from(usages));
    if (name === 'ECDH' || name === 'ECDSA') return importEc(name,format,keyData,alg,extractable,Array.from(usages));
    if (okp[name]) return importOctetKey(name, format, keyData, extractable, Array.from(usages));
    if (name === 'ChaCha20-Poly1305') {
      const requested = Array.from(usages);
      usageCheck(requested,['encrypt','decrypt','wrapKey','unwrapKey']);
      if (format !== 'raw-secret' && format !== 'jwk') throw err('NotSupportedError','Unsupported ChaCha20-Poly1305 key format');
      let bytes;
      if (format === 'jwk') {
        if (!keyData || keyData.kty !== 'oct' || typeof keyData.k !== 'string' ||
            (keyData.alg !== undefined && keyData.alg !== 'C20P') ||
            (requested.length && keyData.use !== undefined && keyData.use !== 'enc') ||
            (extractable && keyData.ext === false)) throw err('DataError','Invalid ChaCha20-Poly1305 JWK');
        if (keyData.key_ops !== undefined) {
          const ops = Array.from(keyData.key_ops);
          if (new Set(ops).size !== ops.length || requested.some(u=>!ops.includes(u))) throw err('DataError','Invalid JWK key operations');
        }
        if (!/^[a-zA-Z0-9_-]{43}$/.test(keyData.k)) throw err('DataError','Invalid ChaCha20-Poly1305 JWK key');
        bytes = unb64u(keyData.k);
        if (b64u(bytes) !== keyData.k) throw err('DataError','Invalid ChaCha20-Poly1305 JWK key');
      } else bytes = bytesOf(keyData);
      if (bytes.length !== 32) throw err('DataError','ChaCha20-Poly1305 requires a 256-bit key');
      if (!requested.length) throw err('SyntaxError','A secret key requires a usage');
      return newKey('secret',extractable,{name},requested,bytes);
    }
    let raw;
    if (format === 'raw' || format === 'raw-secret') raw = bytesOf(keyData);
    else if (format === 'jwk') {
      if (!keyData || keyData.kty !== 'oct' || !keyData.k) throw err('DataError', 'The JWK is not a valid secret key.');
      raw = unb64u(keyData.k);
    } else throw err('NotSupportedError', `Unsupported key format '${format}' for ${name}.`);
    const a = normAlg(alg);
    switch (name) {
      case 'HMAC': {
        usageCheck(usages, ['sign', 'verify']);
        const hash = hashName(a.hash);
        return newKey('secret', extractable, { name: 'HMAC', hash: { name: hash }, length: raw.length * 8 }, usages, raw);
      }
      case 'AES-CBC': case 'AES-CTR': case 'AES-GCM': case 'AES-KW': {
        if (![16, 24, 32].includes(raw.length)) throw err('DataError', 'AES key data must be 128, 192 or 256 bits');
        usageCheck(usages, name === 'AES-KW' ? ['wrapKey', 'unwrapKey'] : ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey']);
        return newKey('secret', extractable, { name, length: raw.length * 8 }, usages, raw);
      }
      case 'PBKDF2': case 'HKDF':
        if (extractable) throw err('SyntaxError', `${name} keys are not extractable.`);
        usageCheck(usages, ['deriveKey', 'deriveBits']);
        return newKey('secret', false, { name }, usages, raw);
      default: throw err('NotSupportedError', `Algorithm ${name} cannot import keys.`);
    }
  }
  function deriveBitsImpl(alg, key, length) {
    const name = algName(alg);
    const a = normAlg(alg);
    if (keyInfo(key).algorithm.name !== name) throw err('InvalidAccessError', 'The requested operation is not valid for the provided key.');
    if (name === 'X25519' || name === 'ECDH') {
      const peer = a.public;
      if (!hasKey(peer)) throw new TypeError('A public CryptoKey is required');
      const priv = keyInfo(key), pub = keyInfo(peer);
      if (priv.type !== 'private' || pub.type !== 'public' || pub.algorithm.name !== name ||
          priv.algorithm.namedCurve !== pub.algorithm.namedCurve)
        throw err('InvalidAccessError', 'Key agreement requires compatible private and public keys');
      const secret = name === 'ECDH' ? N.ecdhDerive(priv.raw,pub.raw) : N.x25519Derive(priv.raw,pub.raw);
      const bits = length == null ? secret.byteLength * 8 : Number(length) >>> 0;
      if (bits > secret.byteLength * 8) throw err('OperationError', 'Requested length exceeds the shared secret');
      const result = new Uint8Array(secret, 0, Math.ceil(bits / 8)).slice();
      if (bits % 8) result[result.length - 1] &= 0xff << (8 - bits % 8);
      return result.buffer;
    }
    if (length == null || !Number.isInteger(length) || length < 0 || length > 0xffffffff || length % 8)
      throw err('OperationError', 'The length must be a multiple of 8.');
    const n = length / 8;
    const hash = hashName(a.hash);
    if (name === 'PBKDF2') {
      const salt = bytesOf(a.salt), iterations = Math.trunc(+a.iterations);
      if (!Number.isFinite(iterations) || iterations < 0 || iterations > 0xffffffff) throw new TypeError('Invalid iteration count');
      if (!iterations) throw err('OperationError', 'PBKDF2 iterations must be greater than 0.');
      if (!n) return new ArrayBuffer(0);
      const op = N.pbkdfStart(hash, keyInfo(key).raw, salt, iterations, n);
      const output = N.pbkdfStep(op);
      if (output) return output;
      return new Promise((resolve, reject) => {
        function next() {
          try {
            const result = N.pbkdfStep(op);
            if (result) resolve(result); else defer(next, 0);
          } catch (error) { reject(error); }
        }
        defer(next, 0);
      });
    }
    const salt = bytesOf(a.salt), info = bytesOf(a.info);
    if (!n) return new ArrayBuffer(0);
    const hl = HASHES[hash][1];
    if (n > 255 * hl) throw err('OperationError', 'HKDF output exceeds 255 blocks.');
    const prk = hmac(hash, salt.length ? salt : new Uint8Array(hl), keyInfo(key).raw);
    const out = new Uint8Array(n);
    let t = new Uint8Array(0);
    for (let i = 1, off = 0; off < n; i++) {
      const m = new Uint8Array(t.length + info.length + 1); m.set(t); m.set(info, t.length); m[m.length - 1] = i;
      t = hmac(hash, prk, m);
      out.set(t.slice(0, Math.min(hl, n - off)), off);
      off += hl;
    }
    return out.buffer;
  }
  const keyLength = (alg) => { const a = normAlg(alg); const n = algName(alg); if (n === 'ChaCha20-Poly1305') return 256; if (n === 'HMAC') return a.length === undefined ? HASHES[hashName(a.hash)][0] * 8 : unsigned(a.length,0xffffffff); return unsigned(a.length,65535); };

  class SubtleCrypto {
    constructor() { ILLEGAL(); }
    digest(alg, data) { return run(() => { const n = algName(alg); if (!HASHES[n]) throw err('NotSupportedError', 'Unrecognized digest algorithm'); return N.cryptoDigest(n, bytesOf(data)); }); }
    generateKey(alg, extractable, usages) {
      return run(() => {
        const name = algName(alg);
        if (rsaAlgorithms.has(name)) return generateRsa(name,alg,extractable,Array.from(usages));
        if (name === 'ECDH' || name === 'ECDSA') {
          const namedCurve = ecCurve(alg), requested = Array.from(usages), signing = name === 'ECDSA';
          const privateOps = signing ? ['sign'] : ['deriveBits','deriveKey'];
          const publicOps = signing ? ['verify'] : [];
          usageCheck(requested, [...privateOps,...publicOps]);
          const privateUsages = requested.filter(u => privateOps.includes(u));
          if (!privateUsages.length) throw err('SyntaxError','A private key requires a usage');
          const handle = N.ecdhKey(namedCurve,'generate',null,!!extractable,signing);
          return {
            publicKey: newKey('public',true,{name,namedCurve},requested.filter(u => publicOps.includes(u)),new Uint8Array(N.ecdhExport(handle,'public'))),
            privateKey: newKey('private',extractable,{name,namedCurve},privateUsages,handle),
          };
        }
        if (okp[name]) {
          const curve = okp[name], requested = Array.from(usages);
          usageCheck(requested, [...curve.privateUsages, ...curve.publicUsages]);
          const privateUsages = requested.filter(u => curve.privateUsages.includes(u));
          if (!privateUsages.length) throw err('SyntaxError', 'A private key requires a usage');
          const handle = curve.key(null, null, !!extractable);
          const privateKey = newKey('private', extractable, {name}, privateUsages, handle);
          const publicKey = newKey('public', true, {name}, requested.filter(u => curve.publicUsages.includes(u)), new Uint8Array(curve.keyBytes(handle, false)));
          return { publicKey, privateKey };
        }
        const len = keyLength(alg);
        if (name !== 'HMAC' && ![128, 192, 256].includes(len)) throw err('OperationError', 'AES key length must be 128, 192 or 256 bits.');
        const raw = new Uint8Array(Math.ceil(len / 8));
        nativeRandom(raw);
        if (!usages || !usages.length) throw err('SyntaxError', 'Usages cannot be empty when creating a key.');
        return importSecret(name === 'ChaCha20-Poly1305' ? 'raw-secret' : 'raw', raw, alg, extractable, usages);
      });
    }
    importKey(format, keyData, alg, extractable, usages) { return run(() => importSecret(format, keyData, alg, extractable, usages)); }
    exportKey(format, key) {
      return run(() => {
        if (!hasKey(key)) throw new TypeError("Failed to execute 'exportKey' on 'SubtleCrypto': parameter 2 is not of type 'CryptoKey'.");
        if (!keyInfo(key).extractable) throw err('InvalidAccessError', 'key is not extractable');
        if (rsaAlgorithms.has(keyInfo(key).algorithm.name)) return exportRsa(format,keyInfo(key));
        if (['ECDH','ECDSA'].includes(keyInfo(key).algorithm.name)) return exportEc(format,keyInfo(key));
        if (okp[keyInfo(key).algorithm.name]) return exportOctetKey(format, keyInfo(key));
        if (keyInfo(key).algorithm.name === 'ChaCha20-Poly1305') {
          const {raw,usages} = keyInfo(key);
          if (format === 'raw-secret') return raw.slice().buffer;
          if (format === 'jwk') return {kty:'oct', k:b64u(raw), alg:'C20P', key_ops:usages.slice(), ext:true};
          throw err('NotSupportedError','Unsupported ChaCha20-Poly1305 key format');
        }
        const raw = keyInfo(key).raw;
        if (format === 'raw' || format === 'raw-secret') return raw.slice().buffer;
        if (format === 'jwk') {
          const a = keyInfo(key).algorithm;
          const alg = a.name === 'HMAC' ? 'HS' + a.hash.name.slice(4) : 'A' + a.length + (a.name === 'AES-KW' ? 'KW' : a.name.slice(4));
          return { kty: 'oct', k: b64u(raw), alg, key_ops: keyInfo(key).usages.slice(), ext: true };
        }
        throw err('NotSupportedError', `Unsupported export format '${format}'.`);
      });
    }
    sign(alg, key, data) { return run(() => {
      const name = algName(alg);
      checkKey(key, name, 'sign');
      if (rsaAlgorithms.has(name)) return rsaSignature(keyInfo(key),'sign',alg,data);
      const k = keyInfo(key), msg = bytesOf(data);
      if (name === 'ECDSA') return N.ecdsaSign(k.raw, ecDigest(alg,msg));
      return name === 'Ed25519' ? N.ed25519Sign(k.raw, msg) : hmac(k.algorithm.hash.name, k.raw, msg).buffer;
    }); }
    verify(alg, key, signature, data) { return run(() => {
      const name = algName(alg);
      checkKey(key, name, 'verify');
      if (rsaAlgorithms.has(name)) return rsaSignature(keyInfo(key),'verify',alg,data,signature);
      const k = keyInfo(key), msg = bytesOf(data), sig = bytesOf(signature);
      if (name === 'ECDSA') return N.ecdsaVerify(encodeEcSpki(k.algorithm.namedCurve,k.raw), ecDigest(alg,msg), sig);
      return name === 'Ed25519' ? N.ed25519Verify(k.raw, msg, sig) : N.cryptoHmac(k.algorithm.hash.name, k.raw, msg, sig);
    }); }
    encrypt(alg, key, data) { return run(() => cipher(alg, key, data, false, 'encrypt').buffer); }
    decrypt(alg, key, data) { return run(() => cipher(alg, key, data, true, 'decrypt').buffer); }
    deriveBits(alg, key, length) { return run(() => { checkKey(key, algName(alg), 'deriveBits'); return deriveBitsImpl(alg, key, length); }); }
    deriveKey(alg, key, derivedAlg, extractable, usages) { return run(() => { checkKey(key, algName(alg), 'deriveKey'); return Promise.resolve(deriveBitsImpl(alg, key, keyLength(derivedAlg))).then(bits => importSecret(algName(derivedAlg) === 'ChaCha20-Poly1305' ? 'raw-secret' : 'raw', bits, derivedAlg, extractable, usages)); }); }
    wrapKey(format, key, wrappingKey, wrapAlg) {
      return this.exportKey(format, key).then((exported) => {
        const bytes = format === 'jwk' ? new TextEncoder().encode(JSON.stringify(exported)) : new Uint8Array(exported);
        checkKey(wrappingKey, algName(wrapAlg), 'wrapKey');
        return algName(wrapAlg) === 'AES-KW' ? aesKw(keyInfo(wrappingKey).raw, bytes, false).buffer : cipher(wrapAlg, wrappingKey, bytes, false, null).buffer;
      });
    }
    unwrapKey(format, wrapped, unwrappingKey, unwrapAlg, keyAlg, extractable, usages) {
      return run(() => {
        checkKey(unwrappingKey, algName(unwrapAlg), 'unwrapKey');
        const bytes = algName(unwrapAlg) === 'AES-KW' ? aesKw(keyInfo(unwrappingKey).raw, bytesOf(wrapped), true) : cipher(unwrapAlg, unwrappingKey, wrapped, true, null);
        const data = format === 'jwk' ? JSON.parse(new TextDecoder().decode(bytes)) : bytes;
        return importSecret(format, data, keyAlg, extractable, usages);
      });
    }
  }
  tag(SubtleCrypto, 'SubtleCrypto');
  function run(fn) { try { return Promise.resolve(fn()); } catch (e) { return Promise.reject(e); } }
  function checkKey(key, name, usage) {
    if (!hasKey(key)) throw new TypeError("parameter is not of type 'CryptoKey'.");
    if (keyInfo(key).algorithm.name !== name) throw err('InvalidAccessError', 'The requested operation is not valid for the provided key');
    if (usage && !keyInfo(key).usages.includes(usage)) throw err('InvalidAccessError', 'The requested operation is not valid for the provided key');
  }
  function cipher(alg, key, data, decrypt, usage) {
    const name = algName(alg);
    checkKey(key, name, usage);
    const a = normAlg(alg);
    switch (name) {
      case 'RSA-OAEP': {
        const label=a.label, k=keyInfo(key);
        const extra=label===undefined ? new Uint8Array(0) : bytesOf(label);
        return new Uint8Array(N.rsaOperate(k.raw,decrypt ? 'decrypt' : 'encrypt',k.algorithm.hash.name,null,bytesOf(data),extra));
      }
      case 'ChaCha20-Poly1305': {
        const aad = a.additionalData === undefined ? new Uint8Array(0) : bytesOf(a.additionalData);
        return new Uint8Array(N.chachaPoly1305(keyInfo(key).raw, bytesOf(a.iv), bytesOf(data), aad, decrypt));
      }
      case 'AES-CBC': return cbc(keyInfo(key).raw, bytesOf(a.iv), bytesOf(data), decrypt);
      case 'AES-CTR': {
        const counter = bytesOf(a.counter), length = unsigned(a.length,255);
        if (counter.length !== 16 || !length || length > 128) throw err('OperationError', 'Invalid AES-CTR counter or bit length');
        return ctr(keyInfo(key).raw, counter, bytesOf(data), length);
      }
      case 'AES-GCM': {
        const additional = a.additionalData, aad = additional === undefined ? new Uint8Array(0) : bytesOf(additional);
        const iv = bytesOf(a.iv), tagLength = a.tagLength;
        const tag = tagLength === undefined ? 128 : unsigned(tagLength,255);
        if (![32, 64, 96, 104, 112, 120, 128].includes(tag)) throw err('OperationError', 'Invalid tag length.');
        return new Uint8Array(N.aesGcm(keyInfo(key).raw, iv, bytesOf(data), aad, tag / 8, decrypt));
      }
      default: throw err('NotSupportedError', `${name} does not support encryption.`);
    }
  }

  // --------------------------------------------------------------- Crypto
  class Crypto {
    constructor() { ILLEGAL(); }
    get subtle() { return subtle; }
    getRandomValues(array) {
      if (!ArrayBuffer.isView(array) || array instanceof Float32Array || array instanceof Float64Array || array instanceof DataView) throw err('TypeMismatchError', "Failed to execute 'getRandomValues' on 'Crypto': The provided ArrayBufferView is of type '" + (array && array.constructor && array.constructor.name) + "', which is not an integer array type.");
      if (array.byteLength > 65536) throw err('QuotaExceededError', `Failed to execute 'getRandomValues' on 'Crypto': The ArrayBufferView's byte length (${array.byteLength}) exceeds the number of bytes of entropy available via this API (65536).`);
      const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength);
      nativeRandom(bytes);
      return array;
    }
    randomUUID() { return nativeUUID(); }
  }
  tag(Crypto, 'Crypto');
  const subtle = Object.create(SubtleCrypto.prototype);
  const cryptoObj = Object.create(Crypto.prototype);
  define('Crypto', Crypto);
  define('CryptoKey', CryptoKey);
  define('SubtleCrypto', SubtleCrypto);
  Object.defineProperty(g, 'crypto', { get() { return cryptoObj; }, configurable: true, enumerable: true });
})();
