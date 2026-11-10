// Web Cryptography: Crypto / CryptoKey / SubtleCrypto with SHA-1/256/384/
// 512 digests, HMAC, AES-CBC / AES-CTR / AES-GCM / AES-KW, PBKDF2, HKDF
// (generate/import/export raw+jwk, sign/verify, encrypt/decrypt, derive,
// wrap/unwrap). Public-key algorithms (RSA, EC, Ed25519, X25519) reject
// with NotSupportedError.
(function () {
  'use strict';
  const g = globalThis;
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
  const hidden = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const S = Symbol('crypto');
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
  const K256 = new Uint32Array([0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2]);
  function pad(msg, blockLen, lenBytes) {
    const bitLen = msg.length * 8;
    let total = msg.length + 1 + lenBytes;
    total = Math.ceil(total / blockLen) * blockLen;
    const out = new Uint8Array(total);
    out.set(msg);
    out[msg.length] = 0x80;
    // Bit length, big-endian (low 53 bits are enough).
    let v = bitLen;
    for (let i = 0; i < 8; i++) { out[total - 1 - i] = v & 0xff; v = Math.floor(v / 256); }
    return out;
  }
  function sha256(msg) {
    const p = pad(msg, 64, 8);
    const h = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
    const w = new Uint32Array(64);
    for (let off = 0; off < p.length; off += 64) {
      for (let i = 0; i < 16; i++) w[i] = (p[off + i * 4] << 24) | (p[off + i * 4 + 1] << 16) | (p[off + i * 4 + 2] << 8) | p[off + i * 4 + 3];
      for (let i = 16; i < 64; i++) {
        const s0 = ((w[i - 15] >>> 7) | (w[i - 15] << 25)) ^ ((w[i - 15] >>> 18) | (w[i - 15] << 14)) ^ (w[i - 15] >>> 3);
        const s1 = ((w[i - 2] >>> 17) | (w[i - 2] << 15)) ^ ((w[i - 2] >>> 19) | (w[i - 2] << 13)) ^ (w[i - 2] >>> 10);
        w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0;
      }
      let [a, b, c, d, e, f, gg, hh] = h;
      for (let i = 0; i < 64; i++) {
        const S1 = ((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7));
        const ch = (e & f) ^ (~e & gg);
        const t1 = (hh + S1 + ch + K256[i] + w[i]) | 0;
        const S0 = ((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10));
        const maj = (a & b) ^ (a & c) ^ (b & c);
        const t2 = (S0 + maj) | 0;
        hh = gg; gg = f; f = e; e = (d + t1) | 0; d = c; c = b; b = a; a = (t1 + t2) | 0;
      }
      h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += gg; h[7] += hh;
    }
    const out = new Uint8Array(32);
    for (let i = 0; i < 8; i++) { out[i * 4] = h[i] >>> 24; out[i * 4 + 1] = (h[i] >>> 16) & 255; out[i * 4 + 2] = (h[i] >>> 8) & 255; out[i * 4 + 3] = h[i] & 255; }
    return out;
  }
  function sha1(msg) {
    const p = pad(msg, 64, 8);
    const h = new Uint32Array([0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0]);
    const w = new Uint32Array(80);
    for (let off = 0; off < p.length; off += 64) {
      for (let i = 0; i < 16; i++) w[i] = (p[off + i * 4] << 24) | (p[off + i * 4 + 1] << 16) | (p[off + i * 4 + 2] << 8) | p[off + i * 4 + 3];
      for (let i = 16; i < 80; i++) { const x = w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]; w[i] = (x << 1) | (x >>> 31); }
      let [a, b, c, d, e] = h;
      for (let i = 0; i < 80; i++) {
        const [f, k] = i < 20 ? [(b & c) | (~b & d), 0x5a827999] : i < 40 ? [b ^ c ^ d, 0x6ed9eba1] : i < 60 ? [(b & c) | (b & d) | (c & d), 0x8f1bbcdc] : [b ^ c ^ d, 0xca62c1d6];
        const t = (((a << 5) | (a >>> 27)) + f + e + k + w[i]) | 0;
        e = d; d = c; c = (b << 30) | (b >>> 2); b = a; a = t;
      }
      h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e;
    }
    const out = new Uint8Array(20);
    for (let i = 0; i < 5; i++) { out[i * 4] = h[i] >>> 24; out[i * 4 + 1] = (h[i] >>> 16) & 255; out[i * 4 + 2] = (h[i] >>> 8) & 255; out[i * 4 + 3] = h[i] & 255; }
    return out;
  }
  // SHA-384/512 with BigInt words.
  const K512 = ['428a2f98d728ae22', '7137449123ef65cd', 'b5c0fbcfec4d3b2f', 'e9b5dba58189dbbc', '3956c25bf348b538', '59f111f1b605d019', '923f82a4af194f9b', 'ab1c5ed5da6d8118', 'd807aa98a3030242', '12835b0145706fbe', '243185be4ee4b28c', '550c7dc3d5ffb4e2', '72be5d74f27b896f', '80deb1fe3b1696b1', '9bdc06a725c71235', 'c19bf174cf692694', 'e49b69c19ef14ad2', 'efbe4786384f25e3', '0fc19dc68b8cd5b5', '240ca1cc77ac9c65', '2de92c6f592b0275', '4a7484aa6ea6e483', '5cb0a9dcbd41fbd4', '76f988da831153b5', '983e5152ee66dfab', 'a831c66d2db43210', 'b00327c898fb213f', 'bf597fc7beef0ee4', 'c6e00bf33da88fc2', 'd5a79147930aa725', '06ca6351e003826f', '142929670a0e6e70', '27b70a8546d22ffc', '2e1b21385c26c926', '4d2c6dfc5ac42aed', '53380d139d95b3df', '650a73548baf63de', '766a0abb3c77b2a8', '81c2c92e47edaee6', '92722c851482353b', 'a2bfe8a14cf10364', 'a81a664bbc423001', 'c24b8b70d0f89791', 'c76c51a30654be30', 'd192e819d6ef5218', 'd69906245565a910', 'f40e35855771202a', '106aa07032bbd1b8', '19a4c116b8d2d0c8', '1e376c085141ab53', '2748774cdf8eeb99', '34b0bcb5e19b48a8', '391c0cb3c5c95a63', '4ed8aa4ae3418acb', '5b9cca4f7763e373', '682e6ff3d6b2b8a3', '748f82ee5defb2fc', '78a5636f43172f60', '84c87814a1f0ab72', '8cc702081a6439ec', '90befffa23631e28', 'a4506cebde82bde9', 'bef9a3f7b2c67915', 'c67178f2e372532b', 'ca273eceea26619c', 'd186b8c721c0c207', 'eada7dd6cde0eb1e', 'f57d4f7fee6ed178', '06f067aa72176fba', '0a637dc5a2c898a6', '113f9804bef90dae', '1b710b35131c471b', '28db77f523047d84', '32caab7b40c72493', '3c9ebe0a15c9bebc', '431d67c49c100d4c', '4cc5d4becb3e42b6', '597f299cfc657e2a', '5fcb6fab3ad6faec', '6c44198c4a475817'].map((x) => BigInt('0x' + x));
  const M64 = (1n << 64n) - 1n;
  const rotr = (x, n) => ((x >> n) | (x << (64n - n))) & M64;
  function sha512(msg, outLen) {
    const p = pad(msg, 128, 16);
    const h = outLen === 48
      ? ['cbbb9d5dc1059ed8', '629a292a367cd507', '9159015a3070dd17', '152fecd8f70e5939', '67332667ffc00b31', '8eb44a8768581511', 'db0c2e0d64f98fa7', '47b5481dbefa4fa4'].map((x) => BigInt('0x' + x))
      : ['6a09e667f3bcc908', 'bb67ae8584caa73b', '3c6ef372fe94f82b', 'a54ff53a5f1d36f1', '510e527fade682d1', '9b05688c2b3e6c1f', '1f83d9abfb41bd6b', '5be0cd19137e2179'].map((x) => BigInt('0x' + x));
    const w = new Array(80);
    for (let off = 0; off < p.length; off += 128) {
      for (let i = 0; i < 16; i++) { let v = 0n; for (let j = 0; j < 8; j++) v = (v << 8n) | BigInt(p[off + i * 8 + j]); w[i] = v; }
      for (let i = 16; i < 80; i++) {
        const s0 = rotr(w[i - 15], 1n) ^ rotr(w[i - 15], 8n) ^ (w[i - 15] >> 7n);
        const s1 = rotr(w[i - 2], 19n) ^ rotr(w[i - 2], 61n) ^ (w[i - 2] >> 6n);
        w[i] = (w[i - 16] + s0 + w[i - 7] + s1) & M64;
      }
      let [a, b, c, d, e, f, gg, hh] = h;
      for (let i = 0; i < 80; i++) {
        const S1 = rotr(e, 14n) ^ rotr(e, 18n) ^ rotr(e, 41n);
        const ch = (e & f) ^ (~e & M64 & gg);
        const t1 = (hh + S1 + ch + K512[i] + w[i]) & M64;
        const S0 = rotr(a, 28n) ^ rotr(a, 34n) ^ rotr(a, 39n);
        const maj = (a & b) ^ (a & c) ^ (b & c);
        const t2 = (S0 + maj) & M64;
        hh = gg; gg = f; f = e; e = (d + t1) & M64; d = c; c = b; b = a; a = (t1 + t2) & M64;
      }
      const add = [a, b, c, d, e, f, gg, hh];
      for (let i = 0; i < 8; i++) h[i] = (h[i] + add[i]) & M64;
    }
    const out = new Uint8Array(64);
    for (let i = 0; i < 8; i++) for (let j = 0; j < 8; j++) out[i * 8 + j] = Number((h[i] >> BigInt(56 - j * 8)) & 255n);
    return out.slice(0, outLen);
  }
  const HASHES = { 'SHA-1': [sha1, 64, 20], 'SHA-256': [sha256, 64, 32], 'SHA-384': [(m) => sha512(m, 48), 128, 48], 'SHA-512': [(m) => sha512(m, 64), 128, 64] };
  function hashName(h) {
    const n = (typeof h === 'string' ? h : h && h.name || '').toUpperCase();
    if (!HASHES[n]) throw err('NotSupportedError', `Unrecognized algorithm name`);
    return n;
  }
  function hmac(hash, key, msg) {
    const [fn, block] = HASHES[hash];
    let k = key.length > block ? fn(key) : key;
    const kk = new Uint8Array(block); kk.set(k);
    const ipad = new Uint8Array(block + msg.length), opad = new Uint8Array(block);
    for (let i = 0; i < block; i++) { ipad[i] = kk[i] ^ 0x36; opad[i] = kk[i] ^ 0x5c; }
    ipad.set(msg, block);
    const inner = fn(ipad);
    const outer = new Uint8Array(block + inner.length); outer.set(opad); outer.set(inner, block);
    return fn(outer);
  }

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
  // GCM: GHASH over GF(2^128).
  function gmul(x, y) {
    const z = new Uint8Array(16), v = new Uint8Array(y);
    for (let i = 0; i < 128; i++) {
      if ((x[i >> 3] >> (7 - (i & 7))) & 1) for (let j = 0; j < 16; j++) z[j] ^= v[j];
      const lsb = v[15] & 1;
      for (let j = 15; j > 0; j--) v[j] = (v[j] >> 1) | ((v[j - 1] & 1) << 7);
      v[0] >>= 1;
      if (lsb) v[0] ^= 0xe1;
    }
    return z;
  }
  function ghash(h, aad, c) {
    let y = new Uint8Array(16);
    const blocks = (d) => { for (let i = 0; i < d.length; i += 16) { const b = new Uint8Array(16); b.set(d.slice(i, i + 16)); for (let j = 0; j < 16; j++) y[j] ^= b[j]; y = gmul(y, h); } };
    blocks(aad); blocks(c);
    const len = new Uint8Array(16);
    const put = (off, n) => { let v = n * 8; for (let i = 7; i >= 0; i--) { len[off + i] = v % 256; v = Math.floor(v / 256); } };
    put(0, aad.length); put(8, c.length);
    for (let j = 0; j < 16; j++) y[j] ^= len[j];
    return gmul(y, h);
  }
  function gcm(key, iv, data, aad, tagLen, decrypt) {
    const ks = expandKey(key);
    const h = encBlock(ks, new Uint8Array(16));
    let j0;
    if (iv.length === 12) { j0 = new Uint8Array(16); j0.set(iv); j0[15] = 1; }
    else j0 = ghash(h, new Uint8Array(0), iv);
    const inc = (b) => { const c = new Uint8Array(b); for (let k = 15; k >= 12; k--) { c[k] = (c[k] + 1) & 255; if (c[k]) break; } return c; };
    const tlen = tagLen / 8;
    if (decrypt) {
      if (data.length < tlen) throw err('OperationError', 'The provided data is too small.');
      const ct = data.slice(0, data.length - tlen), tg = data.slice(data.length - tlen);
      const s = ghash(h, aad, ct), ek = encBlock(ks, j0);
      for (let i = 0; i < tlen; i++) if ((s[i] ^ ek[i]) !== tg[i]) throw err('OperationError', 'The operation failed for an operation-specific reason');
      return ctr(key, inc(j0), ct, 32);
    }
    const ct = ctr(key, inc(j0), data, 32);
    const s = ghash(h, aad, ct), ek = encBlock(ks, j0);
    const out = new Uint8Array(ct.length + tlen);
    out.set(ct);
    for (let i = 0; i < tlen; i++) out[ct.length + i] = s[i] ^ ek[i];
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
    get type() { return this[S].type; }
    get extractable() { return this[S].extractable; }
    get algorithm() { return this[S].algorithm; }
    get usages() { return this[S].usages.slice(); }
  }
  tag(CryptoKey, 'CryptoKey');
  const newKey = (type, extractable, algorithm, usages, raw) => { const k = Object.create(CryptoKey.prototype); hidden(k, S, { type, extractable: !!extractable, algorithm, usages: Array.from(usages), raw }); return k; };

  const normAlg = (a) => (typeof a === 'string' ? { name: a } : Object.assign({}, a));
  const UNSUPPORTED = ['RSASSA-PKCS1-V1_5', 'RSA-PSS', 'RSA-OAEP', 'ECDSA', 'ECDH', 'ED25519', 'X25519', 'ED448', 'X448'];
  function algName(a) {
    const n = String(normAlg(a).name || '').toUpperCase();
    const map = { 'AES-CBC': 'AES-CBC', 'AES-CTR': 'AES-CTR', 'AES-GCM': 'AES-GCM', 'AES-KW': 'AES-KW', HMAC: 'HMAC', PBKDF2: 'PBKDF2', HKDF: 'HKDF', 'SHA-1': 'SHA-1', 'SHA-256': 'SHA-256', 'SHA-384': 'SHA-384', 'SHA-512': 'SHA-512' };
    if (map[n]) return map[n];
    if (UNSUPPORTED.includes(n)) throw err('NotSupportedError', `Algorithm: ${n} is not supported.`);
    throw err('NotSupportedError', 'Algorithm: Unrecognized name');
  }
  const b64u = (bytes) => g.btoa(String.fromCharCode(...bytes)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  const unb64u = (s) => { const t = String(s).replace(/-/g, '+').replace(/_/g, '/'); const bin = g.atob(t + '==='.slice((t.length + 3) % 4)); return Uint8Array.from(bin, (c) => c.charCodeAt(0)); };
  const usageCheck = (usages, allowed) => { for (const u of usages) if (!allowed.includes(u)) throw new SyntaxError(`Cannot create a key using the specified key usages.`); };

  function importSecret(format, keyData, alg, extractable, usages) {
    const name = algName(alg);
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
        if (extractable) throw new SyntaxError(`${name} keys are not extractable.`);
        usageCheck(usages, ['deriveKey', 'deriveBits']);
        return newKey('secret', false, { name }, usages, raw);
      default: throw err('NotSupportedError', `Algorithm ${name} cannot import keys.`);
    }
  }
  function deriveBitsImpl(alg, key, length) {
    const name = algName(alg);
    const a = normAlg(alg);
    if (key[S].algorithm.name !== name) throw err('InvalidAccessError', 'The requested operation is not valid for the provided key.');
    if (length === null || length === undefined || length % 8) throw err('OperationError', 'The length must be a multiple of 8.');
    const n = length / 8;
    const hash = hashName(a.hash);
    if (name === 'PBKDF2') {
      const salt = bytesOf(a.salt), iterations = a.iterations >>> 0;
      if (!iterations) throw err('OperationError', 'PBKDF2 iterations must be greater than 0.');
      const hl = HASHES[hash][2];
      const out = new Uint8Array(n);
      for (let block = 1, off = 0; off < n; block++, off += hl) {
        const s = new Uint8Array(salt.length + 4); s.set(salt); s[salt.length] = block >>> 24; s[salt.length + 1] = (block >>> 16) & 255; s[salt.length + 2] = (block >>> 8) & 255; s[salt.length + 3] = block & 255;
        let u = hmac(hash, key[S].raw, s);
        const t = new Uint8Array(u);
        for (let i = 1; i < iterations; i++) { u = hmac(hash, key[S].raw, u); for (let j = 0; j < t.length; j++) t[j] ^= u[j]; }
        out.set(t.slice(0, Math.min(hl, n - off)), off);
      }
      return out;
    }
    // HKDF
    const salt = bytesOf(a.salt), info = bytesOf(a.info);
    const hl = HASHES[hash][2];
    const prk = hmac(hash, salt.length ? salt : new Uint8Array(hl), key[S].raw);
    const out = new Uint8Array(n);
    let t = new Uint8Array(0);
    for (let i = 1, off = 0; off < n; i++) {
      const m = new Uint8Array(t.length + info.length + 1); m.set(t); m.set(info, t.length); m[m.length - 1] = i;
      t = hmac(hash, prk, m);
      out.set(t.slice(0, Math.min(hl, n - off)), off);
      off += hl;
    }
    return out;
  }
  const keyLength = (alg) => { const a = normAlg(alg); const n = algName(alg); if (n === 'HMAC') return a.length || HASHES[hashName(a.hash)][1] * 8; return a.length; };

  class SubtleCrypto {
    constructor() { ILLEGAL(); }
    digest(alg, data) { return run(() => { const n = algName(alg); if (!HASHES[n]) throw err('NotSupportedError', 'Unrecognized digest algorithm'); return HASHES[n][0](bytesOf(data)).buffer; }); }
    generateKey(alg, extractable, usages) {
      return run(() => {
        const name = algName(alg);
        const len = keyLength(alg);
        if (name !== 'HMAC' && ![128, 192, 256].includes(len)) throw err('OperationError', 'AES key length must be 128, 192 or 256 bits.');
        const raw = new Uint8Array(Math.ceil(len / 8));
        nativeRandom(raw);
        if (!usages || !usages.length) throw new SyntaxError('Usages cannot be empty when creating a key.');
        return importSecret('raw', raw, alg, extractable, usages);
      });
    }
    importKey(format, keyData, alg, extractable, usages) { return run(() => importSecret(format, keyData, alg, extractable, usages)); }
    exportKey(format, key) {
      return run(() => {
        if (!(key instanceof CryptoKey)) throw new TypeError("Failed to execute 'exportKey' on 'SubtleCrypto': parameter 2 is not of type 'CryptoKey'.");
        if (!key[S].extractable) throw err('InvalidAccessError', 'key is not extractable');
        const raw = key[S].raw;
        if (format === 'raw' || format === 'raw-secret') return raw.slice().buffer;
        if (format === 'jwk') {
          const a = key[S].algorithm;
          const alg = a.name === 'HMAC' ? 'HS' + a.hash.name.slice(4) : 'A' + a.length + (a.name === 'AES-KW' ? 'KW' : a.name.slice(4));
          return { kty: 'oct', k: b64u(raw), alg, key_ops: key[S].usages.slice(), ext: true };
        }
        throw err('NotSupportedError', `Unsupported export format '${format}'.`);
      });
    }
    sign(alg, key, data) { return run(() => { checkKey(key, 'HMAC', 'sign'); return hmac(key[S].algorithm.hash.name, key[S].raw, bytesOf(data)).buffer; }); }
    verify(alg, key, signature, data) { return run(() => { checkKey(key, 'HMAC', 'verify'); const mac = hmac(key[S].algorithm.hash.name, key[S].raw, bytesOf(data)); const sig = bytesOf(signature); return sig.length === mac.length && sig.every((b, i) => b === mac[i]); }); }
    encrypt(alg, key, data) { return run(() => cipher(alg, key, bytesOf(data), false, 'encrypt').buffer); }
    decrypt(alg, key, data) { return run(() => cipher(alg, key, bytesOf(data), true, 'decrypt').buffer); }
    deriveBits(alg, key, length) { return run(() => { checkKey(key, algName(alg), 'deriveBits'); return deriveBitsImpl(alg, key, length).buffer; }); }
    deriveKey(alg, key, derivedAlg, extractable, usages) { return run(() => { checkKey(key, algName(alg), 'deriveKey'); const bits = deriveBitsImpl(alg, key, keyLength(derivedAlg)); return importSecret('raw', bits, derivedAlg, extractable, usages); }); }
    wrapKey(format, key, wrappingKey, wrapAlg) {
      return this.exportKey(format, key).then((exported) => {
        const bytes = format === 'jwk' ? new TextEncoder().encode(JSON.stringify(exported)) : new Uint8Array(exported);
        if (!wrappingKey[S].usages.includes('wrapKey')) throw err('InvalidAccessError', 'The wrapping key does not support wrapKey.');
        return algName(wrapAlg) === 'AES-KW' ? aesKw(wrappingKey[S].raw, bytes, false).buffer : cipher(wrapAlg, wrappingKey, bytes, false, null).buffer;
      });
    }
    unwrapKey(format, wrapped, unwrappingKey, unwrapAlg, keyAlg, extractable, usages) {
      return run(() => {
        if (!unwrappingKey[S].usages.includes('unwrapKey')) throw err('InvalidAccessError', 'The unwrapping key does not support unwrapKey.');
        const bytes = algName(unwrapAlg) === 'AES-KW' ? aesKw(unwrappingKey[S].raw, bytesOf(wrapped), true) : cipher(unwrapAlg, unwrappingKey, bytesOf(wrapped), true, null);
        const data = format === 'jwk' ? JSON.parse(new TextDecoder().decode(bytes)) : bytes;
        return importSecret(format, data, keyAlg, extractable, usages);
      });
    }
  }
  tag(SubtleCrypto, 'SubtleCrypto');
  function run(fn) { try { return Promise.resolve(fn()); } catch (e) { return Promise.reject(e); } }
  function checkKey(key, name, usage) {
    if (!(key instanceof CryptoKey)) throw new TypeError("parameter is not of type 'CryptoKey'.");
    if (key[S].algorithm.name !== name) throw err('InvalidAccessError', 'The requested operation is not valid for the provided key');
    if (usage && !key[S].usages.includes(usage)) throw err('InvalidAccessError', 'The requested operation is not valid for the provided key');
  }
  function cipher(alg, key, data, decrypt, usage) {
    const name = algName(alg);
    checkKey(key, name, usage);
    const a = normAlg(alg);
    switch (name) {
      case 'AES-CBC': return cbc(key[S].raw, bytesOf(a.iv), data, decrypt);
      case 'AES-CTR': { const c = bytesOf(a.counter); if (c.length !== 16) throw err('OperationError', 'The counter must be 16 bytes.'); return ctr(key[S].raw, c, data, a.length || 64); }
      case 'AES-GCM': { const tl = a.tagLength === undefined ? 128 : a.tagLength; if (![32, 64, 96, 104, 112, 120, 128].includes(tl)) throw err('OperationError', 'Invalid tag length.'); return gcm(key[S].raw, bytesOf(a.iv), data, a.additionalData ? bytesOf(a.additionalData) : new Uint8Array(0), tl, decrypt); }
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
