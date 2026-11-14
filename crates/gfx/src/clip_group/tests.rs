use super::*;

fn target(w: u32, h: u32) -> Canvas {
    let mut c = Canvas::new(w, h);
    c.clear(Color::WHITE);
    c
}

fn limits(bytes: usize, work: usize) -> GroupBudget {
    GroupBudget::new(GroupLimits { max_live_bytes: bytes, max_work_units: work })
}

fn generous() -> GroupBudget { limits(1_000_000, 1_000_000) }

fn rectangle(r: Rect) -> GroupShape { GroupShape::new(r, [(0.0, 0.0); 4]).unwrap() }

fn black(c: &mut Canvas, _: &mut GroupBudget) -> Result<(), &'static str> {
    c.clear(Color::BLACK);
    Ok(())
}

#[test]
fn owned_masks_match_existing_identity_and_positive_scale_coverage() {
    let local = GroupShape::new(Rect::new(1.25, 1.5, 7.0, 5.0), [(2.0, 1.0); 4]).unwrap();
    for m in [crate::IDENTITY, [2.0, 0.0, 0.0, 2.0, 1.0, 1.0]] {
        let mut parent = target(22, 18);
        let mut budget = generous();
        with_group(&mut parent, &mut budget, GroupMask::Shape { shape: local, local_to_device: m }, 1.0, black).unwrap();
        let expected = RoundedBox::new(Rect::new(1.25 * m[0] + m[4], 1.5 * m[3] + m[5], 7.0 * m[0], 5.0 * m[3]), [(2.0 * m[0], 1.0 * m[3]); 4]);
        for y in 0..18 { for x in 0..22 {
            let a = (expected.coverage(x, y) * 255.0 + 0.5) as u32;
            assert_eq!(parent.data[y as usize * 22 + x as usize], crate::canvas::blend(Color::WHITE.to_u32(), Color::BLACK, a));
        } }
        assert_eq!(budget.stats().live_bytes, 0);
    }
}

#[test]
fn physical_rotation_and_reflection_preserve_asymmetric_bounds() {
    let shape = rectangle(Rect::new(2.0, 3.0, 4.0, 8.0));
    for (m, expected) in [
        ([0.0, 1.0, -1.0, 0.0, 16.0, 0.0], Rect::new(5.0, 2.0, 8.0, 4.0)),
        ([-1.0, 0.0, 0.0, 1.0, 16.0, 0.0], Rect::new(10.0, 3.0, 4.0, 8.0)),
    ] {
        let mut parent = target(18, 16);
        with_group(&mut parent, &mut generous(), GroupMask::Shape { shape, local_to_device: m }, 1.0, black).unwrap();
        for y in 0..16 { for x in 0..18 {
            let pixel = if expected.contains(x as f32 + 0.5, y as f32 + 0.5) { 0 } else { 0xffffff };
            assert_eq!(parent.data[y * 18 + x], pixel, "{m:?} at {x},{y}");
        } }
    }
}

#[test]
fn small_invertible_matrix_is_not_identity_and_singular_is_empty() {
    let shape = rectangle(Rect::new(0.0, 0.0, 20_000_000.0, 20_000_000.0));
    let tiny = [0.0, 0.0000001, -0.0000001, 0.0, 2.0, 0.0];
    let mut parent = target(4, 4);
    with_group(&mut parent, &mut generous(), GroupMask::Shape { shape, local_to_device: tiny }, 1.0, black).unwrap();
    assert_eq!(parent.data[0], 0x000000);
    assert_eq!(parent.data[15], 0xffffff, "identity fallback would incorrectly cover the whole viewport");
    let mut singular = target(4, 4);
    with_group(&mut singular, &mut generous(), GroupMask::Shape { shape, local_to_device: [0.0; 6] }, 1.0, black).unwrap();
    assert!(singular.data.iter().all(|p| *p == 0xffffff));
}

#[test]
fn nonfinite_and_derived_geometry_overflow_fail_without_drawing() {
    assert!(matches!(GroupShape::new(Rect::new(f32::MAX, 0.0, f32::MAX, 1.0), [(0.0, 0.0); 4]), Err(GroupError::InvalidGeometry)));
    assert!(matches!(GroupShape::new(Rect::new(0.0, 0.0, 1.0, 1.0), [(f32::MAX, 1.0); 4]), Err(GroupError::InvalidGeometry)));
    for n in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(matches!(GroupShape::new(Rect::new(n, 0.0, 1.0, 1.0), [(0.0, 0.0); 4]), Err(GroupError::InvalidGeometry)));
    }
    let shape = rectangle(Rect::new(2.0, 2.0, 2.0, 2.0));
    for m in [[f32::NAN, 0.0, 0.0, 1.0, 0.0, 0.0], [f32::MAX, 0.0, 0.0, f32::MAX, 0.0, 0.0], [f32::MIN_POSITIVE, 0.0, 0.0, f32::MIN_POSITIVE, f32::MAX, f32::MAX]] {
        let mut parent = target(4, 4);
        let before = parent.data.clone();
        let result = with_group::<()>(&mut parent, &mut generous(), GroupMask::Shape { shape, local_to_device: m }, 1.0, |_, _| panic!("invalid derived transform must not draw"));
        assert_eq!(result, Err(GroupFailure::Group(GroupError::InvalidTransform)));
        assert_eq!(parent.data, before);
    }
    assert!(!shape.contains(f32::NAN, 2.0));
    assert!(!shape.contains(2.0, f32::INFINITY));
}

#[test]
fn owned_mask_admission_and_matrix_work_precede_all_drawing() {
    let shape = rectangle(Rect::new(0.0, 0.0, 4.0, 4.0));
    let mut parent = target(4, 4);
    let mask = GroupMask::Shape { shape, local_to_device: crate::IDENTITY };
    let mut memory = limits(79, 1000);
    assert_eq!(with_group::<()>(&mut parent, &mut memory, mask, 1.0, |_, _| panic!("memory preflight")), Err(GroupFailure::Group(GroupError::MemoryLimit)));
    assert_eq!(memory.stats().work_units, 0);
    let mut work = limits(80, 320);
    assert_eq!(with_group::<()>(&mut parent, &mut work, mask, 1.0, |_, _| panic!("work preflight")), Err(GroupFailure::Group(GroupError::WorkLimit)));
    let mut exact = limits(80, 321);
    with_group(&mut parent, &mut exact, mask, 1.0, black).unwrap();
    assert!(parent.data.iter().all(|p| *p == 0));
    assert_eq!(exact.stats(), GroupStats { live_bytes: 0, peak_bytes: 80, work_units: 321 });
    for side in [0u32, 1] {
        let mut parent = target(side, side);
        for _ in 0..8 { parent.push_transform(crate::IDENTITY); }
        let n = side as usize * side as usize;
        let alpha = vec![255; n];
        let bytes = n * 5 + 8 * std::mem::size_of::<Matrix>();
        let mut insufficient = limits(bytes, 1 + n * 3);
        assert_eq!(with_group::<()>(&mut parent, &mut insufficient, GroupMask::Device(&alpha), 1.0, |_, _| panic!("matrix work preflight")), Err(GroupFailure::Group(GroupError::WorkLimit)));
        let mut exact = limits(bytes, 1 + n * 3 + 8 * 18);
        with_group(&mut parent, &mut exact, GroupMask::Device(&alpha), 1.0, black).unwrap();
        assert_eq!(exact.stats().live_bytes, 0);
        assert_eq!(exact.stats().work_units, 1 + n * 3 + 8 * 18);
    }
}

#[test]
fn completed_group_keeps_overflow_shadow_and_independent_outline_ink() {
    let mut parent = target(40, 30);
    let mut alpha = vec![0; 40 * 30];
    for y in 5..25 { for x in 6..28 { alpha[y * 40 + x] = 255; } }
    with_group(&mut parent, &mut generous(), GroupMask::Device(&alpha), 1.0, |child, _| {
        child.draw_box_shadow(Rect::new(12.0, 10.0, 8.0, 8.0), [(0.0, 0.0); 4], (0.0, 0.0), 0.0, 4.0, Color::BLACK, false);
        child.stroke_rect(Rect::new(6.0, 6.0, 24.0, 20.0), [2.0; 4], [Color::rgb(0, 0, 255); 4], [0; 4], 0.0);
        child.fill_rect(Rect::new(12.0, 10.0, 8.0, 8.0), Color::rgb(255, 0, 0));
        child.fill_rect(Rect::new(0.0, 0.0, 40.0, 3.0), Color::BLACK);
        Ok::<_, ()>(())
    }).unwrap();
    assert_eq!(parent.data[10 * 40 + 9], 0x000000, "shadow only");
    assert_eq!(parent.data[14 * 40 + 7], 0x0000ff, "outline only");
    assert_eq!(parent.data[14 * 40 + 15], 0xff0000, "border-box content");
    assert_eq!(parent.data[1], 0xffffff, "overflow outside mask was never published");
}

#[test]
fn backdrop_filter_reads_surroundings_but_only_publishes_masked_pixels() {
    let mut parent = target(30, 16);
    parent.fill_rect(Rect::new(0.0, 0.0, 15.0, 16.0), Color::BLACK);
    let before = parent.data.clone();
    let mut alpha = vec![0; 30 * 16];
    for y in 0..16 { alpha[y * 30..y * 30 + 15].fill(255); }
    with_group(&mut parent, &mut generous(), GroupMask::Device(&alpha), 1.0, |child, _| {
        child.blur_region(Rect::new(0.0, 0.0, 30.0, 16.0), 2.0);
        Ok::<_, ()>(())
    }).unwrap();
    assert_ne!(parent.data[8 * 30 + 14], before[8 * 30 + 14]);
    for y in 0..16 { for x in 15..30 { assert_eq!(parent.data[y * 30 + x], before[y * 30 + x]); } }
}

#[test]
fn nested_coverage_and_overlapping_draws_compose_once_with_opacity() {
    let mut parent = target(8, 8);
    let alpha = vec![128; 64];
    let mut budget = limits(640, 386);
    with_group(&mut parent, &mut budget, GroupMask::Device(&alpha), 1.0, |child, budget| {
        with_group(child, budget, GroupMask::Device(&alpha), 1.0, black).map_err(|_| "inner group")
    }).unwrap();
    assert!(parent.data.iter().all(|p| *p == 0xbfbfbf));
    assert_eq!(budget.stats().peak_bytes, 640);
    assert_eq!(budget.stats().live_bytes, 0);
    let mut parent = target(12, 12);
    parent.push_clip_rounded(Rect::new(1.0, 1.0, 10.0, 10.0), [(4.0, 4.0); 4]);
    let alpha = vec![128; 144];
    with_group(&mut parent, &mut generous(), GroupMask::Device(&alpha), 0.5, |child, _| {
        child.fill_rect(Rect::new(0.0, 0.0, 12.0, 12.0), Color::BLACK);
        child.fill_rect(Rect::new(0.0, 0.0, 12.0, 12.0), Color::BLACK);
        Ok::<_, ()>(())
    }).unwrap();
    assert_eq!(parent.data[6 * 12 + 6], 0xbfbfbf);
    assert_eq!(parent.data[0], 0xffffff);
    assert_eq!(parent.clips.len(), 1);
}

#[test]
fn draw_failure_nested_limits_and_unwind_preserve_destination() {
    let mut parent = target(4, 4);
    let before = parent.data.clone();
    let alpha = vec![255; 16];
    let mut budget = generous();
    let error = with_group(&mut parent, &mut budget, GroupMask::Device(&alpha), 1.0, |child, _| { child.clear(Color::BLACK); Err("draw failed") });
    assert_eq!(error, Err(GroupFailure::Draw("draw failed")));
    assert_eq!(parent.data, before);
    let mut nested = limits(159, 1000);
    let error = with_group(&mut parent, &mut nested, GroupMask::Device(&alpha), 1.0, |child, budget| {
        child.clear(Color::BLACK);
        with_group(child, budget, GroupMask::Device(&alpha), 1.0, black).map_err(|_| "inner rejected")
    });
    assert_eq!(error, Err(GroupFailure::Draw("inner rejected")));
    assert_eq!(parent.data, before);
    assert_eq!(nested.stats().live_bytes, 0);
    assert_eq!(nested.stats().work_units, 49);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = with_group::<()>(&mut parent, &mut budget, GroupMask::Device(&alpha), 1.0, |child, _| { child.clear(Color::BLACK); panic!("abort group") });
    }));
    assert!(panic.is_err());
    assert_eq!(parent.data, before);
    assert_eq!(budget.stats().live_bytes, 0);
}

#[test]
fn malformed_inputs_or_unbalanced_canvas_are_explicit_errors() {
    let mut parent = target(4, 4);
    let before = parent.data.clone();
    let alpha = vec![255; 16];
    for opacity in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        assert_eq!(with_group::<()>(&mut parent, &mut generous(), GroupMask::Device(&alpha), opacity, |_, _| panic!("invalid opacity")), Err(GroupFailure::Group(GroupError::InvalidOpacity)));
    }
    assert_eq!(with_group::<()>(&mut parent, &mut generous(), GroupMask::Device(&[]), 1.0, |_, _| panic!("mask length")), Err(GroupFailure::Group(GroupError::InvalidMask)));
    let unbalanced = with_group(&mut parent, &mut generous(), GroupMask::Device(&alpha), 1.0, |child, _| {
        child.clear(Color::BLACK);
        child.push_clip(Rect::new(0.0, 0.0, 1.0, 1.0));
        Ok::<_, ()>(())
    });
    assert_eq!(unbalanced, Err(GroupFailure::Group(GroupError::UnbalancedState)));
    assert_eq!(parent.data, before);
    let invalid = with_group(&mut parent, &mut generous(), GroupMask::Device(&alpha), 1.0, |child, _| { child.width = 3; Ok::<_, ()>(()) });
    assert_eq!(invalid, Err(GroupFailure::Group(GroupError::InvalidCanvas)));
    assert_eq!(parent.data, before);
}

#[test]
fn replacing_budget_handle_cannot_publish_or_corrupt_live_reservations() {
    let mut parent = target(4, 4);
    let before = parent.data.clone();
    let alpha = vec![255; 16];
    let mut budget = generous();
    let mut old = None;
    let result = with_group(&mut parent, &mut budget, GroupMask::Device(&alpha), 1.0, |child, budget| {
        child.clear(Color::BLACK);
        old = Some(std::mem::replace(budget, generous()));
        Ok::<_, ()>(())
    });
    assert_eq!(result, Err(GroupFailure::Group(GroupError::BudgetChanged)));
    assert_eq!(parent.data, before);
    assert_eq!(old.as_ref().unwrap().stats(), GroupStats { live_bytes: 0, peak_bytes: 80, work_units: 49 });
    assert_eq!(budget.stats().work_units, 0);
    let mut old_after_panic = None;
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = with_group::<()>(&mut parent, &mut budget, GroupMask::Device(&alpha), 1.0, |child, budget| {
            child.clear(Color::BLACK);
            old_after_panic = Some(std::mem::replace(budget, generous()));
            panic!("abort after replacing budget")
        });
    }));
    assert!(panic.is_err());
    assert_eq!(parent.data, before);
    assert_eq!(old_after_panic.as_ref().unwrap().stats().live_bytes, 0);
    assert_eq!(old_after_panic.as_ref().unwrap().stats().work_units, 49);
}

#[test]
fn zero_pixel_viewports_validate_masks_without_traversing_empty_rows() {
    let shape = rectangle(Rect::new(0.0, 0.0, 1.0, 1.0));
    for (w, h) in [(0, i32::MAX as u32), (i32::MAX as u32, 0)] {
        let mut parent = target(w, h);
        let mut budget = limits(0, 1);
        let mut calls = 0;
        with_group(&mut parent, &mut budget, GroupMask::Shape { shape, local_to_device: crate::IDENTITY }, 1.0, |_, _| {
            calls += 1;
            Ok::<_, ()>(())
        }).unwrap();
        assert_eq!(calls, 1);
        assert!(parent.data.is_empty());
        assert_eq!((parent.width, parent.height), (w, h));
        assert_eq!(budget.stats(), GroupStats { live_bytes: 0, peak_bytes: 0, work_units: 1 });
        let mut invalid = limits(0, 1);
        assert_eq!(with_group::<()>(&mut parent, &mut invalid, GroupMask::Shape { shape, local_to_device: [f32::NAN; 6] }, 1.0, |_, _| panic!("zero pixels must not skip matrix validation")), Err(GroupFailure::Group(GroupError::InvalidTransform)));
    }
}
