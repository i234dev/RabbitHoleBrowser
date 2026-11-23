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
  const bytesOf = (data) => {
    if (data instanceof ArrayBuffer) return new Uint8Array(data.slice(0));
    if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
    throw new TypeError("Failed to execute on 'SubtleCrypto': The provided value is not of type '(ArrayBuffer or ArrayBufferView)'.");
  };
  const N = g.__rhNative, defer = g.setTimeout;
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
  function ctr(key, counter, data, length = 128) {
    const ks = expandKey(key);
    const block = new Uint8Array(counter);
    const out = new Uint8Array(data.length);
    for (let i = 0; i < data.length; i += 16) {
      const ksb = encBlock(ks, block);
      for (let j = 0; j < 16 && i + j < data.length; j++) out[i + j] = data[i + j] ^ ksb[j];
      // Increment the rightmost `length` bits.
      for (let k = 15, carry = 1, bits = length; k >= 0 && carry && bits > 0; k--, bits -= 8) { const v = block[k] + carry; block[k] = v & 255; carry = v >> 8; }
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
    if (algorithm.hash) publicAlgorithm.hash = { ...algorithm.hash };
    recordKey(k, { type, extractable: !!extractable, algorithm, publicAlgorithm,
      usages: Object.freeze(Array.from(new Set(usages))), raw });
    return k;
  }

  const normAlg = a => typeof a === 'object' && a !== null ? a : { name: `${a}` };
  const UNSUPPORTED = ['RSASSA-PKCS1-V1_5', 'RSA-PSS', 'RSA-OAEP', 'ECDSA', 'ECDH', 'X25519', 'ED448', 'X448'];
  function algName(a) {
    const id = normAlg(a);
    if (id.name === undefined) throw new TypeError('An algorithm name is required');
    const n = String(id.name).toUpperCase();
    const map = { ED25519: 'Ed25519', 'AES-CBC': 'AES-CBC', 'AES-CTR': 'AES-CTR', 'AES-GCM': 'AES-GCM', 'AES-KW': 'AES-KW', HMAC: 'HMAC', PBKDF2: 'PBKDF2', HKDF: 'HKDF', 'SHA-1': 'SHA-1', 'SHA-256': 'SHA-256', 'SHA-384': 'SHA-384', 'SHA-512': 'SHA-512' };
    if (map[n]) return map[n];
    if (UNSUPPORTED.includes(n)) throw err('NotSupportedError', `Algorithm: ${n} is not supported.`);
    throw err('NotSupportedError', 'Algorithm: Unrecognized name');
  }
  const b64u = (bytes) => g.btoa(String.fromCharCode(...bytes)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  const unb64u = (s) => { const t = String(s).replace(/-/g, '+').replace(/_/g, '/'); const bin = g.atob(t + '==='.slice((t.length + 3) % 4)); return Uint8Array.from(bin, (c) => c.charCodeAt(0)); };
  const usageCheck = (usages, allowed) => { for (const u of usages) if (!allowed.includes(u)) throw err('SyntaxError', `Cannot create a key using the specified key usages.`); };

  // RFC 8410 encodes the 32-byte public value in a BIT STRING and the
  // private seed in a nested OCTET STRING. Algorithm parameters are absent.
  const edSpki = Uint8Array.of(0x30,42,0x30,5,6,3,0x2b,0x65,0x70,3,33,0);
  const edPkcs8 = Uint8Array.of(0x30,46,2,1,0,0x30,5,6,3,0x2b,0x65,0x70,4,34,4,32);
  const edDataError = () => err('DataError', 'Invalid Ed25519 key encoding');
  function decodeEdPart(text) {
    if (typeof text !== 'string' || !/^[A-Za-z0-9_-]{43}$/.test(text)) throw edDataError();
    const raw = unb64u(text);
    if (raw.length !== 32 || b64u(raw) !== text) throw edDataError();
    return raw;
  }
  function encodeEdDer(prefix, raw) {
    const result = new Uint8Array(prefix.length + raw.length);
    result.set(prefix); result.set(raw, prefix.length);
    return result.buffer;
  }
  function importEdKey(format, data, extractable, usages) {
    const jwk = format === 'jwk';
    const privateKey = format === 'pkcs8' || format === 'raw-seed' || (jwk && data?.d !== undefined);
    usageCheck(usages, privateKey ? ['sign'] : ['verify']);
    if (privateKey && !usages.length) throw err('SyntaxError', 'A private key requires a signing usage');
    let raw, publicBytes;
    if (jwk) {
      if (!data || data.kty !== 'OKP' || data.crv !== 'Ed25519' ||
          (data.alg !== undefined && data.alg !== 'EdDSA' && data.alg !== 'Ed25519') ||
          (usages.length && data.use !== undefined && data.use !== 'sig') ||
          (data.ext === false && extractable)) throw edDataError();
      if (data.key_ops !== undefined) {
        const ops = Array.from(data.key_ops);
        if (new Set(ops).size !== ops.length || usages.some(u => !ops.includes(u))) throw edDataError();
      }
      publicBytes = decodeEdPart(data.x);
      raw = privateKey ? decodeEdPart(data.d) : publicBytes;
    } else {
      raw = bytesOf(data);
      if (format === 'pkcs8' || format === 'spki') {
        const prefix = privateKey ? edPkcs8 : edSpki;
        if (raw.length !== prefix.length + 32 || !prefix.every((v, i) => raw[i] === v)) throw edDataError();
        raw = raw.subarray(prefix.length);
      } else if (!['raw', 'raw-public', 'raw-seed'].includes(format)) {
        throw err('NotSupportedError', 'Unsupported Ed25519 key format');
      }
      if (raw.length !== 32) throw edDataError();
    }
    const material = privateKey ? N.ed25519Key(raw, publicBytes, !!extractable) : raw;
    return newKey(privateKey ? 'private' : 'public', extractable, { name:'Ed25519' }, usages, material);
  }
  function exportEdKey(format, k) {
    const priv = k.type === 'private';
    if (format === 'jwk') {
      const publicBytes = priv ? new Uint8Array(N.ed25519KeyBytes(k.raw, false)) : k.raw;
      const jwk = { kty:'OKP', crv:'Ed25519', x:b64u(publicBytes), alg:'Ed25519', ext:true, key_ops:k.usages.slice() };
      if (priv) jwk.d = b64u(new Uint8Array(N.ed25519KeyBytes(k.raw, true)));
      return jwk;
    }
    if ((format === 'raw' || format === 'raw-public') && !priv) return k.raw.slice().buffer;
    if (format === 'raw-seed' && priv) return N.ed25519KeyBytes(k.raw, true);
    if (format === 'spki' && !priv) return encodeEdDer(edSpki, k.raw);
    if (format === 'pkcs8' && priv) return encodeEdDer(edPkcs8, new Uint8Array(N.ed25519KeyBytes(k.raw, true)));
    throw err('InvalidAccessError', 'The key type cannot be exported in this format');
  }

  function importSecret(format, keyData, alg, extractable, usages) {
    const name = algName(alg);
    if (name === 'Ed25519') return importEdKey(format, keyData, extractable, Array.from(usages));
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
  const keyLength = (alg) => { const a = normAlg(alg); const n = algName(alg); if (n === 'HMAC') return a.length || HASHES[hashName(a.hash)][0] * 8; return a.length; };

  class SubtleCrypto {
    constructor() { ILLEGAL(); }
    digest(alg, data) { return run(() => { const n = algName(alg); if (!HASHES[n]) throw err('NotSupportedError', 'Unrecognized digest algorithm'); return N.cryptoDigest(n, bytesOf(data)); }); }
    generateKey(alg, extractable, usages) {
      return run(() => {
        const name = algName(alg);
        if (name === 'Ed25519') {
          const requested = Array.from(usages);
          usageCheck(requested, ['sign','verify']);
          if (!requested.includes('sign')) throw err('SyntaxError', 'A private key requires a signing usage');
          const handle = N.ed25519Key(null, null, !!extractable);
          const privateKey = newKey('private', extractable, {name}, ['sign'], handle);
          const publicKey = newKey('public', true, {name}, requested.filter(u => u === 'verify'), new Uint8Array(N.ed25519KeyBytes(handle, false)));
          return { publicKey, privateKey };
        }
        const len = keyLength(alg);
        if (name !== 'HMAC' && ![128, 192, 256].includes(len)) throw err('OperationError', 'AES key length must be 128, 192 or 256 bits.');
        const raw = new Uint8Array(Math.ceil(len / 8));
        nativeRandom(raw);
        if (!usages || !usages.length) throw err('SyntaxError', 'Usages cannot be empty when creating a key.');
        return importSecret('raw', raw, alg, extractable, usages);
      });
    }
    importKey(format, keyData, alg, extractable, usages) { return run(() => importSecret(format, keyData, alg, extractable, usages)); }
    exportKey(format, key) {
      return run(() => {
        if (!hasKey(key)) throw new TypeError("Failed to execute 'exportKey' on 'SubtleCrypto': parameter 2 is not of type 'CryptoKey'.");
        if (!keyInfo(key).extractable) throw err('InvalidAccessError', 'key is not extractable');
        if (keyInfo(key).algorithm.name === 'Ed25519') return exportEdKey(format, keyInfo(key));
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
      const k = keyInfo(key), msg = bytesOf(data);
      return name === 'Ed25519' ? N.ed25519Sign(k.raw, msg) : hmac(k.algorithm.hash.name, k.raw, msg).buffer;
    }); }
    verify(alg, key, signature, data) { return run(() => {
      const name = algName(alg);
      checkKey(key, name, 'verify');
      const k = keyInfo(key), msg = bytesOf(data), sig = bytesOf(signature);
      return name === 'Ed25519' ? N.ed25519Verify(k.raw, msg, sig) : N.cryptoHmac(k.algorithm.hash.name, k.raw, msg, sig);
    }); }
    encrypt(alg, key, data) { return run(() => cipher(alg, key, bytesOf(data), false, 'encrypt').buffer); }
    decrypt(alg, key, data) { return run(() => cipher(alg, key, bytesOf(data), true, 'decrypt').buffer); }
    deriveBits(alg, key, length) { return run(() => { checkKey(key, algName(alg), 'deriveBits'); return deriveBitsImpl(alg, key, length); }); }
    deriveKey(alg, key, derivedAlg, extractable, usages) { return run(() => { checkKey(key, algName(alg), 'deriveKey'); return Promise.resolve(deriveBitsImpl(alg, key, keyLength(derivedAlg))).then(bits => importSecret('raw', bits, derivedAlg, extractable, usages)); }); }
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
        const bytes = algName(unwrapAlg) === 'AES-KW' ? aesKw(keyInfo(unwrappingKey).raw, bytesOf(wrapped), true) : cipher(unwrapAlg, unwrappingKey, bytesOf(wrapped), true, null);
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
      case 'AES-CBC': return cbc(keyInfo(key).raw, bytesOf(a.iv), data, decrypt);
      case 'AES-CTR': { const c = bytesOf(a.counter); if (c.length !== 16) throw err('OperationError', 'The counter must be 16 bytes.'); return ctr(keyInfo(key).raw, c, data, a.length || 64); }
      case 'AES-GCM': {
        const tag = a.tagLength === undefined ? 128 : Number(a.tagLength) & 255;
        if (![32, 64, 96, 104, 112, 120, 128].includes(tag)) throw err('OperationError', 'Invalid tag length.');
        const aad = a.additionalData === undefined ? new Uint8Array(0) : bytesOf(a.additionalData);
        return new Uint8Array(N.aesGcm(keyInfo(key).raw, bytesOf(a.iv), data, aad, tag / 8, decrypt));
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
