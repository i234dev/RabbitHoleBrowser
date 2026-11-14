//! B0 experiment only: a checked backdrop-preserving group on a distinct
//! viewport surface. Nothing in the production renderer calls this module.
//!
//! The supplied mask is already in DEVICE coordinates. Its active byte count
//! is charged, but mask construction and arbitrary operations inside `draw`
//! are outside this operator's budget. Work counts only group copy/composite
//! passes plus matrix copy/comparison, not total renderer CPU/heap. No defaults.

use crate::{Canvas, Color, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupError { Dimensions, Mask, Opacity, MemoryBudget, WorkBudget, Allocation, Unbalanced, Draw }

struct Budget {
    max_live: usize,
    max_work: usize,
    live: usize,
    peak: usize,
    work: usize,
}

impl Budget {
    fn new(max_live: usize, max_work: usize) -> Self {
        Self { max_live, max_work, live: 0, peak: 0, work: 0 }
    }

    fn reserve(&mut self, bytes: usize, work: usize) -> Result<Reservation<'_>, GroupError> {
        let live = self.live.checked_add(bytes).ok_or(GroupError::MemoryBudget)?;
        let total = self.work.checked_add(work).ok_or(GroupError::WorkBudget)?;
        if live > self.max_live { return Err(GroupError::MemoryBudget); }
        if total > self.max_work { return Err(GroupError::WorkBudget); }
        self.live = live;
        self.peak = self.peak.max(live);
        self.work = total;
        Ok(Reservation { budget: self, bytes })
    }
}

struct Reservation<'a> { budget: &'a mut Budget, bytes: usize }
impl Drop for Reservation<'_> {
    fn drop(&mut self) { self.budget.live -= self.bytes; }
}

fn copied<T: Clone>(values: &[T]) -> Result<Vec<T>, GroupError> {
    let mut out = Vec::new();
    out.try_reserve_exact(values.len()).map_err(|_| GroupError::Allocation)?;
    out.extend_from_slice(values);
    Ok(out)
}

fn group(
    parent: &mut Canvas,
    budget: &mut Budget,
    device_alpha: &[u8],
    opacity: f32,
    draw: impl FnOnce(&mut Canvas, &mut Budget) -> Result<(), GroupError>,
) -> Result<(), GroupError> {
    let n = (parent.width as usize).checked_mul(parent.height as usize).ok_or(GroupError::Dimensions)?;
    if parent.width > i32::MAX as u32 || parent.height > i32::MAX as u32 || parent.data.len() != n { return Err(GroupError::Dimensions); }
    if device_alpha.len() != n { return Err(GroupError::Mask); }
    if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) { return Err(GroupError::Opacity); }
    let transform_bytes = parent.transforms.len().checked_mul(std::mem::size_of::<crate::Matrix>()).ok_or(GroupError::MemoryBudget)?;
    let bytes = n.checked_mul(5).and_then(|v| v.checked_add(transform_bytes)).ok_or(GroupError::MemoryBudget)?;
    // A matrix has six scalar components: copying and validating every
    // transform costs at most twelve steps, even for a zero-sized viewport.
    let transform_work = parent.transforms.len().checked_mul(12).ok_or(GroupError::WorkBudget)?;
    let work = n.checked_mul(3).and_then(|v| v.checked_add(transform_work)).ok_or(GroupError::WorkBudget)?;
    let reservation = budget.reserve(bytes, work)?;
    // Separate data: even clear(), shadows, filters or direct device writes
    // cannot affect the parent until successful composition. Keep backdrop
    // colors for backdrop filters. Defer the parent's clip to composition.
    let mut child = Canvas {
        width: parent.width, height: parent.height,
        data: copied(&parent.data)?, clips: Vec::new(),
        transforms: copied(&parent.transforms)?, layers: Vec::new(),
        image_smoothing: parent.image_smoothing,
    };
    draw(&mut child, &mut *reservation.budget)?;
    if child.width != parent.width || child.height != parent.height || child.data.len() != n {
        return Err(GroupError::Dimensions);
    }
    if !child.layers.is_empty() || !child.clips.is_empty() || child.transforms != parent.transforms {
        return Err(GroupError::Unbalanced);
    }
    let clip = parent.clip();
    let clip_mask = parent.current_mask();
    let width = parent.width as usize;
    // No fallible operations after the first destination write.
    for (i, (&pixel, &shape)) in child.data.iter().zip(device_alpha).enumerate() {
        let x = (i % width) as i32;
        let y = (i / width) as i32;
        if !clip.contains(x as f32 + 0.5, y as f32 + 0.5) { continue; }
        let ancestor = clip_mask.as_ref().map(|m| m.get(x, y)).unwrap_or(255);
        let coverage = (shape as u32 * ancestor as u32 + 127) / 255;
        let alpha = (coverage as f32 * opacity + 0.5) as u32;
        parent.data[i] = crate::canvas::blend(parent.data[i], Color::from_u32(pixel), alpha);
    }
    Ok(())
}

fn canvas(w: u32, h: u32, bg: Color) -> Canvas {
    let mut c = Canvas::new(w, h);
    c.clear(bg);
    c
}

fn mask(w: usize, h: usize, inside: impl Fn(usize, usize) -> u8) -> Vec<u8> {
    (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| inside(x, y)).collect()
}

#[test]
fn isolated_group_restores_overflow_ink_and_preserves_allowed_shadow_and_outline() {
    let mut parent = canvas(40, 30, Color::WHITE);
    let alpha = mask(40, 30, |x, y| if (6..28).contains(&x) && (5..25).contains(&y) { 255 } else { 0 });
    let mut budget = Budget::new(40 * 30 * 5, 40 * 30 * 3);
    group(&mut parent, &mut budget, &alpha, 1.0, |child, _| {
        // The nominal border box is 12,10,8,8; ink deliberately extends out.
        child.draw_box_shadow(Rect::new(12.0, 10.0, 8.0, 8.0), [(0.0, 0.0); 4], (0.0, 0.0), 0.0, 4.0, Color::BLACK, false);
        child.stroke_rect(Rect::new(6.0, 6.0, 24.0, 20.0), [2.0; 4], [Color::rgb(0, 0, 255); 4], [3; 4], 0.0);
        child.fill_rect(Rect::new(12.0, 10.0, 8.0, 8.0), Color::rgb(255, 0, 0));
        child.fill_rect(Rect::new(0.0, 0.0, 40.0, 3.0), Color::BLACK);
        Ok(())
    }).unwrap();
    assert_eq!(parent.data[10 * 40 + 9], Color::BLACK.to_u32(), "shadow outside border survives inside clip");
    assert_eq!(parent.data[14 * 40 + 7], 0x0000ff, "outline pixel outside both border and shadow survives");
    assert_eq!(parent.data[14 * 40 + 15], Color::rgb(255, 0, 0).to_u32());
    assert_eq!(parent.data[1], Color::WHITE.to_u32(), "overflow writes outside clip restored");
    assert_eq!(budget.live, 0);
}

#[test]
fn device_mask_clips_transformed_ink_in_physical_coordinates() {
    let mut parent = canvas(30, 20, Color::WHITE);
    parent.push_transform([0.0, 1.0, -1.0, 0.0, 25.0, 2.0]);
    let alpha = mask(30, 20, |x, _| if x < 20 { 255 } else { 0 });
    let mut budget = Budget::new(30 * 20 * 5 + std::mem::size_of::<crate::Matrix>(), 30 * 20 * 3 + 12);
    group(&mut parent, &mut budget, &alpha, 1.0, |child, _| {
        child.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::rgb(255, 0, 0));
        Ok(())
    }).unwrap();
    assert_eq!(parent.data[5 * 30 + 18], Color::rgb(255, 0, 0).to_u32());
    assert_eq!(parent.data[5 * 30 + 22], Color::WHITE.to_u32());
    assert_eq!(parent.transform(), [0.0, 1.0, -1.0, 0.0, 25.0, 2.0]);
}

#[test]
fn backdrop_filter_is_confined_to_the_final_group_mask() {
    let mut parent = canvas(30, 16, Color::WHITE);
    parent.fill_rect(Rect::new(0.0, 0.0, 15.0, 16.0), Color::BLACK);
    let before = parent.data.clone();
    let alpha = mask(30, 16, |x, _| if x < 15 { 255 } else { 0 });
    let mut budget = Budget::new(30 * 16 * 5, 30 * 16 * 3);
    group(&mut parent, &mut budget, &alpha, 1.0, |child, _| {
        child.blur_region(Rect::new(0.0, 0.0, 30.0, 16.0), 2.0);
        Ok(())
    }).unwrap();
    assert_ne!(parent.data[8 * 30 + 14], before[8 * 30 + 14], "blur reads actual backdrop");
    for y in 0..16 { for x in 15..30 { assert_eq!(parent.data[y * 30 + x], before[y * 30 + x], "outside destination restored"); } }
}

#[test]
fn nested_groups_multiply_coverage_once_and_restore_each_background() {
    let mut parent = canvas(8, 8, Color::WHITE);
    let outer = vec![128; 64];
    let inner = mask(8, 8, |x, _| if x < 4 { 128 } else { 0 });
    let mut budget = Budget::new(8 * 8 * 5 * 2, 8 * 8 * 3 * 2);
    group(&mut parent, &mut budget, &outer, 1.0, |child, budget| {
        group(child, budget, &inner, 1.0, |nested, _| {
            nested.clear(Color::BLACK);
            Ok(())
        })
    }).unwrap();
    let once = crate::canvas::blend(Color::WHITE.to_u32(), Color::BLACK, 128);
    let expected = crate::canvas::blend(Color::WHITE.to_u32(), Color::from_u32(once), 128);
    assert_eq!(parent.data[0], expected);
    assert_eq!(parent.data[0], 0xbfbfbf, "two half-coverages over white produce RGB191,191,191");
    assert_eq!(parent.data[7], Color::WHITE.to_u32());
    assert_eq!(budget.live, 0);
    assert_eq!(budget.peak, 8 * 8 * 5 * 2);
}

#[test]
fn overlap_opacity_and_ancestor_clip_apply_at_composition_once() {
    let mut parent = canvas(12, 12, Color::WHITE);
    parent.push_clip_rounded(Rect::new(1.0, 1.0, 10.0, 10.0), [(4.0, 4.0); 4]);
    let ancestor = parent.current_mask().unwrap();
    let alpha = vec![128; 144];
    let mut budget = Budget::new(12 * 12 * 5, 12 * 12 * 3);
    group(&mut parent, &mut budget, &alpha, 0.5, |child, _| {
        child.fill_rect(Rect::new(0.0, 0.0, 12.0, 12.0), Color::BLACK);
        child.fill_rect(Rect::new(0.0, 0.0, 12.0, 12.0), Color::BLACK);
        Ok(())
    }).unwrap();
    for y in 0..12 { for x in 0..12 {
        let coverage = (128 * ancestor.get(x, y) as u32 + 127) / 255;
        let a = (coverage as f32 * 0.5 + 0.5) as u32;
        let expected = crate::canvas::blend(Color::WHITE.to_u32(), Color::BLACK, a);
        assert_eq!(parent.data[y as usize * 12 + x as usize], expected);
    } }
    assert_eq!(parent.clips.len(), 1);
    assert_eq!(parent.data[6 * 12 + 6], 0xbfbfbf, "overlap does not apply shape coverage twice");
}

#[test]
fn nested_memory_rejection_rolls_back_the_whole_outer_group() {
    let mut parent = canvas(4, 4, Color::WHITE);
    let before = parent.data.clone();
    let alpha = vec![255; 16];
    let mut budget = Budget::new(159, 1000);
    assert_eq!(group(&mut parent, &mut budget, &alpha, 1.0, |child, budget| {
        child.clear(Color::BLACK);
        group(child, budget, &alpha, 1.0, |_, _| panic!("nested reservation must fail"))
    }), Err(GroupError::MemoryBudget));
    assert_eq!(parent.data, before);
    assert_eq!(budget.live, 0);
    assert_eq!(budget.work, 48);
}

#[test]
fn failures_unbalanced_state_and_panic_never_modify_destination() {
    let mut parent = canvas(4, 4, Color::WHITE);
    let alpha = vec![255; 16];
    let before = parent.data.clone();
    let mut budget = Budget::new(160, 1000);
    assert_eq!(group(&mut parent, &mut budget, &alpha, 1.0, |child, _| { child.clear(Color::rgb(255, 0, 0)); Err(GroupError::Draw) }), Err(GroupError::Draw));
    assert_eq!(parent.data, before);
    assert_eq!(group(&mut parent, &mut budget, &alpha, 1.0, |child, _| { child.push_clip(Rect::new(0.0, 0.0, 1.0, 1.0)); Ok(()) }), Err(GroupError::Unbalanced));
    assert_eq!(parent.data, before);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = group(&mut parent, &mut budget, &alpha, 1.0, |child, _| { child.clear(Color::rgb(255, 0, 0)); panic!("group abort") });
    }));
    assert!(result.is_err());
    assert_eq!(parent.data, before);
    assert_eq!(budget.live, 0);
}

#[test]
fn explicit_budgets_reject_before_drawing_and_work_is_not_refunded() {
    let mut parent = canvas(4, 4, Color::WHITE);
    let before = parent.data.clone();
    let alpha = vec![255; 16];
    let mut memory = Budget::new(79, 1000);
    assert_eq!(group(&mut parent, &mut memory, &alpha, 1.0, |_, _| panic!("must preflight")), Err(GroupError::MemoryBudget));
    assert_eq!(memory.live, 0);
    assert_eq!(memory.work, 0);
    let mut work = Budget::new(80, 47);
    assert_eq!(group(&mut parent, &mut work, &alpha, 1.0, |_, _| panic!("must preflight")), Err(GroupError::WorkBudget));
    let mut exact = Budget::new(80, 48);
    group(&mut parent, &mut exact, &alpha, 1.0, |_, _| Ok(())).unwrap();
    assert_eq!(exact.live, 0);
    assert_eq!(exact.work, 48);
    assert_eq!(group(&mut parent, &mut exact, &alpha, 1.0, |_, _| panic!("cumulative work")), Err(GroupError::WorkBudget));
    assert_eq!(parent.data, before);
}

#[test]
fn transform_copy_and_comparison_work_is_preflighted_even_without_pixels() {
    for side in [0u32, 1] {
        let mut parent = canvas(side, side, Color::WHITE);
        for _ in 0..8 { parent.push_transform(crate::IDENTITY); }
        let before = parent.data.clone();
        let n = side as usize * side as usize;
        let alpha = vec![255; n];
        let bytes = n * 5 + 8 * std::mem::size_of::<crate::Matrix>();
        let mut short = Budget::new(bytes, n * 3);
        assert_eq!(group(&mut parent, &mut short, &alpha, 1.0, |_, _| panic!("matrix work must be charged before draw")), Err(GroupError::WorkBudget));
        assert_eq!(short.live, 0);
        assert_eq!(short.work, 0);
        let mut exact = Budget::new(bytes, n * 3 + 8 * 12);
        group(&mut parent, &mut exact, &alpha, 1.0, |_, _| Ok(())).unwrap();
        assert_eq!(exact.work, n * 3 + 8 * 12);
        assert_eq!(exact.live, 0);
        assert_eq!(parent.data, before);
    }
}
