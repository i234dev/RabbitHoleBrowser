//! 4×4 matrices for CSS 3D transforms (CSS Transforms 2).
//!
//! Matrices are row-major (`m[row * 4 + col]`) and act on column vectors:
//! `p' = M · p`. `mul(a, b)` applies `b` first. CSS `matrix3d()` lists its
//! arguments column by column; [`from_css`] converts.

use crate::canvas::Matrix;

pub type Mat4 = [f64; 16];

pub const IDENTITY4: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// `a × b` (apply `b` first, then `a`).
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut o = [0.0f64; 16];
    for r in 0..4 {
        for c in 0..4 {
            let mut s = 0.0f64;
            for k in 0..4 {
                s += a[r * 4 + k] * b[k * 4 + c];
            }
            o[r * 4 + c] = s;
        }
    }
    o
}

/// The 3D form of a 2D affine matrix `[a, b, c, d, e, f]`.
pub fn from_2d(m: &Matrix) -> Mat4 {
    let m = m.map(|v| v as f64);
    [m[0], m[2], 0.0, m[4], m[1], m[3], 0.0, m[5], 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
}

/// `matrix3d()` arguments (column-major) as a matrix.
pub fn from_css(a: &[f32; 16]) -> Mat4 {
    let mut m = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            m[r * 4 + c] = a[c * 4 + r] as f64;
        }
    }
    m
}

/// The `matrix3d()` arguments (column-major) of a matrix.
pub fn to_css(m: &Mat4) -> [f32; 16] {
    let mut a = [0.0f32; 16];
    for c in 0..4 {
        for r in 0..4 {
            a[c * 4 + r] = m[r * 4 + c] as f32;
        }
    }
    a
}

pub fn translate(x: f32, y: f32, z: f32) -> Mat4 {
    let (x, y, z) = (x as f64, y as f64, z as f64);
    [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, y, 0.0, 0.0, 1.0, z, 0.0, 0.0, 0.0, 1.0]
}

pub fn scale(x: f32, y: f32, z: f32) -> Mat4 {
    let (x, y, z) = (x as f64, y as f64, z as f64);
    [x, 0.0, 0.0, 0.0, 0.0, y, 0.0, 0.0, 0.0, 0.0, z, 0.0, 0.0, 0.0, 0.0, 1.0]
}

/// `rotate3d(x, y, z, angle)` (radians; CSS Transforms 2 §12): a
/// clockwise rotation (y axis pointing down) about the normalized axis.
/// A zero axis is the identity.
pub fn rotate(x: f32, y: f32, z: f32, angle: f32) -> Mat4 {
    let (x, y, z) = (x as f64, y as f64, z as f64);
    let len = (x * x + y * y + z * z).sqrt();
    if len == 0.0 || !len.is_finite() {
        return IDENTITY4;
    }
    let (x, y, z) = (x / len, y / len, z / len);
    let a = angle as f64;
    let sc = (a / 2.0).sin() * (a / 2.0).cos();
    let sq = (a / 2.0).sin() * (a / 2.0).sin();
    let m = [
        1.0 - 2.0 * (y * y + z * z) * sq,
        2.0 * (x * y * sq - z * sc),
        2.0 * (x * z * sq + y * sc),
        0.0,
        2.0 * (x * y * sq + z * sc),
        1.0 - 2.0 * (x * x + z * z) * sq,
        2.0 * (y * z * sq - x * sc),
        0.0,
        2.0 * (x * z * sq - y * sc),
        2.0 * (y * z * sq + x * sc),
        1.0 - 2.0 * (x * x + y * y) * sq,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ];
    // Quarter turns: exact zeros and ones (no 1e-17 residues).
    m.map(|v| {
        let r = v.round();
        if (v - r).abs() < 1e-12 { r } else { v }
    })
}

/// `perspective(d)`; `None` (or a non-positive length clamped to 1px is
/// done by the caller) is the identity.
pub fn perspective(d: Option<f32>) -> Mat4 {
    let mut m = IDENTITY4;
    if let Some(d) = d {
        let d = d.max(1.0) as f64;
        m[14] = -1.0 / d;
    }
    m
}

/// Is this a 2D affine matrix (no z or perspective terms)?
pub fn is_2d(m: &Mat4) -> bool {
    m[2] == 0.0 && m[6] == 0.0 && m[8] == 0.0 && m[9] == 0.0 && m[10] == 1.0 && m[11] == 0.0 && m[12] == 0.0 && m[13] == 0.0 && m[14] == 0.0 && m[15] == 1.0
}

/// The 2D matrix acting on the z = 0 plane when the result is flattened
/// (drops z): exact when there is no perspective term in x / y.
pub fn flatten_2d(m: &Mat4) -> Matrix {
    let w = if m[15] != 0.0 { m[15] } else { 1.0 };
    [m[0] / w, m[4] / w, m[1] / w, m[5] / w, m[3] / w, m[7] / w].map(|v| v as f32)
}

/// The projective map of the z = 0 plane: (x, y, 1) → (X, Y, W), as a
/// row-major 3×3 matrix (rows x, y, w; columns x, y, 1).
pub fn plane_homography(m: &Mat4) -> [f64; 9] {
    [m[0], m[1], m[3], m[4], m[5], m[7], m[12], m[13], m[15]]
}

/// Does the z = 0 plane map projectively (not affinely) to 2D?
pub fn is_projective(m: &Mat4) -> bool {
    m[12].abs() > 1e-12 || m[13].abs() > 1e-12 || (m[15] != 1.0 && m[15] != 0.0 && (m[15] - 1.0).abs() > 1e-9)
}

pub fn invert3(h: &[f64; 9]) -> Option<[f64; 9]> {
    let det = h[0] * (h[4] * h[8] - h[5] * h[7]) - h[1] * (h[3] * h[8] - h[5] * h[6]) + h[2] * (h[3] * h[7] - h[4] * h[6]);
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let inv = 1.0 / det;
    Some([
        (h[4] * h[8] - h[5] * h[7]) * inv,
        (h[2] * h[7] - h[1] * h[8]) * inv,
        (h[1] * h[5] - h[2] * h[4]) * inv,
        (h[5] * h[6] - h[3] * h[8]) * inv,
        (h[0] * h[8] - h[2] * h[6]) * inv,
        (h[2] * h[3] - h[0] * h[5]) * inv,
        (h[3] * h[7] - h[4] * h[6]) * inv,
        (h[1] * h[6] - h[0] * h[7]) * inv,
        (h[0] * h[4] - h[1] * h[3]) * inv,
    ])
}

fn det3(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64, i: f64) -> f64 {
    a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)
}

/// The cofactor of entry (r, c).
fn cofactor(m: &[f64; 16], r: usize, c: usize) -> f64 {
    let mut s = [0.0f64; 9];
    let mut k = 0;
    for i in 0..4 {
        if i == r {
            continue;
        }
        for j in 0..4 {
            if j == c {
                continue;
            }
            s[k] = m[i * 4 + j];
            k += 1;
        }
    }
    let d = det3(s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8]);
    if (r + c) % 2 == 0 { d } else { -d }
}

pub fn determinant(m: &Mat4) -> f64 {
    let d = *m;
    (0..4).map(|c| d[c] * cofactor(&d, 0, c)).sum()
}

/// The inverse, if the matrix is invertible.
pub fn invert(m: &Mat4) -> Option<Mat4> {
    let d = *m;
    let det: f64 = (0..4).map(|c| d[c] * cofactor(&d, 0, c)).sum();
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let mut o = [0.0f64; 16];
    for r in 0..4 {
        for c in 0..4 {
            // adjugate = transpose of the cofactor matrix
            o[r * 4 + c] = cofactor(&d, c, r) / det;
        }
    }
    Some(o)
}

/// Is the back face of a plane under `m` towards the viewer (CSS
/// Transforms 2 §backface-visibility: the z component of the transformed
/// normal, i.e. entry (2, 2) of the inverse, is negative)?
pub fn backface_showing(m: &Mat4) -> bool {
    let d = *m;
    let det: f64 = (0..4).map(|c| d[c] * cofactor(&d, 0, c)).sum();
    if det == 0.0 || !det.is_finite() {
        return false;
    }
    cofactor(&d, 2, 2) / det < 0.0
}

/// Map a point of the z = 0 plane: (x, y, z, w) before the division.
pub fn map_homogeneous(m: &Mat4, x: f64, y: f64) -> [f64; 4] {
    [m[0] * x + m[1] * y + m[3], m[4] * x + m[5] * y + m[7], m[8] * x + m[9] * y + m[11], m[12] * x + m[13] * y + m[15]]
}

/// A decomposed 3D matrix (CSS Transforms 2 §decomposing a 3D matrix).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decomposed {
    pub translate: [f64; 3],
    pub scale: [f64; 3],
    pub skew: [f64; 3],
    pub perspective: [f64; 4],
    pub quaternion: [f64; 4],
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn combine(a: [f64; 3], b: [f64; 3], sa: f64, sb: f64) -> [f64; 3] {
    [a[0] * sa + b[0] * sb, a[1] * sa + b[1] * sb, a[2] * sa + b[2] * sb]
}

/// Decompose; `None` for a singular matrix.
pub fn decompose(m: &Mat4) -> Option<Decomposed> {
    // The spec's pseudo code indexes matrix[col][row]; use that layout.
    let mut mat = [[0.0f64; 4]; 4];
    for (c, col) in mat.iter_mut().enumerate() {
        for (r, v) in col.iter_mut().enumerate() {
            *v = m[r * 4 + c];
        }
    }
    if mat[3][3] == 0.0 {
        return None;
    }
    for col in mat.iter_mut() {
        for v in col.iter_mut() {
            *v /= m[15];
        }
    }
    let mut pm = mat;
    for c in 0..3 {
        pm[c][3] = 0.0;
    }
    pm[3][3] = 1.0;
    let pm4: Mat4 = {
        let mut o = [0.0f64; 16];
        for c in 0..4 {
            for r in 0..4 {
                o[r * 4 + c] = pm[c][r];
            }
        }
        o
    };
    if determinant(&pm4).abs() < 1e-12 {
        return None;
    }
    let perspective = if mat[0][3] != 0.0 || mat[1][3] != 0.0 || mat[2][3] != 0.0 {
        let rhs = [mat[0][3], mat[1][3], mat[2][3], mat[3][3]];
        let inv = invert(&pm4)?;
        // perspective = transpose(inverse(pm)) · rhs
        let mut p = [0.0f64; 4];
        for (i, pv) in p.iter_mut().enumerate() {
            // transpose(inv)[i][k] = inv[k][i]; inv is row-major (r*4+c) with
            // r = row of the math matrix.
            *pv = (0..4).map(|k| inv[k * 4 + i] * rhs[k]).sum();
        }
        // pm is stored col-major in `mat` terms; the spec's matrix[i][3]
        // are row 3 entries of the math matrix (the bottom row).
        p
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    let translate = [mat[3][0], mat[3][1], mat[3][2]];
    let mut row = [[mat[0][0], mat[0][1], mat[0][2]], [mat[1][0], mat[1][1], mat[1][2]], [mat[2][0], mat[2][1], mat[2][2]]];
    let mut scale = [0.0f64; 3];
    let mut skew = [0.0f64; 3];
    scale[0] = dot(row[0], row[0]).sqrt();
    if scale[0] == 0.0 {
        return None;
    }
    row[0] = row[0].map(|v| v / scale[0]);
    skew[0] = dot(row[0], row[1]);
    row[1] = combine(row[1], row[0], 1.0, -skew[0]);
    scale[1] = dot(row[1], row[1]).sqrt();
    if scale[1] == 0.0 {
        return None;
    }
    row[1] = row[1].map(|v| v / scale[1]);
    skew[0] /= scale[1];
    skew[1] = dot(row[0], row[2]);
    row[2] = combine(row[2], row[0], 1.0, -skew[1]);
    skew[2] = dot(row[1], row[2]);
    row[2] = combine(row[2], row[1], 1.0, -skew[2]);
    scale[2] = dot(row[2], row[2]).sqrt();
    if scale[2] == 0.0 {
        return None;
    }
    row[2] = row[2].map(|v| v / scale[2]);
    skew[1] /= scale[2];
    skew[2] /= scale[2];
    if dot(row[0], cross(row[1], row[2])) < 0.0 {
        for i in 0..3 {
            scale[i] = -scale[i];
            row[i] = row[i].map(|v| -v);
        }
    }
    // The rotation's quaternion (x, y, z, w), from the largest of the
    // trace and the diagonal (stable for half turns, whose w is 0).
    let r = |i: usize, j: usize| row[j][i];
    let tr = r(0, 0) + r(1, 1) + r(2, 2);
    let mut q = if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        [(r(2, 1) - r(1, 2)) / s, (r(0, 2) - r(2, 0)) / s, (r(1, 0) - r(0, 1)) / s, s / 4.0]
    } else if r(0, 0) > r(1, 1) && r(0, 0) > r(2, 2) {
        let s = (1.0 + r(0, 0) - r(1, 1) - r(2, 2)).max(0.0).sqrt() * 2.0;
        [s / 4.0, (r(0, 1) + r(1, 0)) / s, (r(0, 2) + r(2, 0)) / s, (r(2, 1) - r(1, 2)) / s]
    } else if r(1, 1) > r(2, 2) {
        let s = (1.0 + r(1, 1) - r(0, 0) - r(2, 2)).max(0.0).sqrt() * 2.0;
        [(r(0, 1) + r(1, 0)) / s, s / 4.0, (r(1, 2) + r(2, 1)) / s, (r(0, 2) - r(2, 0)) / s]
    } else {
        let s = (1.0 + r(2, 2) - r(0, 0) - r(1, 1)).max(0.0).sqrt() * 2.0;
        [(r(0, 2) + r(2, 0)) / s, (r(1, 2) + r(2, 1)) / s, s / 4.0, (r(1, 0) - r(0, 1)) / s]
    };
    // w ≥ 0 like the spec's square roots; a half turn (w ≈ 0) keeps the
    // positive largest component instead of following rounding noise.
    if q[3] < -1e-6 {
        q = q.map(|v| -v);
    }
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n > 0.0 {
        q = q.map(|v| v / n);
    }
    Some(Decomposed { translate, scale, skew, perspective, quaternion: q })
}

/// Recompose a decomposed matrix (CSS Transforms 2 §recomposing to a 3D
/// matrix).
pub fn recompose(d: &Decomposed) -> Mat4 {
    // Work in the spec's matrix[col][row] layout.
    let mut mat = [[0.0f64; 4]; 4];
    for (i, col) in mat.iter_mut().enumerate() {
        col[i] = 1.0;
    }
    for i in 0..4 {
        mat[i][3] = d.perspective[i];
    }
    for i in 0..4 {
        for j in 0..3 {
            mat[3][i] += d.translate[j] * mat[j][i];
        }
    }
    let [x, y, z, w] = d.quaternion;
    let mut rot = [[0.0f64; 4]; 4];
    rot[0][0] = 1.0 - 2.0 * (y * y + z * z);
    rot[0][1] = 2.0 * (x * y - z * w);
    rot[0][2] = 2.0 * (x * z + y * w);
    rot[1][0] = 2.0 * (x * y + z * w);
    rot[1][1] = 1.0 - 2.0 * (x * x + z * z);
    rot[1][2] = 2.0 * (y * z - x * w);
    rot[2][0] = 2.0 * (x * z - y * w);
    rot[2][1] = 2.0 * (y * z + x * w);
    rot[2][2] = 1.0 - 2.0 * (x * x + y * y);
    rot[3][3] = 1.0;
    // The spec's rotation matrix is written for row vectors: transpose.
    let mut rt = [[0.0f64; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            rt[i][j] = rot[j][i];
        }
    }
    let mmul = |a: [[f64; 4]; 4], b: [[f64; 4]; 4]| -> [[f64; 4]; 4] {
        // spec: matrix = multiply(matrix, b) with col-major arrays means
        // math(a) × math(b)
        let mut o = [[0.0f64; 4]; 4];
        for c in 0..4 {
            for r in 0..4 {
                o[c][r] = (0..4).map(|k| a[k][r] * b[c][k]).sum();
            }
        }
        o
    };
    mat = mmul(mat, rt);
    let mut temp = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    if d.skew[2] != 0.0 {
        temp[2][1] = d.skew[2];
        mat = mmul(mat, temp);
    }
    if d.skew[1] != 0.0 {
        temp[2][1] = 0.0;
        temp[2][0] = d.skew[1];
        mat = mmul(mat, temp);
    }
    if d.skew[0] != 0.0 {
        temp[2][0] = 0.0;
        temp[1][0] = d.skew[0];
        mat = mmul(mat, temp);
    }
    for i in 0..3 {
        for j in 0..4 {
            mat[i][j] *= d.scale[i];
        }
    }
    let mut o = [0.0f64; 16];
    for c in 0..4 {
        for r in 0..4 {
            o[r * 4 + c] = mat[c][r];
        }
    }
    o
}

/// Spherical linear interpolation of unit quaternions.
pub fn slerp(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 4] {
    let mut product = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    product = product.clamp(-1.0, 1.0);
    if (product - 1.0).abs() < 1e-12 || product.abs() >= 1.0 {
        return a;
    }
    let theta = product.acos();
    let w = (t * theta).sin() / (1.0 - product * product).sqrt();
    let sa = (t * theta).cos() - product * w;
    [a[0] * sa + b[0] * w, a[1] * sa + b[1] * w, a[2] * sa + b[2] * w, a[3] * sa + b[3] * w]
}

/// Interpolate two matrices by decomposition (`None` when either cannot
/// be decomposed: the caller falls back to discrete animation).
pub fn interpolate(a: &Mat4, b: &Mat4, t: f32) -> Option<Mat4> {
    let da = decompose(a)?;
    let db = decompose(b)?;
    let t = t as f64;
    let l = |x: f64, y: f64| x + (y - x) * t;
    let l3 = |x: [f64; 3], y: [f64; 3]| [l(x[0], y[0]), l(x[1], y[1]), l(x[2], y[2])];
    let d = Decomposed {
        translate: l3(da.translate, db.translate),
        scale: l3(da.scale, db.scale),
        skew: l3(da.skew, db.skew),
        perspective: [l(da.perspective[0], db.perspective[0]), l(da.perspective[1], db.perspective[1]), l(da.perspective[2], db.perspective[2]), l(da.perspective[3], db.perspective[3])],
        quaternion: slerp(da.quaternion, db.quaternion, t),
    };
    Some(recompose(&d))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Mat4, b: &Mat4) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    #[test]
    fn rotation_about_z_is_2d() {
        let a = 0.3f32;
        let m = rotate(0.0, 0.0, 1.0, a);
        assert!(is_2d(&m));
        let f = flatten_2d(&m);
        assert!((f[0] - a.cos()).abs() < 1e-6 && (f[1] - a.sin()).abs() < 1e-6 && (f[2] + a.sin()).abs() < 1e-6);
    }

    #[test]
    fn rotate_x_flattens_to_vertical_scale() {
        let m = rotate(1.0, 0.0, 0.0, 60f32.to_radians());
        let f = flatten_2d(&m);
        assert!((f[0] - 1.0).abs() < 1e-6 && (f[3] - 0.5).abs() < 1e-6);
        assert!(!is_projective(&m));
    }

    #[test]
    fn inverse_round_trips() {
        let m = mul(&mul(&perspective(Some(500.0)), &rotate(1.0, 1.0, 0.0, 0.7)), &translate(10.0, -5.0, 30.0));
        let inv = invert(&m).unwrap();
        assert!(close(&mul(&m, &inv), &IDENTITY4));
    }

    #[test]
    fn backface() {
        assert!(!backface_showing(&IDENTITY4));
        assert!(backface_showing(&rotate(0.0, 1.0, 0.0, std::f32::consts::PI)));
        assert!(!backface_showing(&scale(-1.0, 1.0, 1.0)));
        assert!(backface_showing(&mul(&perspective(Some(100.0)), &rotate(1.0, 0.0, 0.0, 100f32.to_radians()))));
        assert!(!backface_showing(&mul(&perspective(Some(100.0)), &rotate(1.0, 0.0, 0.0, 80f32.to_radians()))));
    }

    #[test]
    fn perspective_projects() {
        // A point pushed 100px towards the viewer with perspective 200px
        // doubles its distance from the origin.
        let m = mul(&perspective(Some(200.0)), &translate(0.0, 0.0, 100.0));
        let p = map_homogeneous(&m, 10.0, 20.0);
        assert!((p[0] / p[3] - 20.0).abs() < 1e-4 && (p[1] / p[3] - 40.0).abs() < 1e-4);
        assert!(is_projective(&m));
    }

    #[test]
    fn css_column_major() {
        let a = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 5.0, 6.0, 7.0, 1.0];
        assert_eq!(from_css(&a), translate(5.0, 6.0, 7.0));
        assert_eq!(to_css(&translate(5.0, 6.0, 7.0)), a);
    }

    #[test]
    fn decompose_recompose() {
        for m in [
            IDENTITY4,
            translate(3.0, 4.0, 5.0),
            rotate(1.0, 2.0, 3.0, 1.1),
            mul(&translate(10.0, 0.0, -4.0), &mul(&rotate(0.0, 1.0, 0.0, 0.5), &scale(2.0, 3.0, 0.5))),
            mul(&perspective(Some(400.0)), &rotate(1.0, 0.0, 0.0, 0.4)),
            from_2d(&[1.0, 0.0, 0.5, 1.0, 0.0, 0.0]),
        ] {
            let d = decompose(&m).unwrap();
            let r = recompose(&d);
            assert!(close(&r, &m), "{m:?}\n{r:?}\n{d:?}");
        }
    }

    #[test]
    fn half_turn_quaternion_keeps_its_axis() {
        let q = decompose(&rotate(0.0, 1.0, 1.0, std::f32::consts::PI)).unwrap().quaternion;
        let s = if q[1] < 0.0 { -1.0 } else { 1.0 };
        assert!(q[0].abs() < 1e-6 && (s * q[1] - 0.5f64.sqrt()).abs() < 1e-6 && (s * q[2] - 0.5f64.sqrt()).abs() < 1e-6 && q[3].abs() < 1e-6, "{q:?}");
    }

    #[test]
    fn interpolates_rotations_by_slerp() {
        let a = rotate(1.0, 0.0, 0.0, 0.0);
        let b = rotate(1.0, 0.0, 0.0, 1.0);
        let h = interpolate(&a, &b, 0.5).unwrap();
        assert!(close(&h, &rotate(1.0, 0.0, 0.0, 0.5)), "{h:?}");
        let b = rotate(0.0, 1.0, 0.0, 1.2);
        let h = interpolate(&IDENTITY4, &b, 0.25).unwrap();
        assert!(close(&h, &rotate(0.0, 1.0, 0.0, 0.3)), "{h:?}");
    }
}
