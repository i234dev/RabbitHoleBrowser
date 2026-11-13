// Experimental WebGL1: real software GL buffers/shaders/rasterization.
// Unsupported commands set GL errors; WebGL2 and extensions are not advertised.
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
    TEXTURE_2D: 0x0DE1, TEXTURE_CUBE_MAP: 0x8513, TEXTURE0: 0x84C0,
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
    const C = { Shader: WebGLShader, Program: WebGLProgram, Buffer: WebGLBuffer }[kind];
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
  const floats = (value) => value instanceof Float32Array ? value : new Float32Array(value);
  function matrix(s, location, transpose, data) {
    const loc = uniformLocation(s, location);
    if (!loc) return;
    data = floats(data);
    if (transpose) { flag(s, GL.INVALID_VALUE); return; }
    call(s, 'matrix4', loc.id, false, data);
  }
  function uniform(s, location, data) {
    const loc = uniformLocation(s, location);
    if (loc) call(s, 'uniform', loc.id, floats(data));
  }
  function changed(s, result) { if (!result.error) N.canvasMarkDirty(s.surface); }

  class WebGLShader { constructor() { illegal(); } }
  class WebGLProgram { constructor() { illegal(); } }
  class WebGLBuffer { constructor() { illegal(); } }
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
    getContextAttributes() { const s = state(this); return s.lost ? null : { ...s.attrs }; }
    isContextLost() { return state(this).lost; }
    getError() { const s = state(this); return s.errors.length ? s.errors.shift() : GL.NO_ERROR; }
    getSupportedExtensions() { const s = state(this); return s.lost ? null : []; }
    getExtension(name) { state(this); String(name); return null; }
    createShader(type) { required('createShader', 1, arguments); const s = state(this), r = call(s, 'createShader', type >>> 0); return r.error ? null : wrap(s, 'Shader', r.value, { type: type >>> 0, source: '' }); }
    shaderSource(shader, source) { required('shaderSource', 2, arguments); const s = state(this), o = owned(s, shader, 'Shader'); source = String(source); if (o && !call(s, 'shaderSource', o.id, source).error) o.source = source; }
    compileShader(shader) { required('compileShader', 1, arguments); const s = state(this), o = owned(s, shader, 'Shader'); if (o) call(s, 'compileShader', o.id); }
    getShaderParameter(shader, pname) {
      required('getShaderParameter', 2, arguments); const s = state(this), o = owned(s, shader, 'Shader', false, true); if (!o) return null;
      switch (pname >>> 0) { case GL.COMPILE_STATUS: return call(s, 'shaderStatus', o.id).value; case GL.SHADER_TYPE: return o.type; case GL.DELETE_STATUS: return o.deleted; default: flag(s, GL.INVALID_ENUM); return null; }
    }
    getShaderInfoLog(shader) { required('getShaderInfoLog', 1, arguments); const s = state(this), o = owned(s, shader, 'Shader', false, true); return o ? call(s, 'shaderLog', o.id).value : null; }
    getShaderSource(shader) { required('getShaderSource', 1, arguments); const s = state(this), o = owned(s, shader, 'Shader', false, true); return o ? o.source : null; }
    deleteShader(shader) { required('deleteShader', 1, arguments); const s = state(this); if (shader === null) return; const o = owned(s, shader, 'Shader', false, true); if (o && !o.deleted && !call(s, 'deleteShader', o.id).error) o.deleted = true; }
    isShader(shader) { const s = state(this), o = objects.get(shader); return !!o && o.context === s && o.kind === 'Shader' && !o.deleted && !s.lost; }
    createProgram() { const s = state(this), r = call(s, 'createProgram'); return r.error ? null : wrap(s, 'Program', r.value, { shaders: new Set(), generation: 0 }); }
    attachShader(program, shader) { required('attachShader', 2, arguments); const s = state(this), p = owned(s, program, 'Program'), o = owned(s, shader, 'Shader'); if (p && o && !call(s, 'attachShader', p.id, o.id).error) p.shaders.add(shader); }
    getAttachedShaders(program) { const s = state(this), p = owned(s, program, 'Program', false, true); return p ? Array.from(p.shaders) : null; }
    bindAttribLocation(program, location, name) { required('bindAttribLocation', 3, arguments); const s = state(this), p = owned(s, program, 'Program'); if (p) call(s, 'bindAttrib', p.id, location >>> 0, String(name)); }
    linkProgram(program) { required('linkProgram', 1, arguments); const s = state(this), p = owned(s, program, 'Program'); if (p && !call(s, 'linkProgram', p.id).error) p.generation++; }
    getProgramParameter(program, pname) {
      required('getProgramParameter', 2, arguments); const s = state(this), p = owned(s, program, 'Program', false, true); if (!p) return null;
      switch (pname >>> 0) {
        case GL.LINK_STATUS: return call(s, 'programStatus', p.id).value;
        case GL.DELETE_STATUS: return p.deleted;
        case GL.ATTACHED_SHADERS: return p.shaders.size;
        default: flag(s, GL.INVALID_ENUM); return null;
      }
    }
    getProgramInfoLog(program) { const s = state(this), p = owned(s, program, 'Program', false, true); return p ? call(s, 'programLog', p.id).value : null; }
    useProgram(program) { required('useProgram', 1, arguments); const s = state(this), p = owned(s, program, 'Program', true); if (p && !call(s, 'useProgram', p.id).error) s.program = program; }
    deleteProgram(program) { required('deleteProgram', 1, arguments); const s = state(this); if (program === null) return; const p = owned(s, program, 'Program', false, true); if (p && !p.deleted && !call(s, 'deleteProgram', p.id).error) p.deleted = true; }
    isProgram(program) { const s = state(this), p = objects.get(program); return !!p && p.context === s && p.kind === 'Program' && !p.deleted && !s.lost; }
    getAttribLocation(program, name) { required('getAttribLocation', 2, arguments); const s = state(this), p = owned(s, program, 'Program'); if (!p) return -1; const r = call(s, 'attribLocation', p.id, String(name)); return r.error ? -1 : r.value; }
    getUniformLocation(program, name) {
      required('getUniformLocation', 2, arguments); const s = state(this), p = owned(s, program, 'Program'); if (!p) return null;
      const r = call(s, 'uniformLocation', p.id, String(name)); if (r.error || r.value < 0) return null;
      const location = Object.create(WebGLUniformLocation.prototype); locations.set(location, { context: s, program, generation: p.generation, id: r.value }); return location;
    }
    uniform1f(location, x) { required('uniform1f', 2, arguments); uniform(state(this), location, [Number(x)]); }
    uniform2f(location, x, y) { required('uniform2f', 3, arguments); uniform(state(this), location, [Number(x), Number(y)]); }
    uniform3f(location, x, y, z) { required('uniform3f', 4, arguments); uniform(state(this), location, [Number(x), Number(y), Number(z)]); }
    uniform4f(location, x, y, z, w) { required('uniform4f', 5, arguments); uniform(state(this), location, [Number(x), Number(y), Number(z), Number(w)]); }
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
      required('bufferData', 3, arguments); const s = state(this), b = buffer(s, target); if (!b) return;
      usage >>>= 0; if (![GL.STATIC_DRAW, GL.DYNAMIC_DRAW, GL.STREAM_DRAW].includes(usage)) { flag(s, GL.INVALID_ENUM); return; }
      let r, size;
      if (typeof data === 'number') { size = offset(s, data); if (size === null) return; r = call(s, 'bufferSize', target >>> 0, size); }
      else { const bytes = data === null ? new Uint8Array(0) : dataView(data); size = bytes.byteLength; r = call(s, 'bufferData', target >>> 0, bytes); }
      if (!r.error) { b.size = size; b.usage = usage; }
    }
    bufferSubData(target, destination, data) { required('bufferSubData', 3, arguments); const s = state(this); if (!buffer(s, target)) return; destination = offset(s, destination); if (destination !== null) call(s, 'bufferSubData', target >>> 0, destination, dataView(data)); }
    getBufferParameter(target, pname) { required('getBufferParameter', 2, arguments); const s = state(this), b = buffer(s, target); if (!b) return null; if ((pname >>> 0) === GL.BUFFER_SIZE) return b.size; if ((pname >>> 0) === GL.BUFFER_USAGE) return b.usage; flag(s, GL.INVALID_ENUM); return null; }
    deleteBuffer(object) {
      required('deleteBuffer', 1, arguments); const s = state(this); if (object === null) return; const b = owned(s, object, 'Buffer', false, true);
      if (b && !b.deleted && !call(s, 'deleteBuffer', b.id).error) { b.deleted = true; for (const key of Object.keys(s.bindings)) if (s.bindings[key] === object) s.bindings[key] = null; for (const a of s.attribs) if (a.buffer === object) a.buffer = null; }
    }
    isBuffer(object) { const s = state(this), b = objects.get(object); return !!b && b.context === s && b.kind === 'Buffer' && !b.deleted && b.target !== null && !s.lost; }
    vertexAttribPointer(location, size, type, normalized, stride, byteOffset) {
      required('vertexAttribPointer', 6, arguments); const s = state(this), i = index(s, location); if (i === null) return; size |= 0; stride |= 0; byteOffset = offset(s, byteOffset); if (byteOffset === null) return;
      if (size < 1 || size > 4 || stride < 0 || stride > 255) { flag(s, GL.INVALID_VALUE); return; }
      if (!call(s, 'attribPointer', i, size, type >>> 0, !!normalized, stride, byteOffset).error) Object.assign(s.attribs[i], { size, type: type >>> 0, normalized: !!normalized, stride, offset: byteOffset, buffer: s.bindings[GL.ARRAY_BUFFER] || null });
    }
    enableVertexAttribArray(location) { const s = state(this), i = index(s, location); if (i !== null && !call(s, 'enableAttrib', i, true).error) s.attribs[i].enabled = true; }
    disableVertexAttribArray(location) { const s = state(this), i = index(s, location); if (i !== null && !call(s, 'enableAttrib', i, false).error) s.attribs[i].enabled = false; }
    getVertexAttrib(location, pname) {
      const s = state(this), i = index(s, location); if (i === null) return null; const a = s.attribs[i];
      switch (pname >>> 0) { case GL.VERTEX_ATTRIB_ARRAY_ENABLED: return a.enabled; case GL.VERTEX_ATTRIB_ARRAY_BUFFER_BINDING: return a.buffer; case GL.VERTEX_ATTRIB_ARRAY_SIZE: return a.size; case GL.VERTEX_ATTRIB_ARRAY_TYPE: return a.type; case GL.VERTEX_ATTRIB_ARRAY_NORMALIZED: return a.normalized; case GL.VERTEX_ATTRIB_ARRAY_STRIDE: return a.stride; case GL.CURRENT_VERTEX_ATTRIB: return new Float32Array(a.current); default: flag(s, GL.INVALID_ENUM); return null; }
    }
    getVertexAttribOffset(location, pname) { const s = state(this), i = index(s, location); if (i === null) return 0; if ((pname >>> 0) !== GL.VERTEX_ATTRIB_ARRAY_POINTER) { flag(s, GL.INVALID_ENUM); return 0; } return s.attribs[i].offset; }
    viewport(x, y, width, height) { required('viewport', 4, arguments); const s = state(this); width |= 0; height |= 0; if (width < 0 || height < 0) { flag(s, GL.INVALID_VALUE); return; } x |= 0; y |= 0; if (!call(s, 'viewport', x, y, width, height).error) s.viewport = [x, y, Math.min(width, limits.maxDimension), Math.min(height, limits.maxDimension)]; }
    scissor(x, y, width, height) { required('scissor', 4, arguments); const s = state(this); width |= 0; height |= 0; if (width < 0 || height < 0) { flag(s, GL.INVALID_VALUE); return; } x |= 0; y |= 0; if (!call(s, 'scissor', x, y, width, height).error) s.scissor = [x, y, width, height]; }
    enable(cap) { capability(state(this), cap >>> 0, true); }
    disable(cap) { capability(state(this), cap >>> 0, false); }
    isEnabled(cap) { const s = state(this); cap >>>= 0; if (cap === GL.SCISSOR_TEST) return s.scissorEnabled; if (cap === GL.DEPTH_TEST) return s.depthEnabled; flag(s, GL.INVALID_ENUM); return false; }
    clearColor(r, green, b, a) { required('clearColor', 4, arguments); const s = state(this), color = [Number(r), Number(green), Number(b), Number(a)]; if (!call(s, 'clearColor', ...color).error) s.clearColor = color.map(clamp); }
    clearDepth(depth) { required('clearDepth', 1, arguments); const s = state(this); depth = Number(depth); if (!call(s, 'clearDepth', depth).error) s.clearDepth = clamp(depth); }
    colorMask(r, green, b, a) { required('colorMask', 4, arguments); const s = state(this), mask = [!!r, !!green, !!b, !!a]; if (!call(s, 'colorMask', ...mask).error) s.colorMask = mask; }
    depthMask(value) { required('depthMask', 1, arguments); const s = state(this); if (!call(s, 'depthMask', !!value).error) s.depthMask = !!value; }
    depthFunc(func) { required('depthFunc', 1, arguments); const s = state(this); if (!call(s, 'depthFunc', func >>> 0).error) s.depthFunc = func >>> 0; }
    clear(mask) { required('clear', 1, arguments); const s = state(this); changed(s, call(s, 'clear', mask >>> 0)); }
    drawArrays(mode, first, count) { required('drawArrays', 3, arguments); const s = state(this); first |= 0; count |= 0; if (first < 0 || count < 0) { flag(s, GL.INVALID_VALUE); return; } changed(s, call(s, 'drawArrays', mode >>> 0, first, count)); }
    drawElements(mode, count, type, byteOffset) { required('drawElements', 4, arguments); const s = state(this); count |= 0; byteOffset = offset(s, byteOffset); if (count < 0) { flag(s, GL.INVALID_VALUE); return; } if (byteOffset !== null) changed(s, call(s, 'drawElements', mode >>> 0, count, type >>> 0, byteOffset)); }
    readPixels(x, y, width, height, format, type, pixels) {
      required('readPixels', 7, arguments); const s = state(this); width |= 0; height |= 0; x |= 0; y |= 0;
      if (width < 0 || height < 0) { flag(s, GL.INVALID_VALUE); return; }
      if ((format >>> 0) !== GL.RGBA || (type >>> 0) !== GL.UNSIGNED_BYTE) { flag(s, GL.INVALID_ENUM); return; }
      if (pixels !== null && !ArrayBuffer.isView(pixels)) throw new TypeError('Expected an ArrayBufferView');
      if (!(pixels instanceof Uint8Array)) { flag(s, GL.INVALID_OPERATION); return; }
      const row = width * 4, stride = Math.ceil(row / s.packAlignment) * s.packAlignment;
      const length = height ? (height - 1) * stride + row : 0;
      if (!Number.isSafeInteger(length) || length > pixels.byteLength) { flag(s, GL.INVALID_OPERATION); return; }
      const r = call(s, 'readPixels', x, y, width, height); if (r.error) return;
      const bytes = new Uint8Array(r.value);
      for (let y = 0; y < height; y++) pixels.set(bytes.subarray(y * row, (y + 1) * row), y * stride);
    }
    pixelStorei(pname, value) {
      required('pixelStorei', 2, arguments); const s = state(this); pname >>>= 0;
      if (pname === GL.PACK_ALIGNMENT || pname === GL.UNPACK_ALIGNMENT) { value |= 0; if (![1, 2, 4, 8].includes(value)) { flag(s, GL.INVALID_VALUE); return; } if (pname === GL.PACK_ALIGNMENT) s.packAlignment = value; else s.unpackAlignment = value; return; }
      if (pname === GL.UNPACK_FLIP_Y_WEBGL) { s.flipY = !!value; return; } if (pname === GL.UNPACK_PREMULTIPLY_ALPHA_WEBGL) { s.unpackPremultiplied = !!value; return; }
      if (pname === GL.UNPACK_COLORSPACE_CONVERSION_WEBGL) { value >>>= 0; if (value !== GL.NONE && value !== GL.BROWSER_DEFAULT_WEBGL) { flag(s, GL.INVALID_VALUE); return; } s.unpackColorspace = value; return; }
      flag(s, GL.INVALID_ENUM);
    }
    getParameter(pname) {
      required('getParameter', 1, arguments); const s = state(this); syncSize(s); if (s.lost) return null;
      switch (pname >>> 0) {
        case GL.VERSION: return 'WebGL 1.0 (RabbitHole experimental software subset)';
        case GL.SHADING_LANGUAGE_VERSION: return 'WebGL GLSL ES 1.0 (experimental subset)';
        case GL.VENDOR: return 'RabbitHole'; case GL.RENDERER: return 'RabbitHole software rasterizer (experimental)';
        case GL.SHADER_COMPILER: return true;
        case GL.MAX_VERTEX_ATTRIBS: return limits.maxVertexAttribs;
        case GL.MAX_VIEWPORT_DIMS: return new Int32Array([limits.maxDimension, limits.maxDimension]);
        case GL.MAX_RENDERBUFFER_SIZE: return limits.maxDimension;
        case GL.VIEWPORT: return new Int32Array(s.viewport); case GL.SCISSOR_BOX: return new Int32Array(s.scissor);
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
        case GL.FRAMEBUFFER_BINDING: case GL.RENDERBUFFER_BINDING: return null;
        default: flag(s, GL.INVALID_ENUM); return null;
      }
    }
    getShaderPrecisionFormat(shaderType, precisionType) {
      const s = state(this); if (![GL.VERTEX_SHADER, GL.FRAGMENT_SHADER].includes(shaderType >>> 0) || ![GL.LOW_FLOAT, GL.MEDIUM_FLOAT, GL.HIGH_FLOAT].includes(precisionType >>> 0)) { flag(s, GL.INVALID_ENUM); return null; }
      return Object.assign(Object.create(WebGLShaderPrecisionFormat.prototype), { rangeMin: 127, rangeMax: 127, precision: 23 });
    }
    bindFramebuffer(target, framebuffer) { const s = state(this); if ((target >>> 0) !== GL.FRAMEBUFFER) { flag(s, GL.INVALID_ENUM); return; } if (framebuffer !== null) flag(s, GL.INVALID_OPERATION); }
    checkFramebufferStatus(target) { const s = state(this); if ((target >>> 0) !== GL.FRAMEBUFFER) { flag(s, GL.INVALID_ENUM); return 0; } return s.lost ? 0 : GL.FRAMEBUFFER_COMPLETE; }
    flush() { const s = state(this); syncSize(s); if (!s.lost) N.canvasMarkDirty(s.surface); }
    finish() { const s = state(this); syncSize(s); if (!s.lost) N.canvasMarkDirty(s.surface); }
  }
  function capability(s, cap, enabled) {
    if (cap === GL.SCISSOR_TEST) { if (!call(s, 'enableScissor', enabled).error) s.scissorEnabled = enabled; return; }
    if (cap === GL.DEPTH_TEST) { if (!call(s, 'enableDepth', enabled && s.attrs.depth).error) s.depthEnabled = enabled; return; }
    flag(s, [GL.BLEND, GL.CULL_FACE, GL.STENCIL_TEST, GL.DITHER, GL.POLYGON_OFFSET_FILL, GL.SAMPLE_ALPHA_TO_COVERAGE, GL.SAMPLE_COVERAGE].includes(cap) ? GL.INVALID_OPERATION : GL.INVALID_ENUM);
  }
  for (let size = 1; size <= 4; size++) {
    hidden(WebGLRenderingContext.prototype, 'vertexAttrib' + size + 'f', function (location, ...values) {
      const s = state(this), i = index(s, location); if (i === null) return; const v = [0, 0, 0, 1]; for (let j = 0; j < size; j++) v[j] = Number(values[j]); if (!call(s, 'attrib', i, ...v).error) s.attribs[i].current = v;
    });
    hidden(WebGLRenderingContext.prototype, 'vertexAttrib' + size + 'fv', function (location, values) {
      const s = state(this), i = index(s, location); if (i === null) return; values = floats(values); if (values.length < size) { flag(s, GL.INVALID_VALUE); return; } const v = [0, 0, 0, 1]; for (let j = 0; j < size; j++) v[j] = values[j]; if (!call(s, 'attrib', i, ...v).error) s.attribs[i].current = v;
    });
  }
  // These operations are deliberately unavailable until their rendering
  // semantics exist. Libraries get an observable GL error, never fake IDs.
  for (const name of ['createTexture', 'createFramebuffer', 'createRenderbuffer', 'getActiveAttrib', 'getActiveUniform', 'getUniform']) hidden(WebGLRenderingContext.prototype, name, function () { flag(state(this), GL.INVALID_OPERATION); return null; });
  for (const name of ['activeTexture', 'bindTexture', 'deleteTexture', 'texImage2D', 'texSubImage2D', 'texParameteri', 'texParameterf', 'generateMipmap', 'compressedTexImage2D', 'compressedTexSubImage2D', 'copyTexImage2D', 'copyTexSubImage2D', 'bindRenderbuffer', 'deleteFramebuffer', 'deleteRenderbuffer', 'framebufferTexture2D', 'framebufferRenderbuffer', 'renderbufferStorage', 'blendFunc', 'blendFuncSeparate', 'blendEquation', 'blendEquationSeparate', 'blendColor', 'cullFace', 'frontFace', 'polygonOffset', 'sampleCoverage', 'stencilFunc', 'stencilFuncSeparate', 'stencilMask', 'stencilMaskSeparate', 'stencilOp', 'stencilOpSeparate', 'clearStencil', 'depthRange', 'lineWidth', 'detachShader', 'validateProgram', 'uniform1i', 'uniform2i', 'uniform3i', 'uniform4i', 'uniform1iv', 'uniform2iv', 'uniform3iv', 'uniform4iv', 'uniformMatrix2fv', 'uniformMatrix3fv']) hidden(WebGLRenderingContext.prototype, name, function () { flag(state(this), GL.INVALID_OPERATION); });
  for (const [name, value] of Object.entries(GL)) {
    Object.defineProperty(WebGLRenderingContext, name, { value, enumerable: true });
    Object.defineProperty(WebGLRenderingContext.prototype, name, { value, enumerable: true });
  }
  for (const C of [WebGLRenderingContext, WebGLShader, WebGLProgram, WebGLBuffer, WebGLUniformLocation, WebGLActiveInfo, WebGLShaderPrecisionFormat, WebGLContextEvent]) { tag(C, C.name); hidden(g, C.name, C); }
  hidden(N, 'makeWebGLContext', function (canvas, surface, options) {
    if (options !== null && typeof options !== 'object' && typeof options !== 'function') throw new TypeError('WebGL context attributes must be a dictionary');
    const attrs = { alpha: options.alpha !== false, depth: options.depth !== false, stencil: false, antialias: false, premultipliedAlpha: options.premultipliedAlpha !== false, preserveDrawingBuffer: !!options.preserveDrawingBuffer, powerPreference: String(options.powerPreference || 'default'), failIfMajorPerformanceCaveat: !!options.failIfMajorPerformanceCaveat, desynchronized: false };
    // Dictionary getters may have recursively selected another context.
    if (surface.ctx) return null;
    const w = surface.width, h = surface.height;
    if (Math.max(w, 1) > limits.maxDimension || Math.max(h, 1) > limits.maxDimension || Math.max(w, 1) * Math.max(h, 1) > limits.maxPixels) return null;
    if (attrs.failIfMajorPerformanceCaveat) return null;
    const result = N.webglNew(surface.id, w, h, attrs.alpha, attrs.premultipliedAlpha);
    if (result.error) { canvas.dispatchEvent(new WebGLContextEvent('webglcontextcreationerror', { statusMessage: 'Software drawing buffer creation failed' })); return null; }
    const ctx = Object.create(WebGLRenderingContext.prototype);
    const s = { canvas, surface, attrs, lost: false, errors: [], logicalWidth: w, logicalHeight: h, viewport: [0, 0, Math.max(w, 1), Math.max(h, 1)], scissor: [0, 0, Math.max(w, 1), Math.max(h, 1)], scissorEnabled: false, depthEnabled: false, clearColor: [0, 0, 0, 0], clearDepth: 1, depthMask: true, depthFunc: GL.LESS, colorMask: [true, true, true, true], program: null, bindings: {}, packAlignment: 4, unpackAlignment: 4, flipY: false, unpackPremultiplied: false, unpackColorspace: GL.BROWSER_DEFAULT_WEBGL, attribs: Array.from({ length: limits.maxVertexAttribs }, () => ({ enabled: false, buffer: null, size: 4, type: GL.FLOAT, normalized: false, stride: 0, offset: 0, current: [0, 0, 0, 1] })) };
    contexts.set(ctx, s); surface.ctx = ctx; surface.mode = 'webgl';
    N.canvasMarkDirty(surface);
    return ctx;
  });
  hidden(N, 'resizeWebGLSurface', function (surface, w, h) { if (surface.ctx) resize(state(surface.ctx), w, h); });
  hidden(N, 'webglSurfacePresented', function (surface) { const s = state(surface.ctx); if (!s.lost && !s.attrs.preserveDrawingBuffer) { const result = N.webglDiscard(surface.id); if (result.error) flag(s, result.error); } });
})();
