//! Checked, backdrop-preserving RGB groups. No renderer uses this API yet.
//!
//! Limits are explicit: they cover this operator's surfaces, masks, matrix
//! operations and coverage samples, not arbitrary work/allocations in `draw`.
//! A group paints on a distinct viewport surface and publishes pixels only
//! after success. This is not a general transparent RGBA blend-group API.
//! Operational callers must forward the same budget to nested groups. A Rust
//! callback can do unrelated work with another policy; this API cannot impose
//! a global quota on that work or detect a temporary handle swap restored
//! before return. A different final handle is rejected before publication.

use crate::{Canvas, Color, Matrix, Radii, Rect};
use crate::raster::RoundedBox;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupLimits {
    pub max_live_bytes: usize,
    /// Admission units: one/group, pixel passes, coverage samples, and
    /// scalar matrix validation/copy/comparison. Not CPU instructions.
    pub max_work_units: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupStats {
    pub live_bytes: usize,
    pub peak_bytes: usize,
    pub work_units: usize,
}

pub struct GroupBudget {
    state: Rc<BudgetState>,
}

struct BudgetState {
    limits: GroupLimits,
    stats: Cell<GroupStats>,
}

impl GroupBudget {
    pub fn new(limits: GroupLimits) -> Self {
        // One fixed policy/counter allocation per configured budget. Variable
        // surface/mask storage is admitted separately by each operation.
        Self { state: Rc::new(BudgetState { limits, stats: Cell::new(GroupStats { live_bytes: 0, peak_bytes: 0, work_units: 0 }) }) }
    }

    pub fn stats(&self) -> GroupStats { self.state.stats.get() }

    fn reserve(&mut self, bytes: usize, work: usize) -> Result<Reservation, GroupError> {
        let before = self.stats();
        let live = before.live_bytes.checked_add(bytes).ok_or(GroupError::MemoryLimit)?;
        let total = before.work_units.checked_add(work).ok_or(GroupError::WorkLimit)?;
        if live > self.state.limits.max_live_bytes { return Err(GroupError::MemoryLimit); }
        if total > self.state.limits.max_work_units { return Err(GroupError::WorkLimit); }
        self.state.stats.set(GroupStats { live_bytes: live, peak_bytes: before.peak_bytes.max(live), work_units: total });
        Ok(Reservation { state: Rc::clone(&self.state), bytes })
    }
}

struct Reservation { state: Rc<BudgetState>, bytes: usize }
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut stats = self.state.stats.get();
        stats.live_bytes -= self.bytes;
        self.state.stats.set(stats);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupError {
    InvalidCanvas,
    InvalidMask,
    InvalidOpacity,
    InvalidGeometry,
    InvalidTransform,
    MemoryLimit,
    WorkLimit,
    Allocation,
    UnbalancedState,
    BudgetChanged,
}

impl std::fmt::Display for GroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "checked graphics group: {self:?}") }
}
impl std::error::Error for GroupError {}

#[derive(Debug, PartialEq, Eq)]
pub enum GroupFailure<E> {
    Group(GroupError),
    Draw(E),
}

impl<E> From<GroupError> for GroupFailure<E> {
    fn from(error: GroupError) -> Self { Self::Group(error) }
}

/// Validated local geometry shared by mask construction and hit containment.
/// Ellipses use their bounding rectangle with four equal `(rx,ry)` corners.
#[derive(Clone, Copy)]
pub struct GroupShape { rounded: RoundedBox }

impl std::fmt::Debug for GroupShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroupShape").field("rect", &self.rounded.r).field("radii", &self.rounded.radii).finish()
    }
}

fn checked_f32(value: f64, error: GroupError) -> Result<f32, GroupError> {
    if !value.is_finite() || value.abs() > f32::MAX as f64 { return Err(error); }
    let out = value as f32;
    if value != 0.0 && out == 0.0 { return Err(error); }
    Ok(out)
}

impl GroupShape {
    pub fn new(rect: Rect, radii: Radii) -> Result<Self, GroupError> {
        if ![rect.x, rect.y, rect.w, rect.h].iter().all(|n| n.is_finite()) || rect.w < 0.0 || rect.h < 0.0 {
            return Err(GroupError::InvalidGeometry);
        }
        checked_f32(rect.x as f64 + rect.w as f64, GroupError::InvalidGeometry)?;
        checked_f32(rect.y as f64 + rect.h as f64, GroupError::InvalidGeometry)?;
        if radii.iter().any(|(x, y)| !x.is_finite() || !y.is_finite() || *x < 0.0 || *y < 0.0) {
            return Err(GroupError::InvalidGeometry);
        }
        // clamp_radii performs these sums in f32: reject overflow before it
        // could turn an overflowing radius sum into zero-sized corners.
        for (a, b) in [(radii[0].0, radii[1].0), (radii[3].0, radii[2].0), (radii[0].1, radii[3].1), (radii[1].1, radii[2].1)] {
            checked_f32(a as f64 + b as f64, GroupError::InvalidGeometry)?;
        }
        Ok(Self { rounded: RoundedBox::new(rect, radii) })
    }

    pub fn bounds(&self) -> Rect { self.rounded.r }
    pub fn radii(&self) -> Radii { self.rounded.radii }
    pub fn contains(&self, x: f32, y: f32) -> bool { self.rounded.contains(x, y) }
}

/// An explicit mask source. `Shape` owns construction of its alpha after
/// admission; `Device` charges active borrowed storage but not its allocation.
#[derive(Clone, Copy)]
pub enum GroupMask<'a> {
    Device(&'a [u8]),
    Shape { shape: GroupShape, local_to_device: Matrix },
}

/// Unlike the legacy path helper, a small nonzero determinant remains
/// invertible. Unrepresentable derived values fail instead of using identity.
fn checked_inverse(m: &Matrix) -> Result<Option<Matrix>, GroupError> {
    if !m.iter().all(|n| n.is_finite()) { return Err(GroupError::InvalidTransform); }
    let [a, b, c, d, e, f] = m.map(|v| v as f64);
    let det = a * d - b * c;
    if det == 0.0 { return Ok(None); }
    let values = [d / det, -b / det, -c / det, a / det, (c * f - d * e) / det, (b * e - a * f) / det];
    let mut inverse = [0.0; 6];
    for (out, value) in inverse.iter_mut().zip(values) { *out = checked_f32(value, GroupError::InvalidTransform)?; }
    Ok(Some(inverse))
}

fn mapped(m: &Matrix, x: f32, y: f32) -> Result<(f32, f32), GroupError> {
    let px = m[0] as f64 * x as f64 + m[2] as f64 * y as f64 + m[4] as f64;
    let py = m[1] as f64 * x as f64 + m[3] as f64 * y as f64 + m[5] as f64;
    Ok((checked_f32(px, GroupError::InvalidTransform)?, checked_f32(py, GroupError::InvalidTransform)?))
}

fn copy<T: Clone>(values: &[T]) -> Result<Vec<T>, GroupError> {
    let mut out = Vec::new();
    out.try_reserve_exact(values.len()).map_err(|_| GroupError::Allocation)?;
    out.extend_from_slice(values);
    Ok(out)
}

fn shape_alpha(shape: GroupShape, m: Matrix, width: u32, height: u32) -> Result<Vec<u8>, GroupError> {
    let n = width as usize * height as usize; // checked by with_group
    let inverse = checked_inverse(&m)?;
    let r = shape.bounds();
    let device_shape = if let Some(inverse) = inverse {
        for (x, y) in [(r.x, r.y), (r.right(), r.y), (r.x, r.bottom()), (r.right(), r.bottom())] { mapped(&m, x, y)?; }
        // Affine extrema lie at corners: validate derived viewport values
        // before allocation, not after a partially constructed mask.
        for (x, y) in [(0.0, 0.0), (width as f32, 0.0), (0.0, height as f32), (width as f32, height as f32)] { mapped(&inverse, x, y)?; }
        let aligned = m[1] == 0.0 && m[2] == 0.0 && m[0] > 0.0 && m[3] > 0.0;
        if aligned {
            let (x, y) = mapped(&m, r.x, r.y)?;
            let w = checked_f32(r.w as f64 * m[0] as f64, GroupError::InvalidTransform)?;
            let h = checked_f32(r.h as f64 * m[3] as f64, GroupError::InvalidTransform)?;
            let mut radii = shape.radii();
            for (rx, ry) in &mut radii {
                *rx = checked_f32(*rx as f64 * m[0] as f64, GroupError::InvalidTransform)?;
                *ry = checked_f32(*ry as f64 * m[3] as f64, GroupError::InvalidTransform)?;
            }
            Some(GroupShape::new(Rect::new(x, y, w, h), radii)?)
        } else { None }
    } else { None };
    let mut alpha = Vec::new();
    alpha.try_reserve_exact(n).map_err(|_| GroupError::Allocation)?;
    alpha.resize(n, 0);
    let Some(inverse) = inverse else { return Ok(alpha) };
    // A zero width with a huge height otherwise iterates empty rows for work
    // charged as zero pixels. Keep validation above, but do no row traversal.
    if n == 0 || r.is_empty() { return Ok(alpha); }
    for y in 0..height { for x in 0..width {
        let coverage = if let Some(device) = device_shape {
            device.rounded.coverage(x as i32, y as i32)
        } else {
            let mut hits = 0;
            for sy in 0..4 { for sx in 0..4 {
                let (lx, ly) = mapped(&inverse, x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0)?;
                if shape.contains(lx, ly) { hits += 1; }
            } }
            hits as f32 / 16.0
        };
        alpha[y as usize * width as usize + x as usize] = (coverage * 255.0 + 0.5) as u8;
    } }
    Ok(alpha)
}

/// Draw into a private viewport copy, then apply coverage once. Errors and
/// unwinding never publish partial pixels or mutate the parent's stacks.
pub fn with_group<E>(
    parent: &mut Canvas,
    budget: &mut GroupBudget,
    mask: GroupMask<'_>,
    opacity: f32,
    draw: impl FnOnce(&mut Canvas, &mut GroupBudget) -> Result<(), E>,
) -> Result<(), GroupFailure<E>> {
    let n = (parent.width as usize).checked_mul(parent.height as usize).ok_or(GroupError::InvalidCanvas)?;
    if parent.width > i32::MAX as u32 || parent.height > i32::MAX as u32 || parent.data.len() != n { return Err(GroupError::InvalidCanvas.into()); }
    if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) { return Err(GroupError::InvalidOpacity.into()); }
    if let GroupMask::Device(alpha) = mask { if alpha.len() != n { return Err(GroupError::InvalidMask.into()); } }
    let matrices = parent.transforms.len();
    let matrix_bytes = matrices.checked_mul(std::mem::size_of::<Matrix>()).ok_or(GroupError::MemoryLimit)?;
    let bytes = n.checked_mul(5).and_then(|v| v.checked_add(matrix_bytes)).ok_or(GroupError::MemoryLimit)?;
    // At most 16 geometry sample predicates per pixel, one mask fill and
    // three copy/composite passes. Each matrix is validated/copied/compared.
    let pixel_units = if matches!(mask, GroupMask::Shape { .. }) { 20 } else { 3 };
    let matrix_work = matrices.checked_mul(18).ok_or(GroupError::WorkLimit)?;
    let work = n.checked_mul(pixel_units).and_then(|v| v.checked_add(matrix_work)).and_then(|v| v.checked_add(1)).ok_or(GroupError::WorkLimit)?;
    let reservation = budget.reserve(bytes, work)?;
    if parent.transforms.iter().any(|m| m.iter().any(|n| !n.is_finite())) { return Err(GroupError::InvalidTransform.into()); }
    let owned_alpha;
    let alpha = match mask {
        GroupMask::Device(alpha) => alpha,
        GroupMask::Shape { shape, local_to_device } => {
            owned_alpha = shape_alpha(shape, local_to_device, parent.width, parent.height)?;
            &owned_alpha
        }
    };
    let mut child = Canvas {
        width: parent.width, height: parent.height, data: copy(&parent.data)?,
        clips: Vec::new(), transforms: copy(&parent.transforms)?, layers: Vec::new(), image_smoothing: parent.image_smoothing,
    };
    let result = draw(&mut child, budget);
    // The callback may replace its public budget handle. Never refund that
    // new handle or lose the original live reservation. A different final
    // handle is rejected; temporary unrelated policies remain the caller's
    // responsibility. The original state survives even during unwinding.
    if !Rc::ptr_eq(&reservation.state, &budget.state) { return Err(GroupError::BudgetChanged.into()); }
    result.map_err(GroupFailure::Draw)?;
    if child.width != parent.width || child.height != parent.height || child.data.len() != n { return Err(GroupError::InvalidCanvas.into()); }
    if !child.clips.is_empty() || !child.layers.is_empty() || child.transforms != parent.transforms { return Err(GroupError::UnbalancedState.into()); }
    let clip = parent.clip();
    let ancestor = parent.current_mask();
    let width = parent.width as usize;
    // Everything after the first destination write is infallible.
    for (i, (&pixel, &shape)) in child.data.iter().zip(alpha).enumerate() {
        let x = (i % width) as i32;
        let y = (i / width) as i32;
        if !clip.contains(x as f32 + 0.5, y as f32 + 0.5) { continue; }
        let a = ancestor.as_ref().map(|m| m.get(x, y)).unwrap_or(255);
        let coverage = (shape as u32 * a as u32 + 127) / 255;
        let coverage = (coverage as f32 * opacity + 0.5) as u32;
        parent.data[i] = crate::canvas::blend(parent.data[i], Color::from_u32(pixel), coverage);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
