//! Experimental, dependency-free software WebGL rendering. See README.md
//! for the supported GLSL subset and explicit implementation limits.
pub mod gl;
mod raster;
mod shader;
mod texture;
use shader::{Qual, Shader, Ty, Value};
use std::collections::BTreeMap;
pub use texture::MAX_TEXTURE_UNITS;

pub const MAX_DIMENSION: u32 = 4096;
pub const MAX_PIXELS: usize = 4_194_304;
pub const MAX_VERTEX_ATTRIBS: u32 = 16;
const MAX_BUFFER_BYTES: usize = 64 * 1024 * 1024;
const MAX_DRAW_VERTICES: u32 = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidEnum,
    InvalidValue,
    InvalidOperation,
    OutOfMemory,
}
impl Error {
    pub fn code(self) -> u32 {
        match self {
            Self::InvalidEnum => gl::INVALID_ENUM,
            Self::InvalidValue => gl::INVALID_VALUE,
            Self::InvalidOperation => gl::INVALID_OPERATION,
            Self::OutOfMemory => gl::OUT_OF_MEMORY,
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug)]
struct ShaderObject {
    kind: u32,
    source: String,
    compiled: Option<Shader>,
    log: String,
    deleted: bool,
}
#[derive(Clone, Debug, Default)]
struct Program {
    attached: Vec<u32>,
    executable: Option<Executable>,
    log: String,
    deleted: bool,
    bindings: BTreeMap<String, u32>,
}
#[derive(Clone, Debug)]
struct Uniform {
    name: String,
    ty: Ty,
    value: Value,
    location: i32,
}
#[derive(Clone, Debug)]
struct Executable {
    vertex: Shader,
    fragment: Shader,
    attributes: BTreeMap<String, u32>,
    varyings: Vec<(String, Ty)>,
    uniforms: Vec<Uniform>,
}
#[derive(Debug, Default)]
struct Buffer {
    target: Option<u32>,
    bytes: Vec<u8>,
}
#[derive(Clone, Copy, Debug)]
struct Attribute {
    enabled: bool,
    buffer: u32,
    size: u32,
    kind: u32,
    normalized: bool,
    stride: u32,
    offset: usize,
    constant: [f32; 4],
}
impl Default for Attribute {
    fn default() -> Self {
        Self {
            enabled: false,
            buffer: 0,
            size: 4,
            kind: gl::FLOAT,
            normalized: false,
            stride: 0,
            offset: 0,
            constant: [0., 0., 0., 1.],
        }
    }
}

/// Reflection of a linked, active shader input or uniform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveInfo {
    pub name: String,
    pub size: i32,
    pub kind: u32,
}
fn active_info(name: &str, ty: Ty) -> ActiveInfo {
    ActiveInfo {
        name: name.into(),
        size: 1,
        kind: match ty {
            Ty::IntLiteral => unreachable!("literal types are not interface variables"),
            Ty::Sampler2D => gl::SAMPLER_2D,
            Ty::Float => gl::FLOAT,
            Ty::Vec2 => gl::FLOAT_VEC2,
            Ty::Vec3 => gl::FLOAT_VEC3,
            Ty::Vec4 => gl::FLOAT_VEC4,
            Ty::Mat4 => gl::FLOAT_MAT4,
        },
    }
}

/// A context owns its resources; handles are never reused. Pixels for
/// presentation are RGBA bytes in top-left order. read_pixels uses GL's
/// lower-left origin and returns rows from bottom to top.
#[derive(Debug)]
pub struct Context {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    depth: Vec<f32>,
    viewport: [i64; 4],
    scissor: [i64; 4],
    scissor_enabled: bool,
    clear_color: [f32; 4],
    clear_depth: f32,
    color_mask: [bool; 4],
    depth_mask: bool,
    depth_test: bool,
    depth_func: u32,
    next_id: u32,
    next_location: i32,
    shaders: BTreeMap<u32, ShaderObject>,
    programs: BTreeMap<u32, Program>,
    buffers: BTreeMap<u32, Buffer>,
    array_buffer: u32,
    element_buffer: u32,
    current_program: u32,
    attributes: [Attribute; MAX_VERTEX_ATTRIBS as usize],
    buffer_bytes: usize,
    textures: BTreeMap<u32, texture::Texture>,
    bound_textures: [u32; MAX_TEXTURE_UNITS as usize],
    active_texture_unit: usize,
    texture_bytes: usize,
}
fn filled<T: Clone>(n: usize, v: T) -> Result<Vec<T>> {
    let mut out = Vec::new();
    out.try_reserve_exact(n).map_err(|_| Error::OutOfMemory)?;
    out.resize(n, v);
    Ok(out)
}
fn dimensions(w: u32, h: u32) -> Result<usize> {
    let n = (w as usize)
        .checked_mul(h as usize)
        .ok_or(Error::OutOfMemory)?;
    if w > MAX_DIMENSION || h > MAX_DIMENSION || n > MAX_PIXELS {
        return Err(Error::OutOfMemory);
    }
    Ok(n)
}
fn clamp(v: f32) -> f32 {
    if v.is_nan() {
        0.
    } else {
        v.clamp(0., 1.)
    }
}
fn byte(v: f32) -> u8 {
    (clamp(v) * 255.).round() as u8
}
impl Context {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let n = dimensions(width, height)?;
        Ok(Self {
            width,
            height,
            rgba: filled(n * 4, 0)?,
            depth: filled(n, 1.)?,
            viewport: [0, 0, width as i64, height as i64],
            scissor: [0, 0, width as i64, height as i64],
            scissor_enabled: false,
            clear_color: [0.; 4],
            clear_depth: 1.,
            color_mask: [true; 4],
            depth_mask: true,
            depth_test: false,
            depth_func: gl::LESS,
            next_id: 1,
            next_location: 0,
            shaders: BTreeMap::new(),
            programs: BTreeMap::new(),
            buffers: BTreeMap::new(),
            array_buffer: 0,
            element_buffer: 0,
            current_program: 0,
            attributes: [Attribute::default(); MAX_VERTEX_ATTRIBS as usize],
            buffer_bytes: 0,
            textures: BTreeMap::new(),
            bound_textures: [0; MAX_TEXTURE_UNITS as usize],
            active_texture_unit: 0,
            texture_bytes: 0,
        })
    }
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    pub fn framebuffer_rgba(&self) -> &[u8] {
        &self.rgba
    }
    /// Clear the replacement drawing buffer, preserving GL state and resources.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let n = dimensions(width, height)?;
        let rgba = filled(n * 4, 0)?;
        let depth = filled(n, 1.)?;
        self.rgba = rgba;
        self.depth = depth;
        self.width = width;
        self.height = height;
        Ok(())
    }
    pub fn viewport(&mut self, x: i32, y: i32, w: u32, h: u32) -> Result<()> {
        if w > i32::MAX as u32 || h > i32::MAX as u32 {
            return Err(Error::InvalidValue);
        }
        self.viewport = [
            x as i64,
            y as i64,
            w.min(MAX_DIMENSION) as i64,
            h.min(MAX_DIMENSION) as i64,
        ];
        Ok(())
    }
    pub fn scissor(&mut self, x: i32, y: i32, w: u32, h: u32) -> Result<()> {
        if w > i32::MAX as u32 || h > i32::MAX as u32 {
            return Err(Error::InvalidValue);
        }
        self.scissor = [x as i64, y as i64, w as i64, h as i64];
        Ok(())
    }
    pub fn viewport_rect(&self) -> [i32; 4] {
        self.viewport.map(|v| v as i32)
    }
    pub fn scissor_rect(&self) -> [i32; 4] {
        self.scissor.map(|v| v as i32)
    }
    pub fn enable_scissor(&mut self, on: bool) {
        self.scissor_enabled = on;
    }
    pub fn clear_color(&mut self, color: [f32; 4]) {
        self.clear_color = color.map(clamp);
    }
    pub fn clear_depth(&mut self, v: f32) {
        self.clear_depth = clamp(v);
    }
    pub fn color_mask(&mut self, v: [bool; 4]) {
        self.color_mask = v;
    }
    pub fn depth_mask(&mut self, on: bool) {
        self.depth_mask = on;
    }
    pub fn enable_depth_test(&mut self, on: bool) {
        self.depth_test = on;
    }
    pub fn depth_func(&mut self, func: u32) -> Result<()> {
        if !(gl::NEVER..=gl::ALWAYS).contains(&func) {
            return Err(Error::InvalidEnum);
        }
        self.depth_func = func;
        Ok(())
    }
    pub fn clear(&mut self, mask: u32) -> Result<()> {
        if mask & !(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT | gl::STENCIL_BUFFER_BIT) != 0 {
            return Err(Error::InvalidValue);
        }
        let col = self.clear_color.map(byte);
        let r = self.clip_rect(false);
        for y in r[1]..r[3] {
            for x in r[0]..r[2] {
                let i = ((self.height as i64 - 1 - y) * self.width as i64 + x) as usize;
                if mask & gl::COLOR_BUFFER_BIT != 0 {
                    for (k, &v) in col.iter().enumerate() {
                        if self.color_mask[k] {
                            self.rgba[i * 4 + k] = v;
                        }
                    }
                }
                if mask & gl::DEPTH_BUFFER_BIT != 0 && self.depth_mask {
                    self.depth[i] = self.clear_depth;
                }
            }
        }
        Ok(())
    }
    pub fn read_pixels(&self, x: i32, y: i32, w: u32, h: u32) -> Result<Vec<u8>> {
        let n = dimensions(w, h)?;
        let mut out = filled(n * 4, 0)?;
        for j in 0..h as i64 {
            for i in 0..w as i64 {
                let (px, py) = (x as i64 + i, y as i64 + j);
                if px >= 0 && py >= 0 && px < self.width as i64 && py < self.height as i64 {
                    let src = ((self.height as i64 - 1 - py) * self.width as i64 + px) as usize * 4;
                    let dst = (j * w as i64 + i) as usize * 4;
                    out[dst..dst + 4].copy_from_slice(&self.rgba[src..src + 4]);
                }
            }
        }
        Ok(out)
    }
    fn clip_rect(&self, viewport: bool) -> [i64; 4] {
        let mut r = [0, 0, self.width as i64, self.height as i64];
        if viewport {
            let v = self.viewport;
            r = [
                r[0].max(v[0]),
                r[1].max(v[1]),
                r[2].min(v[0] + v[2]),
                r[3].min(v[1] + v[3]),
            ];
        }
        if self.scissor_enabled {
            let s = self.scissor;
            r = [
                r[0].max(s[0]),
                r[1].max(s[1]),
                r[2].min(s[0] + s[2]),
                r[3].min(s[1] + s[3]),
            ];
        }
        if r[2] < r[0] {
            r[2] = r[0];
        }
        if r[3] < r[1] {
            r[3] = r[1];
        }
        r
    }
    fn id(&mut self) -> u32 {
        if self.next_id == 0 {
            return 0;
        }
        let i = self.next_id;
        self.next_id = i.checked_add(1).unwrap_or(0);
        i
    }
    pub fn create_shader(&mut self, kind: u32) -> Result<u32> {
        if !matches!(kind, gl::VERTEX_SHADER | gl::FRAGMENT_SHADER) {
            return Err(Error::InvalidEnum);
        }
        if self.shaders.len() >= 1024 {
            return Err(Error::OutOfMemory);
        }
        let id = self.id();
        if id == 0 {
            return Err(Error::OutOfMemory);
        }
        self.shaders.insert(
            id,
            ShaderObject {
                kind,
                source: String::new(),
                compiled: None,
                log: String::new(),
                deleted: false,
            },
        );
        Ok(id)
    }
    pub fn shader_source(&mut self, id: u32, source: &str) -> Result<()> {
        if source.len() > 65536 {
            return Err(Error::OutOfMemory);
        }
        let s = self.shaders.get_mut(&id).ok_or(Error::InvalidValue)?;
        s.source = source.into();
        Ok(())
    }
    pub fn compile_shader(&mut self, id: u32) -> Result<()> {
        let s = self.shaders.get_mut(&id).ok_or(Error::InvalidValue)?;
        match shader::compile(&s.source, s.kind == gl::VERTEX_SHADER) {
            Ok(c) => {
                s.compiled = Some(c);
                s.log.clear();
            }
            Err(e) => {
                s.compiled = None;
                s.log = e;
            }
        }
        Ok(())
    }
    pub fn shader_compile_status(&self, id: u32) -> Result<bool> {
        Ok(self
            .shaders
            .get(&id)
            .ok_or(Error::InvalidValue)?
            .compiled
            .is_some())
    }
    pub fn shader_info_log(&self, id: u32) -> Result<&str> {
        Ok(&self.shaders.get(&id).ok_or(Error::InvalidValue)?.log)
    }
    pub fn shader_source_text(&self, id: u32) -> Result<&str> {
        Ok(&self.shaders.get(&id).ok_or(Error::InvalidValue)?.source)
    }
    pub fn delete_shader(&mut self, id: u32) -> Result<()> {
        if id == 0 {
            return Ok(());
        }
        self.shaders
            .get_mut(&id)
            .ok_or(Error::InvalidValue)?
            .deleted = true;
        self.collect_shaders();
        Ok(())
    }
    fn collect_shaders(&mut self) {
        self.shaders
            .retain(|id, s| !s.deleted || self.programs.values().any(|p| p.attached.contains(id)));
    }
    pub fn create_program(&mut self) -> u32 {
        if self.programs.len() >= 1024 {
            return 0;
        }
        let id = self.id();
        if id != 0 {
            self.programs.insert(id, Program::default());
        }
        id
    }
    pub fn attach_shader(&mut self, program: u32, shader: u32) -> Result<()> {
        if !self.shaders.contains_key(&shader) {
            return Err(Error::InvalidValue);
        }
        let p = self.programs.get_mut(&program).ok_or(Error::InvalidValue)?;
        if p.attached.contains(&shader) {
            return Err(Error::InvalidOperation);
        }
        p.attached.push(shader);
        Ok(())
    }
    pub fn bind_attrib_location(&mut self, program: u32, index: u32, name: &str) -> Result<()> {
        if index >= MAX_VERTEX_ATTRIBS {
            return Err(Error::InvalidValue);
        }
        if name.starts_with("gl_") {
            return Err(Error::InvalidOperation);
        }
        self.programs
            .get_mut(&program)
            .ok_or(Error::InvalidValue)?
            .bindings
            .insert(name.into(), index);
        Ok(())
    }
    pub fn link_program(&mut self, id: u32) -> Result<()> {
        let p = self.programs.get(&id).ok_or(Error::InvalidValue)?.clone();
        let result = self.link(&p);
        let p = self.programs.get_mut(&id).unwrap();
        match result {
            Ok(e) => {
                p.executable = Some(e);
                p.log.clear();
            }
            Err(e) => {
                p.executable = None;
                p.log = e;
            }
        }
        Ok(())
    }
    fn link(&mut self, p: &Program) -> std::result::Result<Executable, String> {
        let mut vertex = None;
        let mut fragment = None;
        for id in &p.attached {
            let s = self
                .shaders
                .get(id)
                .ok_or("attached shader no longer exists")?;
            let c = s
                .compiled
                .clone()
                .ok_or("attached shader did not compile")?;
            if s.kind == gl::VERTEX_SHADER {
                if vertex.replace(c).is_some() {
                    return Err("one vertex shader is required".into());
                }
            } else if fragment.replace(c).is_some() {
                return Err("one fragment shader is required".into());
            }
        }
        let vertex = vertex.ok_or("missing vertex shader")?;
        let fragment = fragment.ok_or("missing fragment shader")?;
        let mut attributes = BTreeMap::new();
        let mut used = Vec::new();
        for (name, s) in &vertex.symbols {
            if s.qual == Qual::Attribute && vertex.reads.contains(name) {
                if let Some(&location) = p.bindings.get(name) {
                    if used.contains(&location) {
                        return Err("aliased attribute locations are unsupported".into());
                    }
                    used.push(location);
                    attributes.insert(name.clone(), location);
                }
            }
        }
        for (name, s) in &vertex.symbols {
            if s.qual == Qual::Attribute
                && vertex.reads.contains(name)
                && !attributes.contains_key(name)
            {
                let i = (0..MAX_VERTEX_ATTRIBS)
                    .find(|i| !used.contains(i))
                    .ok_or("too many active attributes")?;
                used.push(i);
                attributes.insert(name.clone(), i);
            }
        }
        let mut varyings = Vec::new();
        for (name, s) in &fragment.symbols {
            if s.qual == Qual::Varying && fragment.reads.contains(name) {
                let v = vertex
                    .symbols
                    .get(name)
                    .ok_or_else(|| format!("missing vertex varying {name}"))?;
                if v.qual != Qual::Varying || v.ty != s.ty {
                    return Err(format!("varying type mismatch for {name}"));
                }
                varyings.push((name.clone(), s.ty));
            }
        }
        if varyings.len() > 8 {
            return Err("too many varyings".into());
        }
        for (name, s) in &vertex.symbols {
            if s.qual == Qual::Uniform {
                if let Some(f) = fragment.symbols.get(name) {
                    if f.qual == Qual::Uniform && f.ty != s.ty {
                        return Err(format!("uniform type mismatch for {name}"));
                    }
                }
            }
        }
        let mut uniforms = Vec::<Uniform>::new();
        for sh in [&vertex, &fragment] {
            for (name, s) in &sh.symbols {
                if s.qual == Qual::Uniform && sh.reads.contains(name) {
                    if let Some(u) = uniforms.iter().find(|u| u.name == *name) {
                        if u.ty != s.ty {
                            return Err(format!("uniform type mismatch for {name}"));
                        }
                    } else {
                        if uniforms.len() >= 32 {
                            return Err("too many uniforms".into());
                        }
                        let loc = self.next_location;
                        self.next_location = loc
                            .checked_add(1)
                            .ok_or("uniform location space exhausted")?;
                        uniforms.push(Uniform {
                            name: name.clone(),
                            ty: s.ty,
                            value: Value::zero(s.ty),
                            location: loc,
                        });
                    }
                }
            }
        }
        Ok(Executable {
            vertex,
            fragment,
            attributes,
            varyings,
            uniforms,
        })
    }
    pub fn program_link_status(&self, id: u32) -> Result<bool> {
        Ok(self
            .programs
            .get(&id)
            .ok_or(Error::InvalidValue)?
            .executable
            .is_some())
    }
    pub fn program_info_log(&self, id: u32) -> Result<&str> {
        Ok(&self.programs.get(&id).ok_or(Error::InvalidValue)?.log)
    }
    pub fn use_program(&mut self, id: u32) -> Result<()> {
        if id != 0
            && self
                .programs
                .get(&id)
                .and_then(|p| p.executable.as_ref())
                .is_none()
        {
            return Err(Error::InvalidOperation);
        }
        let old = self.current_program;
        self.current_program = id;
        if old != id && self.programs.get(&old).is_some_and(|p| p.deleted) {
            self.programs.remove(&old);
            self.collect_shaders();
        }
        Ok(())
    }
    pub fn delete_program(&mut self, id: u32) -> Result<()> {
        if id == 0 {
            return Ok(());
        }
        let p = self.programs.get_mut(&id).ok_or(Error::InvalidValue)?;
        if self.current_program == id {
            p.deleted = true;
        } else {
            self.programs.remove(&id);
            self.collect_shaders();
        }
        Ok(())
    }
    pub fn get_attrib_location(&self, id: u32, name: &str) -> Result<i32> {
        let e = self
            .programs
            .get(&id)
            .and_then(|p| p.executable.as_ref())
            .ok_or(Error::InvalidOperation)?;
        Ok(e.attributes.get(name).map_or(-1, |i| *i as i32))
    }
    pub fn get_uniform_location(&self, id: u32, name: &str) -> Result<Option<i32>> {
        let e = self
            .programs
            .get(&id)
            .and_then(|p| p.executable.as_ref())
            .ok_or(Error::InvalidOperation)?;
        Ok(e.uniforms
            .iter()
            .find(|u| u.name == name)
            .map(|u| u.location))
    }
    pub fn active_attributes(&self, id: u32) -> Result<Vec<ActiveInfo>> {
        let e = self
            .programs
            .get(&id)
            .and_then(|p| p.executable.as_ref())
            .ok_or(Error::InvalidOperation)?;
        Ok(e.attributes
            .keys()
            .map(|name| active_info(name, e.vertex.symbols[name].ty))
            .collect())
    }
    pub fn active_uniforms(&self, id: u32) -> Result<Vec<ActiveInfo>> {
        let e = self
            .programs
            .get(&id)
            .and_then(|p| p.executable.as_ref())
            .ok_or(Error::InvalidOperation)?;
        Ok(e.uniforms
            .iter()
            .map(|u| active_info(&u.name, u.ty))
            .collect())
    }
    pub fn get_uniform(&self, id: u32, location: i32) -> Result<Vec<f32>> {
        let e = self
            .programs
            .get(&id)
            .and_then(|p| p.executable.as_ref())
            .ok_or(Error::InvalidOperation)?;
        let u = e
            .uniforms
            .iter()
            .find(|u| u.location == location)
            .ok_or(Error::InvalidOperation)?;
        Ok(u.value.v[..u.ty.n()].to_vec())
    }
    pub fn uniform_f32(&mut self, location: i32, values: &[f32]) -> Result<()> {
        if location < 0 {
            return Ok(());
        }
        let u = self.uniform_mut(location)?;
        if matches!(u.ty, Ty::Mat4 | Ty::Sampler2D) || u.ty.n() != values.len() {
            return Err(Error::InvalidOperation);
        }
        u.value = Value::from(u.ty, values);
        Ok(())
    }
    pub fn uniform_matrix4fv(
        &mut self,
        location: i32,
        transpose: bool,
        values: &[f32],
    ) -> Result<()> {
        if transpose {
            return Err(Error::InvalidValue);
        }
        if location < 0 {
            return Ok(());
        }
        let u = self.uniform_mut(location)?;
        if u.ty != Ty::Mat4 || values.len() != 16 {
            return Err(Error::InvalidOperation);
        }
        u.value = Value::from(u.ty, values);
        Ok(())
    }
    fn uniform_mut(&mut self, location: i32) -> Result<&mut Uniform> {
        self.programs
            .get_mut(&self.current_program)
            .and_then(|p| p.executable.as_mut())
            .and_then(|e| e.uniforms.iter_mut().find(|u| u.location == location))
            .ok_or(Error::InvalidOperation)
    }
    pub fn create_buffer(&mut self) -> u32 {
        if self.buffers.len() >= 4096 {
            return 0;
        }
        let id = self.id();
        if id != 0 {
            self.buffers.insert(id, Buffer::default());
        }
        id
    }
    pub fn bind_buffer(&mut self, target: u32, id: u32) -> Result<()> {
        if !matches!(target, gl::ARRAY_BUFFER | gl::ELEMENT_ARRAY_BUFFER) {
            return Err(Error::InvalidEnum);
        }
        if id != 0 {
            let b = self.buffers.get_mut(&id).ok_or(Error::InvalidOperation)?;
            if b.target.is_some_and(|t| t != target) {
                return Err(Error::InvalidOperation);
            }
            b.target = Some(target);
        }
        if target == gl::ARRAY_BUFFER {
            self.array_buffer = id;
        } else {
            self.element_buffer = id;
        }
        Ok(())
    }
    fn bound_buffer(&self, target: u32) -> Result<u32> {
        let id = match target {
            gl::ARRAY_BUFFER => self.array_buffer,
            gl::ELEMENT_ARRAY_BUFFER => self.element_buffer,
            _ => return Err(Error::InvalidEnum),
        };
        if id == 0 {
            return Err(Error::InvalidOperation);
        }
        Ok(id)
    }
    pub fn buffer_data(&mut self, target: u32, bytes: &[u8]) -> Result<()> {
        self.set_buffer_data(target, bytes.len(), Some(bytes))
    }
    pub fn buffer_data_size(&mut self, target: u32, len: usize) -> Result<()> {
        self.set_buffer_data(target, len, None)
    }
    fn set_buffer_data(&mut self, target: u32, len: usize, bytes: Option<&[u8]>) -> Result<()> {
        let id = self.bound_buffer(target)?;
        let old = self.buffers[&id].bytes.len();
        if len > MAX_BUFFER_BYTES || self.buffer_bytes - old > MAX_BUFFER_BYTES - len {
            return Err(Error::OutOfMemory);
        }
        let mut data = filled(len, 0)?;
        if let Some(bytes) = bytes {
            data.copy_from_slice(bytes);
        }
        self.buffers.get_mut(&id).unwrap().bytes = data;
        self.buffer_bytes = self.buffer_bytes - old + len;
        Ok(())
    }
    pub fn buffer_sub_data(&mut self, target: u32, offset: usize, bytes: &[u8]) -> Result<()> {
        let id = self.bound_buffer(target)?;
        let b = &mut self.buffers.get_mut(&id).unwrap().bytes;
        let end = offset.checked_add(bytes.len()).ok_or(Error::InvalidValue)?;
        let dst = b.get_mut(offset..end).ok_or(Error::InvalidValue)?;
        dst.copy_from_slice(bytes);
        Ok(())
    }
    pub fn delete_buffer(&mut self, id: u32) -> Result<()> {
        if id == 0 {
            return Ok(());
        }
        let b = self.buffers.remove(&id).ok_or(Error::InvalidValue)?;
        self.buffer_bytes -= b.bytes.len();
        if self.array_buffer == id {
            self.array_buffer = 0;
        }
        if self.element_buffer == id {
            self.element_buffer = 0;
        }
        for a in &mut self.attributes {
            if a.buffer == id {
                a.buffer = 0;
            }
        }
        Ok(())
    }
    pub fn vertex_attrib_pointer(
        &mut self,
        index: u32,
        size: u32,
        kind: u32,
        normalized: bool,
        stride: u32,
        offset: usize,
    ) -> Result<()> {
        let bytes = scalar_size(kind)?;
        if index >= MAX_VERTEX_ATTRIBS
            || !(1..=4).contains(&size)
            || stride > 255
            || stride as usize % bytes != 0
            || offset % bytes != 0
        {
            return Err(Error::InvalidValue);
        }
        if self.array_buffer == 0 {
            return Err(Error::InvalidOperation);
        }
        let a = &mut self.attributes[index as usize];
        a.buffer = self.array_buffer;
        a.size = size;
        a.kind = kind;
        a.normalized = normalized;
        a.stride = stride;
        a.offset = offset;
        Ok(())
    }
    pub fn enable_vertex_attrib_array(&mut self, index: u32, enabled: bool) -> Result<()> {
        self.attributes
            .get_mut(index as usize)
            .ok_or(Error::InvalidValue)?
            .enabled = enabled;
        Ok(())
    }
    pub fn vertex_attrib(&mut self, index: u32, value: [f32; 4]) -> Result<()> {
        self.attributes
            .get_mut(index as usize)
            .ok_or(Error::InvalidValue)?
            .constant = value;
        Ok(())
    }
    pub fn draw_arrays(&mut self, mode: u32, first: u32, count: u32) -> Result<()> {
        validate_mode(mode)?;
        if count > MAX_DRAW_VERTICES {
            return Err(Error::OutOfMemory);
        }
        let end = first.checked_add(count).ok_or(Error::InvalidValue)?;
        let indices: Vec<u32> = (first..end).collect();
        self.draw(mode, &indices)
    }
    pub fn draw_elements(&mut self, mode: u32, count: u32, kind: u32, offset: usize) -> Result<()> {
        validate_mode(mode)?;
        if count > MAX_DRAW_VERTICES {
            return Err(Error::OutOfMemory);
        }
        let size = match kind {
            gl::UNSIGNED_BYTE => 1,
            gl::UNSIGNED_SHORT => 2,
            _ => return Err(Error::InvalidEnum),
        };
        if offset % size != 0 {
            return Err(Error::InvalidOperation);
        }
        let id = self.bound_buffer(gl::ELEMENT_ARRAY_BUFFER)?;
        let b = &self.buffers[&id].bytes;
        let end = offset
            .checked_add(count as usize * size)
            .ok_or(Error::InvalidOperation)?;
        let src = b.get(offset..end).ok_or(Error::InvalidOperation)?;
        let indices: Vec<u32> = if size == 1 {
            src.iter().map(|v| *v as u32).collect()
        } else {
            src.chunks_exact(2)
                .map(|v| u16::from_le_bytes([v[0], v[1]]) as u32)
                .collect()
        };
        self.draw(mode, &indices)
    }
    fn validate_arrays(&self, max: Option<u32>) -> Result<()> {
        for a in &self.attributes {
            if a.enabled {
                let b = self.buffers.get(&a.buffer).ok_or(Error::InvalidOperation)?;
                if let Some(max) = max {
                    let unit = scalar_size(a.kind)?;
                    let stride = if a.stride == 0 {
                        a.size as usize * unit
                    } else {
                        a.stride as usize
                    };
                    let end = (max as usize)
                        .checked_mul(stride)
                        .and_then(|v| v.checked_add(a.offset))
                        .and_then(|v| v.checked_add(a.size as usize * unit))
                        .ok_or(Error::InvalidOperation)?;
                    if end > b.bytes.len() {
                        return Err(Error::InvalidOperation);
                    }
                }
            }
        }
        Ok(())
    }
    fn attribute_value(&self, index: u32, vertex: u32, ty: Ty) -> Value {
        let a = self.attributes[index as usize];
        let mut v = a.constant;
        if a.enabled {
            v = [0., 0., 0., 1.];
            let unit = scalar_size(a.kind).unwrap();
            let stride = if a.stride == 0 {
                a.size as usize * unit
            } else {
                a.stride as usize
            };
            let offset = a.offset + vertex as usize * stride;
            let b = &self.buffers[&a.buffer].bytes;
            for i in 0..a.size as usize {
                let p = offset + i * unit;
                v[i] = match a.kind {
                    gl::FLOAT => f32::from_le_bytes(b[p..p + 4].try_into().unwrap()),
                    gl::UNSIGNED_BYTE => {
                        if a.normalized {
                            b[p] as f32 / 255.
                        } else {
                            b[p] as f32
                        }
                    }
                    gl::BYTE => {
                        let n = b[p] as i8;
                        if a.normalized {
                            (n as f32 / 127.).max(-1.)
                        } else {
                            n as f32
                        }
                    }
                    gl::UNSIGNED_SHORT => {
                        let n = u16::from_le_bytes([b[p], b[p + 1]]);
                        if a.normalized {
                            n as f32 / 65535.
                        } else {
                            n as f32
                        }
                    }
                    gl::SHORT => {
                        let n = i16::from_le_bytes([b[p], b[p + 1]]);
                        if a.normalized {
                            (n as f32 / 32767.).max(-1.)
                        } else {
                            n as f32
                        }
                    }
                    _ => unreachable!(),
                };
            }
        }
        Value::from(ty, &v[..ty.n()])
    }
}
fn scalar_size(kind: u32) -> Result<usize> {
    match kind {
        gl::FLOAT => Ok(4),
        gl::BYTE | gl::UNSIGNED_BYTE => Ok(1),
        gl::SHORT | gl::UNSIGNED_SHORT => Ok(2),
        _ => Err(Error::InvalidEnum),
    }
}
fn validate_mode(mode: u32) -> Result<()> {
    if matches!(mode, gl::TRIANGLES | gl::TRIANGLE_STRIP | gl::TRIANGLE_FAN) {
        Ok(())
    } else {
        Err(Error::InvalidEnum)
    }
}
