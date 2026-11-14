//! Percentage policy for the typed intrinsic/initial Grid measurements.
//!
//! A finite width used to measure content under an infinite inline estimate
//! is not a percentage base. This policy runs before a numeric Cb is built;
//! actual definite inputs are retained, including a real definite zero.
//! It is not installed in the legacy layout traversal at this checkpoint.
use super::phases::{AvailableSpace, GridAxis, OperationKind};
use rabbithole_style::{ComputedStyle, Edges, Length};

fn zero_base(value:Length,nonnegative:bool)->Length {
    if !value.has_percent() {return value;}
    let value=value.resolve_or_zero(0.);
    Length::Px(if nonnegative {value.max(0.)} else {value})
}

/// CSS Sizing3 §5.2.1: preferred/max cyclic percentages are initial for
/// non-replaced items, including an expression with an absolute term. Only
/// a replaced item's min-content contribution in that same axis resolves
/// these against zero. Min sizes always keep the non-percentage component.
fn axis_lengths(preferred:&mut Length,minimum:&mut Length,maximum:&mut Length,zero_preferred:bool) {
    for value in [preferred,maximum] {
        if value.has_percent() {*value=if zero_preferred {zero_base(*value,true)} else {Length::Auto};}
    }
    *minimum=zero_base(*minimum,true);
}

fn edges(values:&mut Edges<Length>,nonnegative:bool) {
    values.left=zero_base(values.left,nonnegative);
    values.right=zero_base(values.right,nonnegative);
    values.top=zero_base(values.top,nonnegative);
    values.bottom=zero_base(values.bottom,nonnegative);
}

/// Callers keep `spaces` tagged throughout measurement. `realized_width`
/// must never be fed back here as an invented Definite input. This only
/// changes the supplied temporary style; scope ownership/restoration stays
/// with the measurement caller.
pub(super) fn normalize(style:&mut ComputedStyle,spaces:[AvailableSpace;2],edge_basis:AvailableSpace,contribution:GridAxis,kind:OperationKind,replaced:bool) {
    let compressed=replaced&&kind==OperationKind::IntrinsicMin;
    if spaces[0].definite().is_none() {
        axis_lengths(&mut style.width,&mut style.min_width,&mut style.max_width,compressed&&contribution==GridAxis::Columns);
        style.column_gap=zero_base(style.column_gap,true);
    }
    if spaces[1].definite().is_none() {
        axis_lengths(&mut style.height,&mut style.min_height,&mut style.max_height,compressed&&contribution==GridAxis::Rows);
        style.row_gap=zero_base(style.row_gap,true);
    }
    if edge_basis.definite().is_none() {
        // All four percentage margins and paddings use the CB's inline
        // dimension, which need not be this box's local inline dimension
        // across an orthogonal wrapper. Do not derive it from the proxy.
        // Calc's absolute term remains; negative margins remain.
        edges(&mut style.margin,false);
        edges(&mut style.padding,true);
    }
}

/// The ordinary block layouter receives a finite Cb for execution. Across
/// an orthogonal flow that numeric inline proxy can differ from the actual
/// CB-inline percentage edge base. Freeze those edge percentages once here,
/// against the original base, before entering ordinary layout.
pub(super) fn freeze_edges(style:&mut ComputedStyle,edge_basis:AvailableSpace) {
    let Some(reference)=edge_basis.definite() else {return;};
    for (values,nonnegative) in [(&mut style.margin,false),(&mut style.padding,true)] {
        for value in [&mut values.left,&mut values.right,&mut values.top,&mut values.bottom] {
            if value.has_percent() {
                let resolved=value.resolve_or_zero(reference);
                *value=Length::Px(if nonnegative {resolved.max(0.)} else {resolved});
            }
        }
    }
}

/// A wrapper's finite execution space can be smaller than the real area
/// that supplied a percentage basis. Resolve the item's own dimensional
/// percentages once in each local axis before ordinary layout sees that
/// proxy. Values remain CSS box sizes; box-sizing is applied by sizing.
pub(super) fn freeze_sizes(style:&mut ComputedStyle,bases:[AvailableSpace;2]) {
    if let Some(reference)=bases[0].definite() {
        for value in [&mut style.width,&mut style.min_width,&mut style.max_width] {
            if value.has_percent() {*value=Length::Px(value.resolve_or_zero(reference).max(0.));}
        }
    }
    if let Some(reference)=bases[1].definite() {
        for value in [&mut style.height,&mut style.min_height,&mut style.max_height] {
            if value.has_percent() {*value=Length::Px(value.resolve_or_zero(reference).max(0.));}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_style()->ComputedStyle {
        let mut style=ComputedStyle::initial();
        style.width=Length::Calc(11.,50.);style.height=Length::Percent(25.);
        style.min_width=Length::Calc(7.,20.);style.min_height=Length::Calc(9.,30.);
        style.max_width=Length::Calc(40.,50.);style.max_height=Length::Percent(75.);
        style.padding=Edges::all(Length::Calc(3.,10.));
        style.margin=Edges::all(Length::Calc(-5.,20.));
        style.column_gap=Length::Calc(2.,10.);style.row_gap=Length::Calc(4.,10.);
        style
    }

    #[test]
    fn defined_dimensional_percentages_freeze_before_a_smaller_execution_proxy() {
        let mut style=sample_style();
        freeze_sizes(&mut style,[AvailableSpace::Definite(100.),AvailableSpace::Definite(80.)]);
        assert_eq!(style.width,Length::Px(61.));assert_eq!(style.min_width,Length::Px(27.));assert_eq!(style.max_width,Length::Px(90.));
        assert_eq!(style.height,Length::Px(20.));assert_eq!(style.min_height,Length::Px(33.));assert_eq!(style.max_height,Length::Px(60.));
        // Edge percentages have their own original-inline base and are not
        // resolved by this dimensional operation.
        assert_eq!(style.padding.left,Length::Calc(3.,10.));
        let mut cyclic=sample_style();freeze_sizes(&mut cyclic,[AvailableSpace::Indefinite;2]);
        assert_eq!(cyclic.width,Length::Calc(11.,50.));assert_eq!(cyclic.height,Length::Percent(25.));
        freeze_sizes(&mut cyclic,[AvailableSpace::Definite(0.),AvailableSpace::Definite(0.)]);
        assert_eq!(cyclic.width,Length::Px(11.));assert_eq!(cyclic.height,Length::Px(0.));
    }

    #[test]
    fn infinite_inline_proxy_does_not_become_a_percentage_base() {
        let mut style=sample_style();
        normalize(&mut style,[AvailableSpace::Indefinite,AvailableSpace::Definite(80.)],AvailableSpace::Indefinite,GridAxis::Rows,OperationKind::Layout,false);
        assert_eq!(style.width,Length::Auto);assert_eq!(style.max_width,Length::Auto);assert_eq!(style.min_width,Length::Px(7.));
        assert_eq!(style.height,Length::Percent(25.));assert_eq!(style.min_height,Length::Calc(9.,30.));
        assert_eq!(style.padding.left,Length::Px(3.));assert_eq!(style.padding.bottom,Length::Px(3.));
        assert_eq!(style.margin.right,Length::Px(-5.));assert_eq!(style.margin.top,Length::Px(-5.));
        assert_eq!(style.column_gap,Length::Px(2.));assert_eq!(style.row_gap,Length::Calc(4.,10.));
    }

    #[test]
    fn definite_inline_base_preserves_percent_edges_while_block_percentages_remain_cyclic() {
        let mut style=sample_style();
        normalize(&mut style,[AvailableSpace::Definite(200.),AvailableSpace::Indefinite],AvailableSpace::Definite(200.),GridAxis::Columns,OperationKind::IntrinsicMax,false);
        assert_eq!(style.width,Length::Calc(11.,50.));assert_eq!(style.min_width,Length::Calc(7.,20.));
        assert_eq!(style.height,Length::Auto);assert_eq!(style.max_height,Length::Auto);assert_eq!(style.min_height,Length::Px(9.));
        assert_eq!(style.padding.left,Length::Calc(3.,10.));assert_eq!(style.padding.left.resolve(200.),Some(23.));
        assert_eq!(style.margin.bottom,Length::Calc(-5.,20.));assert_eq!(style.margin.bottom.resolve(200.),Some(35.));
        assert_eq!(style.row_gap,Length::Px(4.));
    }

    #[test]
    fn actual_definite_zero_is_a_real_base_and_never_initializes_percentage_properties() {
        let mut style=sample_style();
        normalize(&mut style,[AvailableSpace::Definite(0.),AvailableSpace::Definite(0.)],AvailableSpace::Definite(0.),GridAxis::Columns,OperationKind::IntrinsicMin,false);
        assert_eq!(style.width,Length::Calc(11.,50.));assert_eq!(style.width.resolve(0.),Some(11.));
        assert_eq!(style.height,Length::Percent(25.));assert_eq!(style.max_width,Length::Calc(40.,50.));
        assert_eq!(style.padding.top,Length::Calc(3.,10.));
    }

    #[test]
    fn intrinsic_constraints_are_not_the_finite_sizes_later_realized_from_them() {
        for inline in [AvailableSpace::MinContent,AvailableSpace::MaxContent] {
            let mut style=sample_style();
            normalize(&mut style,[inline,AvailableSpace::Indefinite],inline,GridAxis::Columns,OperationKind::IntrinsicMax,false);
            assert_eq!(style.width,Length::Auto);assert_eq!(style.max_width,Length::Auto);
            assert_eq!(style.min_width,Length::Px(7.));assert_eq!(style.padding.left,Length::Px(3.));
        }
    }

    #[test]
    fn replaced_min_content_resolves_only_the_contributing_axis_against_zero() {
        let mut style=sample_style();style.aspect_ratio=Some(2.);
        normalize(&mut style,[AvailableSpace::Indefinite,AvailableSpace::Indefinite],AvailableSpace::Indefinite,GridAxis::Columns,OperationKind::IntrinsicMin,true);
        assert_eq!(style.width,Length::Px(11.));assert_eq!(style.max_width,Length::Px(40.));
        assert_eq!(style.height,Length::Auto);assert_eq!(style.max_height,Length::Auto);
        assert_eq!(style.min_height,Length::Px(9.));assert_eq!(style.aspect_ratio,Some(2.));
        let mut other=sample_style();
        normalize(&mut other,[AvailableSpace::Indefinite,AvailableSpace::Indefinite],AvailableSpace::Indefinite,GridAxis::Rows,OperationKind::IntrinsicMin,true);
        assert_eq!(other.height,Length::Px(0.));assert_eq!(other.max_height,Length::Px(0.));assert_eq!(other.width,Length::Auto);
    }

    #[test]
    fn replaced_max_content_uses_initial_preferred_and_max_but_retains_fixed_minimum_parts() {
        let mut style=sample_style();
        normalize(&mut style,[AvailableSpace::Indefinite,AvailableSpace::Indefinite],AvailableSpace::Indefinite,GridAxis::Columns,OperationKind::IntrinsicMax,true);
        assert_eq!(style.width,Length::Auto);assert_eq!(style.max_width,Length::Auto);assert_eq!(style.min_width,Length::Px(7.));
    }

    #[test]
    fn intrinsic_keywords_and_numeric_preferred_sizes_are_preserved_in_infinite_space() {
        for preferred in [Length::MinContent,Length::MaxContent,Length::FitContent,Length::Px(55.)] {
            let mut style=ComputedStyle::initial();style.width=preferred;style.height=preferred;
            normalize(&mut style,[AvailableSpace::Indefinite,AvailableSpace::Indefinite],AvailableSpace::Indefinite,GridAxis::Rows,OperationKind::Layout,false);
            assert_eq!(style.width,preferred);assert_eq!(style.height,preferred);
        }
    }

    #[test]
    fn orthogonal_edge_basis_is_not_inferred_from_the_local_inline_estimate() {
        let mut style=sample_style();
        normalize(&mut style,[AvailableSpace::Indefinite,AvailableSpace::Definite(200.)],AvailableSpace::Definite(300.),GridAxis::Rows,OperationKind::Layout,false);
        assert_eq!(style.width,Length::Auto);
        assert_eq!(style.padding.left,Length::Calc(3.,10.));assert_eq!(style.padding.left.resolve(300.),Some(33.));
        let mut other=sample_style();
        normalize(&mut other,[AvailableSpace::Definite(200.),AvailableSpace::Indefinite],AvailableSpace::Indefinite,GridAxis::Columns,OperationKind::Layout,false);
        assert_eq!(other.width,Length::Calc(11.,50.));assert_eq!(other.padding.left,Length::Px(3.));
    }

    #[test]
    fn real_edge_percentages_are_frozen_against_their_own_base_before_numeric_execution() {
        let mut style=sample_style();style.margin.top=Length::Auto;
        style.padding.bottom=Length::Calc(-50.,10.);style.margin.bottom=Length::Calc(-50.,10.);
        freeze_edges(&mut style,AvailableSpace::Definite(300.));
        assert_eq!(style.padding.left,Length::Px(33.));assert_eq!(style.margin.left,Length::Px(55.));
        assert_eq!(style.padding.bottom,Length::Px(0.));assert_eq!(style.margin.bottom,Length::Px(-20.));
        assert_eq!(style.margin.top,Length::Auto);assert_eq!(style.width,Length::Calc(11.,50.));
    }
}
