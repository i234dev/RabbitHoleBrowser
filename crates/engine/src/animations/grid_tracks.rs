//! Grid track lists combine component by component. Repetitions retain
//! their specified structure, even if two lists would expand to equal sizes.
use super::{add_lp, lerp, lerp_lp};
use rabbithole_style::{Length, TrackCalc, TrackListItem, TrackSize};

#[derive(Clone, Copy)]
pub(super) enum Operation {
    Interpolate(f32),
    Add,
}

impl Operation {
    fn choose<'a, T: ?Sized>(self, from: &'a T, to: &'a T) -> &'a T {
        match self {
            Self::Interpolate(t) if t < 0.5 => from,
            _ => to,
        }
    }

    fn length(self, a: Length, b: Length) -> Option<Length> {
        let value = match self {
            Self::Interpolate(t) => lerp_lp(&a, &b, t)?,
            Self::Add => add_lp(a, b)?,
        };
        Some(match value {
            Length::Px(px) => Length::Px(px.max(0.0)),
            Length::Percent(pct) | Length::Calc(0.0, pct) => Length::Percent(pct.max(0.0)),
            Length::Calc(px, 0.0) => Length::Px(px.max(0.0)),
            mixed => mixed,
        })
    }

    fn size(self, a: &TrackSize, b: &TrackSize) -> TrackSize {
        use TrackSize::*;
        let linear = |value: &TrackSize| match value {
            Length(l) => Some(*l),
            Calc(TrackCalc::Sum, terms) => terms.first().map(|&(px, pct)| rabbithole_style::Length::Calc(px, pct)),
            _ => None,
        };
        if let (Some(a), Some(b)) = (linear(a), linear(b)) {
            if let Some(value) = self.length(a, b) {
                return match value {
                    rabbithole_style::Length::Calc(px, pct) => Calc(TrackCalc::Sum, vec![(px, pct)]),
                    other => Length(other),
                };
            }
        }
        match (a, b) {
            (Fr(x), Fr(y)) => Fr(match self {
                Self::Interpolate(t) => lerp(*x, *y, t),
                Self::Add => x + y,
            }.max(0.0)),
            (FitContent(x), FitContent(y)) => FitContent(self.length(*x, *y).unwrap_or(*self.choose(x, y))),
            (MinMax(xmin, xmax), MinMax(ymin, ymax)) => {
                MinMax(Box::new(self.size(xmin, ymin)), Box::new(self.size(xmax, ymax)))
            }
            _ => self.choose(a, b).clone(),
        }
    }

    pub(super) fn tracks(self, a: &[TrackListItem], b: &[TrackListItem]) -> Option<Vec<TrackListItem>> {
        use TrackListItem::*;
        let sizes = |item: &&TrackListItem| !matches!(item, Names(_));
        let mut left = a.iter().filter(sizes);
        let mut right = b.iter().filter(sizes);
        let template = self.choose(a, b);
        let mut result = Vec::with_capacity(template.len());
        for item in template {
            if matches!(item, Names(_)) {
                result.push(item.clone());
                continue;
            }
            result.push(match (left.next()?, right.next()?) {
                (Track { names: an, size: av }, Track { names: bn, size: bv }) => Track {
                    names: self.choose(an, bn).clone(),
                    size: self.size(av, bv),
                },
                (Repeat { count, auto_fit, tracks }, Repeat { count: n, auto_fit: fit, tracks: other })
                    // Auto-repeat counts depend on layout. Only equal fixed
                    // repetition counts can be combined before layout.
                    if count.is_some() && count == n && auto_fit == fit => Repeat {
                        count: *count,
                        auto_fit: *auto_fit,
                        tracks: self.tracks(tracks, other)?,
                    },
                _ => return None,
            });
        }
        (left.next().is_none() && right.next().is_none()).then_some(result)
    }
}
