use super::{size_tracks, size_tracks_with_constraint, Contribution, Sizing, SizingConstraint, Track};

fn intrinsic_sizes(tracks: &[Track], contributions: &[Contribution]) -> (Vec<f32>, Vec<f32>) {
    let mut min = tracks.to_vec();
    let mut max = tracks.to_vec();
    size_tracks_with_constraint(&mut min, contributions, None, 0.0, false, SizingConstraint::MinContent);
    size_tracks(&mut max, contributions, None, 0.0, false);
    (min.iter().map(|t| t.base).collect(), max.iter().map(|t| t.base).collect())
}

#[test]
fn intrinsic_constraints_keep_fixed_minimum_and_maximum_distinct() {
    let (min, max) = intrinsic_sizes(&[Track::new(Sizing::Fixed(10.0), Sizing::Fixed(100.0))], &[]);
    assert_eq!(min, vec![10.0]);
    assert_eq!(max, vec![100.0]);
}

#[test]
fn max_content_constraint_maximizes_intrinsic_growth_limits() {
    let contributions = [Contribution { start: 0, end: 1, min: 40.0, max: 120.0 }];
    let (min, max) = intrinsic_sizes(&[Track::new(Sizing::Auto, Sizing::Auto)], &contributions);
    assert_eq!(min, vec![40.0]);
    assert_eq!(max, vec![120.0]);

    let (min, max) = intrinsic_sizes(&[Track::new(Sizing::Fixed(0.0), Sizing::MaxContent)], &contributions);
    assert_eq!(min, vec![0.0]);
    assert_eq!(max, vec![120.0]);
}

#[test]
fn min_content_constraint_preserves_max_content_track_minimum() {
    let contributions = [Contribution { start: 0, end: 1, min: 40.0, max: 120.0 }];
    let (min, max) = intrinsic_sizes(&[Track::new(Sizing::MaxContent, Sizing::MaxContent)], &contributions);
    assert_eq!(min, vec![120.0]);
    assert_eq!(max, vec![120.0]);
}

#[test]
fn min_content_constraint_does_not_expand_spanning_flexible_tracks() {
    // Reduction of css/css-grid/grid-flex-spanning-items-001.html: the
    // spanning item overflows the grid's 30px intrinsic minimum width.
    let tracks = [Track::new(Sizing::Auto, Sizing::Flex(1.0)), Track::new(Sizing::Fixed(30.0), Sizing::Fixed(30.0))];
    let contributions = [Contribution { start: 0, end: 2, min: 300.0, max: 300.0 }];
    let (min, max) = intrinsic_sizes(&tracks, &contributions);
    assert_eq!(min, vec![0.0, 30.0]);
    assert_eq!(max, vec![270.0, 30.0]);
}

#[test]
fn max_content_constraint_respects_fit_content_and_collapsed_tracks() {
    let mut collapsed = Track::new(Sizing::Fixed(40.0), Sizing::Fixed(100.0));
    collapsed.collapsed = true;
    let tracks = [Track::new(Sizing::Auto, Sizing::FitContent(80.0)), collapsed];
    let contributions = [Contribution { start: 0, end: 1, min: 40.0, max: 120.0 }];
    let (min, max) = intrinsic_sizes(&tracks, &contributions);
    assert_eq!(min, vec![40.0, 0.0]);
    assert_eq!(max, vec![80.0, 0.0]);
}
