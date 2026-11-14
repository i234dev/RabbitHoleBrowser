use super::{auto_track, expand_tracks, repeat_fixed_size, size_tracks, size_tracks_with_space, Contribution, GridSpace, Sizing, Track};
use rabbithole_style::{Length, TrackListItem, TrackSize};

fn intrinsic_sizes(tracks: &[Track], contributions: &[Contribution]) -> (Vec<f32>, Vec<f32>) {
    let mut min = tracks.to_vec();
    let mut max = tracks.to_vec();
    size_tracks_with_space(&mut min, contributions, GridSpace::MinContent, 0., false);
    size_tracks_with_space(&mut max, contributions, GridSpace::MaxContent, 0., false);
    (min.iter().map(|t| t.base).collect(),max.iter().map(|t| t.base).collect())
}

#[test]
fn intrinsic_constraints_keep_fixed_minimum_and_maximum_distinct() {
    let (min,max)=intrinsic_sizes(&[Track::new(Sizing::Fixed(10.),Sizing::Fixed(100.))],&[]);
    assert_eq!(min,[10.]);assert_eq!(max,[100.]);
}

#[test]
fn max_content_constraint_maximizes_intrinsic_growth_limits() {
    let contributions=[Contribution {start:0,end:1,min:40.,max:120.}];
    let (min,max)=intrinsic_sizes(&[Track::new(Sizing::Auto,Sizing::Auto)],&contributions);
    assert_eq!(min,[40.]);assert_eq!(max,[120.]);
    let (min,max)=intrinsic_sizes(&[Track::new(Sizing::Fixed(0.),Sizing::MaxContent)],&contributions);
    assert_eq!(min,[0.]);assert_eq!(max,[120.]);
}

#[test]
fn min_content_constraint_preserves_max_content_track_minimum() {
    let contributions=[Contribution {start:0,end:1,min:40.,max:120.}];
    let (min,max)=intrinsic_sizes(&[Track::new(Sizing::MaxContent,Sizing::MaxContent)],&contributions);
    assert_eq!(min,[120.]);assert_eq!(max,[120.]);
}

#[test]
fn min_content_constraint_does_not_expand_spanning_flexible_tracks() {
    let tracks=[Track::new(Sizing::Auto,Sizing::Flex(1.)),Track::new(Sizing::Fixed(30.),Sizing::Fixed(30.))];
    let contributions=[Contribution {start:0,end:2,min:300.,max:300.}];
    let (min,max)=intrinsic_sizes(&tracks,&contributions);
    assert_eq!(min,[0.,30.]);assert_eq!(max,[270.,30.]);
}

#[test]
fn max_content_constraint_respects_fit_content_and_collapsed_tracks() {
    let mut collapsed=Track::new(Sizing::Fixed(40.),Sizing::Fixed(100.));collapsed.collapsed=true;
    let tracks=[Track::new(Sizing::Auto,Sizing::FitContent(80.)),collapsed];
    let contributions=[Contribution {start:0,end:1,min:40.,max:120.}];
    let (min,max)=intrinsic_sizes(&tracks,&contributions);
    assert_eq!(min,[40.,0.]);assert_eq!(max,[80.,0.]);
}

#[test]
fn definite_available_space_is_neither_intrinsic_constraint() {
    let original=[Track::new(Sizing::Fixed(10.),Sizing::Fixed(100.))];
    for (space,expected) in [(GridSpace::Definite(80.),80.),(GridSpace::MinContent,10.),(GridSpace::MaxContent,100.)] {
        let mut tracks=original.clone();
        size_tracks_with_space(&mut tracks,&[],space,0.,false);
        assert_eq!(tracks[0].base,expected);
    }
}

#[test]
fn automatic_content_based_layout_size_uses_max_content() {
    let mut tracks=[Track::new(Sizing::Fixed(10.),Sizing::Fixed(100.))];
    size_tracks(&mut tracks,&[],None,0.,false);
    assert_eq!(tracks[0].base,100.);
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

#[test]
fn staged_intrinsic_resolution_preserves_bases_and_limits_for_later_available_space() {
    let original=[Track::new(Sizing::Auto,Sizing::Auto),Track::new(Sizing::Auto,Sizing::Auto)];
    let contributions=[Contribution{start:0,end:1,min:100.,max:100.},Contribution{start:1,end:2,min:10.,max:48.}];
    let mut initial=original;
    resolve_intrinsic_tracks(&mut initial,&contributions,0.);
    assert_eq!([initial[0].base,initial[1].base],[100.,10.]);
    assert_eq!([initial[0].limit,initial[1].limit],[Some(100.),Some(48.)]);
    let mut definite=initial.clone();
    complete_track_sizing(&mut definite,&contributions,GridSpace::Definite(130.),0.,false);
    assert_eq!([definite[0].base,definite[1].base],[100.,30.]);
    complete_track_sizing(&mut initial,&contributions,GridSpace::MaxContent,0.,false);
    assert_eq!([initial[0].base,initial[1].base],[100.,48.]);
}

#[test]
fn completing_staged_tracks_still_expands_flexible_tracks_from_spanning_content() {
    let mut tracks=[Track::new(Sizing::Auto,Sizing::Flex(1.)),Track::new(Sizing::Fixed(30.),Sizing::Fixed(30.))];
    let contributions=[Contribution{start:0,end:2,min:300.,max:300.}];
    resolve_intrinsic_tracks(&mut tracks,&contributions,0.);
    assert_eq!([tracks[0].base,tracks[1].base],[0.,30.]);
    complete_track_sizing(&mut tracks,&contributions,GridSpace::MaxContent,0.,false);
    assert_eq!([tracks[0].base,tracks[1].base],[270.,30.]);
}
