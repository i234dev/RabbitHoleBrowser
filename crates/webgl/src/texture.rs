//! Bounded RGBA8 texture storage with nearest-neighbor and bilinear sampling.
use super::*;
pub const MAX_TEXTURE_UNITS: u32 = 8;
const MAX_TEXTURE_BYTES: usize = 64 * 1024 * 1024;
#[derive(Debug)]
pub(super) struct Texture {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pixels: Vec<u8>,
    pub(super) deleted: bool,
    pub(super) bound: bool,
    min: u32,
    mag: u32,
    wrap_s: u32,
    wrap_t: u32,
}
impl Default for Texture {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            pixels: Vec::new(),
            deleted: false,
            bound: false,
            min: gl::NEAREST_MIPMAP_LINEAR,
            mag: gl::LINEAR,
            wrap_s: gl::REPEAT,
            wrap_t: gl::REPEAT,
        }
    }
}
impl Context {
    pub fn create_texture(&mut self) -> u32 {
        if self.textures.len() >= 256 {
            return 0;
        }
        let id = self.id();
        if id != 0 {
            self.textures.insert(id, Texture::default());
        }
        id
    }
    pub fn active_texture(&mut self, unit: u32) -> Result<()> {
        if !(gl::TEXTURE0..gl::TEXTURE0 + MAX_TEXTURE_UNITS).contains(&unit) {
            return Err(Error::InvalidEnum);
        }
        self.active_texture_unit = (unit - gl::TEXTURE0) as usize;
        Ok(())
    }
    pub fn bind_texture(&mut self, target: u32, id: u32) -> Result<()> {
        if target != gl::TEXTURE_2D {
            return Err(Error::InvalidEnum);
        }
        if id != 0 && self.textures.get(&id).is_none_or(|t| t.deleted) {
            return Err(Error::InvalidOperation);
        }
        self.bound_textures[self.active_texture_unit] = id;
        if id != 0 {
            self.textures.get_mut(&id).unwrap().bound = true;
        }
        Ok(())
    }
    fn bound_texture(&self, target: u32) -> Result<u32> {
        if target != gl::TEXTURE_2D {
            return Err(Error::InvalidEnum);
        }
        let id = self.bound_textures[self.active_texture_unit];
        if id == 0 {
            return Err(Error::InvalidOperation);
        }
        Ok(id)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn tex_image_2d(
        &mut self,
        target: u32,
        level: i32,
        internal_format: u32,
        width: u32,
        height: u32,
        border: i32,
        format: u32,
        kind: u32,
        pixels: Option<&[u8]>,
    ) -> Result<()> {
        let id = self.bound_texture(target)?;
        if level < 0 || border != 0 {
            return Err(Error::InvalidValue);
        }
        if level != 0 {
            return Err(Error::InvalidOperation);
        }
        if internal_format != gl::RGBA || format != gl::RGBA || kind != gl::UNSIGNED_BYTE {
            return Err(Error::InvalidEnum);
        }
        let len = dimensions(width, height)?
            .checked_mul(4)
            .ok_or(Error::OutOfMemory)?;
        let old = self.textures[&id].pixels.len();
        if self.texture_bytes - old > MAX_TEXTURE_BYTES - len {
            return Err(Error::OutOfMemory);
        }
        if pixels.is_some_and(|p| p.len() < len) {
            return Err(Error::InvalidOperation);
        }
        let mut data = filled(len, 0)?;
        if let Some(p) = pixels {
            data.copy_from_slice(&p[..len]);
        }
        let t = self.textures.get_mut(&id).unwrap();
        t.pixels = data;
        t.width = width;
        t.height = height;
        self.texture_bytes = self.texture_bytes - old + len;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn tex_sub_image_2d(
        &mut self,
        target: u32,
        level: i32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        format: u32,
        kind: u32,
        pixels: &[u8],
    ) -> Result<()> {
        let id = self.bound_texture(target)?;
        if level != 0 || x < 0 || y < 0 {
            return Err(Error::InvalidValue);
        }
        if format != gl::RGBA || kind != gl::UNSIGNED_BYTE {
            return Err(Error::InvalidEnum);
        }
        let n = dimensions(width, height)? * 4;
        let t = self.textures.get_mut(&id).unwrap();
        if x as u64 + width as u64 > t.width as u64 || y as u64 + height as u64 > t.height as u64 {
            return Err(Error::InvalidValue);
        }
        if pixels.len() < n {
            return Err(Error::InvalidOperation);
        }
        for row in 0..height as usize {
            let src = row * width as usize * 4;
            let dst = ((y as usize + row) * t.width as usize + x as usize) * 4;
            t.pixels[dst..dst + width as usize * 4]
                .copy_from_slice(&pixels[src..src + width as usize * 4]);
        }
        Ok(())
    }
    pub fn tex_parameter_i(&mut self, target: u32, pname: u32, value: i32) -> Result<()> {
        let id = self.bound_texture(target)?;
        let t = self.textures.get_mut(&id).unwrap();
        let value = value as u32;
        match pname {
            gl::TEXTURE_MIN_FILTER | gl::TEXTURE_MAG_FILTER => {
                if !matches!(value, gl::NEAREST | gl::LINEAR) {
                    return Err(Error::InvalidEnum);
                }
                if pname == gl::TEXTURE_MIN_FILTER {
                    t.min = value;
                } else {
                    t.mag = value;
                }
            }
            gl::TEXTURE_WRAP_S | gl::TEXTURE_WRAP_T => {
                if !matches!(value, gl::CLAMP_TO_EDGE | gl::REPEAT | gl::MIRRORED_REPEAT) {
                    return Err(Error::InvalidEnum);
                }
                if pname == gl::TEXTURE_WRAP_S {
                    t.wrap_s = value;
                } else {
                    t.wrap_t = value;
                }
            }
            _ => return Err(Error::InvalidEnum),
        }
        Ok(())
    }
    pub fn texture_parameter(&self, target: u32, pname: u32) -> Result<i32> {
        let id = self.bound_texture(target)?;
        let t = &self.textures[&id];
        Ok(match pname {
            gl::TEXTURE_MIN_FILTER => t.min,
            gl::TEXTURE_MAG_FILTER => t.mag,
            gl::TEXTURE_WRAP_S => t.wrap_s,
            gl::TEXTURE_WRAP_T => t.wrap_t,
            _ => return Err(Error::InvalidEnum),
        } as i32)
    }
    pub fn texture_dimensions(&self, id: u32) -> Result<(u32, u32)> {
        let t = self.textures.get(&id).ok_or(Error::InvalidValue)?;
        Ok((t.width, t.height))
    }
    pub fn delete_texture(&mut self, id: u32) -> Result<()> {
        if id == 0 {
            return Ok(());
        }
        let t = self.textures.get_mut(&id).ok_or(Error::InvalidValue)?;
        if t.deleted {
            return Ok(());
        }
        t.deleted = true;
        for bound in &mut self.bound_textures {
            if *bound == id {
                *bound = 0;
            }
        }
        self.detach_deleted_texture(id);
        self.collect_framebuffer_resources();
        Ok(())
    }
    pub fn uniform_i32(&mut self, location: i32, values: &[i32]) -> Result<()> {
        if location < 0 {
            return Ok(());
        }
        if values.len() != 1 {
            return Err(Error::InvalidOperation);
        }
        if !(0..MAX_TEXTURE_UNITS as i32).contains(&values[0]) {
            return Err(Error::InvalidValue);
        }
        let u = self.uniform_mut(location)?;
        if u.ty != Ty::Sampler2D {
            return Err(Error::InvalidOperation);
        }
        u.value = Value::from(u.ty, &[values[0] as f32]);
        Ok(())
    }
    pub(super) fn validate_samplers(&self, e: &Executable) -> Result<()> {
        for u in &e.uniforms {
            if u.ty == Ty::Sampler2D {
                let unit = u.value.v[0] as usize;
                let Some(&id) = self.bound_textures.get(unit) else {
                    return Err(Error::InvalidOperation);
                };
                if self.draw_color_texture() == Some(id) {
                    return Err(Error::InvalidOperation);
                }
                if let Some(t) = self.textures.get(&id) {
                    // The rasterizer does not yet calculate texture-coordinate
                    // derivatives. Equal min/mag filters have unambiguous
                    // sampling; mixed filters and mipmaps fail before drawing.
                    if t.width > 0
                        && t.height > 0
                        && (!matches!(t.min, gl::NEAREST | gl::LINEAR) || t.min != t.mag)
                    {
                        return Err(Error::InvalidOperation);
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn sample_texture(&self, unit: i32, uv: [f32; 2]) -> [f32; 4] {
        let black = [0., 0., 0., 1.];
        if uv.iter().any(|v| !v.is_finite()) {
            return black;
        }
        let Some(&id) = self.bound_textures.get(unit as usize) else {
            return black;
        };
        let Some(t) = self.textures.get(&id) else {
            return black;
        };
        if t.width == 0 || t.height == 0 {
            return black;
        }
        if self.context_version == 1
            && (!t.width.is_power_of_two() || !t.height.is_power_of_two())
            && (t.wrap_s != gl::CLAMP_TO_EDGE || t.wrap_t != gl::CLAMP_TO_EDGE)
        {
            return black;
        }
        if t.min == gl::LINEAR {
            let (x0, x1, a) = linear_axis(uv[0], t.width, t.wrap_s);
            let (y0, y1, b) = linear_axis(uv[1], t.height, t.wrap_t);
            let texel = |x: u32, y: u32, channel: usize| {
                t.pixels[(y as usize * t.width as usize + x as usize) * 4 + channel] as f32
            };
            return std::array::from_fn(|i| {
                let bottom = texel(x0, y0, i) * (1. - a) + texel(x1, y0, i) * a;
                let top = texel(x0, y1, i) * (1. - a) + texel(x1, y1, i) * a;
                (bottom * (1. - b) + top * b) / 255.
            });
        }
        let x = coordinate(uv[0], t.width, t.wrap_s);
        let y = coordinate(uv[1], t.height, t.wrap_t);
        let offset = (y as usize * t.width as usize + x as usize) * 4;
        std::array::from_fn(|i| t.pixels[offset + i] as f32 / 255.)
    }
}
// Normalized coordinates address texel centers at (i + 0.5) / size.
// Fold coordinates before multiplying to avoid overflow for large finite UVs.
// Mirrored repeat reflects at texel edges; after folding, clamping the two
// taps repeats the edge texel across each reflection, including negative UVs.
fn linear_axis(v: f32, size: u32, wrap: u32) -> (u32, u32, f32) {
    let c = match wrap {
        gl::REPEAT => v.rem_euclid(1.),
        gl::MIRRORED_REPEAT => {
            let v = v.rem_euclid(2.);
            if v > 1. {
                2. - v
            } else {
                v
            }
        }
        _ => v.clamp(0., 1.),
    };
    let position = c * size as f32 - 0.5;
    let low = position.floor();
    let index = |i: i32| {
        if wrap == gl::REPEAT {
            i.rem_euclid(size as i32) as u32
        } else {
            i.clamp(0, size as i32 - 1) as u32
        }
    };
    (index(low as i32), index(low as i32 + 1), position - low)
}
fn coordinate(v: f32, size: u32, wrap: u32) -> u32 {
    let c = match wrap {
        gl::REPEAT => v.rem_euclid(1.),
        gl::MIRRORED_REPEAT => {
            let v = v.rem_euclid(2.);
            if v > 1. {
                2. - v
            } else {
                v
            }
        }
        _ => v.clamp(0., 1.),
    };
    ((c * size as f32).floor() as u32).min(size - 1)
}
