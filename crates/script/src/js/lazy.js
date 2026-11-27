// The realm owns every live value between registration and first use. Only
// bytecode is shared between realms. Keep page changes to sibling exports
// (including deletion) and never put private bindings back on the global.
(function () {
  'use strict';
  const g = globalThis, N = g.__rhNative;
  const { defineProperty, getOwnPropertyDescriptor, create } = Object;
  const bindings = {
    g, N, Event, EventTarget, DOMException, Symbol, WeakMap, Map, Set,
    defineProperty, getOwnPropertyDescriptor, entries: Object.entries, ownKeys: Reflect.ownKeys,
    setTimeout, clearTimeout, queueMicrotask, HTMLCanvasElementC: g.HTMLCanvasElement,
    invoke: Reflect.apply, UniformFloats: Float32Array, UniformInts: Int32Array,
    uniformLength: getOwnPropertyDescriptor(Object.getPrototypeOf(Float32Array.prototype), 'length').get,
    truncateOffset: Math.trunc, finiteOffset: Number.isFinite,
  };
  function register(module, globals, hooks = '') {
    const names = globals.split(' ');
    const firstHook = names.length;
    if (hooks) names.push(...hooks.split(' '));
    const getters = [];
    let values;
    let loading = false;
    function load() {
      if (values) return values;
      if (loading) throw new TypeError('Recursive initialization of ' + module);
      loading = true;
      try {
        const exports = create(null);
        const initialize = N.lazyInitializer(module);
        initialize(bindings, exports);
        N.webidlEnumerable(undefined, exports);
        for (let i = 0; i < names.length; ++i) {
          const key = names[i], target = i < firstHook ? g : N;
          const desc = getOwnPropertyDescriptor(target, key);
          if (desc && desc.configurable && desc.get === getters[i])
            defineProperty(target, key, getOwnPropertyDescriptor(exports, key));
        }
        values = exports;
        return values;
      } finally {
        loading = false;
      }
    }
    for (let i = 0; i < names.length; ++i) {
      const key = names[i], target = i < firstHook ? g : N;
      const get = function () { return load()[key]; };
      getters.push(get);
      const readonly = key === 'indexedDB';
      defineProperty(target, key, {
        __proto__: null, get, configurable: true, enumerable: readonly,
        set: readonly ? undefined : function (value) {
          defineProperty(this, key, { __proto__: null, value, configurable: true, writable: true, enumerable: this !== target });
        },
      });
    }
  }
  register('idb', 'IDBFactory IDBDatabase IDBTransaction IDBObjectStore IDBIndex IDBCursor IDBCursorWithValue IDBKeyRange IDBRequest IDBOpenDBRequest IDBRecord indexedDB');
  register('webgl', 'WebGLObject WebGL2RenderingContext WebGLVertexArrayObject WebGLRenderingContext WebGLShader WebGLProgram WebGLBuffer WebGLTexture WebGLFramebuffer WebGLRenderbuffer WebGLUniformLocation WebGLActiveInfo WebGLShaderPrecisionFormat WebGLContextEvent',
    'makeWebGLContext resizeWebGLSurface webglSurfacePresented');
  if (N.waVoiceCreate)
    register('webaudio', 'AudioBuffer AudioParam AudioNode AudioDestinationNode GainNode AudioScheduledSourceNode AudioBufferSourceNode OscillatorNode BaseAudioContext AudioContext OfflineAudioContext webkitAudioContext');
  if (!N.workerInfo())
    register('webrtc', 'RTCSessionDescription RTCIceCandidate RTCPeerConnectionIceEvent RTCPeerConnectionIceErrorEvent RTCError RTCErrorEvent RTCCertificate RTCStatsReport RTCIceTransport RTCIceCandidatePair RTCDtlsTransport RTCSctpTransport RTCDataChannel RTCDataChannelEvent RTCPeerConnection');
})();
