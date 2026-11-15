//! Bounded legacy frameset geometry, using the existing grid layouter.
//!
//! WHATWG HTML "Frames and framesets" / "Lists of dimensions": absolute
//! space, then percentages of the original surface, then relative remainder.
//! This is a borderless, non-resizable subset with at most 16 tracks per axis
//! and 64 frameset ancestors. The original computed CSS tree is never changed.

use rabbithole_dom::{Document, NodeId};
use rabbithole_style::{ComputedStyle, Display, Edges, Float, GridLine, Length, Position, TrackListItem, TrackSize};

const MAX_TRACKS: usize = 16;
const MAX_ANCESTORS: usize = 64;

#[derive(Clone, Copy, PartialEq)]
enum Unit { Absolute, Percent, Relative }

fn dimensions(input: Option<&str>) -> Vec<(f64,Unit)> {
    let Some(input)=input else { return vec![(1.0,Unit::Relative)] };
    // A final comma is discarded by the legacy dimension-list algorithm.
    let input=input.strip_suffix(',').unwrap_or(input);
    let mut result=Vec::new();
    for token in input.split(',').take(MAX_TRACKS) {
        let token=token.trim_matches(|c:char|c.is_ascii_whitespace());
        let bytes=token.as_bytes();
        let mut pos=0;
        let mut value=0.0_f64;
        // Saturation is only a finite-representation guard before summing at
        // most16 weights; it never clamps the resulting viewport rectangles.
        let ceiling=f64::MAX/((2*MAX_TRACKS) as f64);
        while let Some(digit)=bytes.get(pos).filter(|digit|digit.is_ascii_digit()) {
            value=(value*10.0+f64::from(*digit-b'0')).min(ceiling);
            pos+=1;
        }
        if bytes.get(pos)==Some(&b'.') {
            pos+=1;
            let mut scale=0.1;
            while let Some(&byte)=bytes.get(pos) {
                if byte.is_ascii_digit() { value=(value+f64::from(byte-b'0')*scale).min(ceiling);scale*=0.1; }
                else if !byte.is_ascii_whitespace() {break;}
                pos+=1;
            }
        }
        while bytes.get(pos).is_some_and(u8::is_ascii_whitespace) {pos+=1;}
        let unit=match bytes.get(pos) {
            Some(b'%')=>Unit::Percent,
            Some(b'*')=>Unit::Relative,
            _ if token.is_empty()=>Unit::Relative,
            _=>Unit::Absolute,
        };
        if unit==Unit::Relative && value==0.0 {value=1.0;}
        result.push((value,unit));
    }
    result
}

fn sizes(input:Option<&str>,dimension:f32)->Vec<f32> {
    let dimension=if dimension.is_finite() {f64::from(dimension.max(0.0))} else {0.0};
    let values=dimensions(input);
    let sum=|unit|values.iter().filter(|value|value.1==unit).map(|value|value.0).sum::<f64>();
    let absolute=sum(Unit::Absolute);
    let percent=sum(Unit::Percent);
    let relative=sum(Unit::Relative);
    let mut remaining=dimension;
    let mut result=vec![0.0;values.len()];
    let mut assign=|unit:Unit,total:f64,available:f64,scaled:bool| {
        for (slot,(number,kind)) in result.iter_mut().zip(&values) {
            if *kind==unit {
                *slot=if scaled {if total>0.0 {number/total*available} else {0.0}}
                    else if unit==Unit::Percent {number/100.0*dimension} else {*number};
            }
        }
    };
    assign(Unit::Absolute,absolute,remaining,absolute>remaining);
    remaining=(remaining-absolute).max(0.0);
    let requested=percent/100.0*dimension;
    assign(Unit::Percent,percent,remaining,requested>remaining);
    remaining=(remaining-requested).max(0.0);
    assign(Unit::Relative,relative,remaining,true);
    result.into_iter().map(|number|number as f32).collect()
}

fn is_panel(doc:&Document,node:NodeId)->bool {
    matches!(doc.tag(node),Some("frame"|"frameset"))
}

fn surface(doc:&Document,node:NodeId,viewport:(f32,f32))->Option<(f32,f32,usize,usize)> {
    let mut path=Vec::new();
    let mut current=node;
    while let Some(parent)=doc.parent(current).filter(|&parent|doc.is_element(parent,"frameset")) {
        if path.len()>=MAX_ANCESTORS {return None;}
        let index=doc.child_elements(parent).filter(|&node|is_panel(doc,node))
            .take(MAX_TRACKS*MAX_TRACKS).position(|child|child==current)?;
        path.push((parent,index));
        current=parent;
    }
    if path.is_empty() && !doc.is_element(node,"frameset") {return None;}
    let finite=|value:f32|if value.is_finite() {value.max(0.0)} else {0.0};
    let (mut width,mut height)=(finite(viewport.0),finite(viewport.1));
    let (mut column,mut row)=(0,0);
    for (parent,index) in path.into_iter().rev() {
        let columns=sizes(doc.attr(parent,"cols"),width);
        let rows=sizes(doc.attr(parent,"rows"),height);
        column=index%columns.len();row=index/columns.len();
        width=*columns.get(column)?;height=*rows.get(row)?;
    }
    Some((width,height,column,row))
}

fn tracks(sizes:Vec<f32>)->Vec<TrackListItem> {
    sizes.into_iter().map(|size|TrackListItem::Track {names:Vec::new(),size:TrackSize::Length(Length::Px(size))}).collect()
}

/// Internal used geometry only: CSSOM still reads the unmodified StyledTree.
pub(crate) fn style(doc:&Document,node:NodeId,style:&ComputedStyle,viewport:(f32,f32))->Option<ComputedStyle> {
    if !is_panel(doc,node) {return None;}
    let (width,height,column,row)=surface(doc,node,viewport)?;
    let mut style=style.clone();
    style.display=Display::Block;
    style.position=Position::Static;
    style.float=Float::None;
    style.width=Length::Px(width);style.height=Length::Px(height);
    style.min_width=Length::ZERO;style.min_height=Length::ZERO;
    style.max_width=Length::Auto;style.max_height=Length::Auto;
    style.margin=Edges::all(Length::ZERO);style.padding=Edges::all(Length::ZERO);
    style.border_width=Edges::all(0.0);
    style.grid_column_start=GridLine::Line(column as i32+1);
    style.grid_column_end=GridLine::Line(column as i32+2);
    style.grid_row_start=GridLine::Line(row as i32+1);
    style.grid_row_end=GridLine::Line(row as i32+2);
    style.order=0;
    if doc.is_element(node,"frameset") {
        style.grid_template_columns=tracks(sizes(doc.attr(node,"cols"),width));
        style.grid_template_rows=tracks(sizes(doc.attr(node,"rows"),height));
        style.grid_template_areas.clear();
        style.grid_lanes=false;
        style.column_gap=Length::ZERO;style.row_gap=Length::ZERO;
    }
    Some(style)
}

pub(crate) fn capacity(style:&ComputedStyle)->usize {
    style.grid_template_columns.len().min(MAX_TRACKS)*style.grid_template_rows.len().min(MAX_TRACKS)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn near(actual:&[f32],expected:&[f32]) {
        assert_eq!(actual.len(),expected.len());
        for (actual,expected) in actual.iter().zip(expected) {assert!((actual-expected).abs()<0.01,"{actual} != {expected}");}
    }
    #[test]
    fn missing_dimensions_relative_zero_and_trailing_comma() {
        near(&sizes(None,800.0),&[800.0]);
        near(&sizes(Some(""),800.0),&[800.0]);
        near(&sizes(Some("0*,2*,"),600.0),&[200.0,400.0]);
    }
    #[test]
    fn absolute_percent_and_relative_share_the_original_surface() {
        near(&sizes(Some("200,25%,1*,2*"),800.0),&[200.0,200.0,400.0/3.0,800.0/3.0]);
        near(&sizes(Some("30%,40%,30%"),800.0),&[240.0,320.0,240.0]);
    }
    #[test]
    fn overcommitted_absolute_then_percentage_space_is_proportional() {
        near(&sizes(Some("900,300,50%,*"),600.0),&[450.0,150.0,0.0,0.0]);
        near(&sizes(Some("200,50%,100%,*"),800.0),&[200.0,200.0,400.0,0.0]);
    }
    #[test]
    fn track_and_numeric_limits_keep_finite_bounded_rectangles() {
        let source=std::iter::repeat_n("1*",1000).collect::<Vec<_>>().join(",");
        let resolved=sizes(Some(&source),800.0);assert_eq!(resolved.len(),16);
        assert!(resolved.iter().all(|&value|value==50.0));
        let huge="9".repeat(1000);let resolved=sizes(Some(&format!("{huge},{huge},*")),800.0);
        near(&resolved,&[400.0,400.0,0.0]);
        assert!(resolved.iter().all(|value|value.is_finite()));
        let nested=|depth|format!("<!doctype html>{}<frame id='leaf'>{}","<frameset>".repeat(depth),"</frameset>".repeat(depth));
        let doc=rabbithole_html::parse(&nested(64));
        assert!(surface(&doc,doc.element_by_id("leaf").unwrap(),(800.0,600.0)).is_some());
        let doc=rabbithole_html::parse(&nested(65));
        assert!(surface(&doc,doc.element_by_id("leaf").unwrap(),(800.0,600.0)).is_none());
    }
}
