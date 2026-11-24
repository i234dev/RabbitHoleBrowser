use super::{auto_track, complete_track_sizing, expand_tracks, repeat_fixed_size, resolve_intrinsic_tracks, size_tracks, size_tracks_with_space, Contribution, GridSpace, Sizing, Track};
use rabbithole_style::{Length, TrackListItem, TrackSize};

fn named_test_lines() -> Vec<Vec<String>> {
    ["A", "B", "C", "A", "B", "C", "A", "B", "C"].into_iter().map(|name| vec![name.into()]).collect()
}

#[test]
fn named_line_indices_count_matching_lines_in_both_directions() {
    let mut names = named_test_lines();
    names[2].push("C".into());
    for (occurrence, expected) in [(1, 2), (2, 5), (3, 8), (4, 9), (-1, 8), (-2, 5), (-3, 2), (-4, -1)] {
        assert_eq!(super::named_line(&names, "C", occurrence), expected);
    }
    assert_eq!(super::named_line(&names, "missing", 1), 9);
    assert_eq!(super::named_line(&names, "missing", -1), -1);
    // The identifier remains case-sensitive.
    assert_eq!(super::named_line(&names, "c", 1), 9);
}

#[test]
fn indexed_names_do_not_use_the_bare_names_implicit_area_suffix() {
    use rabbithole_style::GridLine;
    let names = vec![vec!["area-start".into()], vec!["area".into()], vec!["area-end".into()]];
    assert_eq!(super::line_spec(&GridLine::Named("area".into(), false), &names, 3, false), super::LineSpec::Line(0));
    assert_eq!(super::line_spec(&GridLine::Named("area".into(), false), &names, 3, true), super::LineSpec::Line(2));
    assert_eq!(super::line_spec(&GridLine::NamedNth("area".into(), 1, false), &names, 3, false), super::LineSpec::Line(1));
}

#[test]
fn named_spans_search_from_the_opposite_line_and_respect_the_implicit_side() {
    use rabbithole_style::GridLine;
    use super::LineSpec::{Line, Span};
    let names = named_test_lines();
    let indexed = |count| GridLine::NamedNth("C".into(), count, true);
    assert_eq!(super::line_specs(&GridLine::Line(3), &indexed(2), &names, 9), (Line(2), Line(8)));
    assert_eq!(super::line_specs(&indexed(2), &GridLine::Line(9), &names, 9), (Line(2), Line(8)));
    assert_eq!(super::line_specs(&GridLine::Auto, &indexed(2), &names, 9), (super::LineSpec::Auto, Span(1)));
    assert_eq!(super::line_specs(&indexed(2), &indexed(3), &names, 9), (Span(1), Span(1)));
    let unnamed = vec![vec![], vec![]];
    // Grid2 §8.3's `span foo / 4` example skips end-side implicit lines
    // while searching for a start-side named span.
    assert_eq!(super::named_span_line(&unnamed, "foo", 1, 3, false), -1);
    assert_eq!(super::named_span_line(&unnamed, "foo", 1, -2, true), 2);
    assert_eq!(super::named_span_line(&unnamed, "foo", 2, 4, true), 6);
    assert_eq!(super::named_span_line(&unnamed, "foo", 2, -2, false), -4);
}

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
    let contributions=[Contribution{start:0,end:1,min:40.,max:120.,auto_min:false}];
    let (min,max)=intrinsic_sizes(&[Track::new(Sizing::Auto,Sizing::Auto)],&contributions);
    assert_eq!(min,[40.]);assert_eq!(max,[120.]);
    let (min,max)=intrinsic_sizes(&[Track::new(Sizing::Fixed(0.),Sizing::MaxContent)],&contributions);
    assert_eq!(min,[0.]);assert_eq!(max,[120.]);
}

#[test]
fn min_content_constraint_preserves_max_content_track_minimum() {
    let contributions=[Contribution{start:0,end:1,min:40.,max:120.,auto_min:false}];
    let (min,max)=intrinsic_sizes(&[Track::new(Sizing::MaxContent,Sizing::MaxContent)],&contributions);
    assert_eq!(min,[120.]);assert_eq!(max,[120.]);
}

#[test]
fn min_content_constraint_does_not_expand_spanning_flexible_tracks() {
    // A content-based minimum of an item spanning a flexible track is zero.
    let tracks=[Track::new(Sizing::Auto,Sizing::Flex(1.)),Track::new(Sizing::Fixed(30.),Sizing::Fixed(30.))];
    let contributions=[Contribution{start:0,end:2,min:300.,max:300.,auto_min:true}];
    let (min,max)=intrinsic_sizes(&tracks,&contributions);
    assert_eq!(min,[0.,30.]);assert_eq!(max,[270.,30.]);
}

#[test]
fn max_content_constraint_respects_fit_content_and_collapsed_tracks() {
    let mut collapsed=Track::new(Sizing::Fixed(40.),Sizing::Fixed(100.));collapsed.collapsed=true;
    let tracks=[Track::new(Sizing::Auto,Sizing::FitContent(80.)),collapsed];
    let contributions=[Contribution{start:0,end:1,min:40.,max:120.,auto_min:false}];
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
    let contributions=[Contribution{start:0,end:1,min:100.,max:100.,auto_min:false},Contribution{start:1,end:2,min:10.,max:48.,auto_min:false}];
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
    let contributions=[Contribution{start:0,end:2,min:300.,max:300.,auto_min:true}];
    resolve_intrinsic_tracks(&mut tracks,&contributions,0.);
    assert_eq!([tracks[0].base,tracks[1].base],[0.,30.]);
    complete_track_sizing(&mut tracks,&contributions,GridSpace::MaxContent,0.,false);
    assert_eq!([tracks[0].base,tracks[1].base],[270.,30.]);
}

#[test]
fn legacy_indefinite_completion_keeps_flexible_track_expansion() {
    let original=[Track::new(Sizing::Auto,Sizing::Auto),Track::new(Sizing::Fixed(0.),Sizing::Flex(1.))];
    let contributions=[Contribution{start:0,end:1,min:10.,max:40.,auto_min:false},Contribution{start:1,end:2,min:0.,max:90.,auto_min:false}];
    for (space,expected) in [
        (GridSpace::LegacyIndefinite,[10.,90.]),
        (GridSpace::MinContent,[10.,0.]),
        (GridSpace::MaxContent,[40.,90.]),
    ] {
        let mut tracks=original.clone();
        size_tracks_with_space(&mut tracks,&contributions,space,0.,false);
        assert_eq!([tracks[0].base,tracks[1].base],expected);
    }
    // The ordinary automatic-size caller keeps the strict MaxContent path.
    let mut automatic=original.clone();
    size_tracks(&mut automatic,&contributions,None,0.,false);
    assert_eq!([automatic[0].base,automatic[1].base],[40.,90.]);
}

#[test]
fn explicit_minimums_of_items_spanning_flexible_tracks_grow_them_by_flex_factor() {
    // Grid2 §12.5 step 4: a 100px minimum over `0fr 0fr` splits equally,
    // over `1fr 3fr` by the flex factors; fixed-minimum tracks get nothing.
    for (sizes,expected) in [([0.,0.],[50.,50.]),([1.,3.],[25.,75.])] {
        let mut tracks=[Track::new(Sizing::Auto,Sizing::Flex(sizes[0])),Track::new(Sizing::Auto,Sizing::Flex(sizes[1]))];
        let contributions=[Contribution{start:0,end:2,min:100.,max:100.,auto_min:false}];
        resolve_intrinsic_tracks(&mut tracks,&contributions,0.);
        assert_eq!([tracks[0].base,tracks[1].base],expected);
    }
    let mut tracks=[Track::new(Sizing::Auto,Sizing::Flex(0.)),Track::new(Sizing::Fixed(0.),Sizing::Flex(1.))];
    resolve_intrinsic_tracks(&mut tracks,&[Contribution{start:0,end:2,min:100.,max:100.,auto_min:false}],0.);
    assert_eq!([tracks[0].base,tracks[1].base],[100.,0.]);
}
