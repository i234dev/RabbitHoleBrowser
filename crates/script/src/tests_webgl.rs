//! WebGL bindings exercise the real software backend through JavaScript.
use crate::tests::{run, setup};

const SHADERS: &str = r#"
function shader(gl, type, source) {
  const object = gl.createShader(type);
  gl.shaderSource(object, source); gl.compileShader(object);
  if (!gl.getShaderParameter(object, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(object));
  return object;
}
function triangleProgram(gl) {
  const program = gl.createProgram();
  gl.attachShader(program, shader(gl, gl.VERTEX_SHADER, 'attribute vec2 position; void main() { gl_Position = vec4(position, 0.0, 1.0); }'));
  gl.attachShader(program, shader(gl, gl.FRAGMENT_SHADER, 'precision mediump float; void main() { gl_FragColor = vec4(1.0, 0.0, 0.0, 1.0); }'));
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(program));
  gl.useProgram(program);
  const buffer = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1,-1, 1,-1, 0,1]), gl.STATIC_DRAW);
  const location = gl.getAttribLocation(program, 'position');
  gl.vertexAttribPointer(location, 2, gl.FLOAT, false, 0, 0); gl.enableVertexAttribArray(location);
  return program;
}
"#;

#[test]
fn webgl_clear_context_lock_and_aliases() {
    let (mut env, mut host) = setup("<canvas id=a width=8 height=8></canvas><canvas id=b></canvas>");
    let out = run(&mut env, &mut host, r#"
      const a = document.getElementById('a'), b = document.getElementById('b');
      console.log(a.getContext('webgl2') === null);
      const gl = a.getContext('webgl', {preserveDrawingBuffer:true});
      console.log(gl instanceof WebGLRenderingContext, gl.canvas === a, gl === a.getContext('experimental-webgl'), a.getContext('2d') === null);
      b.getContext('2d'); console.log(b.getContext('webgl') === null);
      gl.clearColor(.25,.5,.75,1); gl.clear(gl.COLOR_BUFFER_BIT);
      const pixel = new Uint8Array(4); gl.readPixels(4,4,1,1,gl.RGBA,gl.UNSIGNED_BYTE,pixel);
      console.log(Array.from(pixel).join(','), gl.getError(), gl.drawingBufferWidth, gl.drawingBufferHeight);
      console.log(gl.getContextAttributes().antialias, gl.getSupportedExtensions().length, gl.getExtension('imaginary') === null);
    "#);
    assert_eq!(out, ["true", "true true true true", "true", "64,128,191,255 0 8 8", "false 0 true"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn webgl_triangle_pixels_reach_canvas2d_sources() {
    let (mut env, mut host) = setup("<canvas id=a width=8 height=8></canvas><canvas id=b width=8 height=8></canvas>");
    let source = format!("{SHADERS}\n{}", r#"
      const a = document.getElementById('a'), gl = a.getContext('webgl', {premultipliedAlpha:false,preserveDrawingBuffer:true});
      triangleProgram(gl); gl.drawArrays(gl.TRIANGLES,0,3);
      const pixel = new Uint8Array(4); gl.readPixels(4,3,1,1,gl.RGBA,gl.UNSIGNED_BYTE,pixel);
      console.log(Array.from(pixel).join(','), gl.getError());
      gl.readPixels(0,7,1,1,gl.RGBA,gl.UNSIGNED_BYTE,pixel); console.log(Array.from(pixel).join(','));
      const copy = document.getElementById('b').getContext('2d'); copy.drawImage(a,0,0);
      console.log(Array.from(copy.getImageData(4,4,1,1).data).join(','));
      console.log(a.toDataURL().startsWith('data:image/png;base64,'));
    "#);
    let out = run(&mut env, &mut host, &source);
    assert_eq!(out, ["255,0,0,255 0", "0,0,0,0", "255,0,0,255", "true"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}
