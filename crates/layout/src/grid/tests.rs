use super::{auto_track, expand_tracks, repeat_fixed_size, size_tracks, size_tracks_with_constraint, Contribution, Sizing, SizingConstraint, Track};
use rabbithole_style::{Length, TrackListItem, TrackSize};

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

#[test]
fn auto_track_patterns_repeat_forward_and_backward() {
    let sizes = [TrackSize::Length(Length::Px(20.0)), TrackSize::Length(Length::Px(30.0)), TrackSize::Length(Length::Px(40.0))];
    for (index, expected) in [(-4, 40.0), (-3, 20.0), (-2, 30.0), (-1, 40.0), (0, 20.0), (1, 30.0), (2, 40.0), (3, 20.0)] {
        let track = auto_track(&sizes, index, None);
        assert_eq!(track.min, Sizing::Fixed(expected));
        assert_eq!(track.max, Sizing::Fixed(expected));
    }
}

#[test]
fn auto_track_patterns_resolve_percentages_against_the_axis() {
    let sizes = [TrackSize::Length(Length::Percent(25.0))];
    assert_eq!(auto_track(&sizes, -1, Some(200.0)).min, Sizing::Fixed(50.0));
    assert_eq!(auto_track(&sizes, 0, None).min, Sizing::Auto);
    assert_eq!(auto_track(&[], -1, None).max, Sizing::Auto);
}

#[test]
fn auto_repeat_floors_a_definite_maximum_by_the_definite_minimum() {
    for (minimum, maximum) in [(50.0, 25.0), (25.0, 50.0)] {
        let size = TrackSize::MinMax(Box::new(TrackSize::Length(Length::Px(minimum))), Box::new(TrackSize::Length(Length::Px(maximum))));
        assert_eq!(repeat_fixed_size(&size, Some(100.0)), 50.0);
        let list = [TrackListItem::Repeat {
            count: None,
            auto_fit: false,
            tracks: vec![TrackListItem::Track { names: vec![], size }],
        }];
        assert_eq!(expand_tracks(&list, Some(100.0), 0.0).tracks.len(), 2);
    }
}
