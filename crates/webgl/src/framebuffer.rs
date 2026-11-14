//! Color-texture framebuffers and depth renderbuffers. Color texture storage
//! is lower-left first; the default drawing buffer remains top-left first.
use super::*;

const MAX_OBJECTS: usize = 256;
const MAX_RENDERBUFFER_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct Framebuffer {
    color: u32,
    depth: u32,
    read_color: bool,
    draw_color: bool,
}
impl Default for Framebuffer {
    fn default() -> Self {
        Self { color: 0, depth: 0, read_color: true, draw_color: true }
    }
}

#[derive(Debug)]
pub(super) struct Renderbuffer {
    width: u32,
    height: u32,
    format: u32,
    depth: Vec<f32>,
    deleted: bool,
}
impl Default for Renderbuffer {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            format: gl::RGBA4,
            depth: Vec::new(),
            deleted: false,
        }
    }
}

#[derive(Clone, Copy)]
struct BlitImage {
    width: u32,
    height: u32,
    // Zero identifies the default image. Complete FBOs always have color.
    color: u32,
    color_enabled: bool,
    // Some(0) is the default depth24 image; FBOs use depth16 renderbuffers.
    depth: Option<u32>,
}
impl BlitImage {
    fn index(self, x: i64, y: i64) -> usize {
        let row = if self.color == 0 { self.height as i64 - 1 - y } else { y };
        (row * self.width as i64 + x) as usize
    }
}

struct BlitAxis { source: i64, source_span: i64, destination: i64, destination_span: i64 }
impl BlitAxis {
    // Rational arithmetic keeps clipping and nearest selection exact even for
    // rectangles near GLint's endpoints. The fraction is only used by LINEAR.
    fn at(&self, pixel: i64, limit: u32) -> Option<(i64, f64)> {
        let mut denominator = 2 * self.destination_span as i128;
        let mut numerator = self.source as i128 * denominator
            + (2 * (pixel - self.destination) as i128 + 1) * self.source_span as i128;
        if denominator < 0 { denominator = -denominator; numerator = -numerator; }
        if numerator < 0 || numerator >= limit as i128 * denominator { return None; }
        Some(((numerator / denominator) as i64, (numerator % denominator) as f64 / denominator as f64))
    }
}

impl Context {
    fn blit_image(&self, framebuffer: u32, reading: bool) -> Result<BlitImage> {
        if self.framebuffer_status(framebuffer) != gl::FRAMEBUFFER_COMPLETE {
            return Err(Error::InvalidFramebufferOperation);
        }
        if framebuffer == 0 {
            Ok(BlitImage { width: self.width, height: self.height, color: 0,
                color_enabled: if reading { self.default_read_color } else { self.default_draw_color },
                depth: self.default_depth_channel.then_some(0) })
        } else {
            let fb = &self.framebuffers[&framebuffer];
            let t = &self.textures[&fb.color];
            Ok(BlitImage { width: t.width, height: t.height, color: fb.color,
                color_enabled: if reading { fb.read_color } else { fb.draw_color },
                depth: (fb.depth != 0).then_some(fb.depth) })
        }
    }
    fn blit_color(&self, image: BlitImage, x: i64, y: i64) -> [u8; 4] {
        let bytes = if image.color == 0 { &self.rgba } else { &self.textures[&image.color].pixels };
        let i = image.index(x, y) * 4;
        let mut pixel: [u8; 4] = bytes[i..i + 4].try_into().unwrap();
        if image.color == 0 && !self.alpha_channel { pixel[3] = 255; }
        pixel
    }
    fn blit_linear_color(&self, image: BlitImage, x: (i64, f64), y: (i64, f64)) -> [u8; 4] {
        let axis = |(pixel, fraction): (i64, f64), limit: u32| {
            let (lo, weight) = if fraction < 0.5 { (pixel - 1, fraction + 0.5) } else { (pixel, fraction - 0.5) };
            (lo.clamp(0, limit as i64 - 1), (lo + 1).clamp(0, limit as i64 - 1), weight)
        };
        let (x0, x1, fx) = axis(x, image.width);
        let (y0, y1, fy) = axis(y, image.height);
        let colors = [self.blit_color(image, x0, y0), self.blit_color(image, x1, y0),
            self.blit_color(image, x0, y1), self.blit_color(image, x1, y1)];
        std::array::from_fn(|k| {
            let bottom = colors[0][k] as f64 * (1. - fx) + colors[1][k] as f64 * fx;
            let top = colors[2][k] as f64 * (1. - fx) + colors[3][k] as f64 * fx;
            (bottom * (1. - fy) + top * fy).round() as u8
        })
    }
    /// Single-sample RGBA8/RGB8 color blits and matching depth16 blits.
    /// See GLES 3.0.6 §4.3.3 and WebGL2's missing-attachment/rectangle rules.
    /// Only scissor affects writes; viewport, masks, blending and depth test do not.
    pub fn blit_framebuffer(&mut self, source: [i32; 4], destination: [i32; 4], mask: u32, filter: u32) -> Result<()> {
        if self.context_version != 2 { return Err(Error::InvalidOperation); }
        if mask & !(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT | gl::STENCIL_BUFFER_BIT) != 0 {
            return Err(Error::InvalidValue);
        }
        if !matches!(filter, gl::NEAREST | gl::LINEAR) { return Err(Error::InvalidEnum); }
        if filter == gl::LINEAR && mask & (gl::DEPTH_BUFFER_BIT | gl::STENCIL_BUFFER_BIT) != 0 {
            return Err(Error::InvalidOperation);
        }
        let s = source.map(i64::from);
        let d = destination.map(i64::from);
        let spans = [s[2] - s[0], s[3] - s[1], d[2] - d[0], d[3] - d[1]];
        if spans.iter().any(|span| span.abs() > i32::MAX as i64) { return Err(Error::InvalidValue); }
        let src = self.blit_image(self.read_framebuffer, true)?;
        let dst = self.blit_image(self.draw_framebuffer, false)?;
        let color = mask & gl::COLOR_BUFFER_BIT != 0 && dst.color_enabled;
        if color && !src.color_enabled { return Err(Error::InvalidOperation); }
        let depth = mask & gl::DEPTH_BUFFER_BIT != 0 && dst.depth.is_some();
        // Image identity, not framebuffer identity: two FBOs may alias storage.
        if color && src.color == dst.color { return Err(Error::InvalidOperation); }
        if depth {
            let (Some(read), Some(draw)) = (src.depth, dst.depth) else { return Err(Error::InvalidOperation); };
            if read == draw || (read == 0) != (draw == 0) { return Err(Error::InvalidOperation); }
        }
        // Stencil and multisample storage cannot be created by this backend.
        // Missing destination attachments are ignored, as required by WebGL2.
        if (!color && !depth) || spans.contains(&0) { return Ok(()); }
        let ax = BlitAxis { source: s[0], source_span: spans[0], destination: d[0], destination_span: spans[2] };
        let ay = BlitAxis { source: s[1], source_span: spans[1], destination: d[1], destination_span: spans[3] };
        let mut rect = [d[0].min(d[2]).max(0), d[1].min(d[3]).max(0),
            d[0].max(d[2]).min(dst.width as i64), d[1].max(d[3]).min(dst.height as i64)];
        if self.scissor_enabled {
            rect[0] = rect[0].max(self.scissor[0]); rect[1] = rect[1].max(self.scissor[1]);
            rect[2] = rect[2].min(self.scissor[0] + self.scissor[2]); rect[3] = rect[3].min(self.scissor[1] + self.scissor[3]);
        }
        // All errors have been checked. No allocation or fallible operation may
        // occur once the first pixel is written, including combined color/depth.
        for y in rect[1]..rect[3] {
            let Some(sy) = ay.at(y, src.height) else { continue };
            for x in rect[0]..rect[2] {
                let Some(sx) = ax.at(x, src.width) else { continue };
                let di = dst.index(x, y);
                if color {
                    let mut pixel = if filter == gl::LINEAR { self.blit_linear_color(src, sx, sy) } else { self.blit_color(src, sx.0, sy.0) };
                    let bytes = if dst.color == 0 {
                        if !self.alpha_channel { pixel[3] = 255; }
                        &mut self.rgba
                    } else { &mut self.textures.get_mut(&dst.color).unwrap().pixels };
                    bytes[di * 4..di * 4 + 4].copy_from_slice(&pixel);
                }
                if depth {
                    let read = src.depth.unwrap(); let draw = dst.depth.unwrap();
                    let si = src.index(sx.0, sy.0);
                    let value = if read == 0 { self.depth[si] } else { self.renderbuffers[&read].depth[si] };
                    if draw == 0 { self.depth[di] = value; } else { self.renderbuffers.get_mut(&draw).unwrap().depth[di] = value; }
                }
            }
        }
        Ok(())
    }
    /// Whether the default drawing buffer has depth storage. Depth-test enable
    /// state is independent and also applies to depth attachments on FBOs.
    pub fn set_depth_channel(&mut self, enabled: bool) {
        self.default_depth_channel = enabled;
    }
    /// This software subset exposes one color output/attachment, not MRT.
    pub fn draw_buffers(&mut self, buffers: &[u32]) -> Result<()> {
        if self.context_version != 2 { return Err(Error::InvalidOperation); }
        if buffers.len() > 1 { return Err(Error::InvalidValue); }
        if buffers.iter().any(|b| !matches!(*b, gl::NONE | gl::BACK | 0x8CE0..=0x8CFF)) {
            return Err(Error::InvalidEnum);
        }
        if self.draw_framebuffer == 0 {
            if buffers.len() != 1 || !matches!(buffers[0], gl::BACK | gl::NONE) { return Err(Error::InvalidOperation); }
            self.default_draw_color = buffers[0] == gl::BACK;
        } else {
            let selection = buffers.first().copied().unwrap_or(gl::NONE);
            if !matches!(selection, gl::COLOR_ATTACHMENT0 | gl::NONE) { return Err(Error::InvalidOperation); }
            self.framebuffers.get_mut(&self.draw_framebuffer).unwrap().draw_color = selection == gl::COLOR_ATTACHMENT0;
        }
        Ok(())
    }
    pub fn read_buffer_select(&mut self, selection: u32) -> Result<()> {
        if self.context_version != 2 { return Err(Error::InvalidOperation); }
        if !matches!(selection, gl::NONE | gl::BACK | 0x8CE0..=0x8CFF) { return Err(Error::InvalidEnum); }
        if self.read_framebuffer == 0 {
            if !matches!(selection, gl::BACK | gl::NONE) { return Err(Error::InvalidOperation); }
            self.default_read_color = selection == gl::BACK;
        } else {
            if !matches!(selection, gl::COLOR_ATTACHMENT0 | gl::NONE) { return Err(Error::InvalidOperation); }
            self.framebuffers.get_mut(&self.read_framebuffer).unwrap().read_color = selection == gl::COLOR_ATTACHMENT0;
        }
        Ok(())
    }
    pub fn read_buffer_selection(&self) -> Result<u32> {
        if self.context_version != 2 { return Err(Error::InvalidOperation); }
        Ok(if self.read_framebuffer == 0 {
            if self.default_read_color { gl::BACK } else { gl::NONE }
        } else if self.framebuffers[&self.read_framebuffer].read_color { gl::COLOR_ATTACHMENT0 } else { gl::NONE })
    }
    pub fn draw_buffer_selection(&self) -> Result<u32> {
        if self.context_version != 2 { return Err(Error::InvalidOperation); }
        Ok(if self.draw_framebuffer == 0 {
            if self.default_draw_color { gl::BACK } else { gl::NONE }
        } else if self.framebuffers[&self.draw_framebuffer].draw_color { gl::COLOR_ATTACHMENT0 } else { gl::NONE })
    }
    fn framebuffer_target(&self, target: u32) -> Result<u32> {
        match target {
            gl::FRAMEBUFFER => Ok(self.draw_framebuffer),
            gl::DRAW_FRAMEBUFFER if self.context_version == 2 => Ok(self.draw_framebuffer),
            gl::READ_FRAMEBUFFER if self.context_version == 2 => Ok(self.read_framebuffer),
            _ => Err(Error::InvalidEnum),
        }
    }
    pub fn framebuffer_binding(&self, target: u32) -> Result<u32> {
        self.framebuffer_target(target)
    }
    pub fn renderbuffer_binding(&self) -> u32 {
        self.renderbuffer_binding
    }
    pub fn create_framebuffer(&mut self) -> u32 {
        if self.framebuffers.len() >= MAX_OBJECTS {
            return 0;
        }
        let id = self.id();
        if id != 0 {
            self.framebuffers.insert(id, Framebuffer::default());
        }
        id
    }
    pub fn bind_framebuffer(&mut self, target: u32, id: u32) -> Result<()> {
        self.framebuffer_target(target)?;
        if id != 0 && !self.framebuffers.contains_key(&id) {
            return Err(Error::InvalidOperation);
        }
        if target != gl::READ_FRAMEBUFFER {
            self.draw_framebuffer = id;
        }
        if target != gl::DRAW_FRAMEBUFFER {
            self.read_framebuffer = id;
        }
        Ok(())
    }
    pub fn delete_framebuffer(&mut self, id: u32) -> Result<()> {
        if id == 0 {
            return Ok(());
        }
        self.framebuffers.remove(&id).ok_or(Error::InvalidValue)?;
        if self.draw_framebuffer == id {
            self.draw_framebuffer = 0;
        }
        if self.read_framebuffer == id {
            self.read_framebuffer = 0;
        }
        self.collect_framebuffer_resources();
        Ok(())
    }
    pub fn create_renderbuffer(&mut self) -> u32 {
        if self.renderbuffers.len() >= MAX_OBJECTS {
            return 0;
        }
        let id = self.id();
        if id != 0 {
            self.renderbuffers.insert(id, Renderbuffer::default());
        }
        id
    }
    pub fn bind_renderbuffer(&mut self, target: u32, id: u32) -> Result<()> {
        if target != gl::RENDERBUFFER {
            return Err(Error::InvalidEnum);
        }
        if id != 0 && self.renderbuffers.get(&id).is_none_or(|r| r.deleted) {
            return Err(Error::InvalidOperation);
        }
        self.renderbuffer_binding = id;
        Ok(())
    }
    pub fn renderbuffer_storage(
        &mut self,
        target: u32,
        format: u32,
        width: u32,
        height: u32,
    ) -> Result<()> {
        if target != gl::RENDERBUFFER {
            return Err(Error::InvalidEnum);
        }
        if format != gl::DEPTH_COMPONENT16 {
            return Err(Error::InvalidEnum);
        }
        if width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(Error::InvalidValue);
        }
        let n = dimensions(width, height)?;
        let old = self
            .renderbuffers
            .get(&self.renderbuffer_binding)
            .ok_or(Error::InvalidOperation)?
            .depth
            .len()
            * 4;
        let bytes = n * 4;
        if self.renderbuffer_bytes - old > MAX_RENDERBUFFER_BYTES - bytes {
            return Err(Error::OutOfMemory);
        }
        let depth = filled(n, 1.)?;
        let rb = self
            .renderbuffers
            .get_mut(&self.renderbuffer_binding)
            .unwrap();
        rb.width = width;
        rb.height = height;
        rb.format = format;
        rb.depth = depth;
        self.renderbuffer_bytes = self.renderbuffer_bytes - old + bytes;
        Ok(())
    }
    pub fn renderbuffer_parameter(&self, target: u32, pname: u32) -> Result<u32> {
        if target != gl::RENDERBUFFER {
            return Err(Error::InvalidEnum);
        }
        let rb = self
            .renderbuffers
            .get(&self.renderbuffer_binding)
            .ok_or(Error::InvalidOperation)?;
        Ok(match pname {
            gl::RENDERBUFFER_WIDTH => rb.width,
            gl::RENDERBUFFER_HEIGHT => rb.height,
            gl::RENDERBUFFER_INTERNAL_FORMAT => rb.format,
            gl::RENDERBUFFER_DEPTH_SIZE => {
                if rb.format == gl::DEPTH_COMPONENT16 {
                    16
                } else {
                    0
                }
            }
            gl::RENDERBUFFER_RED_SIZE
            | gl::RENDERBUFFER_GREEN_SIZE
            | gl::RENDERBUFFER_BLUE_SIZE
            | gl::RENDERBUFFER_ALPHA_SIZE
            | gl::RENDERBUFFER_STENCIL_SIZE => 0,
            _ => return Err(Error::InvalidEnum),
        })
    }
    pub fn delete_renderbuffer(&mut self, id: u32) -> Result<()> {
        if id == 0 {
            return Ok(());
        }
        let rb = self.renderbuffers.get_mut(&id).ok_or(Error::InvalidValue)?;
        if rb.deleted {
            return Ok(());
        }
        rb.deleted = true;
        if self.renderbuffer_binding == id {
            self.renderbuffer_binding = 0;
        }
        for fb in [self.draw_framebuffer, self.read_framebuffer] {
            if let Some(f) = self.framebuffers.get_mut(&fb) {
                if f.depth == id {
                    f.depth = 0;
                }
            }
        }
        self.collect_framebuffer_resources();
        Ok(())
    }
    pub fn framebuffer_texture_2d(
        &mut self,
        target: u32,
        attachment: u32,
        textarget: u32,
        texture: u32,
        level: i32,
    ) -> Result<()> {
        let fb = self.framebuffer_target(target)?;
        if attachment != gl::COLOR_ATTACHMENT0 {
            return Err(Error::InvalidEnum);
        }
        if textarget != gl::TEXTURE_2D {
            return Err(Error::InvalidEnum);
        }
        if level != 0 {
            return Err(if level < 0 || self.context_version == 1 {
                Error::InvalidValue
            } else {
                Error::InvalidOperation
            });
        }
        if fb == 0 {
            return Err(Error::InvalidOperation);
        }
        if texture != 0
            && self
                .textures
                .get(&texture)
                .is_none_or(|t| t.deleted || !t.bound)
        {
            return Err(Error::InvalidOperation);
        }
        self.framebuffers.get_mut(&fb).unwrap().color = texture;
        self.collect_framebuffer_resources();
        Ok(())
    }
    pub fn framebuffer_renderbuffer(
        &mut self,
        target: u32,
        attachment: u32,
        rbtarget: u32,
        renderbuffer: u32,
    ) -> Result<()> {
        let fb = self.framebuffer_target(target)?;
        if attachment != gl::DEPTH_ATTACHMENT {
            return Err(Error::InvalidEnum);
        }
        if rbtarget != gl::RENDERBUFFER {
            return Err(Error::InvalidEnum);
        }
        if fb == 0 {
            return Err(Error::InvalidOperation);
        }
        if renderbuffer != 0
            && self
                .renderbuffers
                .get(&renderbuffer)
                .is_none_or(|r| r.deleted)
        {
            return Err(Error::InvalidOperation);
        }
        self.framebuffers.get_mut(&fb).unwrap().depth = renderbuffer;
        self.collect_framebuffer_resources();
        Ok(())
    }
    fn framebuffer_status(&self, id: u32) -> u32 {
        if id == 0 {
            return gl::FRAMEBUFFER_COMPLETE;
        }
        let fb = &self.framebuffers[&id];
        if fb.color == 0 && fb.depth == 0 {
            return gl::FRAMEBUFFER_INCOMPLETE_MISSING_ATTACHMENT;
        }
        if fb.color == 0 {
            return gl::FRAMEBUFFER_UNSUPPORTED;
        }
        let Some(t) = self.textures.get(&fb.color) else {
            return gl::FRAMEBUFFER_INCOMPLETE_ATTACHMENT;
        };
        if t.width == 0 || t.height == 0 {
            return gl::FRAMEBUFFER_INCOMPLETE_ATTACHMENT;
        }
        if fb.depth != 0 {
            let Some(r) = self.renderbuffers.get(&fb.depth) else {
                return gl::FRAMEBUFFER_INCOMPLETE_ATTACHMENT;
            };
            if r.width == 0 || r.height == 0 || r.format != gl::DEPTH_COMPONENT16 {
                return gl::FRAMEBUFFER_INCOMPLETE_ATTACHMENT;
            }
            if (r.width, r.height) != (t.width, t.height) {
                return if self.context_version == 1 {
                    gl::FRAMEBUFFER_INCOMPLETE_DIMENSIONS
                } else {
                    gl::FRAMEBUFFER_UNSUPPORTED
                };
            }
        }
        gl::FRAMEBUFFER_COMPLETE
    }
    pub fn check_framebuffer_status(&self, target: u32) -> Result<u32> {
        Ok(self.framebuffer_status(self.framebuffer_target(target)?))
    }
    pub fn framebuffer_attachment_parameter(
        &self,
        target: u32,
        attachment: u32,
        pname: u32,
    ) -> Result<u32> {
        let id = self.framebuffer_target(target)?;
        let f = self.framebuffers.get(&id).ok_or(Error::InvalidOperation)?;
        let (kind, name) = match attachment {
            gl::COLOR_ATTACHMENT0 => (gl::TEXTURE, f.color),
            gl::DEPTH_ATTACHMENT => (gl::RENDERBUFFER, f.depth),
            _ => return Err(Error::InvalidEnum),
        };
        Ok(match pname {
            gl::FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE => {
                if name == 0 {
                    gl::NONE
                } else {
                    kind
                }
            }
            gl::FRAMEBUFFER_ATTACHMENT_OBJECT_NAME => name,
            gl::FRAMEBUFFER_ATTACHMENT_TEXTURE_LEVEL
            | gl::FRAMEBUFFER_ATTACHMENT_TEXTURE_CUBE_MAP_FACE
                if kind == gl::TEXTURE && name != 0 =>
            {
                0
            }
            _ => return Err(Error::InvalidEnum),
        })
    }
    pub fn framebuffer_bits(&self, pname: u32) -> Result<u32> {
        let f = self
            .framebuffers
            .get(&self.draw_framebuffer)
            .ok_or(Error::InvalidOperation)?;
        match pname {
            0x0D52..=0x0D55 => Ok(if f.color == 0 { 0 } else { 8 }),
            0x0D56 => Ok(if f.depth == 0 { 0 } else { 16 }),
            0x0D57 => Ok(0),
            _ => Err(Error::InvalidEnum),
        }
    }
    pub fn read_dimensions(&self) -> Result<(u32, u32)> {
        let (w, h, _, _) = self.read_buffer()?;
        Ok((w, h))
    }
    pub fn reads_default_framebuffer(&self) -> bool {
        self.read_framebuffer == 0
    }
    pub(super) fn read_buffer(&self) -> Result<(u32, u32, &[u8], bool)> {
        if self.framebuffer_status(self.read_framebuffer) != gl::FRAMEBUFFER_COMPLETE {
            return Err(Error::InvalidFramebufferOperation);
        }
        if (self.read_framebuffer == 0 && !self.default_read_color)
            || self.framebuffers.get(&self.read_framebuffer).is_some_and(|f| !f.read_color) {
            return Err(Error::InvalidOperation);
        }
        if self.read_framebuffer == 0 {
            return Ok((self.width, self.height, &self.rgba, false));
        }
        let t = &self.textures[&self.framebuffers[&self.read_framebuffer].color];
        Ok((t.width, t.height, &t.pixels, true))
    }
    pub(super) fn draw_color_texture(&self) -> Option<u32> {
        self.framebuffers
            .get(&self.draw_framebuffer)
            .map(|f| f.color)
            .filter(|id| *id != 0)
    }
    pub(super) fn pixel_index(&self, x: i64, y: i64) -> usize {
        let row = if self.drawing_to_texture {
            y
        } else {
            self.height as i64 - 1 - y
        };
        (row * self.width as i64 + x) as usize
    }
    pub(super) fn stored_depth(&self, value: f32) -> f32 {
        if self.drawing_to_texture {
            (clamp(value) * 65535.).round() / 65535.
        } else {
            value
        }
    }
    /// Make the selected target active only for the operation. The default
    /// buffer and all GL state survive binding, rendering and errors unchanged.
    pub(super) fn with_draw_framebuffer<T>(
        &mut self,
        op: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        if self.drawing_to_texture {
            return op(self);
        }
        if self.draw_framebuffer == 0 {
            let saved = (self.depth_test, self.depth_mask, self.color_mask);
            self.depth_test &= self.default_depth_channel;
            self.depth_mask &= self.default_depth_channel;
            if !self.default_draw_color { self.color_mask = [false; 4]; }
            let result = op(self);
            (self.depth_test, self.depth_mask, self.color_mask) = saved;
            return result;
        }
        if self.framebuffer_status(self.draw_framebuffer) != gl::FRAMEBUFFER_COMPLETE {
            return Err(Error::InvalidFramebufferOperation);
        }
        let f = &self.framebuffers[&self.draw_framebuffer];
        let (texture, depth, draw_color) = (f.color, f.depth, f.draw_color);
        let t = self.textures.get_mut(&texture).unwrap();
        let size = (t.width, t.height);
        let rgba = std::mem::take(&mut t.pixels);
        let target_depth = if depth == 0 {
            Vec::new()
        } else {
            std::mem::take(&mut self.renderbuffers.get_mut(&depth).unwrap().depth)
        };
        let old_rgba = std::mem::replace(&mut self.rgba, rgba);
        let old_depth = std::mem::replace(&mut self.depth, target_depth);
        let old_size = (self.width, self.height);
        let old_state = (self.alpha_channel, self.depth_test, self.depth_mask, self.color_mask);
        self.width = size.0;
        self.height = size.1;
        self.alpha_channel = true;
        self.depth_test &= depth != 0;
        self.depth_mask &= depth != 0;
        if !draw_color { self.color_mask = [false; 4]; }
        self.drawing_to_texture = true;
        let result = op(self);
        self.drawing_to_texture = false;
        self.width = old_size.0;
        self.height = old_size.1;
        (self.alpha_channel, self.depth_test, self.depth_mask, self.color_mask) = old_state;
        self.textures.get_mut(&texture).unwrap().pixels =
            std::mem::replace(&mut self.rgba, old_rgba);
        let target_depth = std::mem::replace(&mut self.depth, old_depth);
        if depth != 0 {
            self.renderbuffers.get_mut(&depth).unwrap().depth = target_depth;
        }
        result
    }
    pub(super) fn detach_deleted_texture(&mut self, id: u32) {
        for fb in [self.draw_framebuffer, self.read_framebuffer] {
            if let Some(f) = self.framebuffers.get_mut(&fb) {
                if f.color == id {
                    f.color = 0;
                }
            }
        }
    }
    pub(super) fn collect_framebuffer_resources(&mut self) {
        let textures: Vec<u32> = self
            .textures
            .iter()
            .filter(|(id, t)| t.deleted && !self.framebuffers.values().any(|f| f.color == **id))
            .map(|(&id, _)| id)
            .collect();
        for id in textures {
            self.texture_bytes -= self.textures.remove(&id).unwrap().pixels.len();
        }
        let renderbuffers: Vec<u32> = self
            .renderbuffers
            .iter()
            .filter(|(id, r)| r.deleted && !self.framebuffers.values().any(|f| f.depth == **id))
            .map(|(&id, _)| id)
            .collect();
        for id in renderbuffers {
            self.renderbuffer_bytes -= self.renderbuffers.remove(&id).unwrap().depth.len() * 4;
        }
    }
}
