//! Parsing of CSS component values into typed values: colors, lengths
//! (including `calc()`/`min()`/`max()`/`clamp()`), keywords.

use crate::computed::Length;
use rabbithole_css::Token;
use rabbithole_gfx::Color;

/// A color value as written in a stylesheet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorValue {
    Color(Color),
    /// `currentcolor`: resolved to the element's `color` at apply time.
    CurrentColor,
}

/// Values needed to turn font/viewport relative units into px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LengthContext {
    pub font_size: f32,
    pub root_font_size: f32,
    pub viewport_w: f32,
    pub viewport_h: f32,
    /// `ch`, `ex` and `ic` of the element's font, in em.
    pub ch: f32,
    pub ex: f32,
    pub ic: f32,
    /// The element's computed line height in px (`lh` unit).
    pub lh: f32,
}

thread_local! {
    static UNIT_FONTS: std::cell::RefCell<Option<std::rc::Rc<rabbithole_gfx::FontContext>>> = const { std::cell::RefCell::new(None) };
    static UNIT_CACHE: std::cell::RefCell<std::collections::HashMap<(String, u8, u16, bool, (u64, u64)), (f32, f32, f32)>> = std::cell::RefCell::new(std::collections::HashMap::new());
    /// The last few `font_units` results (consecutive elements mostly share
    /// their font): looked up without building the cache key.
    static UNIT_RECENT: std::cell::RefCell<Vec<RecentUnits>> = const { std::cell::RefCell::new(Vec::new()) };
}

struct RecentUnits {
    families: Vec<String>,
    generic: u8,
    weight: u16,
    italic: bool,
    generation: (u64, u64),
    units: (f32, f32, f32),
}

fn clear_recent_units() {
    UNIT_RECENT.with(|r| r.borrow_mut().clear());
}

/// Fonts used to resolve the font-relative units `ch` and `ex` on this
/// thread (without them both are 0.5em).
pub fn set_unit_fonts(fonts: Option<std::rc::Rc<rabbithole_gfx::FontContext>>) {
    UNIT_FONTS.with(|f| *f.borrow_mut() = fonts);
    UNIT_CACHE.with(|c| c.borrow_mut().clear());
    clear_recent_units();
}

/// Forget the cached `ch` / `ex` (fonts were added).
pub fn clear_unit_cache() {
    UNIT_CACHE.with(|c| c.borrow_mut().clear());
    clear_recent_units();
}

thread_local! {
    /// Web font families whose `unicode-range` excludes the space: they
    /// cannot be the first available font.
    static UNIT_SKIP: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

type UnitCache = std::collections::HashMap<(String, u8, u16, bool, (u64, u64)), (f32, f32, f32)>;

/// Restores all font-unit state, including nested document/style passes.
/// A cache belongs to a view, not merely to its number of registered faces.
#[must_use]
pub struct FontUnitScope {
    fonts: Option<std::rc::Rc<rabbithole_gfx::FontContext>>,
    cache: UnitCache,
    skip: Vec<String>,
}

pub fn scope_unit_fonts(fonts: Option<std::rc::Rc<rabbithole_gfx::FontContext>>, skip: Vec<String>) -> FontUnitScope {
    clear_recent_units();
    FontUnitScope {
        fonts: UNIT_FONTS.with(|f| std::mem::replace(&mut *f.borrow_mut(), fonts)),
        cache: UNIT_CACHE.with(|c| std::mem::take(&mut *c.borrow_mut())),
        skip: UNIT_SKIP.with(|s| std::mem::replace(&mut *s.borrow_mut(), skip)),
    }
}

pub(crate) fn scope_unit_skip(skip: Vec<String>) -> FontUnitScope {
    let fonts = UNIT_FONTS.with(|f| f.borrow().clone());
    scope_unit_fonts(fonts, skip)
}

impl Drop for FontUnitScope {
    fn drop(&mut self) {
        UNIT_FONTS.with(|f| *f.borrow_mut() = self.fonts.take());
        UNIT_CACHE.with(|c| *c.borrow_mut() = std::mem::take(&mut self.cache));
        UNIT_SKIP.with(|s| *s.borrow_mut() = std::mem::take(&mut self.skip));
        clear_recent_units();
    }
}

/// Preserve the existing unicode-range rule: a family whose every face
/// excludes U+0020 cannot provide the first available font's unit metrics.
pub fn font_unit_skip(faces: &[crate::FontFace]) -> Vec<String> {
    let mut families: Vec<_> = faces.iter().map(|f| f.family.to_ascii_lowercase()).collect();
    families.sort();
    families.dedup();
    families.retain(|family| faces.iter().filter(|f| f.family.eq_ignore_ascii_case(family)).all(|f| {
        f.unicode_range.as_ref().is_some_and(|r| !r.iter().any(|(a, b)| *a <= 0x20 && 0x20 <= *b))
    }));
    families
}

/// `ch` (advance of "0") and `ex` (x-height) of a style's first available
/// font, in em.
pub fn font_units(style: &crate::ComputedStyle) -> (f32, f32, f32) {
    UNIT_FONTS.with(|f| {
        let f = f.borrow();
        let Some(fonts) = f.as_ref() else { return (0.5, 0.5, 1.0) };
        let generation = fonts.selection_generation();
        let (generic, weight, italic) = (style.font_family as u8, style.font_weight, style.is_italic());
        let recent = UNIT_RECENT.with(|r| {
            r.borrow()
                .iter()
                .find(|e| e.generation == generation && e.generic == generic && e.weight == weight && e.italic == italic && e.families == style.font_family_list)
                .map(|e| e.units)
        });
        if let Some(v) = recent {
            return v;
        }
        let remember = |units: (f32, f32, f32)| {
            UNIT_RECENT.with(|r| {
                let mut r = r.borrow_mut();
                if r.len() >= 4 {
                    r.remove(0);
                }
                r.push(RecentUnits { families: style.font_family_list.clone(), generic, weight, italic, generation, units });
            })
        };
        let key = (style.font_family_list.join(","), generic, weight, italic, generation);
        if let Some(v) = UNIT_CACHE.with(|c| c.borrow().get(&key).copied()) {
            remember(v);
            return v;
        }
        let list: Vec<String> = UNIT_SKIP.with(|s| {
            let s = s.borrow();
            style.font_family_list.iter().filter(|f| !s.iter().any(|x| x.eq_ignore_ascii_case(f.trim().trim_matches('"').trim_matches('\''))) ).cloned().collect()
        });
        let v = fonts.unit_metrics(&list, style.font_family, style.font_weight, style.is_italic());
        UNIT_CACHE.with(|c| c.borrow_mut().insert(key, v));
        remember(v);
        v
    })
}

/// Cap height (em) of the element's first available font.
pub fn font_cap_height(style: &crate::ComputedStyle) -> Option<f32> {
    UNIT_FONTS.with(|f| {
        let f = f.borrow();
        let fonts = f.as_ref()?;
        fonts.cap_height_em(&style.font_family_list, style.font_family, style.font_weight, style.is_italic())
    })
}

/// A linear combination of length units, the result of evaluating a
/// `calc()` expression (or a plain length).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Lin {
    /// Dimensionless factor (only meaningful while evaluating products).
    pub num: f32,
    pub px: f32,
    pub em: f32,
    pub rem: f32,
    pub percent: f32,
    pub vw: f32,
    pub vh: f32,
    pub vmin: f32,
    pub vmax: f32,
    pub ch: f32,
    pub ex: f32,
    pub ic: f32,
    pub lh: f32,
    /// True when this is a bare number (no unit at all).
    pub is_number: bool,
}

impl Lin {
    fn add(self, o: Lin, sign: f32) -> Lin {
        Lin {
            num: self.num + sign * o.num,
            px: self.px + sign * o.px,
            em: self.em + sign * o.em,
            rem: self.rem + sign * o.rem,
            percent: self.percent + sign * o.percent,
            vw: self.vw + sign * o.vw,
            vh: self.vh + sign * o.vh,
            vmin: self.vmin + sign * o.vmin,
            vmax: self.vmax + sign * o.vmax,
            ch: self.ch + sign * o.ch,
            ex: self.ex + sign * o.ex,
            ic: self.ic + sign * o.ic,
            lh: self.lh + sign * o.lh,
            is_number: self.is_number && o.is_number,
        }
    }
    fn scale(self, f: f32) -> Lin {
        Lin {
            num: self.num * f,
            px: self.px * f,
            em: self.em * f,
            rem: self.rem * f,
            percent: self.percent * f,
            vw: self.vw * f,
            vh: self.vh * f,
            vmin: self.vmin * f,
            vmax: self.vmax * f,
            ch: self.ch * f,
            ex: self.ex * f,
            ic: self.ic * f,
            lh: self.lh * f,
            is_number: self.is_number,
        }
    }
    fn has_percent(&self) -> bool {
        self.percent != 0.0
    }
    /// Px value of all non-percentage terms.
    fn px_part(&self, ctx: &LengthContext) -> f32 {
        self.px
            + self.em * ctx.font_size
            + self.rem * ctx.root_font_size
            + self.vw * ctx.viewport_w / 100.0
            + self.vh * ctx.viewport_h / 100.0
            + self.vmin * ctx.viewport_w.min(ctx.viewport_h) / 100.0
            + self.vmax * ctx.viewport_w.max(ctx.viewport_h) / 100.0
            + self.ch * ctx.font_size * ctx.ch
            + self.ex * ctx.font_size * ctx.ex
            + self.ic * ctx.font_size * ctx.ic
            + self.lh * ctx.lh
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathFn {
    Min,
    Max,
    Clamp,
}

/// A length as written in a stylesheet (before unit resolution).
#[derive(Debug, Clone, PartialEq)]
pub enum LengthValue {
    Px(f32),
    Em(f32),
    Rem(f32),
    Percent(f32),
    Vw(f32),
    Vh(f32),
    Vmin(f32),
    Vmax(f32),
    Ch(f32),
    Ic(f32),
    Ex(f32),
    Lh(f32),
    Auto,
    /// Evaluated `calc()` expression.
    Calc(Lin),
    /// `min()`, `max()`, `clamp()` with evaluated arguments.
    Math(MathFn, Vec<Lin>),
    /// A math expression that mixes percentages with lengths through
    /// `min()`/`max()`/`clamp()` (or nests them inside `calc()`), kept as a
    /// tree until layout knows the percentage base.
    Tree(Box<Expr>),
}

/// A parsed math expression whose leaves are linear unit combinations.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Leaf(Lin),
    Sum(Vec<Expr>),
    Scale(Box<Expr>, f32),
    Fn(MathFn, Vec<Expr>),
}

impl Expr {
    /// The expression as one linear combination, when it has no math
    /// function.
    fn to_lin(&self) -> Option<Lin> {
        match self {
            Expr::Leaf(l) => Some(*l),
            Expr::Sum(v) => {
                let mut acc = v.first()?.to_lin()?;
                for e in &v[1..] {
                    acc = acc.add(e.to_lin()?, 1.0);
                }
                Some(acc)
            }
            Expr::Scale(e, f) => Some(e.to_lin()?.scale(*f)),
            Expr::Fn(..) => None,
        }
    }
    fn is_number(&self) -> Option<f32> {
        match self {
            Expr::Leaf(l) if l.is_number => Some(l.num),
            _ => None,
        }
    }
    /// Resolve the font/viewport units of the leaves into a computed tree.
    fn compute(&self, ctx: &LengthContext) -> crate::computed::MathNode {
        use crate::computed::MathNode;
        match self {
            Expr::Leaf(l) => MathNode::Leaf(l.px_part(ctx), l.percent),
            Expr::Sum(v) => MathNode::Sum(v.iter().map(|e| e.compute(ctx)).collect()),
            Expr::Scale(e, f) => MathNode::Scale(Box::new(e.compute(ctx)), *f),
            Expr::Fn(MathFn::Min, v) => MathNode::Min(v.iter().map(|e| e.compute(ctx)).collect()),
            Expr::Fn(MathFn::Max, v) => MathNode::Max(v.iter().map(|e| e.compute(ctx)).collect()),
            Expr::Fn(MathFn::Clamp, v) if v.len() == 3 => MathNode::Clamp(Box::new([v[0].compute(ctx), v[1].compute(ctx), v[2].compute(ctx)])),
            Expr::Fn(MathFn::Clamp, v) => v.first().map(|e| e.compute(ctx)).unwrap_or(MathNode::Leaf(0.0, 0.0)),
        }
    }
}

/// sum := product (('+' | '-') product)*, keeping math functions.
fn expr_sum(comps: &[&[Token]]) -> Option<Expr> {
    let mut i = 0;
    let mut terms = vec![expr_product(comps, &mut i)?];
    while i < comps.len() {
        let sign = match comps[i] {
            [Token::Delim('+')] => 1.0,
            [Token::Delim('-')] => -1.0,
            _ => return None,
        };
        i += 1;
        let rhs = expr_product(comps, &mut i)?;
        terms.push(if sign < 0.0 { Expr::Scale(Box::new(rhs), -1.0) } else { rhs });
    }
    Some(if terms.len() == 1 { terms.pop()? } else { Expr::Sum(terms) })
}

fn expr_product(comps: &[&[Token]], i: &mut usize) -> Option<Expr> {
    let mut acc = expr_unit(comps, i)?;
    while *i < comps.len() {
        match comps[*i] {
            [Token::Delim('*')] => {
                *i += 1;
                let rhs = expr_unit(comps, i)?;
                acc = match (acc.is_number(), rhs.is_number()) {
                    (Some(a), Some(b)) => Expr::Leaf(Lin { num: a * b, is_number: true, ..Lin::default() }),
                    (_, Some(b)) => Expr::Scale(Box::new(acc), b),
                    (Some(a), None) => Expr::Scale(Box::new(rhs), a),
                    (None, None) => return None,
                };
            }
            [Token::Delim('/')] => {
                *i += 1;
                let d = expr_unit(comps, i)?.is_number().filter(|d| *d != 0.0)?;
                acc = match acc.is_number() {
                    Some(a) => Expr::Leaf(Lin { num: a / d, is_number: true, ..Lin::default() }),
                    None => Expr::Scale(Box::new(acc), 1.0 / d),
                };
            }
            _ => break,
        }
    }
    Some(acc)
}

fn expr_unit(comps: &[&[Token]], i: &mut usize) -> Option<Expr> {
    let c = *comps.get(*i)?;
    *i += 1;
    match c {
        [Token::Number(n)] => Some(Expr::Leaf(Lin { num: *n, is_number: true, ..Lin::default() })),
        [Token::OpenParen, ..] => expr_sum(&components(strip_function(c))),
        [Token::Delim('-')] => Some(Expr::Scale(Box::new(expr_unit(comps, i)?), -1.0)),
        [Token::Function(f), ..] => match f.to_ascii_lowercase().as_str() {
            "calc" | "-webkit-calc" | "-moz-calc" => expr_sum(&components(strip_function(c))),
            "min" | "max" | "clamp" => expr_fn(c, f),
            _ => None,
        },
        _ => Some(Expr::Leaf(lin_of(&parse_length(c)?)?)),
    }
}

fn expr_fn(c: &[Token], f: &str) -> Option<Expr> {
    let f = match f.to_ascii_lowercase().as_str() {
        "min" => MathFn::Min,
        "max" => MathFn::Max,
        "clamp" => MathFn::Clamp,
        _ => return None,
    };
    let args = function_args(c).iter().map(|a| expr_sum(a)).collect::<Option<Vec<_>>>()?;
    if args.is_empty() || (f == MathFn::Clamp && args.len() != 3) {
        return None;
    }
    Some(Expr::Fn(f, args))
}

impl LengthValue {
    /// A `calc()` / math function that mixes percentages with lengths, as
    /// its combinator and (px, percent) terms (None for anything else).
    pub fn mixed_terms(&self, ctx: &LengthContext) -> Option<(crate::computed::TrackCalc, Vec<(f32, f32)>)> {
        use crate::computed::TrackCalc;
        match self {
            LengthValue::Calc(lin) if lin.has_percent() && lin.px_part(ctx) != 0.0 => Some((TrackCalc::Sum, vec![(lin.px_part(ctx), lin.percent)])),
            LengthValue::Math(f, args) if args.iter().any(|a| a.has_percent()) => {
                let op = match f {
                    MathFn::Min => TrackCalc::Min,
                    MathFn::Max => TrackCalc::Max,
                    MathFn::Clamp => TrackCalc::Clamp,
                };
                Some((op, args.iter().map(|a| (a.px_part(ctx), a.percent)).collect()))
            }
            _ => None,
        }
    }

    /// Resolve to a computed `Length`.
    ///
    /// Approximations: a `calc()` mixing percentages with absolute terms
    /// resolves to its percentage part only (`calc(100% - 20px)` → `100%`),
    /// because the containing block is not known at compute time. Math
    /// functions whose arguments contain percentages resolve to their first
    /// percentage-free argument.
    pub fn resolve(&self, ctx: &LengthContext) -> Length {
        match self {
            LengthValue::Px(v) => Length::Px(*v),
            LengthValue::Em(v) => Length::Px(v * ctx.font_size),
            LengthValue::Rem(v) => Length::Px(v * ctx.root_font_size),
            LengthValue::Percent(p) => Length::Percent(*p),
            LengthValue::Vw(v) => Length::Px(v * ctx.viewport_w / 100.0),
            LengthValue::Vh(v) => Length::Px(v * ctx.viewport_h / 100.0),
            LengthValue::Vmin(v) => Length::Px(v * ctx.viewport_w.min(ctx.viewport_h) / 100.0),
            LengthValue::Vmax(v) => Length::Px(v * ctx.viewport_w.max(ctx.viewport_h) / 100.0),
            LengthValue::Ch(v) => Length::Px(v * ctx.font_size * ctx.ch),
            LengthValue::Ex(v) => Length::Px(v * ctx.font_size * ctx.ex),
            LengthValue::Ic(v) => Length::Px(v * ctx.font_size * ctx.ic),
            LengthValue::Lh(v) => Length::Px(v * ctx.lh),
            LengthValue::Auto => Length::Auto,
            LengthValue::Calc(lin) => {
                if lin.has_percent() && lin.px_part(ctx) != 0.0 {
                    Length::Calc(lin.px_part(ctx), lin.percent)
                } else if lin.has_percent() {
                    Length::Percent(lin.percent)
                } else {
                    Length::Px(lin.px_part(ctx))
                }
            }
            LengthValue::Tree(e) => {
                let n = e.compute(ctx);
                if n.has_percent() {
                    Length::Math(crate::computed::intern_math(n))
                } else {
                    Length::Px(n.eval(0.0))
                }
            }
            LengthValue::Math(f, args) => {
                if args.iter().any(|a| a.has_percent()) {
                    return match args.iter().find(|a| !a.has_percent()) {
                        Some(a) => Length::Px(a.px_part(ctx)),
                        None => Length::Percent(args[0].percent),
                    };
                }
                let vals: Vec<f32> = args.iter().map(|a| a.px_part(ctx)).collect();
                let v = match f {
                    MathFn::Min => vals.iter().cloned().fold(f32::INFINITY, f32::min),
                    MathFn::Max => vals.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                    MathFn::Clamp => {
                        if vals.len() == 3 {
                            vals[1].max(vals[0]).min(vals[2])
                        } else {
                            vals[0]
                        }
                    }
                };
                Length::Px(v)
            }
        }
    }

    /// Resolve, but a bare number is also accepted as px (quirk used for a
    /// few properties and presentational attributes).
    pub fn is_auto(&self) -> bool {
        matches!(self, LengthValue::Auto)
    }
}

// --------------------------------------------------------------- components

/// Split a token list into top-level component values: each is either a
/// single token or a function token together with its arguments and
/// closing paren. Whitespace is dropped.
pub fn components(toks: &[Token]) -> Vec<&[Token]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Token::Whitespace => i += 1,
            Token::Function(_) | Token::OpenParen => {
                let start = i;
                let mut depth = 0i32;
                while i < toks.len() {
                    match toks[i] {
                        Token::Function(_) | Token::OpenParen => depth += 1,
                        Token::CloseParen => {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                out.push(&toks[start..i.min(toks.len())]);
            }
            _ => {
                out.push(&toks[i..i + 1]);
                i += 1;
            }
        }
    }
    out
}

/// Split components on top-level commas.
pub fn split_commas<'a>(comps: &[&'a [Token]]) -> Vec<Vec<&'a [Token]>> {
    let mut out = vec![Vec::new()];
    for c in comps {
        if c.len() == 1 && c[0] == Token::Comma {
            out.push(Vec::new());
        } else {
            out.last_mut().unwrap().push(*c);
        }
    }
    out
}

/// Lower-cased identifier of a single-token component.
pub fn ident(c: &[Token]) -> Option<String> {
    match c {
        [Token::Ident(s)] => Some(s.to_ascii_lowercase()),
        _ => None,
    }
}

pub fn is_ident(c: &[Token], name: &str) -> bool {
    matches!(c, [Token::Ident(s)] if s.eq_ignore_ascii_case(name))
}

pub fn number(c: &[Token]) -> Option<f32> {
    match c {
        [Token::Number(n)] => Some(*n),
        // A unitless `calc()` (with the `infinity` constant).
        [Token::Function(f), ..] if f.eq_ignore_ascii_case("calc") => {
            let comps = components(strip_function(c));
            match comps.as_slice() {
                [[Token::Ident(k)]] if k.eq_ignore_ascii_case("infinity") => Some(f32::INFINITY),
                [[Token::Ident(k)]] if k.eq_ignore_ascii_case("-infinity") => Some(f32::NEG_INFINITY),
                _ => eval_sum(&comps).filter(|l| l.is_number).map(|l| l.num),
            }
        }
        [Token::Function(f), ..] if is_math_fn(f) => eval_math_fn(c).filter(|l| l.is_number).map(|l| l.num),
        _ => None,
    }
}

/// A `font-stretch` (`font-width`) keyword in percent.
pub fn font_stretch_keyword(name: &str) -> Option<u16> {
    Some(match name {
        "ultra-condensed" => 50,
        "extra-condensed" => 62,
        "condensed" => 75,
        "semi-condensed" => 87,
        "normal" => 100,
        "semi-expanded" => 112,
        "expanded" => 125,
        "extra-expanded" => 150,
        "ultra-expanded" => 200,
        _ => return None,
    })
}

/// A single `font-stretch` value (keyword or non-negative percentage) in
/// percent, clamped to the representable range.
pub fn font_stretch(c: &[Token]) -> Option<u16> {
    match c {
        [Token::Ident(s)] => font_stretch_keyword(&s.to_ascii_lowercase()),
        [Token::Percentage(p)] if *p >= 0.0 => Some(p.round().clamp(1.0, 1000.0) as u16),
        _ => None,
    }
}

/// Arguments of a function component, split on commas, whitespace removed.
fn function_args(c: &[Token]) -> Vec<Vec<&[Token]>> {
    if c.len() < 2 {
        return Vec::new();
    }
    let inner = if matches!(c.last(), Some(Token::CloseParen)) { &c[1..c.len() - 1] } else { &c[1..] };
    split_commas(&components(inner))
}

// ------------------------------------------------------------------ lengths

/// Parse a length component. Unitless numbers are only accepted as `0`
/// unless `allow_unitless` (quirks: presentational attributes).
pub fn parse_length(c: &[Token]) -> Option<LengthValue> {
    parse_length_opts(c, false)
}

pub fn parse_length_opts(c: &[Token], allow_unitless: bool) -> Option<LengthValue> {
    match c {
        [Token::Dimension(v, u)] => dimension(*v, u),
        [Token::Percentage(p)] => Some(LengthValue::Percent(*p)),
        [Token::Number(n)] => {
            if *n == 0.0 || allow_unitless {
                Some(LengthValue::Px(*n))
            } else {
                None
            }
        }
        [Token::Ident(s)] if s.eq_ignore_ascii_case("auto") => Some(LengthValue::Auto),
        [Token::Function(f), ..] => {
            let lf = f.to_ascii_lowercase();
            match lf.as_str() {
                "calc" | "-webkit-calc" | "-moz-calc" => {
                    let inner = strip_function(c);
                    match eval_sum(&components(inner)) {
                        Some(lin) => Some(LengthValue::Calc(lin)),
                        // Nested min()/max()/clamp(): keep the tree.
                        None => {
                            let e = expr_sum(&components(inner))?;
                            Some(match e.to_lin() {
                                Some(lin) => LengthValue::Calc(lin),
                                None => LengthValue::Tree(Box::new(e)),
                            })
                        }
                    }
                }
                "min" | "max" | "clamp" => {
                    // Arguments with percentages or nested math functions
                    // stay a tree (resolved at layout).
                    let e = expr_fn(c, f)?;
                    let flat = match &e {
                        Expr::Fn(_, args) => args.iter().all(|a| a.to_lin().is_some_and(|l| !l.has_percent())),
                        _ => false,
                    };
                    if !flat {
                        return Some(LengthValue::Tree(Box::new(e)));
                    }
                    let args = function_args(c);
                    let mut lins = Vec::new();
                    for a in &args {
                        lins.push(eval_sum(a)?);
                    }
                    if lins.is_empty() {
                        return None;
                    }
                    let f = match lf.as_str() {
                        "min" => MathFn::Min,
                        "max" => MathFn::Max,
                        _ => MathFn::Clamp,
                    };
                    Some(LengthValue::Math(f, lins))
                }
                "var" | "env" => None,
                _ if is_math_fn(&lf) => {
                    let lin = eval_math_fn(c)?;
                    if lin.is_number && !(lin.num == 0.0 || allow_unitless) {
                        return None;
                    }
                    Some(if lin.is_number { LengthValue::Px(lin.num) } else { LengthValue::Calc(lin) })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn strip_function(c: &[Token]) -> &[Token] {
    if c.len() >= 2 && matches!(c.last(), Some(Token::CloseParen)) {
        &c[1..c.len() - 1]
    } else if !c.is_empty() {
        &c[1..]
    } else {
        c
    }
}

thread_local! {
    /// Width and height `cq*` units of the element being styled resolve
    /// against (its query containers, else the viewport).
    static CONTAINER_UNITS: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((0.0, 0.0)) };
}

/// Set the basis of `cqw`/`cqh`/`cqi`/`cqb`/`cqmin`/`cqmax` for the
/// element about to be styled.
pub fn set_container_unit_basis(width: f32, height: f32) {
    CONTAINER_UNITS.with(|c| c.set((width, height)));
}

fn dimension(v: f32, unit: &str) -> Option<LengthValue> {
    Some(match unit {
        "px" => LengthValue::Px(v),
        "em" => LengthValue::Em(v),
        "rem" => LengthValue::Rem(v),
        "vw" | "svw" | "lvw" | "dvw" => LengthValue::Vw(v),
        "vh" | "svh" | "lvh" | "dvh" => LengthValue::Vh(v),
        "vmin" => LengthValue::Vmin(v),
        "vmax" => LengthValue::Vmax(v),
        "ch" => LengthValue::Ch(v),
        "ex" => LengthValue::Ex(v),
        "ic" => LengthValue::Ic(v),
        "lh" => LengthValue::Lh(v),
        "pt" => LengthValue::Px(v * 4.0 / 3.0),
        "pc" => LengthValue::Px(v * 16.0),
        "cm" => LengthValue::Px(v * 96.0 / 2.54),
        "mm" => LengthValue::Px(v * 96.0 / 25.4),
        "q" => LengthValue::Px(v * 96.0 / 25.4 / 4.0),
        "in" => LengthValue::Px(v * 96.0),
        _ => {
            let cq = unit.strip_prefix("cq")?;
            let (w, h) = CONTAINER_UNITS.with(|c| c.get());
            let basis = match cq {
                "w" | "i" => w,
                "h" | "b" => h,
                "min" => w.min(h),
                "max" => w.max(h),
                _ => return None,
            };
            LengthValue::Px(v * basis / 100.0)
        }
    })
}

fn lin_of(lv: &LengthValue) -> Option<Lin> {
    let mut l = Lin::default();
    match lv {
        LengthValue::Px(v) => l.px = *v,
        LengthValue::Em(v) => l.em = *v,
        LengthValue::Rem(v) => l.rem = *v,
        LengthValue::Percent(v) => l.percent = *v,
        LengthValue::Vw(v) => l.vw = *v,
        LengthValue::Vh(v) => l.vh = *v,
        LengthValue::Vmin(v) => l.vmin = *v,
        LengthValue::Vmax(v) => l.vmax = *v,
        LengthValue::Ch(v) => l.ch = *v,
        LengthValue::Ex(v) => l.ex = *v,
        LengthValue::Ic(v) => l.ic = *v,
        LengthValue::Lh(v) => l.lh = *v,
        LengthValue::Calc(lin) => l = *lin,
        LengthValue::Math(_, args) => l = *args.first()?,
        LengthValue::Tree(_) | LengthValue::Auto => return None,
    }
    Some(l)
}

/// sum := product (('+' | '-') product)*
fn eval_sum(comps: &[&[Token]]) -> Option<Lin> {
    let mut i = 0;
    let mut acc = eval_product(comps, &mut i)?;
    while i < comps.len() {
        let sign = match comps[i] {
            [Token::Delim('+')] => 1.0,
            [Token::Delim('-')] => -1.0,
            _ => return None,
        };
        i += 1;
        let rhs = eval_product(comps, &mut i)?;
        acc = acc.add(rhs, sign);
    }
    Some(acc)
}

/// product := unit (('*' | '/') unit)*
fn eval_product(comps: &[&[Token]], i: &mut usize) -> Option<Lin> {
    let mut acc = eval_unit(comps, i)?;
    while *i < comps.len() {
        match comps[*i] {
            [Token::Delim('*')] => {
                *i += 1;
                let rhs = eval_unit(comps, i)?;
                acc = if rhs.is_number {
                    acc.scale(rhs.num)
                } else if acc.is_number {
                    rhs.scale(acc.num)
                } else {
                    return None;
                };
            }
            [Token::Delim('/')] => {
                *i += 1;
                let rhs = eval_unit(comps, i)?;
                if !rhs.is_number || rhs.num == 0.0 {
                    return None;
                }
                acc = acc.scale(1.0 / rhs.num);
            }
            _ => break,
        }
    }
    Some(acc)
}

/// Names of the CSS Values 4 math functions evaluated here (besides
/// `calc()`, `min()`, `max()`, `clamp()`).
fn is_math_fn(name: &str) -> bool {
    // (No allocation: every function component of a length or number,
    // colors included, is tested.)
    ["round", "mod", "rem", "abs", "sign", "sin", "cos", "tan", "pow", "sqrt", "hypot", "log", "exp"].iter().any(|f| name.eq_ignore_ascii_case(f))
}

fn number_lin(n: f32) -> Lin {
    Lin { num: n, is_number: true, ..Lin::default() }
}

/// The unit terms of a length combination.
fn lin_terms(l: &Lin) -> [f32; 12] {
    [l.px, l.em, l.rem, l.percent, l.vw, l.vh, l.vmin, l.vmax, l.ch, l.ex, l.ic, l.lh]
}

/// Apply `f` to the values of `a` and `b` when they are numbers or
/// lengths of one same unit (a zero length matches any unit); `None` for
/// mixed units, which only layout could combine.
fn same_kind(a: Lin, b: Lin, f: impl Fn(f32, f32) -> f32) -> Option<Lin> {
    if a.is_number && b.is_number {
        return Some(number_lin(f(a.num, b.num)));
    }
    if a.is_number || b.is_number {
        return None;
    }
    let (ta, tb) = (lin_terms(&a), lin_terms(&b));
    let nz = |t: &[f32; 12]| t.iter().enumerate().filter(|(_, v)| **v != 0.0).map(|(i, _)| i).collect::<Vec<_>>();
    let (na, nb) = (nz(&ta), nz(&tb));
    let unit = match (na.as_slice(), nb.as_slice()) {
        ([], []) => 0,
        ([i], []) | ([], [i]) => *i,
        ([i], [j]) if i == j => *i,
        _ => return None,
    };
    let v = f(ta[unit], tb[unit]);
    let mut out = Lin::default();
    let slot = match unit {
        0 => &mut out.px,
        1 => &mut out.em,
        2 => &mut out.rem,
        3 => &mut out.percent,
        4 => &mut out.vw,
        5 => &mut out.vh,
        6 => &mut out.vmin,
        7 => &mut out.vmax,
        8 => &mut out.ch,
        9 => &mut out.ex,
        10 => &mut out.ic,
        _ => &mut out.lh,
    };
    *slot = v;
    Some(out)
}

/// A trigonometric argument in radians: an angle or a number.
fn angle_arg(arg: &[&[Token]]) -> Option<f32> {
    if let [[Token::Dimension(v, u)]] = arg {
        return Some(match u.to_ascii_lowercase().as_str() {
            "deg" => v.to_radians(),
            "rad" => *v,
            "grad" => (v * 0.9).to_radians(),
            "turn" => v * std::f32::consts::TAU,
            _ => return None,
        });
    }
    let l = eval_sum(arg)?;
    l.is_number.then_some(l.num)
}

/// `round()`, `mod()`, `rem()`, `abs()`, `sign()`, `sin()`, `cos()`,
/// `tan()`, `pow()`, `sqrt()`, `hypot()`, `log()`, `exp()` (CSS Values 4
/// §10) on numbers and single-unit lengths.
fn eval_math_fn(c: &[Token]) -> Option<Lin> {
    let [Token::Function(name), ..] = c else { return None };
    let name = name.to_ascii_lowercase();
    let args = function_args(c);
    let num = |a: &Vec<&[Token]>| -> Option<f32> {
        let l = eval_sum(a)?;
        l.is_number.then_some(l.num)
    };
    match (name.as_str(), args.as_slice()) {
        ("round", _) => {
            let (strategy, rest) = match args.first().map(|a| a.as_slice()) {
                Some([[Token::Ident(k)]]) => (k.to_ascii_lowercase(), &args[1..]),
                _ => ("nearest".to_string(), &args[..]),
            };
            let a = eval_sum(rest.first()?)?;
            let b = match rest.get(1) {
                Some(b) => eval_sum(b)?,
                None if a.is_number => number_lin(1.0),
                None => return None,
            };
            if rest.len() > 2 {
                return None;
            }
            let round = |x: f32, step: f32| -> f32 {
                if step == 0.0 {
                    return f32::NAN;
                }
                let q = x / step;
                let r = match strategy.as_str() {
                    "nearest" => (q + 0.5).floor(),
                    "up" => q.ceil(),
                    "down" => q.floor(),
                    "to-zero" => q.trunc(),
                    _ => return f32::NAN,
                };
                r * step
            };
            same_kind(a, b, round).filter(|l| !l.num.is_nan() && lin_terms(l).iter().all(|v| !v.is_nan()))
        }
        ("mod" | "rem", [a, b]) => {
            let (a, b) = (eval_sum(a)?, eval_sum(b)?);
            let floor = name == "mod";
            same_kind(a, b, |x, y| if floor { x - y * (x / y).floor() } else { x - y * (x / y).trunc() })
        }
        ("abs", [a]) => {
            let a = eval_sum(a)?;
            same_kind(a, if a.is_number { number_lin(0.0) } else { Lin::default() }, |x, _| x.abs())
        }
        ("sign", [a]) => {
            let a = eval_sum(a)?;
            let v = if a.is_number { a.num } else { lin_terms(&a).iter().copied().find(|v| *v != 0.0).unwrap_or(0.0) };
            Some(number_lin(if v > 0.0 { 1.0 } else if v < 0.0 { -1.0 } else { 0.0 }))
        }
        ("sin", [a]) => Some(number_lin(angle_arg(a)?.sin())),
        ("cos", [a]) => Some(number_lin(angle_arg(a)?.cos())),
        ("tan", [a]) => Some(number_lin(angle_arg(a)?.tan())),
        ("pow", [a, b]) => Some(number_lin(num(a)?.powf(num(b)?))),
        ("sqrt", [a]) => Some(number_lin(num(a)?.sqrt())),
        ("exp", [a]) => Some(number_lin(num(a)?.exp())),
        ("log", [a]) => Some(number_lin(num(a)?.ln())),
        ("log", [a, b]) => Some(number_lin(num(a)?.ln() / num(b)?.ln())),
        ("hypot", list) if !list.is_empty() => {
            let mut acc = eval_sum(&list[0])?;
            acc = same_kind(acc, acc, |x, _| x * x)?;
            for a in &list[1..] {
                let l = eval_sum(a)?;
                let sq = same_kind(l, l, |x, _| x * x)?;
                acc = same_kind(acc, sq, |x, y| x + y)?;
            }
            same_kind(acc, acc, |x, _| x.sqrt())
        }
        _ => None,
    }
}

fn eval_unit(comps: &[&[Token]], i: &mut usize) -> Option<Lin> {
    let c = *comps.get(*i)?;
    *i += 1;
    match c {
        [Token::Number(n)] => Some(Lin { num: *n, is_number: true, ..Lin::default() }),
        [Token::Function(f), ..] if is_math_fn(f) => eval_math_fn(c),
        // Numeric constants (CSS Values 4 §10.7.1).
        [Token::Ident(k)] => {
            let is = |n: &str| k.eq_ignore_ascii_case(n);
            if is("e") {
                Some(number_lin(std::f32::consts::E))
            } else if is("pi") {
                Some(number_lin(std::f32::consts::PI))
            } else if is("infinity") {
                Some(number_lin(f32::INFINITY))
            } else if is("-infinity") {
                Some(number_lin(f32::NEG_INFINITY))
            } else {
                None
            }
        }
        [Token::OpenParen, ..] => eval_sum(&components(strip_function(c))),
        [Token::Delim('-')] => {
            // unary minus
            let inner = eval_unit(comps, i)?;
            Some(inner.scale(-1.0))
        }
        _ => {
            let lv = parse_length(c)?;
            lin_of(&lv)
        }
    }
}

// ------------------------------------------------------------------- colors

pub fn parse_color(toks: &[Token]) -> Option<ColorValue> {
    let comps = components(toks);
    if comps.len() != 1 {
        return None;
    }
    parse_color_component(comps[0])
}

thread_local! {
    /// The value of `currentcolor` while a cascade applies declarations
    /// (for `color-mix()` and relative colors, which need it at once).
    static CURRENT_COLOR: std::cell::Cell<Option<Color>> = const { std::cell::Cell::new(None) };
}

pub fn set_current_color(c: Option<Color>) {
    CURRENT_COLOR.with(|v| v.set(c));
}

/// A color value with `currentcolor` replaced by the current color, when
/// known.
pub fn resolve_current(v: ColorValue) -> Option<Color> {
    match v {
        ColorValue::Color(c) => Some(c),
        ColorValue::CurrentColor => CURRENT_COLOR.with(|v| v.get()),
    }
}

pub fn parse_color_component(c: &[Token]) -> Option<ColorValue> {
    match c {
        [Token::Ident(name)] => {
            let l = name.to_ascii_lowercase();
            match l.as_str() {
                "transparent" => Some(ColorValue::Color(Color::TRANSPARENT)),
                "currentcolor" => Some(ColorValue::CurrentColor),
                _ => named_color(&l).or_else(|| crate::color_spaces::system_color(&l)).map(ColorValue::Color),
            }
        }
        [Token::Hash(h)] => hex_color(h).map(ColorValue::Color),
        [Token::Function(f), rest @ ..] => {
            let lf = f.to_ascii_lowercase();
            // Relative color syntax: `<fn>(from <color> ...)`.
            if rest.iter().find(|t| !matches!(t, Token::Whitespace)).is_some_and(|t| matches!(t, Token::Ident(k) if k.eq_ignore_ascii_case("from"))) {
                return crate::color_spaces::relative(&lf, c).map(ColorValue::Color);
            }
            match lf.as_str() {
                "rgb" | "rgba" => parse_rgb(c).map(ColorValue::Color),
                "hsl" | "hsla" => parse_hsl(c).map(ColorValue::Color),
                other => crate::color_spaces::parse_function(other, c),
            }
        }
        _ => None,
    }
}

/// Parse a hex color (without '#'). Also accepts quirky values like `FFF`.
pub fn hex_color(h: &str) -> Option<Color> {
    if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = |s: &str| u8::from_str_radix(s, 16).ok();
    let d = |s: &str| v(s).map(|x| x * 17);
    match h.len() {
        3 => Some(Color::rgb(d(&h[0..1])?, d(&h[1..2])?, d(&h[2..3])?)),
        4 => Some(Color::rgba(d(&h[0..1])?, d(&h[1..2])?, d(&h[2..3])?, d(&h[3..4])?)),
        6 => Some(Color::rgb(v(&h[0..2])?, v(&h[2..4])?, v(&h[4..6])?)),
        8 => Some(Color::rgba(v(&h[0..2])?, v(&h[2..4])?, v(&h[4..6])?, v(&h[6..8])?)),
        _ => None,
    }
}

/// Flatten function args: supports both `a, b, c [, alpha]` and
/// `a b c [/ alpha]` syntaxes. Returns (values, alpha).
fn color_args(c: &[Token]) -> Option<(Vec<&[Token]>, Option<&[Token]>)> {
    let inner = strip_function(c);
    let comps = components(inner);
    let mut vals = Vec::new();
    let mut alpha = None;
    let mut after_slash = false;
    for comp in comps {
        match comp {
            [Token::Comma] => continue,
            [Token::Delim('/')] => {
                after_slash = true;
                continue;
            }
            _ => {}
        }
        if after_slash {
            alpha = Some(comp);
        } else if vals.len() == 3 {
            alpha = Some(comp);
        } else {
            vals.push(comp);
        }
    }
    if vals.len() != 3 {
        return None;
    }
    Some((vals, alpha))
}

fn channel(c: &[Token]) -> Option<f32> {
    match c {
        [Token::Number(n)] => Some(*n),
        [Token::Percentage(p)] => Some(p * 255.0 / 100.0),
        [Token::Ident(s)] if s.eq_ignore_ascii_case("none") => Some(0.0),
        _ => None,
    }
}

fn alpha_value(c: Option<&[Token]>) -> Option<f32> {
    match c {
        None => Some(1.0),
        Some([Token::Number(n)]) => Some(*n),
        Some([Token::Percentage(p)]) => Some(p / 100.0),
        Some([Token::Ident(s)]) if s.eq_ignore_ascii_case("none") => Some(0.0),
        _ => None,
    }
}

fn to_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn parse_rgb(c: &[Token]) -> Option<Color> {
    let (vals, alpha) = color_args(c)?;
    let r = channel(vals[0])?;
    let g = channel(vals[1])?;
    let b = channel(vals[2])?;
    let a = alpha_value(alpha)?;
    Some(Color::rgba(to_u8(r), to_u8(g), to_u8(b), to_u8(a * 255.0)))
}

fn parse_hsl(c: &[Token]) -> Option<Color> {
    let (vals, alpha) = color_args(c)?;
    // The legacy comma syntax takes percentages only (`hsla(0, 0, 0, 1)`
    // is invalid); bare numbers belong to the modern syntax (CSS Color 4).
    let legacy = components(strip_function(c)).iter().any(|comp| matches!(comp, [Token::Comma]));
    let h = match vals[0] {
        [Token::Number(n)] => *n,
        [Token::Dimension(n, u)] => match u.as_str() {
            "deg" => *n,
            "rad" => n.to_degrees(),
            "grad" => n * 0.9,
            "turn" => n * 360.0,
            _ => return None,
        },
        [Token::Ident(s)] if s.eq_ignore_ascii_case("none") => 0.0,
        _ => return None,
    };
    let pct = |c: &[Token]| -> Option<f32> {
        match c {
            [Token::Percentage(p)] => Some(p / 100.0),
            [Token::Number(n)] if !legacy => Some(n / 100.0),
            [Token::Ident(s)] if s.eq_ignore_ascii_case("none") => Some(0.0),
            _ => None,
        }
    };
    let s = pct(vals[1])?.clamp(0.0, 1.0);
    let l = pct(vals[2])?.clamp(0.0, 1.0);
    let a = alpha_value(alpha)?;
    let (r, g, b) = hsl_to_rgb(h, s, l);
    Some(Color::rgba(to_u8(r * 255.0), to_u8(g * 255.0), to_u8(b * 255.0), to_u8(a * 255.0)))
}

pub(crate) fn hsl_to_rgb_pub(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    hsl_to_rgb(h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(360.0) / 360.0;
    if s == 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

/// CSS named colors (lower-case name → RGB).
pub fn named_color(name: &str) -> Option<Color> {
    let idx = NAMED.binary_search_by(|(n, _)| n.cmp(&name)).ok()?;
    let v = NAMED[idx].1;
    Some(Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

// Sorted by name.
static NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff), ("beige", 0xf5f5dc), ("bisque", 0xffe4c4), ("black", 0x000000),
    ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff), ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00), ("chocolate", 0xd2691e),
    ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c),
    ("cyan", 0x00ffff), ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9), ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f), ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000), ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f), ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f), ("darkturquoise", 0x00ced1), ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff), ("dimgray", 0x696969), ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff), ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0), ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc), ("ghostwhite", 0xf8f8ff), ("gold", 0xffd700),
    ("goldenrod", 0xdaa520), ("gray", 0x808080), ("green", 0x008000), ("greenyellow", 0xadff2f),
    ("grey", 0x808080), ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082), ("ivory", 0xfffff0), ("khaki", 0xf0e68c), ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5), ("lawngreen", 0x7cfc00), ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080), ("lightcyan", 0xe0ffff), ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1), ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6), ("magenta", 0xff00ff), ("maroon", 0x800000), ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3), ("mediumpurple", 0x9370db), ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee), ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc), ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970), ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1), ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead), ("navy", 0x000080), ("oldlace", 0xfdf5e6), ("olive", 0x808000),
    ("olivedrab", 0x6b8e23), ("orange", 0xffa500), ("orangered", 0xff4500), ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98), ("paleturquoise", 0xafeeee), ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9), ("peru", 0xcd853f), ("pink", 0xffc0cb),
    ("plum", 0xdda0dd), ("powderblue", 0xb0e0e6), ("purple", 0x800080), ("rebeccapurple", 0x663399),
    ("red", 0xff0000), ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1), ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072), ("sandybrown", 0xf4a460), ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d), ("silver", 0xc0c0c0), ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd),
    ("slategray", 0x708090), ("slategrey", 0x708090), ("snow", 0xfffafa), ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4), ("tan", 0xd2b48c), ("teal", 0x008080), ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347), ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3),
    ("white", 0xffffff), ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00), ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;
    use rabbithole_css::Tokenizer;

    fn ctx() -> LengthContext {
        LengthContext { font_size: 16.0, root_font_size: 16.0, viewport_w: 1000.0, viewport_h: 500.0, ch: 0.5, ex: 0.5, ic: 1.0, lh: 19.2 }
    }

    fn len(s: &str) -> Length {
        let t = Tokenizer::tokenize(s);
        let c = components(&t);
        parse_length(c[0]).expect("length").resolve(&ctx())
    }

    fn col(s: &str) -> ColorValue {
        parse_color(&Tokenizer::tokenize(s)).expect("color")
    }

    #[test]
    fn named_sorted() {
        for w in NAMED.windows(2) {
            assert!(w[0].0 < w[1].0, "{} {}", w[0].0, w[1].0);
        }
        assert_eq!(NAMED.len(), 148);
    }

    #[test]
    fn colors() {
        assert_eq!(col("red"), ColorValue::Color(Color::rgb(255, 0, 0)));
        assert_eq!(col("#abc"), ColorValue::Color(Color::rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(col("#11223380"), ColorValue::Color(Color::rgba(0x11, 0x22, 0x33, 0x80)));
        assert_eq!(col("rgb(1, 2, 3)"), ColorValue::Color(Color::rgb(1, 2, 3)));
        assert_eq!(col("rgba(1 2 3 / 50%)"), ColorValue::Color(Color::rgba(1, 2, 3, 128)));
        assert_eq!(col("rgb(100%, 0%, 0%, 0.5)"), ColorValue::Color(Color::rgba(255, 0, 0, 128)));
        assert_eq!(col("hsl(120, 100%, 50%)"), ColorValue::Color(Color::rgb(0, 255, 0)));
        assert_eq!(col("hsl(0 0% 100%)"), ColorValue::Color(Color::rgb(255, 255, 255)));
        // The legacy (comma) syntax needs percentages for saturation and
        // lightness; bare numbers are the modern syntax.
        assert!(parse_color(&Tokenizer::tokenize("hsla(0, 0, 0, 1)")).is_none());
        assert_eq!(col("hsl(0 0 100)"), ColorValue::Color(Color::rgb(255, 255, 255)));
        assert_eq!(col("currentColor"), ColorValue::CurrentColor);
        assert_eq!(col("transparent"), ColorValue::Color(Color::TRANSPARENT));
        assert!(parse_color(&Tokenizer::tokenize("notacolor")).is_none());
    }

    #[test]
    fn lengths() {
        assert_eq!(len("10px"), Length::Px(10.0));
        assert_eq!(len("2em"), Length::Px(32.0));
        assert_eq!(len("1.5rem"), Length::Px(24.0));
        assert_eq!(len("50%"), Length::Percent(50.0));
        assert_eq!(len("10vw"), Length::Px(100.0));
        assert_eq!(len("10vh"), Length::Px(50.0));
        assert_eq!(len("auto"), Length::Auto);
        assert_eq!(len("0"), Length::Px(0.0));
        assert_eq!(len("12pt"), Length::Px(16.0));
        assert_eq!(len("1in"), Length::Px(96.0));
    }

    #[test]
    fn calc() {
        assert_eq!(len("calc(10px + 2em)"), Length::Px(42.0));
        assert_eq!(len("calc(100% - 20px)"), Length::Calc(-20.0, 100.0));
        assert_eq!(len("calc(2 * 10px)"), Length::Px(20.0));
        assert_eq!(len("calc((10px + 10px) / 2)"), Length::Px(10.0));
        assert_eq!(len("min(10px, 2em)"), Length::Px(10.0));
        assert_eq!(len("max(10px, 2em)"), Length::Px(32.0));
        assert_eq!(len("clamp(10px, 100px, 20px)"), Length::Px(20.0));
        // Nested math without percentages still computes to px.
        assert_eq!(len("calc(min(10px, 2em) + 5px)"), Length::Px(15.0));
        assert_eq!(len("max(calc(2 * 10px), 1em)"), Length::Px(20.0));
    }

    /// Math functions mixing percentages and lengths keep their shape until
    /// the percentage base is known.
    #[test]
    fn math_with_percentages() {
        let at = |s: &str, base: f32| {
            let l = len(s);
            assert!(l.has_percent(), "{s} computed to {l:?}");
            l.resolve(base).unwrap()
        };
        assert_eq!((at("min(50%, 30px)", 40.0), at("min(50%, 30px)", 100.0)), (20.0, 30.0));
        assert_eq!((at("min(100%, 500px)", 300.0), at("min(100%, 500px)", 800.0)), (300.0, 500.0));
        assert_eq!((at("max(50%, 300px)", 400.0), at("max(50%, 300px)", 1000.0)), (300.0, 500.0));
        let clamp = "clamp(200px, 50%, 800px)";
        assert_eq!((at(clamp, 300.0), at(clamp, 1000.0), at(clamp, 2000.0)), (200.0, 500.0, 800.0));
        let nested = "min(calc(100% - 32px), 1200px)";
        assert_eq!((at(nested, 500.0), at(nested, 2000.0)), (468.0, 1200.0));
        let inner = "calc(100% - max(10%, 20px))";
        assert_eq!((at(inner, 100.0), at(inner, 1000.0)), (80.0, 900.0));
        let em = "calc(40% - min(2em, 140px) * 2)";
        assert_eq!(at(em, 1000.0), 400.0 - 64.0);
        assert_eq!(at("-webkit-calc(100% - 2 * 1em)", 200.0), 168.0);
        // Equal expressions share one interned entry.
        assert_eq!(len("min(100%, 500px)"), len("min(100%,500px)"));
        let css = |s: &str| match len(s) {
            Length::Math(id) => crate::computed::math_node(id).unwrap().to_css(),
            Length::Calc(px, p) => crate::computed::MathNode::Leaf(px, p).to_css(),
            other => format!("{other:?}"),
        };
        assert_eq!(css("calc(100% - 20px)"), "calc(100% - 20px)");
        assert_eq!(css("min(100%, 500px)"), "min(100%, 500px)");
        assert_eq!(css("clamp(10px, 50% + 1em, 100px)"), "clamp(10px, 50% + 16px, 100px)");
        // Invalid: clamp() needs three arguments.
        let t = Tokenizer::tokenize("clamp(10%, 20px)");
        assert!(parse_length(components(&t)[0]).is_none());
    }
}

#[cfg(test)]
mod font_scope_tests {
    use super::*;
    use std::rc::Rc;
    use rabbithole_gfx::{Family, FontContext};

    #[test]
    fn nested_scopes_restore_fonts_cache_and_unicode_range_skip() {
        let host = FontContext::new();
        let a = Rc::new(host.document_scope());
        let b = Rc::new(host.document_scope());
        a.add_font_data("Unit alias", 400, false, include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf")).unwrap();
        b.add_font_data("Unit alias", 400, false, include_bytes!("../../../assets/fonts/DejaVuSerif.ttf")).unwrap();
        assert_eq!(a.face_count(), b.face_count());
        let mut style = crate::ComputedStyle::initial();
        style.font_family_list = vec!["Unit alias".into()];
        let expected_a = a.unit_metrics(&style.font_family_list, Family::Sans, 400, false);
        let expected_b = b.unit_metrics(&style.font_family_list, Family::Sans, 400, false);
        assert_ne!(expected_a, expected_b);
        let _outer = scope_unit_fonts(Some(a.clone()), vec![]);
        assert_eq!(font_units(&style), expected_a);
        let saved_cache = UNIT_CACHE.with(|c| c.borrow().clone());
        {
            let _inner = scope_unit_fonts(Some(b), vec![]);
            assert_eq!(font_units(&style), expected_b);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _skipped = scope_unit_fonts(Some(a.clone()), vec!["unit alias".into()]);
                assert_eq!(font_units(&style), a.unit_metrics(&[], Family::Sans, 400, false));
                panic!("exercise scope restoration");
            }));
            assert!(result.is_err());
            assert_eq!(font_units(&style), expected_b);
            UNIT_SKIP.with(|s| assert!(s.borrow().is_empty()));
        }
        assert_eq!(UNIT_CACHE.with(|c| c.borrow().clone()), saved_cache);
        assert_eq!(font_units(&style), expected_a);
    }

    #[test]
    fn host_generation_invalidates_units_without_enumerating_faces() {
        let host = Rc::new(FontContext::new());
        let page = Rc::new(host.document_scope());
        let mut style = crate::ComputedStyle::initial();
        style.font_family_list = vec!["Late font".into()];
        let _scope = scope_unit_fonts(Some(page.clone()), vec![]);
        let before = font_units(&style);
        host.add_font_data("Late font", 400, false, include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf")).unwrap();
        let after = font_units(&style);
        assert_ne!(before, after);
        assert_eq!(after, page.unit_metrics(&style.font_family_list, Family::Sans, 400, false));
    }
}
