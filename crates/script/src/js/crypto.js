// Web Cryptography: Crypto, CryptoKey and SubtleCrypto. This file only
// defines the interfaces; the algorithms (crypto_impl.js) are compiled the
// first time a SubtleCrypto method, SubtleCrypto.supports or a CryptoKey
// serialization needs them. CryptoKey and SubtleCrypto are [SecureContext].
(function () {
  'use strict';
  const g = globalThis;
  const N = g.__rhNative, codec = g.__rhClone;
  const native = g.crypto;
  const nativeRandom = native.getRandomValues.bind(native);
  const nativeUUID = native.randomUUID.bind(native);
  const secure = !!g.isSecureContext;
  const ILLEGAL = () => { throw new TypeError('Illegal constructor'); };
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });

  // ------------------------------------------------------------ CryptoKey
  const slots = new WeakMap();
  const record = WeakMap.prototype.get.bind(slots);
  const remember = WeakMap.prototype.set.bind(slots);
  const slot = (key) => {
    const r = record(key);
    if (!r) throw new TypeError("Illegal invocation: not a CryptoKey");
    return r;
  };
  class CryptoKey {
    constructor() { ILLEGAL(); }
    get type() { return slot(this).type; }
    get extractable() { return slot(this).extractable; }
    get algorithm() { return slot(this).algorithmObject; }
    get usages() { return slot(this).usagesObject; }
  }
  tag(CryptoKey, 'CryptoKey');
  function copyAlgorithm(a) {
    const out = {};
    for (const k of Object.keys(a)) {
      const v = a[k];
      out[k] = v instanceof Uint8Array ? v.slice() : v && typeof v === 'object' ? copyAlgorithm(v) : v;
    }
    return out;
  }
  // r: { type, extractable, algorithm, usages, raw }
  function makeKey(r) {
    const key = Object.create(CryptoKey.prototype);
    r.algorithmObject = copyAlgorithm(r.algorithm);
    r.usagesObject = Object.freeze(r.usages.slice());
    remember(key, r);
    return key;
  }

  let impl = null;
  const api = () => impl || (impl = N.cryptoImpl()({ N, makeKey, record, random: nativeRandom, isSubtle: (v) => v === subtle }));

  // ----------------------------------------------------------- SubtleCrypto
  class SubtleCrypto {
    constructor() { ILLEGAL(); }
    encrypt(algorithm, key, data) { return api().method('encrypt', this, arguments); }
    decrypt(algorithm, key, data) { return api().method('decrypt', this, arguments); }
    sign(algorithm, key, data) { return api().method('sign', this, arguments); }
    verify(algorithm, key, signature, data) { return api().method('verify', this, arguments); }
    digest(algorithm, data) { return api().method('digest', this, arguments); }
    generateKey(algorithm, extractable, keyUsages) { return api().method('generateKey', this, arguments); }
    deriveKey(algorithm, baseKey, derivedKeyType, extractable, keyUsages) { return api().method('deriveKey', this, arguments); }
    deriveBits(algorithm, baseKey) { return api().method('deriveBits', this, arguments); }
    importKey(format, keyData, algorithm, extractable, keyUsages) { return api().method('importKey', this, arguments); }
    exportKey(format, key) { return api().method('exportKey', this, arguments); }
    wrapKey(format, key, wrappingKey, wrapAlgorithm) { return api().method('wrapKey', this, arguments); }
    unwrapKey(format, wrappedKey, unwrappingKey, unwrapAlgorithm, unwrappedKeyAlgorithm, extractable, keyUsages) { return api().method('unwrapKey', this, arguments); }
    encapsulateKey(encapsulationAlgorithm, encapsulationKey, sharedKeyAlgorithm, extractable, keyUsages) { return api().method('encapsulateKey', this, arguments); }
    encapsulateBits(encapsulationAlgorithm, encapsulationKey) { return api().method('encapsulateBits', this, arguments); }
    decapsulateKey(decapsulationAlgorithm, decapsulationKey, ciphertext, sharedKeyAlgorithm, extractable, keyUsages) { return api().method('decapsulateKey', this, arguments); }
    decapsulateBits(decapsulationAlgorithm, decapsulationKey, ciphertext) { return api().method('decapsulateBits', this, arguments); }
    getPublicKey(key, keyUsages) { return api().method('getPublicKey', this, arguments); }
    static supports(operation, algorithm) { return api().supports(arguments); }
  }
  tag(SubtleCrypto, 'SubtleCrypto');
  const subtle = Object.create(SubtleCrypto.prototype);

  // CryptoKey objects are serializable (structuredClone, postMessage, IDB).
  if (codec && secure) {
    codec.cryptoKey = {
      encode: (v) => record(v) ? api().serialize(record(v)) : null,
      decode: (data) => api().deserialize(data),
    };
  }

  // --------------------------------------------------------------- Crypto
  class Crypto {
    constructor() { ILLEGAL(); }
    getRandomValues(array) {
      if (this !== cryptoObject) throw new TypeError('Illegal invocation');
      if (arguments.length < 1) throw new TypeError("Failed to execute 'getRandomValues' on 'Crypto': 1 argument required, but only 0 present.");
      if (!ArrayBuffer.isView(array)) throw new TypeError("Failed to execute 'getRandomValues' on 'Crypto': parameter 1 is not of type 'ArrayBufferView'.");
      const kind = Object.prototype.toString.call(array).slice(8, -1);
      if (!['Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array', 'Int32Array', 'Uint32Array', 'BigInt64Array', 'BigUint64Array'].includes(kind) ||
          !(array instanceof Object.getPrototypeOf(Uint8Array))) {
        throw new g.DOMException("Failed to execute 'getRandomValues' on 'Crypto': The provided ArrayBufferView is of type '" + kind + "', which is not an integer array type.", 'TypeMismatchError');
      }
      if (array.byteLength > 65536) {
        const message = `Failed to execute 'getRandomValues' on 'Crypto': The ArrayBufferView's byte length (${array.byteLength}) exceeds the number of bytes of entropy available via this API (65536).`;
        throw typeof g.QuotaExceededError === 'function' ? new g.QuotaExceededError(message) : new g.DOMException(message, 'QuotaExceededError');
      }
      nativeRandom(new Uint8Array(array.buffer, array.byteOffset, array.byteLength));
      return array;
    }
    randomUUID() {
      if (this !== cryptoObject) throw new TypeError('Illegal invocation');
      return nativeUUID();
    }
  }
  tag(Crypto, 'Crypto');
  if (secure) {
    const { get } = Object.getOwnPropertyDescriptor({
      get subtle() {
        if (this !== cryptoObject) throw new TypeError('Illegal invocation');
        return subtle;
      },
    }, 'subtle');
    Object.defineProperty(Crypto.prototype, 'subtle', { get, enumerable: true, configurable: true });
    define('CryptoKey', CryptoKey);
    define('SubtleCrypto', SubtleCrypto);
  }
  const cryptoObject = Object.create(Crypto.prototype);
  define('Crypto', Crypto);
  const { get } = Object.getOwnPropertyDescriptor({
    get crypto() {
      if (this !== undefined && this !== null && this !== g) throw new TypeError('Illegal invocation');
      return cryptoObject;
    },
  }, 'crypto');
  Object.defineProperty(g, 'crypto', { get, configurable: true, enumerable: true });
})();
