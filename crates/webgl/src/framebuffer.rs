//! Color-texture framebuffers and depth renderbuffers. Color texture storage
//! is lower-left first; the default drawing buffer remains top-left first.
use super::*;

const MAX_OBJECTS: usize = 256;
const MAX_RENDERBUFFER_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Default)]
pub(super) struct Framebuffer {
    color: u32,
    depth: u32,
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

impl Context {
    /// Whether the default drawing buffer has depth storage. Depth-test enable
    /// state is independent and also applies to depth attachments on FBOs.
    pub fn set_depth_channel(&mut self, enabled: bool) {
        self.default_depth_channel = enabled;
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
            let saved = (self.depth_test, self.depth_mask);
            self.depth_test &= self.default_depth_channel;
            self.depth_mask &= self.default_depth_channel;
            let result = op(self);
            (self.depth_test, self.depth_mask) = saved;
            return result;
        }
        if self.framebuffer_status(self.draw_framebuffer) != gl::FRAMEBUFFER_COMPLETE {
            return Err(Error::InvalidFramebufferOperation);
        }
        let f = &self.framebuffers[&self.draw_framebuffer];
        let (texture, depth) = (f.color, f.depth);
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
        let old_state = (self.alpha_channel, self.depth_test, self.depth_mask);
        self.width = size.0;
        self.height = size.1;
        self.alpha_channel = true;
        self.depth_test &= depth != 0;
        self.depth_mask &= depth != 0;
        self.drawing_to_texture = true;
        let result = op(self);
        self.drawing_to_texture = false;
        self.width = old_size.0;
        self.height = old_size.1;
        (self.alpha_channel, self.depth_test, self.depth_mask) = old_state;
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
