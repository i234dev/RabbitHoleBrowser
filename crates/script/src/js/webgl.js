// Experimental WebGL software contexts with real buffers, shaders and pixels.
// Unsupported commands set GL errors; extensions remain unavailable.
(function () {
  'use strict';
  const g = globalThis, N = g.__rhNative;
  const hidden = (o, k, value) => Object.defineProperty(o, k, { value, configurable: true, writable: true });
  const tag = (C, name) => Object.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const illegal = () => { throw new TypeError('Illegal constructor'); };
  const contexts = new WeakMap(), objects = new WeakMap(), locations = new WeakMap();
  const limits = N.webglLimits();
  const GL = {
    NO_ERROR: 0, INVALID_ENUM: 0x0500, INVALID_VALUE: 0x0501, INVALID_OPERATION: 0x0502, OUT_OF_MEMORY: 0x0505, INVALID_FRAMEBUFFER_OPERATION: 0x0506, CONTEXT_LOST_WEBGL: 0x9242,
    DEPTH_BUFFER_BIT: 0x0100, STENCIL_BUFFER_BIT: 0x0400, COLOR_BUFFER_BIT: 0x4000,
    POINTS: 0, LINES: 1, LINE_LOOP: 2, LINE_STRIP: 3, TRIANGLES: 4, TRIANGLE_STRIP: 5, TRIANGLE_FAN: 6,
    SRC_COLOR: 0x0300, ONE_MINUS_SRC_COLOR: 0x0301, SRC_ALPHA: 0x0302, ONE_MINUS_SRC_ALPHA: 0x0303,
    DST_ALPHA: 0x0304, ONE_MINUS_DST_ALPHA: 0x0305, DST_COLOR: 0x0306, ONE_MINUS_DST_COLOR: 0x0307, SRC_ALPHA_SATURATE: 0x0308,
    CONSTANT_COLOR: 0x8001, ONE_MINUS_CONSTANT_COLOR: 0x8002, CONSTANT_ALPHA: 0x8003, ONE_MINUS_CONSTANT_ALPHA: 0x8004,
    FUNC_ADD: 0x8006, FUNC_SUBTRACT: 0x800A, FUNC_REVERSE_SUBTRACT: 0x800B, BLEND_COLOR: 0x8005,
    BLEND_EQUATION: 0x8009, BLEND_EQUATION_RGB: 0x8009, BLEND_EQUATION_ALPHA: 0x883D,
    BLEND_DST_RGB: 0x80C8, BLEND_SRC_RGB: 0x80C9, BLEND_DST_ALPHA: 0x80CA, BLEND_SRC_ALPHA: 0x80CB,
    ZERO: 0, ONE: 1, NONE: 0, BYTE: 0x1400, UNSIGNED_BYTE: 0x1401, SHORT: 0x1402, UNSIGNED_SHORT: 0x1403, INT: 0x1404, UNSIGNED_INT: 0x1405, FLOAT: 0x1406,
    ARRAY_BUFFER: 0x8892, ELEMENT_ARRAY_BUFFER: 0x8893, ARRAY_BUFFER_BINDING: 0x8894, ELEMENT_ARRAY_BUFFER_BINDING: 0x8895,
    STREAM_DRAW: 0x88E0, STATIC_DRAW: 0x88E4, DYNAMIC_DRAW: 0x88E8, BUFFER_SIZE: 0x8764, BUFFER_USAGE: 0x8765,
    VERTEX_SHADER: 0x8B31, FRAGMENT_SHADER: 0x8B30, SHADER_TYPE: 0x8B4F, DELETE_STATUS: 0x8B80, COMPILE_STATUS: 0x8B81, LINK_STATUS: 0x8B82, VALIDATE_STATUS: 0x8B83,
    ATTACHED_SHADERS: 0x8B85, ACTIVE_UNIFORMS: 0x8B86, ACTIVE_ATTRIBUTES: 0x8B89, CURRENT_PROGRAM: 0x8B8D,
    FLOAT_VEC2: 0x8B50, FLOAT_VEC3: 0x8B51, FLOAT_VEC4: 0x8B52, FLOAT_MAT4: 0x8B5C,
    HIGH_FLOAT: 0x8DF2, MEDIUM_FLOAT: 0x8DF1, LOW_FLOAT: 0x8DF0, HIGH_INT: 0x8DF5, MEDIUM_INT: 0x8DF4, LOW_INT: 0x8DF3, SHADER_COMPILER: 0x8DFA,
    RGBA: 0x1908, RGB: 0x1907, ALPHA: 0x1906, LUMINANCE: 0x1909, LUMINANCE_ALPHA: 0x190A,
    SCISSOR_TEST: 0x0C11, DEPTH_TEST: 0x0B71, BLEND: 0x0BE2, CULL_FACE: 0x0B44, STENCIL_TEST: 0x0B90, DITHER: 0x0BD0, POLYGON_OFFSET_FILL: 0x8037, SAMPLE_ALPHA_TO_COVERAGE: 0x809E, SAMPLE_COVERAGE: 0x80A0,
    NEVER: 0x0200, LESS: 0x0201, EQUAL: 0x0202, LEQUAL: 0x0203, GREATER: 0x0204, NOTEQUAL: 0x0205, GEQUAL: 0x0206, ALWAYS: 0x0207,
    VIEWPORT: 0x0BA2, SCISSOR_BOX: 0x0C10, COLOR_CLEAR_VALUE: 0x0C22, COLOR_WRITEMASK: 0x0C23, DEPTH_CLEAR_VALUE: 0x0B73, DEPTH_WRITEMASK: 0x0B72, DEPTH_FUNC: 0x0B74,
    VERSION: 0x1F02, VENDOR: 0x1F00, RENDERER: 0x1F01, SHADING_LANGUAGE_VERSION: 0x8B8C,
    MAX_VERTEX_ATTRIBS: 0x8869, MAX_VIEWPORT_DIMS: 0x0D3A, MAX_RENDERBUFFER_SIZE: 0x84E8, MAX_TEXTURE_SIZE: 0x0D33,
    RED_BITS: 0x0D52, GREEN_BITS: 0x0D53, BLUE_BITS: 0x0D54, ALPHA_BITS: 0x0D55, DEPTH_BITS: 0x0D56, STENCIL_BITS: 0x0D57, SAMPLES: 0x80A9, SAMPLE_BUFFERS: 0x80A8,
    PACK_ALIGNMENT: 0x0D05, UNPACK_ALIGNMENT: 0x0CF5, UNPACK_FLIP_Y_WEBGL: 0x9240, UNPACK_PREMULTIPLY_ALPHA_WEBGL: 0x9241, UNPACK_COLORSPACE_CONVERSION_WEBGL: 0x9243, BROWSER_DEFAULT_WEBGL: 0x9244,
    IMPLEMENTATION_COLOR_READ_TYPE: 0x8B9A, IMPLEMENTATION_COLOR_READ_FORMAT: 0x8B9B,
    VERTEX_ATTRIB_ARRAY_ENABLED: 0x8622, VERTEX_ATTRIB_ARRAY_SIZE: 0x8623, VERTEX_ATTRIB_ARRAY_STRIDE: 0x8624, VERTEX_ATTRIB_ARRAY_TYPE: 0x8625, CURRENT_VERTEX_ATTRIB: 0x8626, VERTEX_ATTRIB_ARRAY_NORMALIZED: 0x886A, VERTEX_ATTRIB_ARRAY_POINTER: 0x8645, VERTEX_ATTRIB_ARRAY_BUFFER_BINDING: 0x889F,
    FRAMEBUFFER: 0x8D40, RENDERBUFFER: 0x8D41, FRAMEBUFFER_BINDING: 0x8CA6, RENDERBUFFER_BINDING: 0x8CA7, FRAMEBUFFER_COMPLETE: 0x8CD5,
    FRAMEBUFFER_INCOMPLETE_ATTACHMENT: 0x8CD6, FRAMEBUFFER_INCOMPLETE_MISSING_ATTACHMENT: 0x8CD7, FRAMEBUFFER_INCOMPLETE_DIMENSIONS: 0x8CD9, FRAMEBUFFER_UNSUPPORTED: 0x8CDD,
    COLOR_ATTACHMENT0: 0x8CE0, DEPTH_ATTACHMENT: 0x8D00, STENCIL_ATTACHMENT: 0x8D20, DEPTH_STENCIL_ATTACHMENT: 0x821A,
    FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE: 0x8CD0, FRAMEBUFFER_ATTACHMENT_OBJECT_NAME: 0x8CD1, FRAMEBUFFER_ATTACHMENT_TEXTURE_LEVEL: 0x8CD2, FRAMEBUFFER_ATTACHMENT_TEXTURE_CUBE_MAP_FACE: 0x8CD3,
    TEXTURE: 0x1702, DEPTH_COMPONENT16: 0x81A5, RGBA4: 0x8056,
    RENDERBUFFER_WIDTH: 0x8D42, RENDERBUFFER_HEIGHT: 0x8D43, RENDERBUFFER_INTERNAL_FORMAT: 0x8D44,
    RENDERBUFFER_RED_SIZE: 0x8D50, RENDERBUFFER_GREEN_SIZE: 0x8D51, RENDERBUFFER_BLUE_SIZE: 0x8D52, RENDERBUFFER_ALPHA_SIZE: 0x8D53, RENDERBUFFER_DEPTH_SIZE: 0x8D54, RENDERBUFFER_STENCIL_SIZE: 0x8D55,
    TEXTURE_2D: 0x0DE1, TEXTURE_CUBE_MAP: 0x8513, TEXTURE0: 0x84C0, ACTIVE_TEXTURE: 0x84E0, TEXTURE_BINDING_2D: 0x8069,
    TEXTURE_MIN_FILTER: 0x2801, TEXTURE_MAG_FILTER: 0x2800, TEXTURE_WRAP_S: 0x2802, TEXTURE_WRAP_T: 0x2803,
    NEAREST: 0x2600, LINEAR: 0x2601, NEAREST_MIPMAP_LINEAR: 0x2702, NEAREST_MIPMAP_NEAREST: 0x2700, LINEAR_MIPMAP_NEAREST: 0x2701, LINEAR_MIPMAP_LINEAR: 0x2703,
    CLAMP_TO_EDGE: 0x812F, REPEAT: 0x2901, MIRRORED_REPEAT: 0x8370, SAMPLER_2D: 0x8B5E,
    MAX_TEXTURE_IMAGE_UNITS: 0x8872, MAX_COMBINED_TEXTURE_IMAGE_UNITS: 0x8B4D,
  };
  for (let i = 1; i < 32; i++) GL['TEXTURE' + i] = GL.TEXTURE0 + i;
  const clamp = (v) => Math.max(0, Math.min(1, Number.isNaN(v) ? 0 : v));
  const state = (ctx) => { const s = contexts.get(ctx); if (!s) throw new TypeError('Illegal invocation'); return s; };
  const flag = (s, error) => { if (!s.lost && !s.errors.includes(error)) s.errors.push(error); };
  const required = (name, count, args) => { if (args.length < count) throw new TypeError(`${name}: ${count} arguments required`); };
  const call = (s, command, ...args) => {
    syncSize(s);
    if (s.lost) return { error: GL.CONTEXT_LOST_WEBGL, value: null };
    const result = N.webglCall(s.surface.id, command, ...args);
    if (result.error) flag(s, result.error);
    return result;
  };
  function resize(s, width, height) {
    if (s.lost) return;
    const result = N.webglResize(s.surface.id, width, height);
    s.logicalWidth = width; s.logicalHeight = height;
    if (result.error) {
      N.webglLose(s.surface.id);
      s.lost = true; s.surface.lost = true;
      s.errors = [GL.CONTEXT_LOST_WEBGL];
      queueMicrotask(() => s.canvas.dispatchEvent(new WebGLContextEvent('webglcontextlost', { cancelable: true, statusMessage: 'Drawing buffer allocation failed' })));
    }
  }
  function syncSize(s) {
    if (s.lost) return;
    if (s.canvas instanceof g.HTMLCanvasElement) N.canvasSurface(s.canvas);
    else if (s.logicalWidth !== s.surface.width || s.logicalHeight !== s.surface.height) resize(s, s.surface.width, s.surface.height);
  }
  function owned(s, object, kind, nullable = false, deleted = false) {
    if (object === null && nullable) return { id: 0 };
    if (object === null || (typeof object !== 'object' && typeof object !== 'function')) throw new TypeError(`Expected WebGL${kind}`);
    const record = objects.get(object);
    if (!record || record.kind !== kind) throw new TypeError(`Expected WebGL${kind}`);
    if (record.context !== s || (record.deleted && !deleted)) { flag(s, GL.INVALID_OPERATION); return null; }
    return record;
  }
  function wrap(s, kind, id, extra = {}) {
    if (!id) { flag(s, GL.OUT_OF_MEMORY); return null; }
    const C = { Shader: WebGLShader, Program: WebGLProgram, Buffer: WebGLBuffer, VertexArrayObject: WebGLVertexArrayObject, Texture: WebGLTexture, Framebuffer: WebGLFramebuffer, Renderbuffer: WebGLRenderbuffer }[kind];
    const object = Object.create(C.prototype);
    objects.set(object, { context: s, kind, id, deleted: false, ...extra });
    return object;
  }
  function buffer(s, target) {
    target >>>= 0;
    if (target !== GL.ARRAY_BUFFER && target !== GL.ELEMENT_ARRAY_BUFFER) { flag(s, GL.INVALID_ENUM); return null; }
    const object = s.bindings[target];
    if (!object) { flag(s, GL.INVALID_OPERATION); return null; }
    return objects.get(object);
  }
  function framebufferTarget(s, target) {
    if (target === GL.FRAMEBUFFER || (s.version === 2 && (target === 0x8CA8 || target === 0x8CA9))) return true;
    flag(s, GL.INVALID_ENUM); return false;
  }
  function framebufferObject(s, target) { return target === 0x8CA8 ? s.readFramebuffer : s.drawFramebuffer; }
  function detachBoundAttachment(s, object, attachment) {
    for (const fb of [s.drawFramebuffer, s.readFramebuffer]) {
      const f = fb && objects.get(fb);
      if (f && f.attachments[attachment] === object) f.attachments[attachment] = null;
    }
  }
  function dataView(value) {
    if (value instanceof ArrayBuffer || (typeof SharedArrayBuffer === 'function' && value instanceof SharedArrayBuffer)) return new Uint8Array(value);
    if (!ArrayBuffer.isView(value)) throw new TypeError('Expected an ArrayBuffer or ArrayBufferView');
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  function offset(s, value) {
    value = +value;
    if (!Number.isSafeInteger(value) || value < 0) { flag(s, GL.INVALID_VALUE); return null; }
    return value;
  }
  const index = (s, value) => { value >>>= 0; if (value >= limits.maxVertexAttribs) { flag(s, GL.INVALID_VALUE); return null; } return value; };
  function uniformLocation(s, value) {
    if (value === null) return null;
    const location = locations.get(value);
    if (!location) throw new TypeError('Expected WebGLUniformLocation');
    const p = objects.get(location.program);
    if (location.context !== s || s.program !== location.program || p.generation !== location.generation) { flag(s, GL.INVALID_OPERATION); return null; }
    return location;
  }
  function floats(value) {
    if (value instanceof Float32Array) return value;
    if (value === null || typeof value !== 'object' || typeof value[Symbol.iterator] !== 'function') throw new TypeError('Expected a Float32Array or a sequence of numbers');
    const values = [];
    for (const n of value) { values.push(+(n)); if (values.length > 16) break; }
    return new Float32Array(values);
  }
  function ints(value) {
    if (value instanceof Int32Array) return value;
    if (value === null || typeof value !== 'object' || typeof value[Symbol.iterator] !== 'function') throw new TypeError('Expected an Int32Array or a sequence of integers');
    const values = [];
    for (const n of value) { values.push(+(n) | 0); if (values.length > 16) break; }
    return new Int32Array(values);
  }
  function matrix(s, location, transpose, data) {
    const loc = uniformLocation(s, location);
    if (!loc) return;
    data = floats(data);
    if (transpose) { flag(s, GL.INVALID_VALUE); return; }
    if (data.length !== 16) { flag(s, GL.INVALID_OPERATION); return; }
    call(s, 'matrix4', loc.id, false, data);
  }
  function uniform(s, location, data) {
    const loc = uniformLocation(s, location);
    if (loc) { data = floats(data); if (data.length > 4) { flag(s, GL.INVALID_OPERATION); return; } call(s, 'uniform', loc.id, data); }
  }
  function changed(s, result) { if (!result.error && !s.drawFramebuffer) N.canvasMarkDirty(s.surface); }

  class WebGLObject { constructor() { illegal(); } }
  class WebGLShader extends WebGLObject { constructor() { illegal(); } }
  class WebGLProgram extends WebGLObject { constructor() { illegal(); } }
  class WebGLBuffer extends WebGLObject { constructor() { illegal(); } }
  class WebGLTexture extends WebGLObject { constructor() { illegal(); } }
  class WebGLFramebuffer extends WebGLObject { constructor() { illegal(); } }
  class WebGLRenderbuffer extends WebGLObject { constructor() { illegal(); } }
  class WebGLVertexArrayObject extends WebGLObject { constructor() { illegal(); } }
  class WebGLUniformLocation { constructor() { illegal(); } }
  class WebGLActiveInfo { constructor() { illegal(); } }
  class WebGLShaderPrecisionFormat { constructor() { illegal(); } }
  class WebGLContextEvent extends g.Event {
    constructor(type, init = {}) { super(type, init); Object.defineProperty(this, 'statusMessage', { value: String(init.statusMessage || ''), enumerable: true }); }
  }
  class WebGLRenderingContext {
    constructor() { illegal(); }
    get canvas() { return state(this).canvas; }
    get drawingBufferWidth() { const s = state(this); return s.lost ? 0 : call(s, 'width').value || 0; }
    get drawingBufferHeight() { const s = state(this); return s.lost ? 0 : call(s, 'height').value || 0; }
    getContextAttributes() { const s = state(this); syncSize(s); return s.lost ? null : { ...s.attrs }; }
    isContextLost() { const s = state(this); syncSize(s); return s.lost; }
    getError() { const s = state(this); syncSize(s); return s.errors.length ? s.errors.shift() : GL.NO_ERROR; }
    getSupportedExtensions() { const s = state(this); return s.lost ? null : []; }
    getExtension(name) { required('getExtension', 1, arguments); state(this); String(name); return null; }
    createShader(type) { required('createShader', 1, arguments); const s = state(this); type >>>= 0; const r = call(s, 'createShader', type); return r.error ? null : wrap(s, 'Shader', r.value, { type, source: '' }); }
    shaderSource(shader, source) { required('shaderSource', 2, arguments); const s = state(this), o = owned(s, shader, 'Shader'); source = String(source); if (o && !call(s, 'shaderSource', o.id, source).error) o.source = source; }
    compileShader(shader) { required('compileShader', 1, arguments); const s = state(this), o = owned(s, shader, 'Shader'); if (o) call(s, 'compileShader', o.id); }
    getShaderParameter(shader, pname) {
      required('getShaderParameter', 2, arguments); const s = state(this), o = owned(s, shader, 'Shader', false, true); if (!o) return null;
      if (o.deleted && !call(s, 'isShader', o.id).value) { flag(s, GL.INVALID_VALUE); return null; }
      switch (pname >>> 0) { case GL.COMPILE_STATUS: return call(s, 'shaderStatus', o.id).value; case GL.SHADER_TYPE: return o.type; case GL.DELETE_STATUS: return o.deleted; default: flag(s, GL.INVALID_ENUM); return null; }
    }
    getShaderInfoLog(shader) { required('getShaderInfoLog', 1, arguments); const s = state(this), o = owned(s, shader, 'Shader', false, true); return o ? call(s, 'shaderLog', o.id).value : null; }
    getShaderSource(shader) { required('getShaderSource', 1, arguments); const s = state(this), o = owned(s, shader, 'Shader', false, true); return o ? call(s, 'getShaderSource', o.id).value : null; }
    deleteShader(shader) { required('deleteShader', 1, arguments); const s = state(this); if (shader === null) return; const o = owned(s, shader, 'Shader', false, true); if (o && !o.deleted && !call(s, 'deleteShader', o.id).error) o.deleted = true; }
    isShader(shader) { required('isShader', 1, arguments); const s = state(this), o = objects.get(shader); return !!o && o.context === s && o.kind === 'Shader' && !s.lost && !!call(s, 'isShader', o.id).value; }
    createProgram() { const s = state(this), r = call(s, 'createProgram'); return r.error ? null : wrap(s, 'Program', r.value, { shaders: new Set(), generation: 0 }); }
    attachShader(program, shader) { required('attachShader', 2, arguments); const s = state(this), p = owned(s, program, 'Program'), o = owned(s, shader, 'Shader'); if (p && o && !call(s, 'attachShader', p.id, o.id).error) p.shaders.add(shader); }
    getAttachedShaders(program) { const s = state(this), p = owned(s, program, 'Program', false, true); return p ? Array.from(p.shaders) : null; }
    bindAttribLocation(program, location, name) { required('bindAttribLocation', 3, arguments); const s = state(this), p = owned(s, program, 'Program'); if (p) call(s, 'bindAttrib', p.id, location >>> 0, String(name)); }
    linkProgram(program) { required('linkProgram', 1, arguments); const s = state(this), p = owned(s, program, 'Program'); if (p && !call(s, 'linkProgram', p.id).error) p.generation++; }
    getProgramParameter(program, pname) {
      required('getProgramParameter', 2, arguments); const s = state(this), p = owned(s, program, 'Program', false, true); if (!p) return null;
      if (p.deleted && !call(s, 'isProgram', p.id).value) { flag(s, GL.INVALID_VALUE); return null; }
      switch (pname >>> 0) {
        case GL.LINK_STATUS: return call(s, 'programStatus', p.id).value;
        case GL.DELETE_STATUS: return p.deleted;
        case GL.ATTACHED_SHADERS: return p.shaders.size;
        case GL.ACTIVE_ATTRIBUTES: { const r = call(s, 'activeAttribs', p.id); return r.error ? null : r.value.length; }
        case GL.ACTIVE_UNIFORMS: { const r = call(s, 'activeUniforms', p.id); return r.error ? null : r.value.length; }
        default: flag(s, GL.INVALID_ENUM); return null;
      }
    }
    getProgramInfoLog(program) { const s = state(this), p = owned(s, program, 'Program', false, true); return p ? call(s, 'programLog', p.id).value : null; }
    useProgram(program) { required('useProgram', 1, arguments); const s = state(this), p = owned(s, program, 'Program', true); if (p && !call(s, 'useProgram', p.id).error) s.program = program; }
    deleteProgram(program) { required('deleteProgram', 1, arguments); const s = state(this); if (program === null) return; const p = owned(s, program, 'Program', false, true); if (p && !p.deleted && !call(s, 'deleteProgram', p.id).error) p.deleted = true; }
    isProgram(program) { required('isProgram', 1, arguments); const s = state(this), p = objects.get(program); return !!p && p.context === s && p.kind === 'Program' && !s.lost && !!call(s, 'isProgram', p.id).value; }
    getAttribLocation(program, name) { required('getAttribLocation', 2, arguments); const s = state(this), p = owned(s, program, 'Program'); if (!p) return -1; const r = call(s, 'attribLocation', p.id, String(name)); return r.error ? -1 : r.value; }
    getUniformLocation(program, name) {
      required('getUniformLocation', 2, arguments); const s = state(this), p = owned(s, program, 'Program'); if (!p) return null;
      const r = call(s, 'uniformLocation', p.id, String(name)); if (r.error || r.value < 0) return null;
      const location = Object.create(WebGLUniformLocation.prototype); locations.set(location, { context: s, program, generation: p.generation, id: r.value }); return location;
    }
    getActiveAttrib(program, index) { required('getActiveAttrib', 2, arguments); return activeInfo(state(this), program, index, 'activeAttribs'); }
    getActiveUniform(program, index) { required('getActiveUniform', 2, arguments); return activeInfo(state(this), program, index, 'activeUniforms'); }
    getUniform(program, location) {
      required('getUniform', 2, arguments); const s = state(this), p = owned(s, program, 'Program'); if (!p) return null;
      const loc = locations.get(location); if (!loc) throw new TypeError('Expected WebGLUniformLocation');
      if (loc.context !== s || loc.program !== program || loc.generation !== p.generation) { flag(s, GL.INVALID_OPERATION); return null; }
      const r = call(s, 'getUniform', p.id, loc.id); if (r.error) return null;
      const values = new Float32Array(r.value); return values.length === 1 ? values[0] : values;
    }
    uniform1i(location, value) { required('uniform1i', 2, arguments); const s = state(this), loc = uniformLocation(s, location); if (loc) call(s, 'uniformI', loc.id, new Int32Array([value | 0])); }
    uniform1iv(location, values) { required('uniform1iv', 2, arguments); const s = state(this), loc = uniformLocation(s, location); if (!loc) return; values = ints(values); if (values.length !== 1) { flag(s, GL.INVALID_OPERATION); return; } call(s, 'uniformI', loc.id, values); }
    uniform1f(location, x) { required('uniform1f', 2, arguments); uniform(state(this), location, [+(x)]); }
    uniform2f(location, x, y) { required('uniform2f', 3, arguments); uniform(state(this), location, [+(x), +(y)]); }
    uniform3f(location, x, y, z) { required('uniform3f', 4, arguments); uniform(state(this), location, [+(x), +(y), +(z)]); }
    uniform4f(location, x, y, z, w) { required('uniform4f', 5, arguments); uniform(state(this), location, [+(x), +(y), +(z), +(w)]); }
    uniform1fv(location, value) { required('uniform1fv', 2, arguments); uniform(state(this), location, value); }
    uniform2fv(location, value) { required('uniform2fv', 2, arguments); uniform(state(this), location, value); }
    uniform3fv(location, value) { required('uniform3fv', 2, arguments); uniform(state(this), location, value); }
    uniform4fv(location, value) { required('uniform4fv', 2, arguments); uniform(state(this), location, value); }
    uniformMatrix4fv(location, transpose, value) { required('uniformMatrix4fv', 3, arguments); matrix(state(this), location, !!transpose, value); }
    createBuffer() { const s = state(this), r = call(s, 'createBuffer'); return r.error ? null : wrap(s, 'Buffer', r.value, { target: null, size: 0, usage: GL.STATIC_DRAW }); }
    bindBuffer(target, object) {
      required('bindBuffer', 2, arguments); const s = state(this), b = owned(s, object, 'Buffer', true); target >>>= 0;
      if (b && !call(s, 'bindBuffer', target, b.id).error) { s.bindings[target] = object; if (object) b.target = target; }
    }
    bufferData(target, data, usage) {
      required('bufferData', 3, arguments); const s = state(this); target >>>= 0; const b = buffer(s, target); if (!b) return;
      usage >>>= 0; if (![GL.STATIC_DRAW, GL.DYNAMIC_DRAW, GL.STREAM_DRAW].includes(usage)) { flag(s, GL.INVALID_ENUM); return; }
      let r, size;
      if (typeof data === 'number') { size = offset(s, data); if (size === null) return; if (size > limits.maxUploadBytes) { flag(s, GL.OUT_OF_MEMORY); return; } r = call(s, 'bufferSize', target, size); }
      else { const bytes = data === null ? new Uint8Array(0) : dataView(data); size = bytes.byteLength; if (size > limits.maxUploadBytes) { flag(s, GL.OUT_OF_MEMORY); return; } r = call(s, 'bufferData', target, bytes); }
      if (!r.error) { b.size = size; b.usage = usage; }
    }
    bufferSubData(target, destination, data) { required('bufferSubData', 3, arguments); const s = state(this); target >>>= 0; const b = buffer(s, target); if (!b) return; destination = offset(s, destination); if (destination === null) return; const bytes = dataView(data); if (destination + bytes.byteLength > b.size) { flag(s, GL.INVALID_VALUE); return; } call(s, 'bufferSubData', target, destination, bytes); }
    getBufferParameter(target, pname) { required('getBufferParameter', 2, arguments); const s = state(this), b = buffer(s, target); if (!b) return null; if ((pname >>> 0) === GL.BUFFER_SIZE) return b.size; if ((pname >>> 0) === GL.BUFFER_USAGE) return b.usage; flag(s, GL.INVALID_ENUM); return null; }
    deleteBuffer(object) {
      required('deleteBuffer', 1, arguments); const s = state(this); if (object === null) return; const b = owned(s, object, 'Buffer', false, true);
      if (b && !b.deleted && !call(s, 'deleteBuffer', b.id).error) { b.deleted = true; for (const key of Object.keys(s.bindings)) if (s.bindings[key] === object) s.bindings[key] = null; for (const a of s.attribs) if (a.buffer === object) a.buffer = null; }
    }
    isBuffer(object) { required('isBuffer', 1, arguments); const s = state(this), b = objects.get(object); return !!b && b.context === s && b.kind === 'Buffer' && !b.deleted && b.target !== null && !s.lost; }
    vertexAttribPointer(location, size, type, normalized, stride, byteOffset) {
      required('vertexAttribPointer', 6, arguments); const s = state(this), i = index(s, location); if (i === null) return; size |= 0; stride |= 0; byteOffset = offset(s, byteOffset); if (byteOffset === null) return;
      if (size < 1 || size > 4 || stride < 0 || stride > 255) { flag(s, GL.INVALID_VALUE); return; }
      if (!call(s, 'attribPointer', i, size, type >>> 0, !!normalized, stride, byteOffset).error) Object.assign(s.attribs[i], { size, type: type >>> 0, normalized: !!normalized, stride, offset: byteOffset, buffer: s.bindings[GL.ARRAY_BUFFER] || null });
    }
    enableVertexAttribArray(location) { required('enableVertexAttribArray', 1, arguments); const s = state(this), i = index(s, location); if (i !== null && !call(s, 'enableAttrib', i, true).error) s.attribs[i].enabled = true; }
    disableVertexAttribArray(location) { required('disableVertexAttribArray', 1, arguments); const s = state(this), i = index(s, location); if (i !== null && !call(s, 'enableAttrib', i, false).error) s.attribs[i].enabled = false; }
    getVertexAttrib(location, pname) {
      required('getVertexAttrib', 2, arguments); const s = state(this), i = index(s, location); if (i === null) return null; const a = s.attribs[i]; pname >>>= 0;
      if (pname === 0x88FE) { if (s.version === 2) return a.divisor; flag(s, GL.INVALID_ENUM); return null; }
      switch (pname) { case GL.VERTEX_ATTRIB_ARRAY_ENABLED: return a.enabled; case GL.VERTEX_ATTRIB_ARRAY_BUFFER_BINDING: return a.buffer; case GL.VERTEX_ATTRIB_ARRAY_SIZE: return a.size; case GL.VERTEX_ATTRIB_ARRAY_TYPE: return a.type; case GL.VERTEX_ATTRIB_ARRAY_NORMALIZED: return a.normalized; case GL.VERTEX_ATTRIB_ARRAY_STRIDE: return a.stride; case GL.CURRENT_VERTEX_ATTRIB: return new Float32Array(a.current); default: flag(s, GL.INVALID_ENUM); return null; }
    }
    getVertexAttribOffset(location, pname) { required('getVertexAttribOffset', 2, arguments); const s = state(this), i = index(s, location); if (i === null) return 0; if ((pname >>> 0) !== GL.VERTEX_ATTRIB_ARRAY_POINTER) { flag(s, GL.INVALID_ENUM); return 0; } return s.attribs[i].offset; }
    viewport(x, y, width, height) { required('viewport', 4, arguments); const s = state(this); width |= 0; height |= 0; if (width < 0 || height < 0) { flag(s, GL.INVALID_VALUE); return; } x |= 0; y |= 0; if (!call(s, 'viewport', x, y, width, height).error) s.viewport = [x, y, Math.min(width, limits.maxDimension), Math.min(height, limits.maxDimension)]; }
    scissor(x, y, width, height) { required('scissor', 4, arguments); const s = state(this); width |= 0; height |= 0; if (width < 0 || height < 0) { flag(s, GL.INVALID_VALUE); return; } x |= 0; y |= 0; if (!call(s, 'scissor', x, y, width, height).error) s.scissor = [x, y, width, height]; }
    enable(cap) { required('enable', 1, arguments); capability(state(this), cap >>> 0, true); }
    disable(cap) { required('disable', 1, arguments); capability(state(this), cap >>> 0, false); }
    isEnabled(cap) { required('isEnabled', 1, arguments); const s = state(this); cap >>>= 0; syncSize(s); if (s.lost) return false; if (cap === GL.SCISSOR_TEST) return s.scissorEnabled; if (cap === GL.DEPTH_TEST) return s.depthEnabled; if (cap === GL.BLEND) return s.blendEnabled; flag(s, GL.INVALID_ENUM); return false; }
    blendFunc(source, destination) {
      required('blendFunc', 2, arguments); const s = state(this); source >>>= 0; destination >>>= 0;
      if (!call(s, 'blendFunc', source, destination, source, destination).error) s.blendFactors = [source, destination, source, destination];
    }
    blendFuncSeparate(srcRGB, dstRGB, srcAlpha, dstAlpha) {
      required('blendFuncSeparate', 4, arguments); const s = state(this); srcRGB >>>= 0; dstRGB >>>= 0; srcAlpha >>>= 0; dstAlpha >>>= 0;
      if (!call(s, 'blendFunc', srcRGB, dstRGB, srcAlpha, dstAlpha).error) s.blendFactors = [srcRGB, dstRGB, srcAlpha, dstAlpha];
    }
    blendEquation(equation) {
      required('blendEquation', 1, arguments); const s = state(this); equation >>>= 0;
      if (!call(s, 'blendEquation', equation, equation).error) s.blendEquations = [equation, equation];
    }
    blendEquationSeparate(rgb, alpha) {
      required('blendEquationSeparate', 2, arguments); const s = state(this); rgb >>>= 0; alpha >>>= 0;
      if (!call(s, 'blendEquation', rgb, alpha).error) s.blendEquations = [rgb, alpha];
    }
    blendColor(r, green, b, a) {
      required('blendColor', 4, arguments); const s = state(this), color = new Float32Array([+r, +green, +b, +a]);
      if (!call(s, 'blendColor', ...color).error) s.blendColor = Array.from(color, clamp);
    }
    clearColor(r, green, b, a) { required('clearColor', 4, arguments); const s = state(this), color = [+(r), +(green), +(b), +(a)]; if (!call(s, 'clearColor', ...color).error) s.clearColor = color.map(clamp); }
    clearDepth(depth) { required('clearDepth', 1, arguments); const s = state(this); depth = +(depth); if (!call(s, 'clearDepth', depth).error) s.clearDepth = clamp(depth); }
    colorMask(r, green, b, a) { required('colorMask', 4, arguments); const s = state(this), mask = [!!r, !!green, !!b, !!a]; if (!call(s, 'colorMask', ...mask).error) s.colorMask = mask; }
    depthMask(value) { required('depthMask', 1, arguments); const s = state(this); if (!call(s, 'depthMask', !!value).error) s.depthMask = !!value; }
    depthFunc(func) { required('depthFunc', 1, arguments); const s = state(this); if (!call(s, 'depthFunc', func >>> 0).error) s.depthFunc = func >>> 0; }
    clear(mask) { required('clear', 1, arguments); const s = state(this); changed(s, call(s, 'clear', mask >>> 0)); }
    drawArrays(mode, first, count) { required('drawArrays', 3, arguments); const s = state(this); first |= 0; count |= 0; if (first < 0 || count < 0) { flag(s, GL.INVALID_VALUE); return; } changed(s, call(s, 'drawArrays', mode >>> 0, first, count)); }
    drawElements(mode, count, type, byteOffset) { required('drawElements', 4, arguments); const s = state(this); count |= 0; byteOffset = offset(s, byteOffset); if (count < 0) { flag(s, GL.INVALID_VALUE); return; } if (byteOffset !== null) changed(s, call(s, 'drawElements', mode >>> 0, count, type >>> 0, byteOffset)); }
    readPixels(x, y, width, height, format, type, pixels) {
      required('readPixels', 7, arguments); const s = state(this); width |= 0; height |= 0; x |= 0; y |= 0;
      if (width < 0 || height < 0) { flag(s, GL.INVALID_VALUE); return; }
      if ((format >>> 0) !== GL.RGBA || (type >>> 0) !== GL.UNSIGNED_BYTE) { flag(s, GL.INVALID_OPERATION); return; }
      if (pixels !== null && !ArrayBuffer.isView(pixels)) throw new TypeError('Expected an ArrayBufferView');
      if (pixels === null) { flag(s, GL.INVALID_VALUE); return; }
      if (!(pixels instanceof Uint8Array) && !(pixels instanceof Uint8ClampedArray)) { flag(s, GL.INVALID_OPERATION); return; }
      const row = width * 4, stride = Math.ceil(row / s.packAlignment) * s.packAlignment;
      const length = height ? (height - 1) * stride + row : 0;
      if (!Number.isSafeInteger(length) || length > pixels.byteLength) { flag(s, GL.INVALID_OPERATION); return; }
      const r = call(s, 'readPixels', x, y, width, height); if (r.error) return;
      const bytes = new Uint8Array(r.value);
      const bw = call(s, 'readWidth').value, bh = call(s, 'readHeight').value;
      const first = Math.max(0, -x), last = Math.min(width, bw - x);
      for (let iy = 0; iy < height; iy++) if (y + iy >= 0 && y + iy < bh && last > first) {
        pixels.set(bytes.subarray(iy * row + first * 4, iy * row + last * 4), iy * stride + first * 4);
      }
    }
    createTexture() { const s = state(this), r = call(s, 'createTexture'); return r.error ? null : wrap(s, 'Texture', r.value, { bound: false }); }
    activeTexture(unit) { required('activeTexture', 1, arguments); const s = state(this); unit >>>= 0; if (!call(s, 'activeTexture', unit).error) s.activeTexture = unit - GL.TEXTURE0; }
    bindTexture(target, object) { required('bindTexture', 2, arguments); const s = state(this), t = owned(s, object, 'Texture', true); target >>>= 0; if (t && !call(s, 'bindTexture', target, t.id).error) { s.textures[s.activeTexture] = object; if (object) t.bound = true; } }
    deleteTexture(object) { required('deleteTexture', 1, arguments); const s = state(this); if (object === null) return; const t = owned(s, object, 'Texture', false, true); if (t && !t.deleted && !call(s, 'deleteTexture', t.id).error) { t.deleted = true; for (let i = 0; i < s.textures.length; i++) if (s.textures[i] === object) s.textures[i] = null; detachBoundAttachment(s, object, GL.COLOR_ATTACHMENT0); } }
    isTexture(object) { required('isTexture', 1, arguments); const s = state(this), t = objects.get(object); return !!t && t.context === s && t.kind === 'Texture' && t.bound && !t.deleted && !s.lost; }
    texParameteri(target, pname, value) { required('texParameteri', 3, arguments); call(state(this), 'texParameter', target >>> 0, pname >>> 0, value | 0); }
    texParameterf(target, pname, value) { required('texParameterf', 3, arguments); const s = state(this); value = +(value); if (!Number.isInteger(value)) { flag(s, GL.INVALID_ENUM); return; } call(s, 'texParameter', target >>> 0, pname >>> 0, value); }
    getTexParameter(target, pname) { required('getTexParameter', 2, arguments); return call(state(this), 'getTexParameter', target >>> 0, pname >>> 0).value; }
    texImage2D(...args) { uploadTexture(state(this), false, args); }
    texSubImage2D(...args) { uploadTexture(state(this), true, args); }
    pixelStorei(pname, value) {
      required('pixelStorei', 2, arguments); const s = state(this); pname >>>= 0;
      if (pname === GL.PACK_ALIGNMENT || pname === GL.UNPACK_ALIGNMENT) { value |= 0; if (![1, 2, 4, 8].includes(value)) { flag(s, GL.INVALID_VALUE); return; } if (pname === GL.PACK_ALIGNMENT) s.packAlignment = value; else s.unpackAlignment = value; return; }
      if (pname === GL.UNPACK_FLIP_Y_WEBGL) { s.flipY = !!value; return; } if (pname === GL.UNPACK_PREMULTIPLY_ALPHA_WEBGL) { s.unpackPremultiplied = !!value; return; }
      if (pname === GL.UNPACK_COLORSPACE_CONVERSION_WEBGL) { value >>>= 0; if (value !== GL.NONE && value !== GL.BROWSER_DEFAULT_WEBGL) { flag(s, GL.INVALID_VALUE); return; } s.unpackColorspace = value; return; }
      flag(s, GL.INVALID_ENUM);
    }
    getParameter(pname) {
      required('getParameter', 1, arguments); const s = state(this); syncSize(s); if (s.lost) return null;
      pname >>>= 0;
      if (s.drawFramebuffer && pname >= GL.RED_BITS && pname <= GL.STENCIL_BITS) { const r = call(s, 'framebufferBits', pname); return r.error ? null : r.value; }
      switch (pname) {
        case GL.VERSION: return s.version === 2 ? 'WebGL 2.0 (RabbitHole experimental software subset)' : 'WebGL 1.0 (RabbitHole experimental software subset)';
        case GL.SHADING_LANGUAGE_VERSION: return s.version === 2 ? 'WebGL GLSL ES 3.00 (experimental subset)' : 'WebGL GLSL ES 1.0 (experimental subset)';
        case GL.VENDOR: return 'RabbitHole'; case GL.RENDERER: return 'RabbitHole software rasterizer (experimental)';
        case GL.SHADER_COMPILER: return true;
        case 0x85B5: if (s.version === 2) return s.currentVAO; flag(s, GL.INVALID_ENUM); return null;
        case 0x8D6B: if (s.version === 2) return 4294967294; flag(s, GL.INVALID_ENUM); return null;
        case GL.MAX_VERTEX_ATTRIBS: return limits.maxVertexAttribs;
        case GL.MAX_VIEWPORT_DIMS: return new Int32Array([limits.maxDimension, limits.maxDimension]);
        case GL.MAX_RENDERBUFFER_SIZE: case GL.MAX_TEXTURE_SIZE: return limits.maxDimension;
        case GL.MAX_TEXTURE_IMAGE_UNITS: case GL.MAX_COMBINED_TEXTURE_IMAGE_UNITS: return limits.maxTextureUnits;
        case GL.ACTIVE_TEXTURE: return GL.TEXTURE0 + s.activeTexture; case GL.TEXTURE_BINDING_2D: return s.textures[s.activeTexture] || null;
        case GL.VIEWPORT: return new Int32Array(s.viewport); case GL.SCISSOR_BOX: return new Int32Array(s.scissor);
        case GL.BLEND: return s.blendEnabled;
        case GL.BLEND_SRC_RGB: return s.blendFactors[0]; case GL.BLEND_DST_RGB: return s.blendFactors[1];
        case GL.BLEND_SRC_ALPHA: return s.blendFactors[2]; case GL.BLEND_DST_ALPHA: return s.blendFactors[3];
        case GL.BLEND_EQUATION_RGB: return s.blendEquations[0]; case GL.BLEND_EQUATION_ALPHA: return s.blendEquations[1];
        case GL.BLEND_COLOR: return new Float32Array(s.blendColor);
        case GL.SCISSOR_TEST: return s.scissorEnabled; case GL.DEPTH_TEST: return s.depthEnabled;
        case GL.COLOR_CLEAR_VALUE: return new Float32Array(s.clearColor); case GL.COLOR_WRITEMASK: return s.colorMask.slice();
        case GL.DEPTH_CLEAR_VALUE: return s.clearDepth; case GL.DEPTH_WRITEMASK: return s.depthMask; case GL.DEPTH_FUNC: return s.depthFunc;
        case GL.CURRENT_PROGRAM: return s.program;
        case GL.ARRAY_BUFFER_BINDING: return s.bindings[GL.ARRAY_BUFFER] || null; case GL.ELEMENT_ARRAY_BUFFER_BINDING: return s.bindings[GL.ELEMENT_ARRAY_BUFFER] || null;
        case GL.PACK_ALIGNMENT: return s.packAlignment; case GL.UNPACK_ALIGNMENT: return s.unpackAlignment;
        case GL.UNPACK_FLIP_Y_WEBGL: return s.flipY; case GL.UNPACK_PREMULTIPLY_ALPHA_WEBGL: return s.unpackPremultiplied; case GL.UNPACK_COLORSPACE_CONVERSION_WEBGL: return s.unpackColorspace;
        case GL.IMPLEMENTATION_COLOR_READ_FORMAT: return GL.RGBA; case GL.IMPLEMENTATION_COLOR_READ_TYPE: return GL.UNSIGNED_BYTE;
        case GL.RED_BITS: case GL.GREEN_BITS: case GL.BLUE_BITS: return 8; case GL.ALPHA_BITS: return s.attrs.alpha ? 8 : 0;
        case GL.DEPTH_BITS: return s.attrs.depth ? 24 : 0; case GL.STENCIL_BITS: case GL.SAMPLES: case GL.SAMPLE_BUFFERS: return 0;
        case GL.FRAMEBUFFER_BINDING: return s.drawFramebuffer;
        case 0x8CAA: if (s.version === 2) return s.readFramebuffer; flag(s, GL.INVALID_ENUM); return null;
        case GL.RENDERBUFFER_BINDING: return s.renderbuffer;
        default: flag(s, GL.INVALID_ENUM); return null;
      }
    }
    getShaderPrecisionFormat(shaderType, precisionType) {
      required('getShaderPrecisionFormat', 2, arguments); const s = state(this); if (![GL.VERTEX_SHADER, GL.FRAGMENT_SHADER].includes(shaderType >>> 0) || ![GL.LOW_FLOAT, GL.MEDIUM_FLOAT, GL.HIGH_FLOAT].includes(precisionType >>> 0)) { flag(s, GL.INVALID_ENUM); return null; }
      return Object.assign(Object.create(WebGLShaderPrecisionFormat.prototype), { rangeMin: 127, rangeMax: 127, precision: 23 });
    }
    createFramebuffer() { const s = state(this), r = call(s, 'createFramebuffer'); return r.error ? null : wrap(s, 'Framebuffer', r.value, { bound: false, attachments: {} }); }
    bindFramebuffer(target, object) {
      required('bindFramebuffer', 2, arguments); const s = state(this); target >>>= 0;
      const f = owned(s, object, 'Framebuffer', true); if (!f || !framebufferTarget(s, target)) return;
      if (!call(s, 'bindFramebuffer', target, f.id).error) { if (target !== 0x8CA8) s.drawFramebuffer = object; if (target !== 0x8CA9) s.readFramebuffer = object; f.bound = true; }
    }
    deleteFramebuffer(object) {
      required('deleteFramebuffer', 1, arguments); const s = state(this); if (object === null) return;
      const f = owned(s, object, 'Framebuffer', false, true);
      if (f && !f.deleted && !call(s, 'deleteFramebuffer', f.id).error) { f.deleted = true; f.attachments = {}; if (s.drawFramebuffer === object) s.drawFramebuffer = null; if (s.readFramebuffer === object) s.readFramebuffer = null; }
    }
    isFramebuffer(object) { required('isFramebuffer', 1, arguments); const s = state(this); syncSize(s); const f = objects.get(object); return !!f && f.context === s && f.kind === 'Framebuffer' && f.bound && !f.deleted && !s.lost; }
    checkFramebufferStatus(target) { required('checkFramebufferStatus', 1, arguments); const s = state(this); target >>>= 0; if (!framebufferTarget(s, target)) return 0; const r = call(s, 'checkFramebuffer', target); return r.error ? 0 : r.value; }
    framebufferTexture2D(target, attachment, textarget, texture, level) {
      required('framebufferTexture2D', 5, arguments); const s = state(this); target >>>= 0; attachment >>>= 0; textarget >>>= 0; level |= 0;
      const t = owned(s, texture, 'Texture', true); if (!t || !framebufferTarget(s, target)) return;
      if (!call(s, 'framebufferTexture', target, attachment, textarget, t.id, level).error) objects.get(framebufferObject(s, target)).attachments[attachment] = texture;
    }
    createRenderbuffer() { const s = state(this), r = call(s, 'createRenderbuffer'); return r.error ? null : wrap(s, 'Renderbuffer', r.value, { bound: false }); }
    bindRenderbuffer(target, object) {
      required('bindRenderbuffer', 2, arguments); const s = state(this); target >>>= 0; const r = owned(s, object, 'Renderbuffer', true); if (!r) return;
      if (!call(s, 'bindRenderbuffer', target, r.id).error) { s.renderbuffer = object; r.bound = true; }
    }
    deleteRenderbuffer(object) {
      required('deleteRenderbuffer', 1, arguments); const s = state(this); if (object === null) return; const r = owned(s, object, 'Renderbuffer', false, true);
      if (r && !r.deleted && !call(s, 'deleteRenderbuffer', r.id).error) { r.deleted = true; if (s.renderbuffer === object) s.renderbuffer = null; detachBoundAttachment(s, object, GL.DEPTH_ATTACHMENT); }
    }
    isRenderbuffer(object) { required('isRenderbuffer', 1, arguments); const s = state(this); syncSize(s); const r = objects.get(object); return !!r && r.context === s && r.kind === 'Renderbuffer' && r.bound && !r.deleted && !s.lost; }
    renderbufferStorage(target, format, width, height) { required('renderbufferStorage', 4, arguments); const s = state(this); target >>>= 0; format >>>= 0; width |= 0; height |= 0; if (width < 0 || height < 0) { flag(s, GL.INVALID_VALUE); return; } call(s, 'renderbufferStorage', target, format, width, height); }
    framebufferRenderbuffer(target, attachment, rbtarget, object) {
      required('framebufferRenderbuffer', 4, arguments); const s = state(this); target >>>= 0; attachment >>>= 0; rbtarget >>>= 0;
      const r = owned(s, object, 'Renderbuffer', true); if (!r || !framebufferTarget(s, target)) return;
      if (!call(s, 'framebufferRenderbuffer', target, attachment, rbtarget, r.id).error) objects.get(framebufferObject(s, target)).attachments[attachment] = object;
    }
    getRenderbufferParameter(target, pname) { required('getRenderbufferParameter', 2, arguments); const s = state(this), r = call(s, 'renderbufferParameter', target >>> 0, pname >>> 0); return r.error ? null : r.value; }
    getFramebufferAttachmentParameter(target, attachment, pname) {
      required('getFramebufferAttachmentParameter', 3, arguments); const s = state(this); target >>>= 0; attachment >>>= 0; pname >>>= 0;
      if (!framebufferTarget(s, target)) return null; const r = call(s, 'framebufferAttachment', target, attachment, pname); if (r.error) return null;
      return pname === GL.FRAMEBUFFER_ATTACHMENT_OBJECT_NAME ? objects.get(framebufferObject(s, target)).attachments[attachment] || null : r.value;
    }
    flush() { const s = state(this); syncSize(s); if (!s.lost) N.canvasMarkDirty(s.surface); }
    finish() { const s = state(this); syncSize(s); if (!s.lost) N.canvasMarkDirty(s.surface); }
  }

  function activeInfo(s, program, index, command) {
    const p = owned(s, program, 'Program'); if (!p) return null;
    const r = call(s, command, p.id); if (r.error) return null; index >>>= 0;
    if (index >= r.value.length) { flag(s, GL.INVALID_VALUE); return null; }
    const item = r.value[index], result = Object.create(WebGLActiveInfo.prototype);
    for (const key of ['name', 'size', 'type']) Object.defineProperty(result, key, { value: item[key], enumerable: true });
    return result;
  }
  function uploadTexture(s, sub, args) {
    const typed = args.length === 9;
    const sourced = sub ? args.length === 7 : args.length === 6;
    if (!typed && !sourced) throw new TypeError('No matching texImage2D/texSubImage2D overload');
    const target = args[0] >>> 0, level = args[1] | 0;
    let width, height, border = 0, internal, format, type, pixels, bitmap = false;
    const x = sub ? args[2] | 0 : 0, y = sub ? args[3] | 0 : 0;
    if (typed) {
      width = args[sub ? 4 : 3] | 0; height = args[sub ? 5 : 4] | 0; border = sub ? 0 : args[5] | 0;
      internal = sub ? 0 : args[2] >>> 0; format = args[6] >>> 0; type = args[7] >>> 0; pixels = args[8];
      if (pixels != null && !ArrayBuffer.isView(pixels)) throw new TypeError('Expected an ArrayBufferView');
      if (pixels != null && !(pixels instanceof Uint8Array) && !(pixels instanceof Uint8ClampedArray)) { flag(s, GL.INVALID_OPERATION); return; }
    } else {
      internal = sub ? 0 : args[2] >>> 0; format = args[sub ? 4 : 3] >>> 0; type = args[sub ? 5 : 4] >>> 0;
      const source = args[sub ? 6 : 5]; bitmap = source instanceof g.ImageBitmap;
      const image = N.canvasSourcePixels(source); if (!image) { flag(s, GL.INVALID_VALUE); return; }
      if (source.colorSpace && source.colorSpace !== 'srgb' && s.unpackColorspace !== GL.NONE) { flag(s, GL.INVALID_OPERATION); return; }
      width = image.width; height = image.height; pixels = image.data;
    }
    if (width < 0 || height < 0 || border !== 0 || (sub && (x < 0 || y < 0))) { flag(s, GL.INVALID_VALUE); return; }
    if (width > limits.maxDimension || height > limits.maxDimension || width * height > limits.maxPixels) { flag(s, GL.OUT_OF_MEMORY); return; }
    if (format !== GL.RGBA || type !== GL.UNSIGNED_BYTE || (!sub && internal !== GL.RGBA)) { flag(s, GL.INVALID_ENUM); return; }
    let bytes = null;
    if (pixels != null) {
      const source = dataView(pixels), row = width * 4, stride = typed ? Math.ceil(row / s.unpackAlignment) * s.unpackAlignment : row;
      const length = height ? (height - 1) * stride + row : 0;
      if (length > source.byteLength) { flag(s, GL.INVALID_OPERATION); return; }
      bytes = new Uint8Array(width * height * 4);
      for (let iy = 0; iy < height; iy++) {
        const from = (!bitmap && s.flipY ? height - iy - 1 : iy) * stride;
        bytes.set(source.subarray(from, from + row), iy * row);
      }
      if (!bitmap && s.unpackPremultiplied) for (let i = 0; i < bytes.length; i += 4) for (let c = 0; c < 3; c++) bytes[i + c] = Math.round(bytes[i + c] * bytes[i + 3] / 255);
    } else if (sub) { flag(s, GL.INVALID_VALUE); return; }
    if (sub) call(s, 'texSubImage', target, level, x, y, width, height, format, type, bytes);
    else call(s, 'texImage', target, level, internal, width, height, border, format, type, bytes);
  }

  function capability(s, cap, enabled) {
    if (cap === GL.SCISSOR_TEST) { if (!call(s, 'enableScissor', enabled).error) s.scissorEnabled = enabled; return; }
    if (cap === GL.BLEND) { if (!call(s, 'enableBlend', enabled).error) s.blendEnabled = enabled; return; }
    if (cap === GL.DEPTH_TEST) { if (!call(s, 'enableDepth', enabled).error) s.depthEnabled = enabled; return; }
    flag(s, [GL.BLEND, GL.CULL_FACE, GL.STENCIL_TEST, GL.DITHER, GL.POLYGON_OFFSET_FILL, GL.SAMPLE_ALPHA_TO_COVERAGE, GL.SAMPLE_COVERAGE].includes(cap) ? GL.INVALID_OPERATION : GL.INVALID_ENUM);
  }
  for (let size = 1; size <= 4; size++) {
    hidden(WebGLRenderingContext.prototype, 'vertexAttrib' + size + 'f', function (location, ...values) {
      required('vertexAttrib' + size + 'f', size + 1, arguments); const s = state(this), i = index(s, location); if (i === null) return; const v = [0, 0, 0, 1]; for (let j = 0; j < size; j++) v[j] = +(values[j]); if (!call(s, 'attrib', i, ...v).error) s.attribs[i].current = v;
    });
    hidden(WebGLRenderingContext.prototype, 'vertexAttrib' + size + 'fv', function (location, values) {
      required('vertexAttrib' + size + 'fv', 2, arguments); const s = state(this), i = index(s, location); if (i === null) return; values = floats(values); if (values.length < size) { flag(s, GL.INVALID_VALUE); return; } const v = [0, 0, 0, 1]; for (let j = 0; j < size; j++) v[j] = values[j]; if (!call(s, 'attrib', i, ...v).error) s.attribs[i].current = v;
    });
  }
  // These operations are deliberately unavailable until their rendering
  // semantics exist. Libraries get an observable GL error, never fake IDs.
  for (const name of ['generateMipmap', 'compressedTexImage2D', 'compressedTexSubImage2D', 'copyTexImage2D', 'copyTexSubImage2D', 'cullFace', 'frontFace', 'polygonOffset', 'sampleCoverage', 'stencilFunc', 'stencilFuncSeparate', 'stencilMask', 'stencilMaskSeparate', 'stencilOp', 'stencilOpSeparate', 'clearStencil', 'depthRange', 'lineWidth', 'detachShader', 'validateProgram', 'uniform2i', 'uniform3i', 'uniform4i', 'uniform2iv', 'uniform3iv', 'uniform4iv', 'uniformMatrix2fv', 'uniformMatrix3fv']) hidden(WebGLRenderingContext.prototype, name, function () { flag(state(this), GL.INVALID_OPERATION); });
  for (const [name, value] of Object.entries(GL)) {
    Object.defineProperty(WebGLRenderingContext, name, { value, enumerable: true });
    Object.defineProperty(WebGLRenderingContext.prototype, name, { value, enumerable: true });
  }
  class WebGL2RenderingContext { constructor() { illegal(); }
    blitFramebuffer(srcX0, srcY0, srcX1, srcY1, dstX0, dstY0, dstX1, dstY1, mask, filter) {
      required('blitFramebuffer', 10, arguments); const s = state(this); if (s.version !== 2) throw new TypeError('Illegal invocation');
      srcX0 |= 0; srcY0 |= 0; srcX1 |= 0; srcY1 |= 0; dstX0 |= 0; dstY0 |= 0; dstX1 |= 0; dstY1 |= 0; mask >>>= 0; filter >>>= 0;
      changed(s, call(s, 'blitFramebuffer', srcX0, srcY0, srcX1, srcY1, dstX0, dstY0, dstX1, dstY1, mask, filter));
    }
    vertexAttribDivisor(location, divisor) {
      required('vertexAttribDivisor', 2, arguments); const s = state(this); if (s.version !== 2) throw new TypeError('Illegal invocation'); location >>>= 0; divisor >>>= 0; const i = index(s, location);
      if (i !== null && !call(s, 'attribDivisor', i, divisor).error) s.attribs[i].divisor = divisor;
    }
    drawArraysInstanced(mode, first, count, instances) {
      required('drawArraysInstanced', 4, arguments); const s = state(this); if (s.version !== 2) throw new TypeError('Illegal invocation'); mode >>>= 0; first |= 0; count |= 0; instances |= 0;
      if (first < 0 || count < 0 || instances < 0) { flag(s, GL.INVALID_VALUE); return; }
      changed(s, call(s, 'drawArraysInstanced', mode, first, count, instances));
    }
    drawElementsInstanced(mode, count, type, byteOffset, instances) {
      required('drawElementsInstanced', 5, arguments); const s = state(this); if (s.version !== 2) throw new TypeError('Illegal invocation'); mode >>>= 0; count |= 0; type >>>= 0; byteOffset = +byteOffset; instances |= 0; byteOffset = offset(s, byteOffset);
      if (count < 0 || instances < 0) { flag(s, GL.INVALID_VALUE); return; }
      if (byteOffset !== null) changed(s, call(s, 'drawElementsInstanced', mode, count, type, byteOffset, instances));
    }
    createVertexArray() { const s = state(this); if (s.version !== 2) throw new TypeError('Illegal invocation'); const r = call(s, 'createVertexArray'); return r.error ? null : wrap(s, 'VertexArrayObject', r.value, { bound: false, element: null, attribs: freshAttributes() }); }
    bindVertexArray(object) {
      required('bindVertexArray', 1, arguments); const s = state(this); if (s.version !== 2) throw new TypeError('Illegal invocation'); const v = owned(s, object, 'VertexArrayObject', true); if (!v || call(s, 'bindVertexArray', v.id).error) return;
      const old = s.currentVAO ? objects.get(s.currentVAO) : s.defaultVAO;
      old.element = s.bindings[GL.ELEMENT_ARRAY_BUFFER] || null; old.attribs = s.attribs;
      const next = object ? v : s.defaultVAO, constants = s.attribs.map(a => a.current);
      s.attribs = next.attribs; for (let i = 0; i < constants.length; i++) s.attribs[i].current = constants[i];
      s.bindings[GL.ELEMENT_ARRAY_BUFFER] = next.element; s.currentVAO = object; if (object) v.bound = true;
    }
    deleteVertexArray(object) {
      required('deleteVertexArray', 1, arguments); const s = state(this); if (s.version !== 2) throw new TypeError('Illegal invocation'); if (object === null) return;
      const v = owned(s, object, 'VertexArrayObject', false, true); if (!v || v.deleted) return;
      if (!call(s, 'deleteVertexArray', v.id).error) { if (s.currentVAO === object) this.bindVertexArray(null); v.deleted = true; }
    }
    isVertexArray(object) { required('isVertexArray', 1, arguments); const s = state(this); if (s.version !== 2) throw new TypeError('Illegal invocation'); const v = objects.get(object); return !!v && v.context === s && v.kind === 'VertexArrayObject' && !v.deleted && !s.lost && !!call(s, 'isVertexArray', v.id).value; }
  }
  function freshAttributes() { return Array.from({ length: limits.maxVertexAttribs }, () => ({ enabled: false, buffer: null, size: 4, type: GL.FLOAT, normalized: false, stride: 0, offset: 0, divisor: 0, current: [0, 0, 0, 1] })); }
  // These interfaces implement separate WebIDL mixins; WebGL2 is not a WebGL1 subclass.
  for (const key of Reflect.ownKeys(WebGLRenderingContext.prototype)) if (key !== 'constructor') Object.defineProperty(WebGL2RenderingContext.prototype, key, Object.getOwnPropertyDescriptor(WebGLRenderingContext.prototype, key));
  // WebGL2 adds source/destination-offset overloads; reject these until implemented.
  for (const [name, maxArgs] of Object.entries({ bufferData: 3, bufferSubData: 3, readPixels: 7, texImage2D: 9, texSubImage2D: 9, uniform1fv: 2, uniform2fv: 2, uniform3fv: 2, uniform4fv: 2, uniform1iv: 2, uniformMatrix4fv: 3 })) {
    const base = WebGL2RenderingContext.prototype[name];
    hidden(WebGL2RenderingContext.prototype, name, function (...args) { const s = state(this); if (s.version !== 2) throw new TypeError('Illegal invocation'); if (args.length > maxArgs) { flag(s, GL.INVALID_OPERATION); return null; } return base.apply(this, args); });
  }
  for (const [name, value] of Object.entries({ ...GL, VERTEX_ARRAY_BINDING: 0x85B5, MAX_ELEMENT_INDEX: 0x8D6B, VERTEX_ATTRIB_ARRAY_DIVISOR: 0x88FE, READ_FRAMEBUFFER: 0x8CA8, DRAW_FRAMEBUFFER: 0x8CA9, READ_FRAMEBUFFER_BINDING: 0x8CAA, DRAW_FRAMEBUFFER_BINDING: 0x8CA6 })) { Object.defineProperty(WebGL2RenderingContext, name, { value, enumerable: true }); if (!(name in WebGL2RenderingContext.prototype)) Object.defineProperty(WebGL2RenderingContext.prototype, name, { value, enumerable: true }); }
  for (const name of ['drawRangeElements', 'vertexAttribIPointer', 'vertexAttribI4i', 'vertexAttribI4ui', 'vertexAttribI4iv', 'vertexAttribI4uiv', 'texImage3D', 'texSubImage3D', 'copyTexSubImage3D', 'compressedTexImage3D', 'compressedTexSubImage3D', 'texStorage2D', 'texStorage3D', 'drawBuffers', 'readBuffer', 'invalidateFramebuffer', 'invalidateSubFramebuffer', 'framebufferTextureLayer', 'renderbufferStorageMultisample', 'copyBufferSubData', 'getBufferSubData', 'bindBufferBase', 'bindBufferRange', 'getIndexedParameter', 'getInternalformatParameter', 'getUniformIndices', 'getActiveUniforms', 'getUniformBlockIndex', 'getActiveUniformBlockParameter', 'getActiveUniformBlockName', 'uniformBlockBinding', 'createQuery', 'deleteQuery', 'isQuery', 'beginQuery', 'endQuery', 'getQuery', 'getQueryParameter', 'createSampler', 'deleteSampler', 'isSampler', 'bindSampler', 'samplerParameteri', 'samplerParameterf', 'getSamplerParameter', 'fenceSync', 'isSync', 'deleteSync', 'clientWaitSync', 'waitSync', 'getSyncParameter', 'createTransformFeedback', 'deleteTransformFeedback', 'isTransformFeedback', 'bindTransformFeedback', 'beginTransformFeedback', 'endTransformFeedback', 'transformFeedbackVaryings', 'getTransformFeedbackVarying', 'pauseTransformFeedback', 'resumeTransformFeedback', 'getFragDataLocation', 'clearBufferiv', 'clearBufferuiv', 'clearBufferfv', 'clearBufferfi', 'uniform1ui', 'uniform2ui', 'uniform3ui', 'uniform4ui', 'uniform1uiv', 'uniform2uiv', 'uniform3uiv', 'uniform4uiv', 'uniformMatrix2x3fv', 'uniformMatrix3x2fv', 'uniformMatrix2x4fv', 'uniformMatrix4x2fv', 'uniformMatrix3x4fv', 'uniformMatrix4x3fv']) hidden(WebGL2RenderingContext.prototype, name, function () { const s = state(this); if (s.version !== 2) throw new TypeError('Illegal invocation'); flag(s, GL.INVALID_OPERATION); return null; });
  for (const C of [WebGLObject, WebGL2RenderingContext, WebGLVertexArrayObject, WebGLRenderingContext, WebGLShader, WebGLProgram, WebGLBuffer, WebGLTexture, WebGLFramebuffer, WebGLRenderbuffer, WebGLUniformLocation, WebGLActiveInfo, WebGLShaderPrecisionFormat, WebGLContextEvent]) { tag(C, C.name); hidden(g, C.name, C); }
  hidden(N, 'makeWebGLContext', function (canvas, surface, options, version = 1) {
    if (options !== null && typeof options !== 'object' && typeof options !== 'function') throw new TypeError('WebGL context attributes must be a dictionary');
    const alpha = options.alpha, depth = options.depth, premultipliedAlpha = options.premultipliedAlpha, powerPreference = options.powerPreference;
    const attrs = { alpha: alpha === undefined ? true : !!alpha, depth: depth === undefined ? true : !!depth, stencil: false, antialias: false, premultipliedAlpha: premultipliedAlpha === undefined ? true : !!premultipliedAlpha, preserveDrawingBuffer: !!options.preserveDrawingBuffer, powerPreference: powerPreference === undefined ? 'default' : String(powerPreference), failIfMajorPerformanceCaveat: !!options.failIfMajorPerformanceCaveat, desynchronized: false };
    if (!['default', 'high-performance', 'low-power'].includes(attrs.powerPreference)) throw new TypeError('Invalid WebGL powerPreference');
    // Dictionary getters may have recursively selected another context.
    if (surface.ctx) return null;
    const w = surface.width, h = surface.height;
    if (Math.max(w, 1) > limits.maxDimension || Math.max(h, 1) > limits.maxDimension || Math.max(w, 1) * Math.max(h, 1) > limits.maxPixels) return null;
    if (attrs.failIfMajorPerformanceCaveat) { canvas.dispatchEvent(new WebGLContextEvent('webglcontextcreationerror', { cancelable: true, statusMessage: 'The software renderer is a major performance caveat' })); return null; }
    const result = N.webglNew(surface.id, w, h, attrs.alpha, attrs.premultipliedAlpha, version, attrs.depth);
    if (result.error) { canvas.dispatchEvent(new WebGLContextEvent('webglcontextcreationerror', { cancelable: true, statusMessage: 'Software drawing buffer creation failed' })); return null; }
    const ctx = Object.create(version === 2 ? WebGL2RenderingContext.prototype : WebGLRenderingContext.prototype);
    const s = { canvas, surface, attrs, version, drawFramebuffer: null, readFramebuffer: null, renderbuffer: null, currentVAO: null, defaultVAO: null, lost: false, errors: [], logicalWidth: w, logicalHeight: h, viewport: [0, 0, Math.max(w, 1), Math.max(h, 1)], scissor: [0, 0, Math.max(w, 1), Math.max(h, 1)], scissorEnabled: false, depthEnabled: false, blendEnabled: false, blendFactors: [GL.ONE, GL.ZERO, GL.ONE, GL.ZERO], blendEquations: [GL.FUNC_ADD, GL.FUNC_ADD], blendColor: [0, 0, 0, 0], clearColor: [0, 0, 0, 0], clearDepth: 1, depthMask: true, depthFunc: GL.LESS, colorMask: [true, true, true, true], program: null, bindings: {}, activeTexture: 0, textures: Array.from({ length: limits.maxTextureUnits }, () => null), packAlignment: 4, unpackAlignment: 4, flipY: false, unpackPremultiplied: false, unpackColorspace: GL.BROWSER_DEFAULT_WEBGL, attribs: Array.from({ length: limits.maxVertexAttribs }, () => ({ enabled: false, buffer: null, size: 4, type: GL.FLOAT, normalized: false, stride: 0, offset: 0, divisor: 0, current: [0, 0, 0, 1] })) };
    s.defaultVAO = { attribs: s.attribs, element: null };
    contexts.set(ctx, s); surface.ctx = ctx; surface.mode = 'webgl';
    N.canvasMarkDirty(surface);
    return ctx;
  });
  hidden(N, 'resizeWebGLSurface', function (surface, w, h) { if (surface.ctx) resize(state(surface.ctx), w, h); });
  hidden(N, 'webglSurfacePresented', function (surface) { const s = state(surface.ctx); if (!s.lost && !s.attrs.preserveDrawingBuffer) { const result = N.webglDiscard(surface.id); if (result.error) flag(s, result.error); } });
})();
