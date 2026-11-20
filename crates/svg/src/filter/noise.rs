//! feTurbulence: the reference Perlin noise of Filter Effects 1 §15.24.

const BSIZE: usize = 0x100;
const BM: i32 = 0xff;
const PERLIN_N: i32 = 0x1000;

pub struct Turbulence {
    lattice: [usize; BSIZE + BSIZE + 2],
    gradient: [[[f64; 2]; BSIZE + BSIZE + 2]; 4],
}

fn random(seed: &mut i64) -> i64 {
    const RAND_M: i64 = 2147483647;
    const RAND_A: i64 = 16807;
    const RAND_Q: i64 = 127773;
    const RAND_R: i64 = 2836;
    *seed = RAND_A * (*seed % RAND_Q) - RAND_R * (*seed / RAND_Q);
    if *seed <= 0 {
        *seed += RAND_M;
    }
    *seed
}

/// Stitching state.
#[derive(Clone, Copy)]
struct Stitch {
    width: i32,
    height: i32,
    wrap_x: i32,
    wrap_y: i32,
}

impl Turbulence {
    pub fn new(seed: f32) -> Box<Turbulence> {
        // The seed is rounded (half up) and clamped to the generator's range.
        let mut seed = (seed as f64 + 0.5).floor() as i64;
        const RAND_M: i64 = 2147483647;
        if seed <= 0 {
            seed = -(seed % (RAND_M - 1)) + 1;
        }
        if seed > RAND_M - 1 {
            seed = RAND_M - 1;
        }
        let mut t = Box::new(Turbulence { lattice: [0; BSIZE + BSIZE + 2], gradient: [[[0.0; 2]; BSIZE + BSIZE + 2]; 4] });
        for k in 0..4 {
            for i in 0..BSIZE {
                t.lattice[i] = i;
                for j in 0..2 {
                    t.gradient[k][i][j] = ((random(&mut seed) % (BSIZE as i64 + BSIZE as i64)) - BSIZE as i64) as f64 / BSIZE as f64;
                }
                let g = t.gradient[k][i];
                let s = (g[0] * g[0] + g[1] * g[1]).sqrt();
                if s > 0.0 {
                    t.gradient[k][i] = [g[0] / s, g[1] / s];
                }
            }
        }
        for i in (1..BSIZE).rev() {
            let k = t.lattice[i];
            let j = (random(&mut seed) % BSIZE as i64) as usize;
            t.lattice[i] = t.lattice[j];
            t.lattice[j] = k;
        }
        for i in 0..BSIZE + 2 {
            t.lattice[BSIZE + i] = t.lattice[i];
            for k in 0..4 {
                t.gradient[k][BSIZE + i] = t.gradient[k][i];
            }
        }
        t
    }

    fn noise2(&self, channel: usize, vec: [f64; 2], stitch: Option<&Stitch>) -> f64 {
        let s_curve = |t: f64| t * t * (3.0 - 2.0 * t);
        let lerp = |t: f64, a: f64, b: f64| a + t * (b - a);
        let t = vec[0] + PERLIN_N as f64;
        let mut bx0 = (t as i32) & BM;
        let mut bx1 = (bx0 + 1) & BM;
        let rx0 = t - (t as i64) as f64;
        let rx1 = rx0 - 1.0;
        let t = vec[1] + PERLIN_N as f64;
        let mut by0 = (t as i32) & BM;
        let mut by1 = (by0 + 1) & BM;
        let ry0 = t - (t as i64) as f64;
        let ry1 = ry0 - 1.0;
        if let Some(s) = stitch {
            if bx0 >= s.wrap_x {
                bx0 -= s.width;
            }
            if bx1 >= s.wrap_x {
                bx1 -= s.width;
            }
            if by0 >= s.wrap_y {
                by0 -= s.height;
            }
            if by1 >= s.wrap_y {
                by1 -= s.height;
            }
        }
        let (bx0, bx1, by0, by1) = ((bx0 & BM) as usize, (bx1 & BM) as usize, (by0 & BM) as usize, (by1 & BM) as usize);
        let i = self.lattice[bx0];
        let j = self.lattice[bx1];
        let b00 = self.lattice[i + by0];
        let b10 = self.lattice[j + by0];
        let b01 = self.lattice[i + by1];
        let b11 = self.lattice[j + by1];
        let sx = s_curve(rx0);
        let sy = s_curve(ry0);
        let g = &self.gradient[channel];
        let at = |q: [f64; 2], rx: f64, ry: f64| rx * q[0] + ry * q[1];
        let u = at(g[b00], rx0, ry0);
        let v = at(g[b10], rx1, ry0);
        let a = lerp(sx, u, v);
        let u = at(g[b01], rx0, ry1);
        let v = at(g[b11], rx1, ry1);
        let b = lerp(sx, u, v);
        lerp(sy, a, b)
    }

    /// The noise value of `channel` at `point` (user space). `tile` is the
    /// primitive subregion (x, y, w, h) for `stitchTiles="stitch"`.
    #[allow(clippy::too_many_arguments)]
    pub fn turbulence(&self, channel: usize, point: [f64; 2], base_x: f64, base_y: f64, octaves: u32, fractal: bool, tile: Option<[f64; 4]>) -> f64 {
        let (mut bfx, mut bfy) = (base_x, base_y);
        let mut stitch = None;
        if let Some([tx, ty, tw, th]) = tile {
            if bfx != 0.0 {
                let lo = (tw * bfx).floor() / tw;
                let hi = (tw * bfx).ceil() / tw;
                bfx = if bfx / lo < hi / bfx { lo } else { hi };
            }
            if bfy != 0.0 {
                let lo = (th * bfy).floor() / th;
                let hi = (th * bfy).ceil() / th;
                bfy = if bfy / lo < hi / bfy { lo } else { hi };
            }
            let width = (tw * bfx + 0.5) as i32;
            let height = (th * bfy + 0.5) as i32;
            stitch = Some(Stitch {
                width,
                height,
                wrap_x: (tx * bfx + PERLIN_N as f64 + width as f64) as i32,
                wrap_y: (ty * bfy + PERLIN_N as f64 + height as f64) as i32,
            });
        }
        let mut sum = 0.0;
        let mut v = [point[0] * bfx, point[1] * bfy];
        let mut ratio = 1.0;
        for _ in 0..octaves {
            let n = self.noise2(channel, v, stitch.as_ref());
            sum += if fractal { n / ratio } else { n.abs() / ratio };
            v = [v[0] * 2.0, v[1] * 2.0];
            ratio *= 2.0;
            if let Some(s) = stitch.as_mut() {
                s.width *= 2;
                s.wrap_x = 2 * s.wrap_x - PERLIN_N;
                s.height *= 2;
                s.wrap_y = 2 * s.wrap_y - PERLIN_N;
            }
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turbulence_is_deterministic_and_bounded() {
        let t = Turbulence::new(0.0);
        let a = t.turbulence(0, [10.5, 3.5], 0.05, 0.05, 2, false, None);
        let b = Turbulence::new(0.0).turbulence(0, [10.5, 3.5], 0.05, 0.05, 2, false, None);
        assert_eq!(a, b);
        for i in 0..50 {
            let v = t.turbulence(3, [i as f64 * 1.7, i as f64 * 0.3], 0.1, 0.1, 3, true, None);
            assert!(v.abs() < 2.0);
        }
        // A zero frequency gives a constant.
        assert_eq!(t.turbulence(1, [5.0, 5.0], 0.0, 0.0, 1, false, None), t.turbulence(1, [50.0, 9.0], 0.0, 0.0, 1, false, None));
    }
}
