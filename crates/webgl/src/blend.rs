//! WebGL1 blend equations and factors, following GLES 2.0 section 4.1.6
//! and WebGL1's additional constant-color/constant-alpha restriction.
use super::*;

#[derive(Debug)]
pub(crate) struct BlendState {
    enabled: bool,
    factors: [u32; 4], // source RGB, destination RGB, source alpha, destination alpha
    equations: [u32; 2],
    constant: [f32; 4],
}
impl Default for BlendState {
    fn default() -> Self {
        Self {
            enabled: false,
            factors: [gl::ONE, gl::ZERO, gl::ONE, gl::ZERO],
            equations: [gl::FUNC_ADD; 2],
            constant: [0.; 4],
        }
    }
}
fn valid_factor(factor: u32, source: bool) -> bool {
    matches!(
        factor,
        gl::ZERO
            | gl::ONE
            | gl::SRC_COLOR
            | gl::ONE_MINUS_SRC_COLOR
            | gl::DST_COLOR
            | gl::ONE_MINUS_DST_COLOR
            | gl::SRC_ALPHA
            | gl::ONE_MINUS_SRC_ALPHA
            | gl::DST_ALPHA
            | gl::ONE_MINUS_DST_ALPHA
            | gl::CONSTANT_COLOR
            | gl::ONE_MINUS_CONSTANT_COLOR
            | gl::CONSTANT_ALPHA
            | gl::ONE_MINUS_CONSTANT_ALPHA
    ) || (source && factor == gl::SRC_ALPHA_SATURATE)
}
fn constant_color(f: u32) -> bool {
    matches!(f, gl::CONSTANT_COLOR | gl::ONE_MINUS_CONSTANT_COLOR)
}
fn constant_alpha(f: u32) -> bool {
    matches!(f, gl::CONSTANT_ALPHA | gl::ONE_MINUS_CONSTANT_ALPHA)
}

impl Context {
    /// Select whether the drawing buffer actually has an alpha channel.
    /// An RGB-only canvas treats destination alpha as one for blending.
    pub fn set_alpha_channel(&mut self, enabled: bool) {
        self.alpha_channel = enabled;
        if !enabled {
            for pixel in self.rgba.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        }
    }
    pub fn enable_blend(&mut self, enabled: bool) {
        self.blend.enabled = enabled;
    }
    pub fn blend_enabled(&self) -> bool {
        self.blend.enabled
    }
    pub fn blend_functions(&self) -> [u32; 4] {
        self.blend.factors
    }
    pub fn blend_equations(&self) -> [u32; 2] {
        self.blend.equations
    }
    pub fn blend_constant(&self) -> [f32; 4] {
        self.blend.constant
    }
    pub fn blend_color(&mut self, color: [f32; 4]) {
        self.blend.constant = color.map(clamp);
    }
    pub fn blend_func(&mut self, source: u32, destination: u32) -> Result<()> {
        self.blend_func_separate(source, destination, source, destination)
    }
    pub fn blend_func_separate(
        &mut self,
        src_rgb: u32,
        dst_rgb: u32,
        src_alpha: u32,
        dst_alpha: u32,
    ) -> Result<()> {
        if !valid_factor(src_rgb, true)
            || !valid_factor(dst_rgb, false)
            || !valid_factor(src_alpha, true)
            || !valid_factor(dst_alpha, false)
        {
            return Err(Error::InvalidEnum);
        }
        // The WebGL restriction applies only to the RGB pair in the Separate API.
        if (constant_color(src_rgb) && constant_alpha(dst_rgb))
            || (constant_alpha(src_rgb) && constant_color(dst_rgb))
        {
            return Err(Error::InvalidOperation);
        }
        self.blend.factors = [src_rgb, dst_rgb, src_alpha, dst_alpha];
        Ok(())
    }
    pub fn blend_equation(&mut self, equation: u32) -> Result<()> {
        self.blend_equation_separate(equation, equation)
    }
    pub fn blend_equation_separate(&mut self, rgb: u32, alpha: u32) -> Result<()> {
        if ![rgb, alpha].iter().all(|e| {
            matches!(
                *e,
                gl::FUNC_ADD | gl::FUNC_SUBTRACT | gl::FUNC_REVERSE_SUBTRACT
            )
        }) {
            return Err(Error::InvalidEnum);
        }
        self.blend.equations = [rgb, alpha];
        Ok(())
    }
}
impl BlendState {
    pub(crate) fn apply(&self, source: [f32; 4], destination: [f32; 4]) -> [f32; 4] {
        if !self.enabled {
            return source;
        }
        std::array::from_fn(|channel| {
            let slot = if channel == 3 { 2 } else { 0 };
            let src =
                source[channel] * self.factor(self.factors[slot], channel, source, destination);
            let dst = destination[channel]
                * self.factor(self.factors[slot + 1], channel, source, destination);
            clamp(match self.equations[usize::from(channel == 3)] {
                gl::FUNC_SUBTRACT => src - dst,
                gl::FUNC_REVERSE_SUBTRACT => dst - src,
                _ => src + dst,
            })
        })
    }
    fn factor(&self, factor: u32, channel: usize, source: [f32; 4], destination: [f32; 4]) -> f32 {
        match factor {
            gl::ZERO => 0.,
            gl::ONE => 1.,
            gl::SRC_COLOR => source[channel],
            gl::ONE_MINUS_SRC_COLOR => 1. - source[channel],
            gl::DST_COLOR => destination[channel],
            gl::ONE_MINUS_DST_COLOR => 1. - destination[channel],
            gl::SRC_ALPHA => source[3],
            gl::ONE_MINUS_SRC_ALPHA => 1. - source[3],
            gl::DST_ALPHA => destination[3],
            gl::ONE_MINUS_DST_ALPHA => 1. - destination[3],
            gl::CONSTANT_COLOR => self.constant[channel],
            gl::ONE_MINUS_CONSTANT_COLOR => 1. - self.constant[channel],
            gl::CONSTANT_ALPHA => self.constant[3],
            gl::ONE_MINUS_CONSTANT_ALPHA => 1. - self.constant[3],
            gl::SRC_ALPHA_SATURATE if channel == 3 => 1.,
            gl::SRC_ALPHA_SATURATE => source[3].min(1. - destination[3]),
            _ => unreachable!("validated blend factor"),
        }
    }
}
