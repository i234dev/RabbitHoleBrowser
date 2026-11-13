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
    let (mut env, mut host) =
        setup("<canvas id=a width=8 height=8></canvas><canvas id=b></canvas>");
    let out = run(
        &mut env,
        &mut host,
        r#"
      const a = document.getElementById('a'), b = document.getElementById('b');
      console.log(a.getContext('webgl2') === null);
      const gl = a.getContext('webgl', {preserveDrawingBuffer:true});
      console.log(gl instanceof WebGLRenderingContext, gl.canvas === a, gl === a.getContext('experimental-webgl'), a.getContext('2d') === null);
      b.getContext('2d'); console.log(b.getContext('webgl') === null);
      gl.clearColor(.25,.5,.75,1); gl.clear(gl.COLOR_BUFFER_BIT);
      const pixel = new Uint8Array(4); gl.readPixels(4,4,1,1,gl.RGBA,gl.UNSIGNED_BYTE,pixel);
      console.log(Array.from(pixel).join(','), gl.getError(), gl.drawingBufferWidth, gl.drawingBufferHeight);
      console.log(gl.getContextAttributes().antialias, gl.getSupportedExtensions().length, gl.getExtension('imaginary') === null);
    "#,
    );
    assert_eq!(
        out,
        [
            "true",
            "true true true true",
            "true",
            "64,128,191,255 0 8 8",
            "false 0 true"
        ]
    );
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn webgl_triangle_pixels_reach_canvas2d_sources() {
    let (mut env, mut host) =
        setup("<canvas id=a width=8 height=8></canvas><canvas id=b width=8 height=8></canvas>");
    let source = format!(
        "{SHADERS}\n{}",
        r#"
      const a = document.getElementById('a'), gl = a.getContext('webgl', {premultipliedAlpha:false,preserveDrawingBuffer:true});
      triangleProgram(gl); gl.drawArrays(gl.TRIANGLES,0,3);
      const pixel = new Uint8Array(4); gl.readPixels(4,3,1,1,gl.RGBA,gl.UNSIGNED_BYTE,pixel);
      console.log(Array.from(pixel).join(','), gl.getError());
      gl.readPixels(0,7,1,1,gl.RGBA,gl.UNSIGNED_BYTE,pixel); console.log(Array.from(pixel).join(','));
      const copy = document.getElementById('b').getContext('2d'); copy.drawImage(a,0,0);
      console.log(Array.from(copy.getImageData(4,4,1,1).data).join(','));
      console.log(a.toDataURL().startsWith('data:image/png;base64,'));
    "#
    );
    let out = run(&mut env, &mut host, &source);
    assert_eq!(out, ["255,0,0,255 0", "0,0,0,0", "255,0,0,255", "true"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn webgl_objects_errors_and_shader_failures_are_observable() {
    let (mut env, mut host) =
        setup("<canvas id=a width=4 height=4></canvas><canvas id=b width=4 height=4></canvas>");
    let out = run(
        &mut env,
        &mut host,
        r#"
      const gl=document.getElementById('a').getContext('webgl',{preserveDrawingBuffer:true});
      const other=document.getElementById('b').getContext('webgl',{preserveDrawingBuffer:true});
      console.log(gl.createShader(12345)===null,gl.getError()===gl.INVALID_ENUM);
      const bad=gl.createShader(gl.FRAGMENT_SHADER);gl.shaderSource(bad,'void main(){gl_FragColor=vec4(1.0);}');gl.compileShader(bad);
      console.log(gl.getShaderParameter(bad,gl.COMPILE_STATUS)===false,gl.getShaderInfoLog(bad).length>0);
      const p=gl.createProgram();gl.attachShader(p,bad);gl.linkProgram(p);
      console.log(gl.getProgramParameter(p,gl.LINK_STATUS)===false,gl.getProgramInfoLog(p).length>0);
      gl.useProgram(p);console.log(gl.getError()===gl.INVALID_OPERATION);
      gl.viewport(0,0,-1,4);console.log(gl.getError()===gl.INVALID_VALUE);
      let threw=false;try{gl.bindBuffer(gl.ARRAY_BUFFER,bad);}catch(e){threw=e instanceof TypeError;}console.log(threw);
      gl.compileShader(other.createShader(gl.VERTEX_SHADER));console.log(gl.getError()===gl.INVALID_OPERATION);
      console.log(gl.createFramebuffer()===null,gl.getError()===gl.INVALID_OPERATION);
      gl.enable(gl.CULL_FACE);console.log(gl.getError()===gl.INVALID_OPERATION);
      const data=new Uint8Array([17,18,19,20]);gl.readPixels(0,0,2,2,gl.RGBA,gl.UNSIGNED_BYTE,data);
      console.log(gl.getError()===gl.INVALID_OPERATION,Array.from(data).join(','));
      let constructor=false;try{new WebGLBuffer();}catch(e){constructor=e instanceof TypeError;}console.log(constructor);
      const buffer=gl.createBuffer();console.log(gl.isBuffer(buffer));gl.bindBuffer(gl.ARRAY_BUFFER,buffer);gl.bufferData(gl.ARRAY_BUFFER,16,gl.STATIC_DRAW);
      console.log(gl.isBuffer(buffer),gl.getBufferParameter(gl.ARRAY_BUFFER,gl.BUFFER_SIZE));gl.deleteBuffer(buffer);console.log(gl.isBuffer(buffer),gl.getParameter(gl.ARRAY_BUFFER_BINDING)===null);
    "#,
    );
    assert_eq!(
        out,
        [
            "true true",
            "true true",
            "true true",
            "true",
            "true",
            "true",
            "true",
            "true true",
            "true",
            "true 17,18,19,20",
            "true",
            "false",
            "true 16",
            "false true"
        ]
    );
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn webgl_resize_clears_pixels_but_preserves_gl_state() {
    let (mut env, mut host) = setup("<canvas id=a width=4 height=4></canvas>");
    let out = run(
        &mut env,
        &mut host,
        r#"
      const a=document.getElementById('a'),gl=a.getContext('webgl',{preserveDrawingBuffer:true});
      gl.clearColor(1,0,0,1);gl.clear(gl.COLOR_BUFFER_BIT);gl.viewport(1,1,2,2);
      a.width=4;
      const pixel=new Uint8Array(4);gl.readPixels(1,1,1,1,gl.RGBA,gl.UNSIGNED_BYTE,pixel);console.log(Array.from(pixel).join(','));
      a.width=8;a.height=6;
      console.log(gl.drawingBufferWidth,gl.drawingBufferHeight,Array.from(gl.getParameter(gl.VIEWPORT)).join(','));
      console.log(Array.from(gl.getParameter(gl.COLOR_CLEAR_VALUE)).join(','));
      gl.clear(gl.COLOR_BUFFER_BIT);gl.readPixels(7,5,1,1,gl.RGBA,gl.UNSIGNED_BYTE,pixel);console.log(Array.from(pixel).join(','),gl.getError());
    "#,
    );
    assert_eq!(out, ["0,0,0,0", "8 6 1,1,2,2", "1,0,0,1", "255,0,0,255 0"]);
}

#[test]
fn webgl_readback_orientation_alignment_and_outside_pixels() {
    let (mut env, mut host) =
        setup("<canvas id=a width=4 height=4></canvas><canvas id=b width=4 height=4></canvas>");
    let out = run(
        &mut env,
        &mut host,
        r#"
      const a=document.getElementById('a'),gl=a.getContext('webgl',{preserveDrawingBuffer:true});
      gl.clearColor(0,0,1,1);gl.clear(gl.COLOR_BUFFER_BIT);gl.enable(gl.SCISSOR_TEST);gl.scissor(0,0,4,2);gl.clearColor(1,0,0,1);gl.clear(gl.COLOR_BUFFER_BIT);
      const rgba=new Uint8ClampedArray(64);gl.readPixels(0,0,4,4,gl.RGBA,gl.UNSIGNED_BYTE,rgba);
      console.log(Array.from(rgba.slice(0,4)).join(','),Array.from(rgba.slice(60,64)).join(','));
      const copy=document.getElementById('b').getContext('2d');copy.drawImage(a,0,0);
      console.log(Array.from(copy.getImageData(0,0,1,1).data).join(','),Array.from(copy.getImageData(0,3,1,1).data).join(','));
      const padded=new Uint8Array(12);padded.fill(7);gl.pixelStorei(gl.PACK_ALIGNMENT,8);gl.readPixels(0,0,1,2,gl.RGBA,gl.UNSIGNED_BYTE,padded);
      console.log(Array.from(padded).join(','));
      gl.pixelStorei(gl.PACK_ALIGNMENT,4);const outside=new Uint8Array(16);outside.fill(9);gl.readPixels(-1,-1,2,2,gl.RGBA,gl.UNSIGNED_BYTE,outside);
      console.log(Array.from(outside).join(','),gl.getError());
    "#,
    );
    assert_eq!(
        out,
        [
            "255,0,0,255 0,0,255,255",
            "0,0,255,255 255,0,0,255",
            "255,0,0,255,7,7,7,7,255,0,0,255",
            "9,9,9,9,9,9,9,9,9,9,9,9,255,0,0,255 0"
        ]
    );
}

#[test]
fn webgl_linked_reflection_and_uniform_values_are_real() {
    let (mut env, mut host) = setup("<canvas id=a width=4 height=4></canvas>");
    let source = format!(
        "{SHADERS}\n{}",
        r#"
      const gl=document.getElementById('a').getContext('webgl',{preserveDrawingBuffer:true});
      const p=gl.createProgram();gl.attachShader(p,shader(gl,gl.VERTEX_SHADER,'attribute vec2 position;void main(){gl_Position=vec4(position,0.0,1.0);}'));
      gl.attachShader(p,shader(gl,gl.FRAGMENT_SHADER,'precision mediump float;uniform vec4 color;void main(){gl_FragColor=color;}'));gl.linkProgram(p);gl.useProgram(p);
      console.log(gl.getProgramParameter(p,gl.ACTIVE_ATTRIBUTES),gl.getProgramParameter(p,gl.ACTIVE_UNIFORMS));
      const a=gl.getActiveAttrib(p,0),u=gl.getActiveUniform(p,0);console.log(a.name,a.type===gl.FLOAT_VEC2,u.name,u.type===gl.FLOAT_VEC4);
      const location=gl.getUniformLocation(p,'color');console.log(location instanceof WebGLUniformLocation,Array.from(gl.getUniform(p,location)).join(','));
      gl.uniform4fv(location,new Float32Array([0,.5,1,1]));console.log(Array.from(gl.getUniform(p,location)).join(','),gl.getError());
      gl.linkProgram(p);gl.uniform4f(location,1,0,0,1);console.log(gl.getError()===gl.INVALID_OPERATION);
    "#
    );
    let out = run(&mut env, &mut host, &source);
    assert_eq!(
        out,
        [
            "1 1",
            "position true color true",
            "true 0,0,0,0",
            "0,0.5,1,1 0",
            "true"
        ]
    );
}

#[test]
fn webgl_context_loss_on_oversized_resize_is_bounded() {
    let (mut env, mut host) = setup("<canvas id=a width=4 height=4></canvas>");
    let out = run(
        &mut env,
        &mut host,
        r#"
      const a=document.getElementById('a'),gl=a.getContext('webgl');
      a.addEventListener('webglcontextlost',event=>console.log(event instanceof WebGLContextEvent,event.cancelable,event.statusMessage.length>0));
      a.width=50000;
      console.log(gl.isContextLost(),gl.drawingBufferWidth,gl.getContextAttributes()===null,gl.getError()===gl.CONTEXT_LOST_WEBGL,gl.getError());
      console.log(a.toDataURL()==='data:,');
    "#,
    );
    assert_eq!(out, ["true 0 true true 0", "true", "true true true"]);
}

#[test]
fn webgl_presentation_respects_alpha_and_default_buffer_discard() {
    let (mut env, mut host) = setup("<canvas id=a width=2 height=2></canvas><canvas id=b width=2 height=2></canvas><canvas id=c width=2 height=2></canvas>");
    let out = run(
        &mut env,
        &mut host,
        r#"
      const a=document.getElementById('a'),gl=a.getContext('webgl',{preserveDrawingBuffer:true,premultipliedAlpha:true});
      gl.clearColor(.5,0,0,.5);gl.clear(gl.COLOR_BUFFER_BIT);const raw=new Uint8Array(4);gl.readPixels(0,0,1,1,gl.RGBA,gl.UNSIGNED_BYTE,raw);console.log(Array.from(raw).join(','));
      const copy=document.getElementById('b').getContext('2d');copy.drawImage(a,0,0);console.log(Array.from(copy.getImageData(0,0,1,1).data).join(','));
      const opaque=document.getElementById('c').getContext('webgl',{alpha:false,preserveDrawingBuffer:true});opaque.clearColor(0,1,0,0);opaque.clear(opaque.COLOR_BUFFER_BIT);opaque.readPixels(0,0,1,1,opaque.RGBA,opaque.UNSIGNED_BYTE,raw);console.log(Array.from(raw).join(','),opaque.getContextAttributes().alpha);
      const transient=document.createElement('canvas');document.body.appendChild(transient);transient.width=2;transient.height=2;const t=transient.getContext('webgl');t.clearColor(1,0,0,1);t.clear(t.COLOR_BUFFER_BIT);
      queueMicrotask(()=>{const data=new Uint8Array(4);t.readPixels(0,0,1,1,t.RGBA,t.UNSIGNED_BYTE,data);console.log(Array.from(data).join(','));});
    "#,
    );
    assert_eq!(
        out,
        ["128,0,0,128", "255,0,0,128", "0,255,0,255 false", "0,0,0,0"]
    );
}

#[test]
fn webgl_textured_quad_reads_real_rgba_and_unpack_transformations() {
    let (mut env, mut host) = setup("<canvas id=a width=4 height=4></canvas>");
    let source = format!(
        "{SHADERS}\n{}",
        r#"
      const gl=document.getElementById('a').getContext('webgl',{preserveDrawingBuffer:true,premultipliedAlpha:false});
      const p=gl.createProgram();
      gl.attachShader(p,shader(gl,gl.VERTEX_SHADER,'attribute vec2 position;attribute vec2 uv;varying vec2 vUv;void main(){gl_Position=vec4(position,0.0,1.0);vUv=uv;}'));
      gl.attachShader(p,shader(gl,gl.FRAGMENT_SHADER,'precision mediump float;uniform sampler2D image;varying vec2 vUv;void main(){gl_FragColor=texture2D(image,vUv);}'));gl.linkProgram(p);gl.useProgram(p);
      for(const pair of [['position',[-1,-1,1,-1,-1,1,1,1]],['uv',[0,0,1,0,0,1,1,1]]]){const b=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,b);gl.bufferData(gl.ARRAY_BUFFER,new Float32Array(pair[1]),gl.STATIC_DRAW);const a=gl.getAttribLocation(p,pair[0]);gl.vertexAttribPointer(a,2,gl.FLOAT,false,0,0);gl.enableVertexAttribArray(a);}
      const t=gl.createTexture();gl.bindTexture(gl.TEXTURE_2D,t);gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MIN_FILTER,gl.NEAREST);gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MAG_FILTER,gl.NEAREST);
      const source=new Uint8Array([255,0,0,255,0,255,0,255,0,0,255,255,255,255,255,255]);
      gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,2,2,0,gl.RGBA,gl.UNSIGNED_BYTE,source);gl.uniform1i(gl.getUniformLocation(p,'image'),0);gl.drawArrays(gl.TRIANGLE_STRIP,0,4);
      const data=new Uint8Array(4);gl.readPixels(0,0,1,1,gl.RGBA,gl.UNSIGNED_BYTE,data);console.log(Array.from(data).join(','),gl.getError());
      gl.readPixels(0,3,1,1,gl.RGBA,gl.UNSIGNED_BYTE,data);console.log(Array.from(data).join(','));
      gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL,true);gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,2,2,0,gl.RGBA,gl.UNSIGNED_BYTE,source);gl.drawArrays(gl.TRIANGLE_STRIP,0,4);gl.readPixels(0,0,1,1,gl.RGBA,gl.UNSIGNED_BYTE,data);console.log(Array.from(data).join(','));
      gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MAG_FILTER,gl.NEAREST_MIPMAP_LINEAR);console.log(gl.getError()===gl.INVALID_ENUM,gl.getTexParameter(gl.TEXTURE_2D,gl.TEXTURE_MAG_FILTER)===gl.NEAREST);
      gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,2,2,0,gl.RGBA,gl.UNSIGNED_BYTE,new Uint8Array(3));console.log(gl.getError()===gl.INVALID_OPERATION);
      gl.deleteTexture(t);console.log(gl.isTexture(t),gl.getParameter(gl.TEXTURE_BINDING_2D)===null);
    "#
    );
    let out = run(&mut env, &mut host, &source);
    assert_eq!(
        out,
        [
            "255,0,0,255 0",
            "0,0,255,255",
            "0,0,255,255",
            "true true",
            "true",
            "false true"
        ]
    );
}

const TEXTURED_QUAD: &str = r#"
function textureQuad(gl) {
  const p=gl.createProgram();
  gl.attachShader(p,shader(gl,gl.VERTEX_SHADER,'attribute vec2 position;attribute vec2 uv;varying vec2 vUv;void main(){gl_Position=vec4(position,0.0,1.0);vUv=uv;}'));
  gl.attachShader(p,shader(gl,gl.FRAGMENT_SHADER,'precision mediump float;uniform sampler2D image;varying vec2 vUv;void main(){gl_FragColor=texture2D(image,vUv);}'));
  gl.linkProgram(p);gl.useProgram(p);
  for(const pair of [['position',[-1,-1,1,-1,-1,1,1,1]],['uv',[0,0,1,0,0,1,1,1]]]){
    const b=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,b);gl.bufferData(gl.ARRAY_BUFFER,new Float32Array(pair[1]),gl.STATIC_DRAW);
    const a=gl.getAttribLocation(p,pair[0]);gl.vertexAttribPointer(a,2,gl.FLOAT,false,0,0);gl.enableVertexAttribArray(a);
  }
  const t=gl.createTexture();gl.bindTexture(gl.TEXTURE_2D,t);gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MIN_FILTER,gl.NEAREST);gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MAG_FILTER,gl.NEAREST);
  gl.uniform1i(gl.getUniformLocation(p,'image'),0);return p;
}
"#;

#[test]
fn webgl_texture_uploads_canvas_imagedata_and_imagebitmap_pixels() {
    let (mut env, mut host) = setup(
        "<canvas id=a width=4 height=4></canvas><canvas id=source width=2 height=2></canvas>",
    );
    let source = format!(
        "{SHADERS}\n{TEXTURED_QUAD}\n{}",
        r#"
      const gl=document.getElementById('a').getContext('webgl',{preserveDrawingBuffer:true,premultipliedAlpha:false});textureQuad(gl);
      const image=document.getElementById('source'),ctx=image.getContext('2d');ctx.fillStyle='rgba(255,0,0,0.5)';ctx.fillRect(0,0,2,1);ctx.fillStyle='blue';ctx.fillRect(0,1,2,1);
      const data=new Uint8Array(4);function pixel(){gl.drawArrays(gl.TRIANGLE_STRIP,0,4);gl.readPixels(0,0,1,1,gl.RGBA,gl.UNSIGNED_BYTE,data);return Array.from(data).join(',');}
      gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,gl.RGBA,gl.UNSIGNED_BYTE,image);console.log(pixel());
      gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL,true);gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,gl.RGBA,gl.UNSIGNED_BYTE,image);console.log(pixel());
      gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL,true);gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,gl.RGBA,gl.UNSIGNED_BYTE,image);console.log(pixel());
      createImageBitmap(image).then(bitmap=>{
        gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,gl.RGBA,gl.UNSIGNED_BYTE,bitmap);console.log(pixel());
        gl.texSubImage2D(gl.TEXTURE_2D,0,0,0,gl.RGBA,gl.UNSIGNED_BYTE,new ImageData(new Uint8ClampedArray([0,255,0,255]),1,1));console.log(pixel(),gl.getError());
      });
    "#
    );
    let out = run(&mut env, &mut host, &source);
    assert_eq!(
        out,
        [
            "255,0,0,128",
            "128,0,0,128",
            "0,0,255,255",
            "255,0,0,128",
            "0,255,0,255 0"
        ]
    );
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn webgl_deleted_attached_resources_remain_live_until_released() {
    let (mut env, mut host) = setup("<canvas id=a width=4 height=4></canvas>");
    let source = format!(
        "{SHADERS}\n{}",
        r#"
      const gl=document.getElementById('a').getContext('webgl',{preserveDrawingBuffer:true});const p=triangleProgram(gl);
      const attached=gl.getAttachedShaders(p)[0];console.log(attached instanceof WebGLObject,p instanceof WebGLObject);
      gl.deleteShader(attached);console.log(gl.isShader(attached),gl.getShaderParameter(attached,gl.DELETE_STATUS));
      gl.deleteProgram(p);console.log(gl.isProgram(p),gl.getProgramParameter(p,gl.DELETE_STATUS));gl.drawArrays(gl.TRIANGLES,0,3);console.log(gl.getError());
      gl.useProgram(null);console.log(gl.isProgram(p),gl.isShader(attached));
      console.log(gl.getShaderParameter(attached,gl.DELETE_STATUS)===null,gl.getError()===gl.INVALID_VALUE);
    "#
    );
    let out = run(&mut env, &mut host, &source);
    assert_eq!(
        out,
        [
            "true true",
            "true true",
            "true true",
            "0",
            "false false",
            "true true"
        ]
    );
}

#[test]
fn webgl_argument_errors_alignment_and_creation_limits_do_not_allocate_huge_buffers() {
    let (mut env,mut host)=setup("<canvas id=a width=4 height=4></canvas><canvas id=huge width=50000 height=50000></canvas><canvas id=reentrant width=4 height=4></canvas>");
    let out = run(
        &mut env,
        &mut host,
        r#"
      const a=document.getElementById('a'),gl=a.getContext('webgl',{preserveDrawingBuffer:true});
      let thrown=0;for(const callback of [()=>gl.enableVertexAttribArray(),()=>gl.getExtension(),()=>gl.getShaderPrecisionFormat(),()=>gl.isShader(),()=>gl.clear()]){try{callback();}catch(error){if(error instanceof TypeError)thrown++;}}console.log(thrown);
      const b=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,b);gl.bufferData(gl.ARRAY_BUFFER,32,gl.STATIC_DRAW);gl.vertexAttribPointer(0,2,gl.FLOAT,false,0,1);console.log(gl.getError()===gl.INVALID_OPERATION);
      gl.vertexAttribPointer(0,2,gl.FLOAT,false,256,0);console.log(gl.getError()===gl.INVALID_VALUE);
      const huge=document.getElementById('huge');huge.addEventListener('webglcontextcreationerror',event=>console.log(event.cancelable,event.statusMessage.length>0));console.log(huge.getContext('webgl')===null);
      const c=document.getElementById('reentrant');const options={get alpha(){c.getContext('2d');return true;}};console.log(c.getContext('webgl',options)===null,c.getContext('2d')!==null);
    "#,
    );
    assert_eq!(out, ["5", "true", "true", "true true", "true", "true true"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn webgl1_does_not_advertise_webgl2_or_accept_glsl300() {
    let (mut env, mut host) = setup("<canvas id=a width=4 height=4></canvas>");
    let out = run(
        &mut env,
        &mut host,
        r#"
      const canvas=document.getElementById('a');console.log(canvas.getContext('webgl2')===null);
      const gl=canvas.getContext('webgl'),s=gl.createShader(gl.FRAGMENT_SHADER);
      gl.shaderSource(s,'#version 300 es\nprecision mediump float;out vec4 color;void main(){color=vec4(1.0);}');gl.compileShader(s);
      console.log(gl.getShaderParameter(s,gl.COMPILE_STATUS)===false,gl.getShaderInfoLog(s).length>0,gl.getParameter(gl.VERSION).startsWith('WebGL 1.0'),gl.getError());
    "#,
    );
    assert_eq!(out, ["true", "true true true 0"]);
}

#[test]
fn webgl_sampler_integer_sequences_do_not_round_through_float32() {
    let (mut env, mut host) = setup("<canvas id=a width=4 height=4></canvas>");
    let source = format!(
        "{SHADERS}\n{TEXTURED_QUAD}\n{}",
        r#"
      const gl=document.getElementById('a').getContext('webgl',{preserveDrawingBuffer:true});const p=textureQuad(gl);
      const loc=gl.getUniformLocation(p,'image');gl.uniform1iv(loc,[4294967297]);console.log(gl.getUniform(p,loc),gl.getError());
    "#
    );
    assert_eq!(run(&mut env, &mut host, &source), ["1 0"]);
}

#[test]
fn webgl_enum_arguments_are_converted_once_before_native_commands() {
    let (mut env, mut host) = setup("<canvas id=a width=4 height=4></canvas>");
    let out = run(
        &mut env,
        &mut host,
        r#"
      const gl=document.getElementById('a').getContext('webgl',{preserveDrawingBuffer:true});
      let shaderConversions=0;const shader=gl.createShader({valueOf(){shaderConversions++;return shaderConversions===1?gl.VERTEX_SHADER:gl.FRAGMENT_SHADER;}});
      console.log(shaderConversions,gl.getShaderParameter(shader,gl.SHADER_TYPE)===gl.VERTEX_SHADER);
      const b=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,b);
      let targets=0;gl.bufferData({valueOf(){targets++;return targets===1?gl.ARRAY_BUFFER:gl.ELEMENT_ARRAY_BUFFER;}},new Float32Array([1]),gl.STATIC_DRAW);
      console.log(targets,gl.getBufferParameter(gl.ARRAY_BUFFER,gl.BUFFER_SIZE),gl.getError());
      let numberError=false;try{gl.clearColor(1n,0,0,1);}catch(error){numberError=error instanceof TypeError;}console.log(numberError);
      targets=0;gl.bufferSubData({valueOf(){targets++;return targets===1?gl.ARRAY_BUFFER:gl.ELEMENT_ARRAY_BUFFER;}},0,new Float32Array([.5]));console.log(targets,gl.getError());
    "#,
    );
    assert_eq!(out, ["1 true", "1 4 0", "true", "1 0"]);
}

#[test]
fn webgl_linear_texture_filtering_reads_interpolated_pixels() {
    let (mut env, mut host) = setup("<canvas id=a width=3 height=3></canvas>");
    let source = format!("{SHADERS}\n{TEXTURED_QUAD}\n{}", r#"
      const gl=document.getElementById('a').getContext('webgl',{preserveDrawingBuffer:true,premultipliedAlpha:false});textureQuad(gl);
      gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_WRAP_S,gl.CLAMP_TO_EDGE);gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_WRAP_T,gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MIN_FILTER,gl.LINEAR);gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MAG_FILTER,gl.LINEAR);
      gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,2,2,0,gl.RGBA,gl.UNSIGNED_BYTE,new Uint8Array([255,0,0,255,0,255,0,255,0,0,255,255,255,255,255,255]));
      gl.drawArrays(gl.TRIANGLE_STRIP,0,4);const data=new Uint8Array(4);
      for(const xy of [[0,0],[1,1],[0,2]]){gl.readPixels(xy[0],xy[1],1,1,gl.RGBA,gl.UNSIGNED_BYTE,data);console.log(Array.from(data).join(','));}
      console.log(gl.getError(),gl.getTexParameter(gl.TEXTURE_2D,gl.TEXTURE_MIN_FILTER)===gl.LINEAR,gl.getTexParameter(gl.TEXTURE_2D,gl.TEXTURE_MAG_FILTER)===gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MAG_FILTER,gl.NEAREST);gl.drawArrays(gl.TRIANGLE_STRIP,0,4);console.log(gl.getError()===gl.INVALID_OPERATION);
      gl.readPixels(1,1,1,1,gl.RGBA,gl.UNSIGNED_BYTE,data);console.log(Array.from(data).join(','));
    "#);
    assert_eq!(run(&mut env, &mut host, &source), ["255,0,0,255", "128,128,128,255", "0,0,255,255", "0 true true", "true", "128,128,128,255"]);
}

#[test]
fn webgl_blending_state_validates_enums_atomically_and_converts_once() {
    let (mut env, mut host) = setup("<canvas id=a width=4 height=4></canvas>");
    let out = run(
        &mut env,
        &mut host,
        r#"
      const gl=document.getElementById('a').getContext('webgl',{preserveDrawingBuffer:true});
      console.log(gl.isEnabled(gl.BLEND),gl.getParameter(gl.BLEND),gl.getParameter(gl.BLEND_SRC_RGB)===gl.ONE,gl.getParameter(gl.BLEND_DST_ALPHA)===gl.ZERO,gl.getParameter(gl.BLEND_EQUATION)===gl.FUNC_ADD);
      gl.enable(gl.BLEND);console.log(gl.isEnabled(gl.BLEND),gl.getParameter(gl.BLEND),gl.getError());
      let count=0;gl.blendFunc({valueOf(){count++;return gl.SRC_ALPHA;}},gl.ONE_MINUS_SRC_ALPHA);
      console.log(count,gl.getParameter(gl.BLEND_SRC_RGB)===gl.SRC_ALPHA,gl.getParameter(gl.BLEND_SRC_ALPHA)===gl.SRC_ALPHA);
      gl.blendFuncSeparate(gl.CONSTANT_COLOR,gl.CONSTANT_ALPHA,gl.ONE,gl.ZERO);
      console.log(gl.getError()===gl.INVALID_OPERATION,gl.getParameter(gl.BLEND_SRC_RGB)===gl.SRC_ALPHA);
      gl.blendFuncSeparate(gl.ONE,gl.ZERO,gl.CONSTANT_COLOR,gl.CONSTANT_ALPHA);console.log(gl.getError());
      gl.blendFuncSeparate(gl.ONE,gl.SRC_ALPHA_SATURATE,gl.ONE,gl.ZERO);console.log(gl.getError()===gl.INVALID_ENUM,gl.getParameter(gl.BLEND_DST_ALPHA)===gl.CONSTANT_ALPHA);
      gl.blendEquationSeparate(gl.FUNC_SUBTRACT,0x8007);console.log(gl.getError()===gl.INVALID_ENUM,gl.getParameter(gl.BLEND_EQUATION_RGB)===gl.FUNC_ADD);
      gl.blendEquationSeparate(gl.FUNC_REVERSE_SUBTRACT,gl.FUNC_SUBTRACT);console.log(gl.getParameter(gl.BLEND_EQUATION_RGB)===gl.FUNC_REVERSE_SUBTRACT,gl.getParameter(gl.BLEND_EQUATION_ALPHA)===gl.FUNC_SUBTRACT);
      gl.blendColor(-1,2,0.25,NaN);const c=gl.getParameter(gl.BLEND_COLOR);c[2]=0;console.log(Array.from(gl.getParameter(gl.BLEND_COLOR)).join(','));
      let errors=0;for(const f of [()=>gl.blendFunc(1),()=>gl.blendFuncSeparate(1,0,1),()=>gl.blendEquation(),()=>gl.blendEquationSeparate(1),()=>gl.blendColor(1,0,0),()=>gl.blendColor(1n,0,0,0)]){try{f();}catch(e){if(e instanceof TypeError)errors++;}}
      gl.disable(gl.BLEND);console.log(errors,gl.isEnabled(gl.BLEND),gl.getError());
      document.getElementById('a').width=4;console.log(gl.getParameter(gl.BLEND_EQUATION_RGB)===gl.FUNC_REVERSE_SUBTRACT,gl.getParameter(gl.BLEND_SRC_ALPHA)===gl.CONSTANT_COLOR);
      gl.enable(gl.BLEND);document.getElementById('a').width=5000;console.log(gl.isEnabled(gl.BLEND),gl.getParameter(gl.BLEND));
    "#,
    );
    assert_eq!(
        out,
        [
            "false false true true true",
            "true true 0",
            "1 true true",
            "true true",
            "0",
            "true true",
            "true true",
            "true true",
            "0,1,0.25,0",
            "6 false 0",
            "true true",
            "false null"
        ]
    );
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn webgl_blending_draws_real_pixels_and_clear_ignores_equations() {
    let (mut env, mut host) = setup("<canvas id=a width=4 height=4></canvas>");
    run(&mut env, &mut host, SHADERS);
    let out = run(
        &mut env,
        &mut host,
        r#"
      const gl=document.getElementById('a').getContext('webgl',{preserveDrawingBuffer:true,premultipliedAlpha:false});
      const p=triangleProgram(gl),fs=gl.getAttachedShaders(p).find(s=>gl.getShaderParameter(s,gl.SHADER_TYPE)===gl.FRAGMENT_SHADER);
      gl.shaderSource(fs,'precision mediump float; uniform vec4 color; void main(){gl_FragColor=color;}');gl.compileShader(fs);gl.linkProgram(p);gl.useProgram(p);
      const color=gl.getUniformLocation(p,'color'),pixel=new Uint8Array(4);
      const sample=()=>{gl.readPixels(1,1,1,1,gl.RGBA,gl.UNSIGNED_BYTE,pixel);return Array.from(pixel).join(',');};
      gl.clearColor(0,0,1,1);gl.clear(gl.COLOR_BUFFER_BIT);gl.enable(gl.BLEND);
      gl.blendFuncSeparate(gl.SRC_ALPHA,gl.ONE_MINUS_SRC_ALPHA,gl.ONE,gl.ONE_MINUS_SRC_ALPHA);
      gl.uniform4f(color,1,0,0,0.5);gl.drawArrays(gl.TRIANGLES,0,3);console.log(sample(),gl.getError());
      gl.blendEquation(gl.FUNC_REVERSE_SUBTRACT);gl.clearColor(0.25,0.5,0.75,0.5);gl.clear(gl.COLOR_BUFFER_BIT);console.log(sample());
      gl.blendFunc(gl.ONE,gl.ONE);gl.uniform4f(color,0.125,0.25,0.5,0.25);gl.drawArrays(gl.TRIANGLES,0,3);console.log(sample());
      gl.disable(gl.BLEND);gl.uniform4f(color,1,0,0,0.5);gl.drawArrays(gl.TRIANGLES,0,3);console.log(sample());
    "#,
    );
    assert_eq!(
        out,
        [
            "128,0,128,255 0",
            "64,128,191,128",
            "32,64,64,64",
            "255,0,0,128"
        ]
    );
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn webgl_blending_rgb_only_destination_alpha_and_canvas_premultiplication() {
    let (mut env,mut host)=setup("<canvas id=a width=4 height=4></canvas><canvas id=b width=4 height=4></canvas><canvas id=c width=4 height=4></canvas>");
    run(&mut env, &mut host, SHADERS);
    let out = run(
        &mut env,
        &mut host,
        r#"
      for(const id of ['a','b','c']){
        const gl=document.getElementById(id).getContext('webgl',{preserveDrawingBuffer:true,alpha:id!=='a',premultipliedAlpha:id==='c'}),p=triangleProgram(gl);
        gl.clearColor(0,0,0,0);gl.clear(gl.COLOR_BUFFER_BIT);gl.enable(gl.BLEND);
        gl.blendFunc(gl.DST_ALPHA,gl.ZERO);gl.drawArrays(gl.TRIANGLES,0,3);
        const pixel=new Uint8Array(4);gl.readPixels(1,1,1,1,gl.RGBA,gl.UNSIGNED_BYTE,pixel);console.log(Array.from(pixel).join(','),gl.getError());
      }
    "#,
    );
    assert_eq!(out, ["255,0,0,255 0", "0,0,0,0 0", "0,0,0,0 0"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}
