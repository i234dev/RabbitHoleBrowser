use super::{auto_track, expand_tracks, repeat_fixed_size, Sizing};
use rabbithole_style::{Length, TrackListItem, TrackSize};

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
