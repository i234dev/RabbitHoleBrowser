//! Experimental WebGL bridge to our own software GL implementation.
//!
//! JavaScript owns WebIDL conversion, object brands and the GL error flags.
//! The host owns the real contexts and copies their drawing buffers into the
//! same RGBA surfaces that Canvas2D presents to the engine.

use crate::canvas2d::{self, Surface};
use crate::host::{arg, arg_str, object_from};
use rabbithole_js::{JsResult, ObjectId, Realm, Value};
use rabbithole_webgl::Context;
use std::collections::HashMap;

const INVALID_OPERATION: u32 = 0x0502;
// Limit a single upload before copying JS-owned bytes. The backend also
// enforces its own aggregate resource budget across all buffer objects.
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct WebGlStore {
    pub(crate) contexts: HashMap<u32, DrawingContext>,
}

pub(crate) struct DrawingContext {
    ctx: Context,
    alpha: bool,
    premultiplied: bool,
}

impl DrawingContext {
    fn sync(&self, surface: &mut Surface) {
        let (w, h) = self.ctx.dimensions();
        surface.width = w;
        surface.height = h;
        surface.rgba.clear();
        surface.rgba.extend_from_slice(self.ctx.framebuffer_rgba());
        for pixel in surface.rgba.chunks_exact_mut(4) {
            if !self.alpha {
                pixel[3] = 255;
            } else if self.premultiplied {
                // The GL buffer is interpreted as premultiplied by the page
                // compositor; Canvas2D's source surfaces use straight RGBA.
                let a = pixel[3] as u32;
                for channel in &mut pixel[..3] {
                    *channel = if a == 0 {
                        0
                    } else {
                        ((*channel as u32 * 255 + a / 2) / a).min(255) as u8
                    };
                }
            }
        }
    }
}

enum Reply {
    Void,
    Number(f64),
    Bool(bool),
    Text(String),
    Bytes(Vec<u8>),
    Infos(Vec<rabbithole_webgl::ActiveInfo>),
    Values(Vec<f32>),
}

type GlResult = Result<Reply, u32>;

fn response(realm: &mut Realm, result: GlResult) -> Value {
    let (error, value) = match result {
        Ok(Reply::Void) => (0, Value::Undefined),
        Ok(Reply::Number(n)) => (0, Value::Number(n)),
        Ok(Reply::Bool(b)) => (0, Value::Bool(b)),
        Ok(Reply::Text(s)) => (0, Value::str(&s)),
        Ok(Reply::Bytes(b)) => (0, crate::fetch::make_array_buffer(realm, b)),
        Ok(Reply::Values(values)) => (
            0,
            crate::fetch::make_array_buffer(
                realm,
                values.iter().flat_map(|v| v.to_le_bytes()).collect(),
            ),
        ),
        Ok(Reply::Infos(infos)) => {
            let values = infos
                .into_iter()
                .map(|info| {
                    object_from(
                        realm,
                        vec![
                            ("name", Value::str(&info.name)),
                            ("size", Value::Number(info.size as f64)),
                            ("type", Value::Number(info.kind as f64)),
                        ],
                    )
                })
                .collect();
            (0, realm.new_array_value(values))
        }
        Err(e) => (e, Value::Null),
    };
    object_from(
        realm,
        vec![("error", Value::Number(error as f64)), ("value", value)],
    )
}

fn number(realm: &mut Realm, args: &[Value], i: usize) -> JsResult<f64> {
    realm.to_number(arg(args, i))
}

fn byte_source(realm: &mut Realm, value: &Value) -> JsResult<Vec<u8>> {
    let Some(id) = value.as_object() else {
        return Err(realm.throw_type_error("WebGL: expected an ArrayBuffer or ArrayBufferView"));
    };
    let length = match &realm.obj(id).kind {
        rabbithole_js::ObjectKind::ArrayBuffer(buffer) => buffer.bytes.borrow().len(),
        rabbithole_js::ObjectKind::TypedArray(view) => {
            rabbithole_js::builtins::typed_array::typed_array_length(realm, view)
                .saturating_mul(view.kind.element_size())
        }
        _ => return Err(realm.throw_type_error("WebGL: expected an ArrayBuffer or typed array")),
    };
    if length > MAX_UPLOAD_BYTES {
        return Err(realm.throw_range_error("WebGL upload exceeds the renderer's memory limit"));
    }
    Ok(crate::fetch::body_bytes_from_value(realm, value)?.0)
}

fn limits(realm: &mut Realm, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(object_from(
        realm,
        vec![
            ("maxUploadBytes", Value::Number(MAX_UPLOAD_BYTES as f64)),
            (
                "maxDimension",
                Value::Number(rabbithole_webgl::MAX_DIMENSION as f64),
            ),
            (
                "maxPixels",
                Value::Number(rabbithole_webgl::MAX_PIXELS as f64),
            ),
            (
                "maxVertexAttribs",
                Value::Number(rabbithole_webgl::MAX_VERTEX_ATTRIBS as f64),
            ),
            (
                "maxTextureUnits",
                Value::Number(rabbithole_webgl::MAX_TEXTURE_UNITS as f64),
            ),
        ],
    ))
}

fn create(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let id = number(realm, args, 0)? as u32;
    let w = (number(realm, args, 1)? as u32).max(1);
    let h = (number(realm, args, 2)? as u32).max(1);
    let alpha = realm.to_boolean(&arg(args, 3));
    let premultiplied = realm.to_boolean(&arg(args, 4));
    let version = number(realm, args, 5)? as u32;
    let depth = realm.to_boolean(&arg(args, 6));
    let result = match version {
        1 => Context::new(w, h),
        2 => Context::new_webgl2(w, h),
        _ => Err(rabbithole_webgl::Error::InvalidValue),
    }
    .map_err(|e| e.code())
    .and_then(|mut ctx| {
        ctx.set_alpha_channel(alpha);
        ctx.set_depth_channel(depth);
        canvas2d::store(|store| {
            if !store.surfaces.contains_key(&id) || store.webgl.contexts.contains_key(&id) {
                return Err(INVALID_OPERATION);
            }
            let context = DrawingContext {
                ctx,
                alpha,
                premultiplied,
            };
            context.sync(store.surfaces.get_mut(&id).expect("checked canvas"));
            store.webgl.contexts.insert(id, context);
            Ok(Reply::Void)
        })
    });
    Ok(response(realm, result))
}

pub(crate) fn resize_surface(id: u32, w: u32, h: u32) -> Result<(), u32> {
    canvas2d::store(|store| {
        let record = store.webgl.contexts.get_mut(&id).ok_or(INVALID_OPERATION)?;
        record
            .ctx
            .resize(w.max(1), h.max(1))
            .map_err(|e| e.code())?;
        if let Some(surface) = store.surfaces.get_mut(&id) {
            record.sync(surface);
        }
        Ok(())
    })
}

fn resize(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let id = number(realm, args, 0)? as u32;
    let w = number(realm, args, 1)? as u32;
    let h = number(realm, args, 2)? as u32;
    Ok(response(
        realm,
        resize_surface(id, w, h).map(|_| Reply::Void),
    ))
}

fn lose(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let id = number(realm, args, 0)? as u32;
    canvas2d::store(|store| {
        store.webgl.contexts.remove(&id);
        if let Some(surface) = store.surfaces.get_mut(&id) {
            surface.width = 0;
            surface.height = 0;
            surface.rgba.clear();
        }
    });
    Ok(Value::Undefined)
}

fn discard(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let id = number(realm, args, 0)? as u32;
    let result = canvas2d::store(|store| {
        let record = store.webgl.contexts.get_mut(&id).ok_or(INVALID_OPERATION)?;
        let (w, h) = record.ctx.dimensions();
        // Discard the GL drawing buffer after presentation. The Surface is
        // deliberately retained: it is the image already handed to the page.
        record.ctx.resize(w, h).map_err(|e| e.code())?;
        Ok(Reply::Void)
    });
    Ok(response(realm, result))
}

// Parse every argument before borrowing host state: ToNumber/ToString may
// execute arbitrary page JavaScript, including more canvas commands.
enum Command {
    CreateShader(u32),
    ShaderSource(u32, String),
    CompileShader(u32),
    ShaderStatus(u32),
    ShaderLog(u32),
    IsShader(u32),
    GetShaderSource(u32),
    DeleteShader(u32),
    CreateProgram,
    AttachShader(u32, u32),
    LinkProgram(u32),
    ProgramStatus(u32),
    ProgramLog(u32),
    IsProgram(u32),
    UseProgram(u32),
    DeleteProgram(u32),
    AttribLocation(u32, String),
    BindAttrib(u32, u32, String),
    UniformLocation(u32, String),
    Uniform(i32, Vec<f32>),
    Matrix(i32, bool, Vec<f32>),
    CreateVertexArray,
    BindVertexArray(u32),
    DeleteVertexArray(u32),
    IsVertexArray(u32),
    CreateBuffer,
    BindBuffer(u32, u32),
    BufferData(u32, Vec<u8>),
    BufferSize(u32, usize),
    BufferSubData(u32, usize, Vec<u8>),
    DeleteBuffer(u32),
    AttribPointer(u32, u32, u32, bool, u32, usize),
    EnableAttrib(u32, bool),
    Attrib(u32, [f32; 4]),
    Viewport(i32, i32, u32, u32),
    Scissor(i32, i32, u32, u32),
    EnableScissor(bool),
    EnableDepth(bool),
    EnableBlend(bool),
    BlendFunc(u32, u32, u32, u32),
    BlendEquation(u32, u32),
    BlendColor([f32; 4]),
    DepthMask(bool),
    DepthFunc(u32),
    ColorMask([bool; 4]),
    ClearColor([f32; 4]),
    ClearDepth(f32),
    Clear(u32),
    AttribDivisor(u32, u32),
    DrawArraysInstanced(u32, u32, u32, u32),
    DrawElementsInstanced(u32, u32, u32, usize, u32),
    DrawArrays(u32, u32, u32),
    DrawElements(u32, u32, u32, usize),
    ReadPixels(i32, i32, u32, u32),
    BlitFramebuffer([i32; 4], [i32; 4], u32, u32),
    Width,
    Height,
    ReadWidth,
    ReadHeight,
    CreateFramebuffer,
    BindFramebuffer(u32, u32),
    DeleteFramebuffer(u32),
    CheckFramebuffer(u32),
    FramebufferTexture(u32, u32, u32, u32, i32),
    FramebufferRenderbuffer(u32, u32, u32, u32),
    FramebufferAttachment(u32, u32, u32),
    FramebufferBits(u32),
    CreateRenderbuffer,
    BindRenderbuffer(u32, u32),
    DeleteRenderbuffer(u32),
    RenderbufferStorage(u32, u32, u32, u32),
    RenderbufferParameter(u32, u32),
    ActiveAttribs(u32),
    ActiveUniforms(u32),
    GetUniform(u32, i32),
    UniformI(i32, Vec<i32>),
    CreateTexture,
    ActiveTexture(u32),
    BindTexture(u32, u32),
    DeleteTexture(u32),
    TexParameter(u32, u32, i32),
    GetTexParameter(u32, u32),
    TexImage(u32, i32, u32, u32, u32, i32, u32, u32, Option<Vec<u8>>),
    TexSubImage(u32, i32, i32, i32, u32, u32, u32, u32, Vec<u8>),
}

impl Command {
    fn parse(realm: &mut Realm, op: &str, args: &[Value]) -> JsResult<Option<Self>> {
        macro_rules! n {
            ($i:expr) => {
                number(realm, args, $i)?
            };
        }
        macro_rules! u {
            ($i:expr) => {
                n!($i) as u32
            };
        }
        macro_rules! b {
            ($i:expr) => {
                realm.to_boolean(&arg(args, $i))
            };
        }
        let floats = |realm: &mut Realm, i| -> JsResult<Vec<f32>> {
            let bytes = byte_source(realm, &arg(args, i))?;
            Ok(bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect())
        };
        Ok(Some(match op {
            "createShader" => Self::CreateShader(u!(2)),
            "shaderSource" => Self::ShaderSource(u!(2), arg_str(realm, args, 3)?),
            "compileShader" => Self::CompileShader(u!(2)),
            "shaderStatus" => Self::ShaderStatus(u!(2)),
            "shaderLog" => Self::ShaderLog(u!(2)),
            "isShader" => Self::IsShader(u!(2)),
            "getShaderSource" => Self::GetShaderSource(u!(2)),
            "deleteShader" => Self::DeleteShader(u!(2)),
            "createProgram" => Self::CreateProgram,
            "attachShader" => Self::AttachShader(u!(2), u!(3)),
            "linkProgram" => Self::LinkProgram(u!(2)),
            "programStatus" => Self::ProgramStatus(u!(2)),
            "programLog" => Self::ProgramLog(u!(2)),
            "isProgram" => Self::IsProgram(u!(2)),
            "useProgram" => Self::UseProgram(u!(2)),
            "deleteProgram" => Self::DeleteProgram(u!(2)),
            "attribLocation" => Self::AttribLocation(u!(2), arg_str(realm, args, 3)?),
            "bindAttrib" => Self::BindAttrib(u!(2), u!(3), arg_str(realm, args, 4)?),
            "uniformLocation" => Self::UniformLocation(u!(2), arg_str(realm, args, 3)?),
            "uniform" => Self::Uniform(n!(2) as i32, floats(realm, 3)?),
            "matrix4" => Self::Matrix(n!(2) as i32, b!(3), floats(realm, 4)?),
            "createVertexArray" => Self::CreateVertexArray,
            "bindVertexArray" => Self::BindVertexArray(u!(2)),
            "deleteVertexArray" => Self::DeleteVertexArray(u!(2)),
            "isVertexArray" => Self::IsVertexArray(u!(2)),
            "createBuffer" => Self::CreateBuffer,
            "bindBuffer" => Self::BindBuffer(u!(2), u!(3)),
            "bufferData" => Self::BufferData(u!(2), byte_source(realm, &arg(args, 3))?),
            "bufferSize" => Self::BufferSize(u!(2), n!(3) as usize),
            "bufferSubData" => {
                Self::BufferSubData(u!(2), n!(3) as usize, byte_source(realm, &arg(args, 4))?)
            }
            "deleteBuffer" => Self::DeleteBuffer(u!(2)),
            "attribPointer" => {
                Self::AttribPointer(u!(2), u!(3), u!(4), b!(5), u!(6), n!(7) as usize)
            }
            "enableAttrib" => Self::EnableAttrib(u!(2), b!(3)),
            "attrib" => Self::Attrib(
                u!(2),
                [n!(3) as f32, n!(4) as f32, n!(5) as f32, n!(6) as f32],
            ),
            "viewport" => Self::Viewport(n!(2) as i32, n!(3) as i32, u!(4), u!(5)),
            "scissor" => Self::Scissor(n!(2) as i32, n!(3) as i32, u!(4), u!(5)),
            "enableScissor" => Self::EnableScissor(b!(2)),
            "enableDepth" => Self::EnableDepth(b!(2)),
            "enableBlend" => Self::EnableBlend(b!(2)),
            "blendFunc" => Self::BlendFunc(u!(2), u!(3), u!(4), u!(5)),
            "blendEquation" => Self::BlendEquation(u!(2), u!(3)),
            "blendColor" => {
                Self::BlendColor([n!(2) as f32, n!(3) as f32, n!(4) as f32, n!(5) as f32])
            }
            "depthMask" => Self::DepthMask(b!(2)),
            "depthFunc" => Self::DepthFunc(u!(2)),
            "colorMask" => Self::ColorMask([b!(2), b!(3), b!(4), b!(5)]),
            "clearColor" => {
                Self::ClearColor([n!(2) as f32, n!(3) as f32, n!(4) as f32, n!(5) as f32])
            }
            "clearDepth" => Self::ClearDepth(n!(2) as f32),
            "clear" => Self::Clear(u!(2)),
            "attribDivisor" => Self::AttribDivisor(u!(2), u!(3)),
            "drawArraysInstanced" => Self::DrawArraysInstanced(u!(2), u!(3), u!(4), u!(5)),
            "drawElementsInstanced" => {
                Self::DrawElementsInstanced(u!(2), u!(3), u!(4), n!(5) as usize, u!(6))
            }
            "drawArrays" => Self::DrawArrays(u!(2), u!(3), u!(4)),
            "drawElements" => Self::DrawElements(u!(2), u!(3), u!(4), n!(5) as usize),
            "readPixels" => Self::ReadPixels(n!(2) as i32, n!(3) as i32, u!(4), u!(5)),
            "blitFramebuffer" => Self::BlitFramebuffer(
                [n!(2) as i32, n!(3) as i32, n!(4) as i32, n!(5) as i32],
                [n!(6) as i32, n!(7) as i32, n!(8) as i32, n!(9) as i32], u!(10), u!(11)),
            "width" => Self::Width,
            "height" => Self::Height,
            "readWidth" => Self::ReadWidth,
            "readHeight" => Self::ReadHeight,
            "createFramebuffer" => Self::CreateFramebuffer,
            "bindFramebuffer" => Self::BindFramebuffer(u!(2), u!(3)),
            "deleteFramebuffer" => Self::DeleteFramebuffer(u!(2)),
            "checkFramebuffer" => Self::CheckFramebuffer(u!(2)),
            "framebufferTexture" => {
                Self::FramebufferTexture(u!(2), u!(3), u!(4), u!(5), n!(6) as i32)
            }
            "framebufferRenderbuffer" => Self::FramebufferRenderbuffer(u!(2), u!(3), u!(4), u!(5)),
            "framebufferAttachment" => Self::FramebufferAttachment(u!(2), u!(3), u!(4)),
            "framebufferBits" => Self::FramebufferBits(u!(2)),
            "createRenderbuffer" => Self::CreateRenderbuffer,
            "bindRenderbuffer" => Self::BindRenderbuffer(u!(2), u!(3)),
            "deleteRenderbuffer" => Self::DeleteRenderbuffer(u!(2)),
            "renderbufferStorage" => Self::RenderbufferStorage(u!(2), u!(3), u!(4), u!(5)),
            "renderbufferParameter" => Self::RenderbufferParameter(u!(2), u!(3)),
            "activeAttribs" => Self::ActiveAttribs(u!(2)),
            "activeUniforms" => Self::ActiveUniforms(u!(2)),
            "getUniform" => Self::GetUniform(u!(2), n!(3) as i32),
            "uniformI" => Self::UniformI(
                n!(2) as i32,
                byte_source(realm, &arg(args, 3))?
                    .chunks_exact(4)
                    .map(|v| i32::from_le_bytes(v.try_into().unwrap()))
                    .collect(),
            ),
            "createTexture" => Self::CreateTexture,
            "activeTexture" => Self::ActiveTexture(u!(2)),
            "bindTexture" => Self::BindTexture(u!(2), u!(3)),
            "deleteTexture" => Self::DeleteTexture(u!(2)),
            "texParameter" => Self::TexParameter(u!(2), u!(3), n!(4) as i32),
            "getTexParameter" => Self::GetTexParameter(u!(2), u!(3)),
            "texImage" => Self::TexImage(
                u!(2),
                n!(3) as i32,
                u!(4),
                u!(5),
                u!(6),
                n!(7) as i32,
                u!(8),
                u!(9),
                if arg(args, 10).is_nullish() {
                    None
                } else {
                    Some(byte_source(realm, &arg(args, 10))?)
                },
            ),
            "texSubImage" => Self::TexSubImage(
                u!(2),
                n!(3) as i32,
                n!(4) as i32,
                n!(5) as i32,
                u!(6),
                u!(7),
                u!(8),
                u!(9),
                byte_source(realm, &arg(args, 10))?,
            ),
            _ => return Ok(None),
        }))
    }

    fn draws(&self) -> bool {
        matches!(
            self,
            Self::Clear(_)
                | Self::BlitFramebuffer(..)
                | Self::DrawArrays(..)
                | Self::DrawElements(..)
                | Self::DrawArraysInstanced(..)
                | Self::DrawElementsInstanced(..)
        )
    }

    fn execute(self, record: &mut DrawingContext) -> GlResult {
        let ctx = &mut record.ctx;
        let result: Result<Reply, rabbithole_webgl::Error> = (|| {
            Ok(match self {
                Self::CreateShader(kind) => Reply::Number(ctx.create_shader(kind)? as f64),
                Self::ShaderSource(id, source) => {
                    ctx.shader_source(id, &source)?;
                    Reply::Void
                }
                Self::CompileShader(id) => {
                    ctx.compile_shader(id)?;
                    Reply::Void
                }
                Self::ShaderStatus(id) => Reply::Bool(ctx.shader_compile_status(id)?),
                Self::ShaderLog(id) => Reply::Text(ctx.shader_info_log(id)?.to_string()),
                Self::IsShader(id) => Reply::Bool(ctx.shader_compile_status(id).is_ok()),
                Self::GetShaderSource(id) => Reply::Text(ctx.shader_source_text(id)?.to_string()),
                Self::DeleteShader(id) => {
                    ctx.delete_shader(id)?;
                    Reply::Void
                }
                Self::CreateProgram => Reply::Number(ctx.create_program() as f64),
                Self::AttachShader(p, s) => {
                    ctx.attach_shader(p, s)?;
                    Reply::Void
                }
                Self::LinkProgram(p) => {
                    ctx.link_program(p)?;
                    Reply::Void
                }
                Self::ProgramStatus(p) => Reply::Bool(ctx.program_link_status(p)?),
                Self::ProgramLog(p) => Reply::Text(ctx.program_info_log(p)?.to_string()),
                Self::IsProgram(p) => Reply::Bool(ctx.program_link_status(p).is_ok()),
                Self::UseProgram(p) => {
                    ctx.use_program(p)?;
                    Reply::Void
                }
                Self::DeleteProgram(p) => {
                    ctx.delete_program(p)?;
                    Reply::Void
                }
                Self::AttribLocation(p, name) => {
                    Reply::Number(ctx.get_attrib_location(p, &name)? as f64)
                }
                Self::BindAttrib(p, index, name) => {
                    ctx.bind_attrib_location(p, index, &name)?;
                    Reply::Void
                }
                Self::UniformLocation(p, name) => {
                    Reply::Number(ctx.get_uniform_location(p, &name)?.unwrap_or(-1) as f64)
                }
                Self::Uniform(location, values) => {
                    ctx.uniform_f32(location, &values)?;
                    Reply::Void
                }
                Self::Matrix(location, transpose, values) => {
                    ctx.uniform_matrix4fv(location, transpose, &values)?;
                    Reply::Void
                }
                Self::CreateVertexArray => Reply::Number(ctx.create_vertex_array()? as f64),
                Self::BindVertexArray(id) => {
                    ctx.bind_vertex_array(id)?;
                    Reply::Void
                }
                Self::DeleteVertexArray(id) => {
                    ctx.delete_vertex_array(id)?;
                    Reply::Void
                }
                Self::IsVertexArray(id) => Reply::Bool(ctx.is_vertex_array(id)),
                Self::CreateBuffer => Reply::Number(ctx.create_buffer() as f64),
                Self::BindBuffer(target, buffer) => {
                    ctx.bind_buffer(target, buffer)?;
                    Reply::Void
                }
                Self::BufferData(target, bytes) => {
                    ctx.buffer_data(target, &bytes)?;
                    Reply::Void
                }
                Self::BufferSize(target, size) => {
                    ctx.buffer_data_size(target, size)?;
                    Reply::Void
                }
                Self::BufferSubData(target, offset, bytes) => {
                    ctx.buffer_sub_data(target, offset, &bytes)?;
                    Reply::Void
                }
                Self::DeleteBuffer(id) => {
                    ctx.delete_buffer(id)?;
                    Reply::Void
                }
                Self::AttribPointer(index, size, ty, normalize, stride, offset) => {
                    ctx.vertex_attrib_pointer(index, size, ty, normalize, stride, offset)?;
                    Reply::Void
                }
                Self::EnableAttrib(index, enabled) => {
                    ctx.enable_vertex_attrib_array(index, enabled)?;
                    Reply::Void
                }
                Self::Attrib(index, value) => {
                    ctx.vertex_attrib(index, value)?;
                    Reply::Void
                }
                Self::Viewport(x, y, w, h) => {
                    ctx.viewport(x, y, w, h)?;
                    Reply::Void
                }
                Self::Scissor(x, y, w, h) => {
                    ctx.scissor(x, y, w, h)?;
                    Reply::Void
                }
                Self::EnableScissor(enabled) => {
                    ctx.enable_scissor(enabled);
                    Reply::Void
                }
                Self::EnableDepth(enabled) => {
                    ctx.enable_depth_test(enabled);
                    Reply::Void
                }
                Self::EnableBlend(enabled) => {
                    ctx.enable_blend(enabled);
                    Reply::Void
                }
                Self::BlendFunc(sr, dr, sa, da) => {
                    ctx.blend_func_separate(sr, dr, sa, da)?;
                    Reply::Void
                }
                Self::BlendEquation(rgb, alpha) => {
                    ctx.blend_equation_separate(rgb, alpha)?;
                    Reply::Void
                }
                Self::BlendColor(color) => {
                    ctx.blend_color(color);
                    Reply::Void
                }
                Self::DepthMask(enabled) => {
                    ctx.depth_mask(enabled);
                    Reply::Void
                }
                Self::DepthFunc(func) => {
                    ctx.depth_func(func)?;
                    Reply::Void
                }
                Self::ColorMask(mask) => {
                    ctx.color_mask(mask);
                    Reply::Void
                }
                Self::ClearColor(color) => {
                    ctx.clear_color(color);
                    Reply::Void
                }
                Self::ClearDepth(value) => {
                    ctx.clear_depth(value);
                    Reply::Void
                }
                Self::Clear(mask) => {
                    ctx.clear(mask)?;
                    Reply::Void
                }
                Self::AttribDivisor(index, divisor) => {
                    ctx.vertex_attrib_divisor(index, divisor)?;
                    Reply::Void
                }
                Self::DrawArraysInstanced(mode, first, count, instances) => {
                    ctx.draw_arrays_instanced(mode, first, count, instances)?;
                    Reply::Void
                }
                Self::DrawElementsInstanced(mode, count, kind, offset, instances) => {
                    ctx.draw_elements_instanced(mode, count, kind, offset, instances)?;
                    Reply::Void
                }
                Self::DrawArrays(mode, first, count) => {
                    ctx.draw_arrays(mode, first, count)?;
                    Reply::Void
                }
                Self::DrawElements(mode, count, ty, offset) => {
                    ctx.draw_elements(mode, count, ty, offset)?;
                    Reply::Void
                }
                Self::ReadPixels(x, y, w, h) => {
                    let mut bytes = ctx.read_pixels(x, y, w, h)?;
                    if !record.alpha && ctx.reads_default_framebuffer() {
                        let (bw, bh) = ctx.dimensions();
                        for (i, pixel) in bytes.chunks_exact_mut(4).enumerate() {
                            let px = x as i64 + (i % w.max(1) as usize) as i64;
                            let py = y as i64 + (i / w.max(1) as usize) as i64;
                            if px >= 0 && py >= 0 && px < bw as i64 && py < bh as i64 {
                                pixel[3] = 255;
                            }
                        }
                    }
                    Reply::Bytes(bytes)
                }
                Self::BlitFramebuffer(source, destination, mask, filter) => {
                    ctx.blit_framebuffer(source, destination, mask, filter)?;
                    Reply::Void
                }
                Self::Width => Reply::Number(ctx.dimensions().0 as f64),
                Self::Height => Reply::Number(ctx.dimensions().1 as f64),
                Self::ReadWidth => Reply::Number(ctx.read_dimensions()?.0 as f64),
                Self::ReadHeight => Reply::Number(ctx.read_dimensions()?.1 as f64),
                Self::CreateFramebuffer => Reply::Number(ctx.create_framebuffer() as f64),
                Self::BindFramebuffer(target, id) => {
                    ctx.bind_framebuffer(target, id)?;
                    Reply::Void
                }
                Self::DeleteFramebuffer(id) => {
                    ctx.delete_framebuffer(id)?;
                    Reply::Void
                }
                Self::CheckFramebuffer(target) => {
                    Reply::Number(ctx.check_framebuffer_status(target)? as f64)
                }
                Self::FramebufferTexture(target, attachment, textarget, id, level) => {
                    ctx.framebuffer_texture_2d(target, attachment, textarget, id, level)?;
                    Reply::Void
                }
                Self::FramebufferRenderbuffer(target, attachment, rbtarget, id) => {
                    ctx.framebuffer_renderbuffer(target, attachment, rbtarget, id)?;
                    Reply::Void
                }
                Self::FramebufferAttachment(target, attachment, pname) => Reply::Number(
                    ctx.framebuffer_attachment_parameter(target, attachment, pname)? as f64,
                ),
                Self::FramebufferBits(pname) => Reply::Number(ctx.framebuffer_bits(pname)? as f64),
                Self::CreateRenderbuffer => Reply::Number(ctx.create_renderbuffer() as f64),
                Self::BindRenderbuffer(target, id) => {
                    ctx.bind_renderbuffer(target, id)?;
                    Reply::Void
                }
                Self::DeleteRenderbuffer(id) => {
                    ctx.delete_renderbuffer(id)?;
                    Reply::Void
                }
                Self::RenderbufferStorage(target, format, width, height) => {
                    ctx.renderbuffer_storage(target, format, width, height)?;
                    Reply::Void
                }
                Self::RenderbufferParameter(target, pname) => {
                    Reply::Number(ctx.renderbuffer_parameter(target, pname)? as f64)
                }
                Self::ActiveAttribs(p) => Reply::Infos(ctx.active_attributes(p)?),
                Self::ActiveUniforms(p) => Reply::Infos(ctx.active_uniforms(p)?),
                Self::GetUniform(p, location) => {
                    let values = ctx.get_uniform(p, location)?;
                    if ctx.get_uniform_kind(p, location)? == rabbithole_webgl::gl::BOOL {
                        Reply::Bool(values[0] != 0.)
                    } else { Reply::Values(values) }
                }
                Self::UniformI(location, values) => {
                    ctx.uniform_i32(location, &values)?;
                    Reply::Void
                }
                Self::CreateTexture => Reply::Number(ctx.create_texture() as f64),
                Self::ActiveTexture(unit) => {
                    ctx.active_texture(unit)?;
                    Reply::Void
                }
                Self::BindTexture(target, id) => {
                    ctx.bind_texture(target, id)?;
                    Reply::Void
                }
                Self::DeleteTexture(id) => {
                    ctx.delete_texture(id)?;
                    Reply::Void
                }
                Self::TexParameter(target, name, value) => {
                    ctx.tex_parameter_i(target, name, value)?;
                    Reply::Void
                }
                Self::GetTexParameter(target, name) => {
                    Reply::Number(ctx.texture_parameter(target, name)? as f64)
                }
                Self::TexImage(target, level, internal, w, h, border, format, ty, data) => {
                    ctx.tex_image_2d(
                        target,
                        level,
                        internal,
                        w,
                        h,
                        border,
                        format,
                        ty,
                        data.as_deref(),
                    )?;
                    Reply::Void
                }
                Self::TexSubImage(target, level, x, y, w, h, format, ty, data) => {
                    ctx.tex_sub_image_2d(target, level, x, y, w, h, format, ty, &data)?;
                    Reply::Void
                }
            })
        })();
        result.map_err(|e| e.code())
    }
}

fn command(realm: &mut Realm, _: Value, args: &[Value]) -> JsResult<Value> {
    let id = number(realm, args, 0)? as u32;
    let op = arg_str(realm, args, 1)?;
    let Some(command) = Command::parse(realm, &op, args)? else {
        return Ok(response(realm, Err(INVALID_OPERATION)));
    };
    let draws = command.draws();
    let result = canvas2d::store(|store| {
        let record = store.webgl.contexts.get_mut(&id).ok_or(INVALID_OPERATION)?;
        let result = command.execute(record);
        if draws
            && result.is_ok()
            && record
                .ctx
                .framebuffer_binding(rabbithole_webgl::gl::FRAMEBUFFER)
                .unwrap_or(0)
                == 0
        {
            if let Some(surface) = store.surfaces.get_mut(&id) {
                record.sync(surface);
            }
        }
        result
    });
    Ok(response(realm, result))
}

pub(crate) fn install(realm: &mut Realm, target: ObjectId) {
    realm.define_method(target, "webglLimits", 0, limits);
    realm.define_method(target, "webglNew", 7, create);
    realm.define_method(target, "webglResize", 3, resize);
    realm.define_method(target, "webglCall", 2, command);
    realm.define_method(target, "webglLose", 1, lose);
    realm.define_method(target, "webglDiscard", 1, discard);
}
