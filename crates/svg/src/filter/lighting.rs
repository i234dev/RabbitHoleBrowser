//! feDiffuseLighting / feSpecularLighting (Filter Effects 1 §15.8, §15.22).

use super::ops::{IRect, Pixels};

/// A light source with positions already in device pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Light {
    /// Azimuth and elevation in radians.
    Distant { azimuth: f32, elevation: f32 },
    Point { x: f32, y: f32, z: f32 },
    Spot { x: f32, y: f32, z: f32, at: [f32; 3], exponent: f32, cone: Option<f32> },
}

/// Diffuse (`specular: None`) or specular (`Some((ks, exponent))`)
/// lighting of the alpha surface of `src` within `r`. `kd` is the
/// diffuse constant; `color` the linear or sRGB lighting colour.
pub fn light(src: &Pixels, r: IRect, surface_scale: f32, kd: f32, specular: Option<(f32, f32)>, color: [f32; 3], light: Light) -> Pixels {
    let mut out = Pixels::new(src.w, src.h);
    let r = r.intersect(&IRect::new(0, 0, src.w as i32, src.h as i32));
    if r.is_empty() {
        return out;
    }
    let w = src.w;
    let a = |x: i32, y: i32| src.px[y as usize * w + x as usize][3];
    let norm3 = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if l > 0.0 {
            [v[0] / l, v[1] / l, v[2] / l]
        } else {
            [0.0, 0.0, 1.0]
        }
    };
    let dot = |p: [f32; 3], q: [f32; 3]| p[0] * q[0] + p[1] * q[1] + p[2] * q[2];
    let spot_dir = match light {
        Light::Spot { x, y, z, at, .. } => norm3([at[0] - x, at[1] - y, at[2] - z]),
        _ => [0.0; 3],
    };
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            // Sobel normal with the edge kernels of the spec: neighbours
            // outside the image collapse onto the pixel itself.
            let normal = |horizontal: bool| -> f32 {
                let (lo, hi) = if horizontal {
                    ((x - 1).max(r.x0), (x + 1).min(r.x1 - 1))
                } else {
                    ((y - 1).max(r.y0), (y + 1).min(r.y1 - 1))
                };
                let dist = (hi - lo) as f32;
                if dist <= 0.0 {
                    return 0.0;
                }
                let mut sum = 0.0;
                let mut wsum = 0.0;
                for (d, wt) in [(-1, 1.0), (0, 2.0), (1, 1.0)] {
                    if horizontal {
                        let yy = y + d;
                        if yy < r.y0 || yy >= r.y1 {
                            continue;
                        }
                        sum += wt * (a(hi, yy) - a(lo, yy));
                    } else {
                        let xx = x + d;
                        if xx < r.x0 || xx >= r.x1 {
                            continue;
                        }
                        sum += wt * (a(xx, hi) - a(xx, lo));
                    }
                    wsum += wt;
                }
                -surface_scale * 2.0 / (wsum * dist) * sum
            };
            let n = norm3([normal(true), normal(false), 1.0]);
            let z = surface_scale * a(x, y);
            let (px, py) = (x as f32, y as f32);
            let (l, lc) = match light {
                Light::Distant { azimuth, elevation } => ([azimuth.cos() * elevation.cos(), azimuth.sin() * elevation.cos(), elevation.sin()], color),
                Light::Point { x: lx, y: ly, z: lz } => (norm3([lx - px, ly - py, lz - z]), color),
                Light::Spot { x: lx, y: ly, z: lz, exponent, cone, .. } => {
                    let l = norm3([lx - px, ly - py, lz - z]);
                    let minus_ls = -dot(l, spot_dir);
                    let lit = minus_ls > 0.0 && cone.is_none_or(|c| minus_ls >= c.cos());
                    let f = if lit { minus_ls.powf(exponent) } else { 0.0 };
                    (l, [color[0] * f, color[1] * f, color[2] * f])
                }
            };
            let o = &mut out.px[y as usize * w + x as usize];
            match specular {
                None => {
                    let f = kd * dot(n, l).max(0.0);
                    *o = [(f * lc[0]).clamp(0.0, 1.0), (f * lc[1]).clamp(0.0, 1.0), (f * lc[2]).clamp(0.0, 1.0), 1.0];
                }
                Some((ks, exp)) => {
                    let h = norm3([l[0], l[1], l[2] + 1.0]);
                    let f = ks * dot(n, h).max(0.0).powf(exp);
                    let c = [(f * lc[0]).clamp(0.0, 1.0), (f * lc[1]).clamp(0.0, 1.0), (f * lc[2]).clamp(0.0, 1.0)];
                    let al = c[0].max(c[1]).max(c[2]);
                    *o = [c[0], c[1], c[2], al];
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_surface_under_overhead_light() {
        let src = Pixels { w: 3, h: 3, px: vec![[0.0, 0.0, 0.0, 1.0]; 9] };
        let r = IRect::new(0, 0, 3, 3);
        let up = Light::Distant { azimuth: 0.0, elevation: std::f32::consts::FRAC_PI_2 };
        let d = light(&src, r, 1.0, 1.0, None, [1.0, 0.5, 0.0], up);
        assert!((d.px[4][0] - 1.0).abs() < 1e-4 && (d.px[4][1] - 0.5).abs() < 1e-4 && d.px[4][3] == 1.0);
        // Grazing light leaves a flat surface dark.
        let side = Light::Distant { azimuth: 0.0, elevation: 0.0 };
        assert!(light(&src, r, 1.0, 1.0, None, [1.0; 3], side).px[4][0].abs() < 1e-4);
        let s = light(&src, r, 1.0, 0.0, Some((1.0, 1.0)), [1.0; 3], up);
        assert!((s.px[4][3] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn slope_faces_the_light() {
        // Alpha rising to the right: the normal leans left.
        let mut src = Pixels::new(3, 3);
        for y in 0..3 {
            for x in 0..3 {
                src.px[y * 3 + x][3] = x as f32 / 2.0;
            }
        }
        let r = IRect::new(0, 0, 3, 3);
        let from_left = Light::Distant { azimuth: std::f32::consts::PI, elevation: 0.3 };
        let from_right = Light::Distant { azimuth: 0.0, elevation: 0.3 };
        let l = light(&src, r, 5.0, 1.0, None, [1.0; 3], from_left).px[4][0];
        let rr = light(&src, r, 5.0, 1.0, None, [1.0; 3], from_right).px[4][0];
        assert!(l > rr);
    }
}
